use std::{
    io::{ErrorKind::InvalidData, IoSlice, IoSliceMut},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering}, Arc, Mutex
    },
    time::{Duration, UNIX_EPOCH},
};

use memfile::MemFile;
use memmap2::{Mmap, MmapMut};
use tokio::{task::JoinHandle, time::timeout};
use tokio_seqpacket::{UCred, UnixSeqpacket, ancillary::AncillaryMessageWriter};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::{
    BroadIpcSendState, SIZE_HEADER, SIZE_RECV_STATE, SIZE_SEND_STATE, ST_ACTIVE, ST_CLOSED,
    fd_from_ancillary, seals,
};

struct TrackReceiver {
    /// Shared memory from the receiver
    mem: Mmap,
    /// Unix Socket for the open connection to the receiver.
    conn: UnixSeqpacket,
}

struct BroadcasterSync {
    recv: Mutex<Vec<TrackReceiver>>,
    done_signal: AtomicBool,
}

pub struct Broadcaster {
    sync: Arc<BroadcasterSync>,
    mem: MmapMut,
    buf_head: usize,
    buf_tail: usize,
    close_listener: CancellationToken,
    addr: PathBuf,
}

impl Broadcaster {
    /// Construct a new sender.
    ///
    /// The sender is a fixed buffer size, with a fixed queue depth. Once
    /// created, any receiver can connect and receive broadcast messages going
    /// forward.
    pub fn new<P: AsRef<Path>>(
        address: P,
        buf_size: usize,
        queue_depth: usize,
    ) -> std::io::Result<Self> {
        Self::with_cred_filter(address, buf_size, queue_depth, |_| true)
    }

    pub fn with_cred_filter<P, F>(
        address: P,
        buf_size: usize,
        queue_depth: usize,
        mut filter: F,
    ) -> std::io::Result<Self>
    where
        P: AsRef<Path>,
        F: FnMut(UCred) -> bool + Send + 'static,
    {
        // Set up the shared memory file, mmap it, and seal it.
        let timestamp = UNIX_EPOCH.elapsed().unwrap();
        let name = format!(
            "ipc-teal-broadcast.{}.{}",
            timestamp.as_secs(),
            timestamp.subsec_nanos()
        );
        let queue_depth = queue_depth.next_power_of_two();
        let mem_file = MemFile::create_sealable(&name)?;
        mem_file.set_len((SIZE_SEND_STATE + queue_depth * SIZE_HEADER + buf_size) as u64)?;
        let mut mem = unsafe { memmap2::MmapMut::map_mut(&mem_file)? };
        mem_file.add_seals(seals())?;

        // Initialize the shared memory state.
        let ipc = unsafe { &mut *(mem.as_mut_ptr() as *mut BroadIpcSendState) };
        ipc.send_state.value.store(0, Ordering::Release);
        ipc.queue_depth = queue_depth;
        ipc.send_state.value.store(ST_ACTIVE, Ordering::Release);

        let mut socket = tokio_seqpacket::UnixSeqpacketListener::bind(address.as_ref())?;

        let sync = Arc::new(BroadcasterSync {
            recv: Mutex::new(Vec::new()),
            done_signal: AtomicBool::new(false),
        });
        let sync_spawn = sync.clone();

        let cancel = tokio_util::sync::CancellationToken::new();
        let close_listener = cancel.clone();
        let addr = address.as_ref().to_path_buf().canonicalize()?;

        tokio::spawn(cancel.run_until_cancelled_owned(async move {
            let sync = sync_spawn;
            loop {
                if let Ok(conn) = socket.accept().await {
                    // Verify we're ok talking to this receiver.
                    let Ok(cred) = conn.peer_cred() else { continue };
                    if !filter(cred) {
                        continue;
                    }

                    async fn finish_conn(
                        mem_file: &MemFile,
                        conn: UnixSeqpacket,
                    ) -> std::io::Result<TrackReceiver> {
                        // Send the shared state memory.
                        let mut ancillary_buffer = [0; 128];
                        let mut anc = AncillaryMessageWriter::new(&mut ancillary_buffer);
                        anc.add_fds(&[&mem_file]).unwrap();
                        let data = IoSlice::new(&[]);
                        conn.send_vectored_with_ancillary(&[data], &mut anc).await?;

                        // Get the receiver's shared state memory.
                        let mut ancillary_buffer = [0; 128];
                        let io_buf = IoSliceMut::new(&mut []);
                        let (read_size, anc) = conn
                            .recv_vectored_with_ancillary(&mut [io_buf], &mut ancillary_buffer)
                            .await?;
                        if read_size != 0 {
                            return Err(InvalidData.into());
                        }

                        // Recover the shared receive state memory
                        let fd = fd_from_ancillary(anc)?;

                        let recv_file = MemFile::from_fd(fd)?;
                        let seals = recv_file.get_seals()?;
                        if seals != crate::seals() {
                            return Err(InvalidData.into());
                        }

                        if recv_file.metadata()?.len() > (1 << 16) {
                            return Err(InvalidData.into());
                        }

                        let recv_mem = unsafe { memmap2::Mmap::map(&recv_file)? };
                        if recv_mem.len() < SIZE_RECV_STATE {
                            return Err(InvalidData.into());
                        }

                        Ok(TrackReceiver {
                            mem: recv_mem,
                            conn,
                        })
                    }

                    let Ok(Ok(track)) =
                        timeout(Duration::from_millis(10), finish_conn(&mem_file, conn)).await
                    else {
                        continue;
                    };
                    let Ok(mut mutex) = sync.recv.lock() else {
                        break;
                    };
                    mutex.push(track);
                    drop(mutex);
                }
            }
        }));

        Ok(Self {
            mem,
            buf_head: 0,
            buf_tail: 0,
            sync,
            addr,
            close_listener,
        })
    }

    fn state(&mut self) -> &mut BroadIpcSendState {
        unsafe { &mut *(self.mem.as_mut_ptr() as *mut BroadIpcSendState) }
    }

    /// Access the next `len` bytes for writing to. Pauses until the buffer has
    /// that many contiguous bytes available.
    /// 
    /// # Panics
    /// 
    /// Panics if the requested length is larger than could ever fit in the
    /// send buffer.
    pub async fn prepare_message(&mut self, len: usize) -> &mut [u8] {
        todo!()
    }


    /// Submit a message of length `len`. Pauses until there is an open slot in
    /// the queue. In general, the sum of messages submitted with
    /// `submit_message` should be no more than the length requested from the
    /// last `prepare_message` call.
    /// 
    /// # Panics
    /// 
    /// Panics if the submitted message length is larger than the remaining free
    /// space in the buffer, or if it would go past the end of the buffer.
    pub async fn submit_message(&mut self, len: usize) {
        todo!()
    }
}

impl Drop for Broadcaster {
    fn drop(&mut self) {
        self.close_listener.cancel();
        let _ = std::fs::remove_file(&self.addr);
        self.state()
            .send_state
            .value
            .store(ST_CLOSED, Ordering::Release);
    }
}
