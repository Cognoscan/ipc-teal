use std::{io, path::Path};

use memmap2::{Mmap, MmapMut};
use tokio::io::Interest;
use tokio_seqpacket::{UCred, UnixSeqpacket, UnixSeqpacketListener};

use crate::{
    IpcState, MemQueue, MutMemQueue, SIZE_STATE, ST_ACTIVE, ST_CLOSED, ST_WAITING, WAKE_SEND,
    recv_mem, send_memfile,
};

pub struct Server {
    listener: UnixSeqpacketListener,
}

impl Server {
    /// Create a new server.
    pub fn new<P: AsRef<Path>>(address: P) -> io::Result<Self> {
        let listener = UnixSeqpacketListener::bind(address.as_ref())?;
        Ok(Self { listener })
    }

    /// Accept the next connection request.
    pub async fn accept(&mut self) -> io::Result<Connecting> {
        let conn = self.listener.accept().await?;
        Ok(Connecting { conn })
    }
}

/// A open connection request. Completing it will create an active connection.
pub struct Connecting {
    conn: UnixSeqpacket,
}

impl Connecting {
    /// Get the peer credentials of the process attempting to connect.
    pub fn peer_cred(&self) -> io::Result<UCred> {
        self.conn.peer_cred()
    }

    /// Complete the handshake and open a channel.
    pub async fn complete(self, buf_size: usize, queue_depth: usize) -> io::Result<Channel> {
        // Get the remote
        let remote = recv_mem(&self.conn).await?;
        let remote = MemQueue::from_mem(remote)?;

        // set up our own fd
        let (fd, local) = MutMemQueue::new(buf_size, queue_depth)?;
        send_memfile(&self.conn, &fd).await?;

        Ok(Channel {
            conn: self.conn,
            local,
            remote,
        })
    }
}

pub struct Channel {
    conn: UnixSeqpacket,
    local: MutMemQueue,
    remote: MemQueue,
}

impl Channel {
    /// Get the peer credentials of the connected-to process.
    pub fn peer_cret(&self) -> io::Result<UCred> {
        self.conn.peer_cred()
    }

    /// Try to get a buffer for the next message.
    ///
    /// The final message length can be smaller.
    pub fn try_reserve(&mut self, len: usize) -> io::Result<Option<&mut [u8]>> {
        // Check for closure
        let head = self.remote.other_head();
        Ok(self.local.try_reserve(head, len))
    }

    /// Submit the next message.
    ///
    /// Errors if the other end is closed.
    pub async fn submit(&mut self, len: usize) -> io::Result<()> {
        self.local.write_message(len);
        match self.remote.other_state() {
            ST_CLOSED => return Err(io::ErrorKind::BrokenPipe.into()),
            ST_WAITING => {
                self.conn.send(&[WAKE_SEND]).await?;
            }
            _ => (),
        }
        Ok(())
    }

    pub async fn reserve(&mut self, len: usize) -> io::Result<&mut [u8]> {
        // See if we're not blocked
        let head = self.remote.other_head();
        if let Some(buf) = self.local.try_reserve(head, len) {
            return Ok(buf);
        }
        // warn the other process that we might be suspended
        self.local.set_state(ST_WAITING);
        // Try one more time before suspending on a UDS packet
        // Wait until we're woken up by the packet, check

        // We're done and don't need to be woken up anymore
        self.local.set_state(ST_ACTIVE);
        todo!()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.local.set_state(ST_CLOSED);
        self.local.set_other_state(ST_CLOSED);
    }
}
