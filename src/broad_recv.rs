use std::{io, path::Path, sync::atomic::Ordering};

use tokio_seqpacket::UnixSeqpacket;

use crate::{
    BroadIpcRecvState, BroadIpcSendState, MessageHeader, SIZE_HEADER, SIZE_RECV_STATE,
    SIZE_SEND_STATE, ST_ACTIVE, ST_CLOSED, ST_WAITING, new_mem, recv_mem, send_memfile,
};

/// Receiver side of a shared-mem Broadcasting IPC channel.
pub struct BroadReceiver {
    /// Receive state
    state: memmap2::MmapMut,
    /// Shared memory from sender
    send: memmap2::Mmap,
    /// Queue depth, which we retrieved early from the sender's buffer.
    queue_depth: usize,
    /// The open connection to the sender
    conn: UnixSeqpacket,
}

impl BroadReceiver {
    pub async fn open<P: AsRef<Path>>(address: P) -> std::io::Result<Self> {
        let conn = UnixSeqpacket::connect(address).await?;

        let send = recv_mem(&conn).await?;

        let (mem_file, mut state) = new_mem(SIZE_RECV_STATE)?;

        // Load our initial config from the send state.
        let ipc = unsafe { &mut *(state.as_mut_ptr() as *mut BroadIpcRecvState) };
        let send_state = unsafe { &*(send.as_ptr() as *const BroadIpcSendState) };
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
        if send.len() < (SIZE_SEND_STATE + queue_depth * SIZE_HEADER) {
            return Err(io::Error::other(
                "memory map from sender is too small for buffer and state",
            ));
        }

        send_memfile(&conn, &mem_file).await?;

        Ok(Self {
            state,
            send,
            queue_depth,
            conn,
        })
    }

    fn state(&mut self) -> &mut BroadIpcRecvState {
        unsafe { &mut *(self.state.as_mut_ptr() as *mut BroadIpcRecvState) }
    }

    fn send_state(&self) -> &BroadIpcSendState {
        unsafe { &*(self.send.as_ptr() as *const &BroadIpcSendState) }
    }

    /// Determine if the sender is closed.
    pub fn is_closed(&self) -> bool {
        self.send_state().send_state.value.load(Ordering::Acquire) == ST_CLOSED
    }

    /// Receive a message. Returns None if the sender has closed the channel.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        let ret = unsafe { self.peek_raw() }.await.map(Vec::from);
        self.advance().await;
        ret
    }

    /// Peek the next message, and directly expose the byte buffer.
    ///
    /// Returns None if the sender has closed the channel.
    ///
    /// # SAFETY
    ///
    /// There's no guarantee the sending process won't modify the message after
    /// it's been sent. You either must totally trust the sending process to
    /// behave well, or be able to not care if the data is changed as you're
    /// reading it.
    pub async unsafe fn peek_raw(&mut self) -> Option<&[u8]> {
        // 1. Set our state to waiting
        // 2. Check the head & tail
        // 3. If equal, no messages available. Sleep and wait for a UDS message (or failure).
        // 4. After message, check tail again. If no change, go back to 3.
        // 5. Messages available. Set our state to IDLE.
        // 6. Try reading off the UDS connection until no messages are left.
        // 7. Read a message header by copying it, verifying it, and then
        //    returning a slice of memory pointing to the message.

        let head = self.state().head.value.load(Ordering::Relaxed);
        let tail = self.send_state().tail.value.load(Ordering::Acquire);
        todo!()
    }

    /// Advance the receiver by one message. Panics if there was no pending message.
    ///
    /// This should be used to advance after using [`peek_raw`][Self::peek_raw].
    pub async fn advance(&mut self) {
        // Check that we can actually advance the head pointer, then do so.
        let tail = self.send_state().tail.value.load(Ordering::Acquire);
        let mut head = self.state().head.value.load(Ordering::Relaxed);
        head += 1;
        if head > tail {
            panic!("Tried advancing past the end of the queue");
        }
        self.state().head.value.store(head, Ordering::Release);

        // We might have held up the sender. Check it it went inactive, and wake it if so.
        let send_state = self.send_state().send_state.value.load(Ordering::Acquire);
        if send_state == ST_WAITING {
            // TODO: Should we shut down the receiver on a failed send?
            let _ = self.conn.send(&[]).await;
        }
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
