
struct Server;
impl Server {
    pub fn new(location: &str) -> Result<Self> { todo!() }
    pub async fn accept(&self) -> Result<Connecting> { todo!() }
}

pub async fn connect(location: &str, mem_size: usize, slots: usize) -> Result<(Sender, Receiver)> { todo!() }

struct Connecting;
impl Connecting {
    pub async fn complete(self, mem_size: usize, slots: usize) -> Result<(Sender, Receiver)> { todo!() }
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }
    pub fn peer_mem_size(&self) -> usize { todo!() }
    pub fn perr_slots(&self) -> usize { todo!() }
}

struct Sender;
impl Sender {
    pub async fn allocate(&mut self, max_len: usize) -> Result<SendMessage> { todo!() }
    pub async fn send(&mut self, message: &[u8]) -> Result<()> { todo!() }
    pub fn peer_cred(&self) -> Result<UCred> { todo!() }
}

// Note: Any data copied to SendMessage can be seen by the receiver, though it will be marked as 
// ignorable and a well-behaved receiver will skip it.
struct SendMessage;
impl SendMessage {
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
    pub async fn peek(&mut self) -> Result<RecvMessage> { todo!() }
    pub async fn receive(&mut self) -> Result<Vec<u8>> { todo!() }
}

// Received message that hasn't yet been copied out of the shared memory space.
// The content can be changed by a misbehaving sender, violating Rust's memory safety. Extracting 
// the data by copying it into a `Vec<u8>` is safe though.
struct RecvMessage;
impl RecvMessage {
    pub unsafe fn content(&self) -> &[u8] { todo!() }
    pub fn extract(self) -> Vec<u8> { todo!() }
}
