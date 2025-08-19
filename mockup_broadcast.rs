/*!
Broadcast channel.

A sender can broadcast messages to any number of attached receivers. The sender will never block on 
slow receivers. A receiver, however, will be able to detect if it lagged behind and can catch up 
without losing the connection.
*/

struct Server;
impl Server {

    /// Initialize a broadcaster.
    pub fn new(location: &str, mem_size: usize, slots: usize) -> Result<(Self, Sender)> { todo!() }

    /// Get the next connection attempt.
    pub async fn accept(&self) -> Result<Connecting> { todo!() }

    /// Take a raw UDS socket file descriptor and attempt to connect it to the broadcaster.
    pub async fn connect_raw_fd(&self, fd: RawFd) -> Result<()> { todo!() }

    /// Get a list of all attached peers.
    pub fn peers(&self) -> Result<Vec<UCred>> { todo!() }
}

/// Connect to a broadcaster.
pub async fn connect(location: &str) -> Result<Receiver> { todo!() }

struct Connecting;
impl Connecting {
    /// Allow the connection to complete.
    pub async fn complete(self) -> Result<()> { todo!() }

    /// Get peer credentials
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }

    /// Get a list of all already attached peers.
    pub fn peers(&self) -> Result<Vec<UCred>> { todo!() }
}

struct Sender;
impl Sender {

    /// Allocate space for a message, but don't send it yet.
    pub fn allocate(&mut self, max_len: usize) -> Result<SendMessage> { todo!() }

    /// Copy a message into shared memory and send it.
    pub fn send(&mut self, message: &[u8]) -> Result<()> { todo!() }

    /// Get a list of all attached peers.
    pub fn peers(&self) -> Result<Vec<UCred>> { todo!() }
}


struct SendMessage {
    pub fn as_mut_ptr(&mut self) -> *mut u8 { todo!() }
    pub fn as_ptr(&self) -> *const u8 { todo!() }
    pub fn max_size(&self) -> usize { todo!() }
    pub fn len(&self) -> usize { todo!() }
    pub fn send(self) -> Result<()> { todo!() }
}

impl std::io::Write for SendMessage {
    fn write(&mut self, buf: &[u8]) -> Result<usize> { todo!() }
    fn flush(&mut self) -> Result<()> { Ok(()) }
}

// Dropping must update the slot and advance the pointer too, indicating the slot isn't actually a 
// real message.
impl Drop for SendMessage {
    fn drop(&mut self) { todo!() }
}

// Probably implement AsRef, AsMut, Deref, and DerefMut too for SendMessage

struct Receiver;
impl Receiver {
    /// Peek the next message but don't immediately copy it out.
    pub async fn peek(&mut self) -> Result<RecvMessage> { todo!() }

    /// Receive the next message, copying it out to a Vec<u8>.
    pub async fn receive(&mut self) -> Result<Vec<u8>> { todo!() }

    /// Indicate if a lag has happened since the last reset
    pub fn lagged(&self) -> bool { todo!() }

    /// Reset the receiver by consuming all pending messages and catching up to the sender.
    pub fn reset(&mut self) { todo!() }
}

// Received message that hasn't yet been copied out of the shared memory space.
// The content can be changed by a sender at any time, but a well-behaved sender will indicate if 
// the message was overwritten (eg. the receive lagged and took too long) while `RecvMessage` is 
// held. Ill-behaved ones may change the data without indicating an overwrite.
struct RecvMessage;
impl RecvMessage {

    /// Access the raw message data.
    pub unsafe fn content(&self) -> &[u8] { todo!() }

    /// Copy the data out, failing if we lagged.
    pub fn extract(self) -> Option<Vec<u8>> { todo!() }

    /// Atomically check for overwrite (lag).
    pub fn lagged(&self) -> bool { todo!() }
}
