use std::{
    ffi::c_void,
    os::fd::{IntoRawFd, OwnedFd},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

const ST_ACTIVE: usize = 0;
const ST_WAITING: usize = 1;
const ST_CLOSED: usize = 2;

const SIZE_RECV_STATE: usize = core::mem::size_of::<BroadIpcRecvState>();
const SIZE_SEND_STATE: usize = core::mem::size_of::<BroadIpcSendState>();
const SIZE_HEADER: usize = core::mem::size_of::<MessageHeader>();

#[inline]
fn seals() -> Seals {
    Seal::FutureWrite | Seal::Grow | Seal::Shrink | Seal::Seal
}

use memfile::{Seal, Seals};

mod broad_recv;
mod broadcast;

pub use broad_recv::BroadReceiver;
pub use broadcast::Broadcaster;
use tokio_seqpacket::ancillary::{AncillaryMessageReader, OwnedAncillaryMessage};

/// Simplest cache padding - nobody goes above 128 byte alignment. Could be
/// swapped for the one in crossbeam-utils later.
#[repr(C, align(128))]
#[derive(Debug)]
struct CachePadded<T> {
    pub value: T,
}

/// Message Data
#[repr(C)]
#[derive(Debug)]
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
    head: CachePadded<AtomicUsize>,
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
    tail: CachePadded<AtomicUsize>,
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

fn fd_from_ancillary(anc: AncillaryMessageReader) -> std::io::Result<OwnedFd> {
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
        return Err(std::io::ErrorKind::InvalidData.into());
    };
    if err {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(fd)
}
