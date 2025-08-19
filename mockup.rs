/*!
Bidirectional shared-memory channel. A server is set up to initialize the connection, but otherwise 
sender and receiver are agnostic to who initiates the connection.

Supports bidirectional data transfer, though the connections do not need symmetric buffer sizes.
*/

struct Server;
impl Server {

    /// Initialize a new server with the UDS socket located at the given path.
    pub fn new(location: &str) -> Result<Self> { todo!() }

    /// Get the next connection attempt.
    pub async fn accept(&self) -> Result<Connecting> { todo!() }
}

/// Connect to a server.
pub async fn connect(location: &str, mem_size: usize, slots: usize) -> Result<(Sender, Receiver)> { todo!() }

/// Take a raw UDS socket file descriptor and attempt to make it into a shared-memory channel.
///
/// This can be helpful if using [`UnixSeqpakcet::pair`][uds_pair] to create a socket pair, with 
/// one of ends being handed off to a remote or child process before starting the connection.
///
/// [uds_pair]: https://docs.rs/tokio-seqpacket/latest/tokio_seqpacket/struct.UnixSeqpacket.html#method.pair
pub async fn connect_raw_fd(fd: RawFd) -> Result<(Sender, Receiver)> { todo!() }

struct Connecting;
impl Connecting {
    pub async fn complete(self, mem_size: usize, slots: usize) -> Result<(Sender, Receiver)> { todo!() }
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }
    pub fn peer_mem_size(&self) -> usize { todo!() }
    pub fn perr_slots(&self) -> usize { todo!() }
}

struct Sender;
impl Sender {

    /// Allocate space for a new message. The maximum size must be known in advance.
    pub async fn allocate(&mut self, max_len: usize) -> Result<SendMessage> { todo!() }

    /// Send a message that's already built, copying it into shared memory.
    pub async fn send(&mut self, message: &[u8]) -> Result<()> { todo!() }

    /// Get the peer credentials of the receiver.
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }
}

// Note: Any data copied to SendMessage can be seen by the receiver, though it will be marked as 
// ignorable and a well-behaved receiver will skip it.
struct SendMessage;
impl SendMessage {
    /// Raw mutable pointer to the allocated memory space
    pub fn as_mut_ptr(&mut self) -> *mut u8 { todo!() }

    /// Raw pointer to the allocated memory space
    pub fn as_ptr(&self) -> *const u8 { todo!() }

    /// Maximum size that this message allocation can hold.
    pub fn max_size(&self) -> usize { todo!() }

    /// Current size of the message being built in this allocation.
    pub fn len(&self) -> usize { todo!() }

    /// Send the message.
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
    /// Get the next message without immediately copying it out.
    pub async fn peek(&mut self) -> Result<RecvMessage> { todo!() }

    /// Copy out the next message.
    pub async fn receive(&mut self) -> Result<Vec<u8>> { todo!() }

    /// Get the sender's peer credentials.
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }
}

// Received message that hasn't yet been copied out of the shared memory space.
// The content can be changed by a misbehaving sender, violating Rust's memory safety. Extracting 
// the data by copying it into a `Vec<u8>` is safe though.
struct RecvMessage;
impl RecvMessage {
    /// Raw message data, still in the shared memory space.
    pub unsafe fn content(&self) -> &[u8] { todo!() }

    /// Copy out the message data.
    pub fn extract(self) -> Vec<u8> { todo!() }
}

