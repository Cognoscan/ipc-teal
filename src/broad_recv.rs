use std::{
    ffi::c_void,
    io::{self, IoSlice, IoSliceMut},
    path::Path,
    sync::atomic::Ordering,
    time::UNIX_EPOCH,
};

use memfile::MemFile;
use tokio_seqpacket::{
    UnixSeqpacket,
    ancillary::{AncillaryMessageReader, AncillaryMessageWriter, OwnedAncillaryMessage},
};

use crate::{
    fd_from_ancillary, BroadIpcRecvState, BroadIpcSendState, MessageHeader, SIZE_HEADER, SIZE_RECV_STATE, SIZE_SEND_STATE, ST_ACTIVE, ST_CLOSED, ST_WAITING
};

/// Receiver side of a shared-mem Broadcasting IPC channel.
pub struct BroadReceiver {
    /// Receive state
    state: memmap2::MmapMut,
    /// Shared memory from sender
    buf: memmap2::Mmap,
    /// Queue depth, which we retrieved early from the sender's buffer.
    queue_depth: usize,
    /// The open connection to the sender
    conn: UnixSeqpacket,
}

impl BroadReceiver {
    pub async fn open<P: AsRef<Path>>(address: P) -> std::io::Result<Self> {
        let conn = UnixSeqpacket::connect(address).await?;

        // Get the shared send state memory file
        let mut ancillary_buffer = [0; 128];
        let io_buf = IoSliceMut::new(&mut []);
        let (read_size, anc) = conn
            .recv_vectored_with_ancillary(&mut [io_buf], &mut ancillary_buffer)
            .await?;

        // Recover the shared send state memory
        let fd = fd_from_ancillary(anc)?;
        if read_size != 0 {
            return Err(io::Error::other("non-zero read data on first ipc message"));
        }

        // Map the send state memory and do initial verification of it.
        let fd = MemFile::from_fd(fd)?;
        let seals = fd.get_seals()?;
        if seals != crate::seals() {
            return Err(io::Error::other("seals weren't set up as expected"));
        }
        let buf = unsafe { memmap2::Mmap::map(&fd)? };
        if buf.len() < SIZE_SEND_STATE {
            return Err(io::Error::other("memory map from sender is too small"));
        }

        // Construct our shared memory file, mmap it, and seal it.
        let timestamp = UNIX_EPOCH.elapsed().unwrap();
        let name = format!(
            "ipc-teal-broadrecv.{}.{}",
            timestamp.as_secs(),
            timestamp.subsec_nanos()
        );
        let mem_file = MemFile::create_sealable(&name)?;
        mem_file.set_len(SIZE_RECV_STATE as u64)?;
        let mut state = unsafe { memmap2::MmapMut::map_mut(&mem_file)? };
        mem_file.add_seals(crate::seals())?;

        // Load our initial config from the send state.
        let ipc = unsafe { &mut *(state.as_mut_ptr() as *mut BroadIpcRecvState) };
        let send_state = unsafe { &*(buf.as_ptr() as *const BroadIpcSendState) };
        ipc.recv_state.value.store(ST_ACTIVE, Ordering::Release);
        ipc.head.value.store(
            send_state.tail.value.load(Ordering::Acquire),
            Ordering::Release,
        );
        let queue_depth = send_state.queue_depth;

        // Make sure the queue depth makes sense.
        if !queue_depth.is_power_of_two() || queue_depth > (1 << 20) {
            return Err(io::Error::other("sent queue depth isn't a valid size"));
        }

        // Verify the buffer is at least large enough for the sender's state
        // struct and the send queue.
        if buf.len() < (SIZE_SEND_STATE + queue_depth * SIZE_HEADER) {
            return Err(io::Error::other("memory map from sender is too small for buffer and state"));
        }

        // Send the shared receive state memory file across
        let mut ancillary_buffer = [0; 128];
        let mut anc = AncillaryMessageWriter::new(&mut ancillary_buffer);
        anc.add_fds(&[&mem_file])?;
        let data = IoSlice::new(&[]);
        conn.send_vectored_with_ancillary(&[data], &mut anc).await?;

        Ok(Self {
            state,
            buf,
            queue_depth,
            conn,
        })
    }

    fn state(&mut self) -> &mut BroadIpcRecvState {
        unsafe { &mut *(self.state.as_mut_ptr() as *mut BroadIpcRecvState) }
    }
}

impl Drop for BroadReceiver {
    fn drop(&mut self) {
        // Gracefully close down by updating our state to closed.
        self.state()
            .recv_state
            .value
            .store(ST_CLOSED, Ordering::Release);
    }
}
