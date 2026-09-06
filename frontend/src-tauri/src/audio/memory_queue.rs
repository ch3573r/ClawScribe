//! Nonblocking producer queue with independent payload-byte and item limits.
//! On overload callers MUST report an incomplete recording, never hide a drop.
//! Accepted items remain drainable after all senders close.
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tokio::sync::{mpsc, oneshot};

pub trait QueuePayload { fn heap_bytes(&self) -> usize; }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError { Full, Closed }
impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self { Self::Full => "audio queue capacity exceeded", Self::Closed => "audio queue closed" })
    }
}
impl std::error::Error for QueueError {}
struct Reservation { bytes: usize, used: Arc<AtomicUsize> }
impl Drop for Reservation {
    fn drop(&mut self) { self.used.fetch_sub(self.bytes, Ordering::AcqRel); }
}
struct Envelope<T> { value: T, reservation: Reservation }
pub struct MemorySender<T> { sender: mpsc::Sender<Envelope<T>>, used: Arc<AtomicUsize>, limit: usize }
pub struct MemoryReceiver<T> { receiver: mpsc::Receiver<Envelope<T>> }
impl<T> Clone for MemorySender<T> {
    fn clone(&self) -> Self { Self { sender: self.sender.clone(), used: self.used.clone(), limit: self.limit } }
}
pub fn channel<T>(byte_limit: usize, item_limit: usize) -> (MemorySender<T>, MemoryReceiver<T>) {
    let (sender, receiver) = mpsc::channel(item_limit.max(1));
    (MemorySender { sender, used: Arc::new(AtomicUsize::new(0)), limit: byte_limit }, MemoryReceiver { receiver })
}
impl<T: QueuePayload> MemorySender<T> {
    /// Never waits for the consumer, storage or capacity (safe for capture paths).
    pub fn send(&self, value: T) -> Result<(), QueueError> {
        if self.sender.is_closed() { return Err(QueueError::Closed); }
        let bytes = value.heap_bytes().checked_add(std::mem::size_of::<Envelope<T>>()).ok_or(QueueError::Full)?;
        self.used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|total| *total <= self.limit)
        }).map_err(|_| QueueError::Full)?;
        let envelope = Envelope { value, reservation: Reservation { bytes, used: self.used.clone() } };
        self.sender.try_send(envelope).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => QueueError::Full,
            mpsc::error::TrySendError::Closed(_) => QueueError::Closed,
        })
    }
    pub fn pending_bytes(&self) -> usize { self.used.load(Ordering::Acquire) }
}
impl<T> MemoryReceiver<T> {
    fn unpack(envelope: Envelope<T>) -> T {
        let Envelope { value, reservation } = envelope;
        drop(reservation);
        value
    }
    pub async fn recv(&mut self) -> Option<T> { self.receiver.recv().await.map(Self::unpack) }
    pub fn blocking_recv(&mut self) -> Option<T> { self.receiver.blocking_recv().map(Self::unpack) }
}

/// Long-running encoding belongs on a dedicated thread, not an async executor.
/// Completion is observable: stop awaits this receipt rather than sleeping and
/// hoping accepted audio has been written.
pub fn spawn_drain_worker<T: Send + 'static>(
    mut receiver: MemoryReceiver<T>,
    mut consume: impl FnMut(T) -> Result<(), String> + Send + 'static,
) -> std::io::Result<oneshot::Receiver<Result<(), String>>> {
    let (sender, done) = oneshot::channel();
    std::thread::Builder::new().name("clawscribe-audio-writer".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Some(value) = receiver.blocking_recv() { consume(value)?; }
            Ok(())
        })).unwrap_or_else(|_| Err("Audio checkpoint worker panicked; recovery files have been retained".into()));
        let _ = sender.send(result);
    })?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    impl QueuePayload for Vec<u8> { fn heap_bytes(&self) -> usize { self.capacity() } }
    #[tokio::test]
    async fn limits_allocated_capacity_not_only_length() {
        let (sender, mut receiver) = channel(512, 10);
        let mut small = Vec::with_capacity(200); small.push(1);
        sender.send(small).unwrap();
        assert!(sender.pending_bytes() >= 200);
        assert_eq!(sender.send(Vec::with_capacity(400)), Err(QueueError::Full));
        assert_eq!(receiver.recv().await.unwrap(), vec![1]);
        assert_eq!(sender.pending_bytes(), 0);
        sender.send(Vec::with_capacity(400)).unwrap();
    }
    #[tokio::test]
    async fn item_limit_releases_failed_reservation() {
        let (sender, mut receiver) = channel(4096, 1);
        sender.send(vec![1]).unwrap();
        let pending = sender.pending_bytes();
        assert_eq!(sender.send(vec![2]), Err(QueueError::Full));
        assert_eq!(sender.pending_bytes(), pending);
        receiver.recv().await.unwrap();
        assert_eq!(sender.pending_bytes(), 0);
    }
    #[test]
    fn receiver_drop_releases_queued_memory() {
        let (sender, receiver) = channel(4096, 10);
        sender.send(vec![1; 100]).unwrap();
        drop(receiver);
        assert_eq!(sender.pending_bytes(), 0);
        assert_eq!(sender.send(vec![2]), Err(QueueError::Closed));
    }
    #[tokio::test]
    async fn slow_writer_drains_every_accepted_item_before_completion() {
        let (sender, receiver) = channel(16384, 100);
        let collected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let result = collected.clone();
        let done = spawn_drain_worker(receiver, move |item: Vec<u8>| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            result.lock().unwrap().push(item[0]); Ok(())
        }).unwrap();
        for i in 0..100u8 { sender.send(vec![i]).unwrap(); }
        drop(sender);
        done.await.unwrap().unwrap();
        assert_eq!(*collected.lock().unwrap(), (0..100u8).collect::<Vec<_>>());
    }
    #[tokio::test]
    async fn failed_writer_cannot_report_success_or_accept_more_audio() {
        let (sender, receiver) = channel(4096, 10);
        let done = spawn_drain_worker(receiver, |_item: Vec<u8>| Err("Disk full".to_string())).unwrap();
        sender.send(vec![1]).unwrap();
        assert_eq!(done.await.unwrap(), Err("Disk full".to_string()));
        assert_eq!(sender.send(vec![2]), Err(QueueError::Closed));
    }
    #[test]
    fn concurrent_producers_cannot_exceed_the_byte_budget() {
        let (sender, _receiver) = channel(8192, 1000);
        std::thread::scope(|scope| {
            for _ in 0..8 { let sender = sender.clone(); scope.spawn(move || {
                for _ in 0..100 { let _ = sender.send(vec![0; 100]); assert!(sender.pending_bytes() <= 8192); }
            }); }
        });
        assert!(sender.pending_bytes() <= 8192);
    }
}
