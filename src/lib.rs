use std::{
    ffi::c_void,
    io,
    os::fd::{IntoRawFd, OwnedFd},
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

const ST_ACTIVE: usize = 0;
const ST_WAITING: usize = 1;
const ST_CLOSED: usize = 2;

const WAKE_SEND: u8 = 0;
const WAKE_RECV: u8 = 1;

const SIZE_STATE: usize = core::mem::size_of::<IpcState>();
const SIZE_RECV_STATE: usize = core::mem::size_of::<BroadIpcRecvState>();
const SIZE_SEND_STATE: usize = core::mem::size_of::<BroadIpcSendState>();
const SIZE_HEADER: usize = core::mem::size_of::<MessageHeader>();

#[inline]
fn seals() -> Seals {
    Seal::FutureWrite | Seal::Grow | Seal::Shrink | Seal::Seal
}

use memfile::{MemFile, Seal, Seals};

mod broad_recv;
mod broadcast;
mod server;

pub use broad_recv::BroadReceiver;
pub use broadcast::Broadcaster;
use memmap2::{Mmap, MmapMut};
pub use server::Server;
use tokio_seqpacket::{
    UnixSeqpacket,
    ancillary::{AncillaryMessageReader, AncillaryMessageWriter, OwnedAncillaryMessage},
    borrow_fd::BorrowFd,
};

/// Simplest cache padding - nobody goes above 128 byte alignment. Could be
/// swapped for the one in crossbeam-utils later.
#[repr(C, align(128))]
#[derive(Debug)]
struct CachePadded<T> {
    pub value: T,
}

/// Message Data
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MessageHeader {
    pub offset: usize,
    pub len: usize,
}

#[repr(C)]
#[derive(Debug)]
struct BroadIpcRecvState {
    /// Receiver increments this whenever we take a message off the queue. It
    /// points to the next spot a message header should be read from. Must not
    /// be incremented past the tail value modulo the buffer size.
    head: CachePadded<AtomicU64>,
    /// Receiver updates.
    ///
    /// Sender: 0 when we can just update the tail and move on, 1 when we need
    /// to hit up the `event_valid` eventfd to wake the receiver up, 2 when the
    /// receiver has hung up.
    ///
    /// Receiver: set to 1 when we're about to sleep the thread, set to 0 when
    /// we are about to start reading messages off the queue. Set to 2 when
    /// we're closed.
    recv_state: CachePadded<AtomicUsize>,
}

#[repr(C)]
#[derive(Debug)]
struct BroadIpcSendState {
    /// Sender should never change this.
    queue_depth: usize,
    /// Sender increments this whenever we add a message to the queue. It points
    /// to the *next* spot where a message header will be placed. Must not be
    /// incremented past the head value modulo the buffer size.
    tail: CachePadded<AtomicU64>,
    /// Sender updates.
    ///
    /// Sender: set to 1 when we're about to sleep the thread, set to 0 when we
    /// are about to start sending messages on the queue, set to 2 when we are
    /// hanging up.
    ///
    /// Receiver: 0 when we can just update the head and move on, 1 when we need
    /// to hit up the `event_ready` eventfd to wake the sender up, 2 when the
    /// sender has hung up on us.
    send_state: CachePadded<AtomicUsize>,
}

/// IPC State for bidirectional channel
#[repr(C)]
#[derive(Debug)]
struct IpcState {
    /// Queue size of the buffer in this memory map. Should never change.
    queue_depth: CachePadded<usize>,
    /// State of the sender with control of this memory map.
    state: CachePadded<AtomicUsize>,
    /// State of the receiver with control of this memory map.
    other_state: CachePadded<AtomicUsize>,
    /// Tail pointer to the buffer in this memory map.
    tail: CachePadded<AtomicU64>,
    /// Head pointer to a buffer in a different memory map.
    other_head: CachePadded<AtomicU64>,
}

/// Construct a shared memory file, mmap it, and seal it.
fn new_mem(len: usize) -> io::Result<(MemFile, MmapMut)> {
    let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap();
    let name = format!(
        "ipc-teal.{}.{}",
        timestamp.as_secs(),
        timestamp.subsec_nanos()
    );
    let mem_file = MemFile::create_sealable(&name)?;
    mem_file.set_len(len as u64)?;
    let mem_map = unsafe { memmap2::MmapMut::map_mut(&mem_file)? };
    mem_file.add_seals(crate::seals())?;
    Ok((mem_file, mem_map))
}

/// Send a memory backed file as a file descriptor over a connection.
async fn send_memfile(conn: &UnixSeqpacket, memfile: &MemFile) -> io::Result<()> {
    let mut ancillary_buffer = [0; 128];
    let mut anc = AncillaryMessageWriter::new(&mut ancillary_buffer);
    anc.add_fds(&[memfile])?;
    let data = io::IoSlice::new(&[]);
    conn.send_vectored_with_ancillary(&[data], &mut anc).await?;
    Ok(())
}

/// Receive a memory backed file, verify it has the expected seals, and memmap it.
async fn recv_mem(conn: &UnixSeqpacket) -> io::Result<Mmap> {
    let mut ancillary_buffer = [0; 128];
    let io_buf = io::IoSliceMut::new(&mut []);
    let (read_size, anc) = conn
        .recv_vectored_with_ancillary(&mut [io_buf], &mut ancillary_buffer)
        .await?;

    // Recover the shared send state memory
    let fd = fd_from_ancillary(anc)?;
    if read_size != 0 {
        return Err(io::Error::other("non-zero read data on FD ipc message"));
    }

    // Verify the seals
    let fd = MemFile::from_fd(fd)?;
    let seals = fd.get_seals()?;
    if seals != crate::seals() {
        return Err(io::Error::other("seals weren't set up as expected"));
    }

    // Map the memory. This operation is unsafe, but we're kicking that up
    // to the individual shared-mem implementations to deal with that correctly.
    // We at least can make *some* guarantees here: only the sender would've
    // been able to set up mutable memory maps of this memory, and the map
    // will never grow or shrink.
    let buf = unsafe { Mmap::map(&fd)? };
    Ok(buf)
}

fn fd_from_ancillary(anc: AncillaryMessageReader) -> io::Result<OwnedFd> {
    // We have to process all messages, just so we can close any file
    // descriptors we got sent (even if we didn't want any others)
    let mut fd = None;
    let mut err = false;
    for msg in anc.into_messages() {
        match msg {
            OwnedAncillaryMessage::FileDescriptors(fds) => {
                for a_fd in fds {
                    if fd.is_none() {
                        fd = Some(a_fd);
                    } else {
                        err = true;
                    }
                }
            }
            _ => {
                err = true;
            }
        }
    }
    let Some(fd) = fd else {
        return Err(io::ErrorKind::InvalidData.into());
    };
    if err {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(fd)
}

struct MemQueue {
    queue_depth: usize,
    mem: Mmap,
}

impl MemQueue {
    pub fn from_mem(mem: Mmap) -> io::Result<Self> {
        if mem.len() < SIZE_STATE {
            return Err(io::Error::other(
                "memory map from is too small for buffer and state",
            ));
        }
        let mut s = Self {
            queue_depth: 0,
            mem,
        };
        let qd = s.header().queue_depth.value;
        if !qd.is_power_of_two() || qd > (1 << 20) {
            return Err(io::Error::other("queue depth isn't a valid size"));
        }
        s.queue_depth = qd;

        if s.mem.len() < (SIZE_SEND_STATE + qd * SIZE_HEADER) {
            return Err(io::Error::other(
                "memory map is too small for buffer and state",
            ));
        }

        Ok(s)
    }

    fn header(&self) -> &IpcState {
        unsafe { &*(self.mem.as_ptr() as *const IpcState) }
    }

    /// Atomically get the tail pointer.
    pub fn tail(&self) -> u64 {
        self.header().tail.value.load(Ordering::Acquire)
    }

    /// Atomically load the state.
    pub fn state(&self) -> usize {
        self.header().state.value.load(Ordering::Acquire)
    }

    /// Atomically load the receive state.
    pub fn other_state(&self) -> usize {
        self.header().other_state.value.load(Ordering::Acquire)
    }

    /// Atomically load the other head pointer.
    pub fn other_head(&self) -> u64 {
        self.header().other_head.value.load(Ordering::Acquire)
    }

    /// Get the queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue_depth
    }

    /// Load a message given a local pointer. The pointed-to memory is *unsafe*
    /// to read because the memory can be changed by anything with mutable
    /// access to the memory, but it is guaranteed to be a valid memory range at
    /// least.
    pub fn read(&self, idx: u64) -> Option<&[u8]> {
        // Load the message header
        let idx = (idx & (self.queue_depth as u64 - 1)) as usize;
        let header = unsafe {
            (self.mem.as_ptr().byte_add(SIZE_STATE) as *const MessageHeader)
                .add(idx)
                .read()
        };
        let offset = header.offset + SIZE_STATE + self.queue_depth * SIZE_HEADER;
        self.mem.get(offset..(offset + header.len))
    }
}

struct MutMemQueue {
    mem: MmapMut,
}

impl MutMemQueue {
    pub fn new(buf_len: usize, queue_depth: usize) -> io::Result<(MemFile, Self)> {
        let queue_depth = queue_depth.next_power_of_two();
        let len = buf_len + queue_depth * SIZE_HEADER + SIZE_STATE;
        let (file, mem) = new_mem(len)?;
        let mut s = Self { mem };
        let header = s.header_mut();

        // Initialize the queue state
        header.queue_depth.value = queue_depth;
        header.other_head.value.store(0, Ordering::Release);
        header.state.value.store(ST_ACTIVE, Ordering::Release);
        header.other_state.value.store(ST_ACTIVE, Ordering::Release);
        header.tail.value.store(0, Ordering::Release);
        // Set all queue slots to 0 to begin with
        unsafe {
            (s.mem.as_mut_ptr().byte_add(SIZE_STATE) as *mut MessageHeader)
                .write_bytes(0, queue_depth);
        }
        Ok((file, s))
    }

    fn queue(&self) -> &[MessageHeader] {
        unsafe {
            let ptr = self.mem.as_ptr().byte_add(SIZE_STATE) as *const MessageHeader;
            core::slice::from_raw_parts(ptr, self.queue_depth())
        }
    }

    fn queue_mut(&mut self) -> &mut [MessageHeader] {
        unsafe {
            let ptr = self.mem.as_mut_ptr() as *mut MessageHeader;
            core::slice::from_raw_parts_mut(ptr, self.queue_depth())
        }
    }

    fn buf_mut(&mut self) -> &mut [u8] {
        let offset = SIZE_STATE + self.queue_depth() * SIZE_HEADER;
        unsafe { self.mem.get_unchecked_mut(offset..) }
    }

    fn header_mut(&mut self) -> &mut IpcState {
        unsafe { &mut *(self.mem.as_mut_ptr() as *mut IpcState) }
    }

    fn header(&self) -> &IpcState {
        unsafe { &*(self.mem.as_ptr() as *const IpcState) }
    }

    pub fn other_head(&self) -> u64 {
        self.header().other_head.value.load(Ordering::Relaxed)
    }

    pub fn queue_depth(&self) -> usize {
        self.header().queue_depth.value
    }

    pub fn state(&self) -> usize {
        self.header().state.value.load(Ordering::Relaxed)
    }

    pub fn other_state(&self) -> usize {
        self.header().other_state.value.load(Ordering::Relaxed)
    }

    pub fn tail(&self) -> u64 {
        self.header().tail.value.load(Ordering::Relaxed)
    }

    pub fn set_other_head(&mut self, val: u64) {
        self.header_mut()
            .other_head
            .value
            .store(val, Ordering::Release);
    }

    pub fn set_tail(&mut self, val: u64) {
        self.header_mut().tail.value.store(val, Ordering::Release);
    }

    pub fn set_state(&mut self, val: usize) {
        self.header_mut().state.value.store(val, Ordering::Release);
    }

    pub fn set_other_state(&mut self, val: usize) {
        self.header_mut().other_state.value.store(val, Ordering::Release);
    }

    /// The maximum amount of data that can be put in a queue message.
    pub fn max_len(&self) -> usize {
        self.mem.len() - SIZE_STATE - (SIZE_HEADER * self.queue_depth())
    }

    /// Given a head pointer from somewhere, see if we can reserve `len` bytes
    /// for a write operation, returning the buffer if we can.
    ///
    /// Also returns `None` if the queue is full and there'd be no place to put
    /// another message.
    ///
    /// # Panics
    ///
    /// Panics if the head pointer is greater than the tail pointer, or if they
    /// differ by more than one lap around the queue.
    pub fn try_reserve(&mut self, head: u64, len: usize) -> Option<&mut [u8]> {
        // Initial checks
        let queue_depth = self.queue_depth() as u64;
        let mask = queue_depth - 1;
        let tail = self.tail();
        assert!(head <= tail, "Head pointer exceeded tail pointer");
        assert!((tail - head) <= queue_depth, "Lapped the tail pointer");
        if (tail - head) == queue_depth {
            return None;
        }
        if tail == head {
            // No data currently in buffer, go nuts
            return self.buf_mut().get_mut(..len);
        }

        // Load the oldest and newest message headers...
        let head_idx = (head & mask) as usize;
        let tail_idx = (tail.saturating_sub(1) & mask) as usize;
        let buf_len = self.max_len();
        let queue = self.queue_mut();
        let head_header = queue[head_idx];
        let tail_header = queue[tail_idx];

        // figure out the bounds of the buffer space.
        if head_header.offset > tail_header.offset {
            // available buffer is one chunk in middle of buffer
            let offset = tail_header.offset + tail_header.len;
            let avail_len = head_header.offset - offset;
            let tail_idx = (tail & mask) as usize;
            if avail_len >= len {
                queue[tail_idx] = MessageHeader { offset, len };
                Some(unsafe { self.buf_mut().get_unchecked_mut(offset..offset + len) })
            } else {
                None
            }
        } else {
            // split available buffers: one at end, one at start. Try one at end
            // first, then one at start as fallback.
            let end_offset = tail_header.offset + tail_header.len;
            let end_len = buf_len - end_offset;
            if end_len >= len {
                queue[tail_idx] = MessageHeader {
                    offset: end_offset,
                    len,
                };
                return Some(unsafe {
                    self.buf_mut()
                        .get_unchecked_mut(end_offset..end_offset + len)
                });
            }
            let start_len = head_header.offset;
            if start_len >= len {
                queue[tail_idx] = MessageHeader {
                    offset: 0,
                    len,
                };
                return Some(unsafe {
                    self.buf_mut().get_unchecked_mut(..len)
                })
            }
            None
        }
    }

    /// Write out a message by fixing the actual length and updating the tail pointer.
    pub fn write_message(&mut self, len: usize) {
        let tail = self.tail();

        // Update our tail with the actual length
        let msg = &mut self.queue_mut()[tail as usize];
        assert!(msg.len >= len);
        msg.len = len;

        // Increment the tail pointer
        self.set_tail(tail + 1);
    }
}
