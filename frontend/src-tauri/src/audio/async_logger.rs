use log::{Level, Record};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Best-effort diagnostics for performance-critical audio processing.
/// Bound both the queue and batch; overload drops logs, never audio samples.
pub struct AsyncLogger {
    sender: mpsc::Sender<LogMessage>,
    _handle: JoinHandle<()>,
}

#[derive(Debug)]
struct LogMessage {
    level: Level,
    target: String,
    message: String,
}

impl AsyncLogger {
    /// Create a bounded logger. A zero-sized request is clamped to one message.
    pub fn new(buffer_size: usize) -> Self {
        let capacity = buffer_size.max(1);
        let (sender, mut receiver) = mpsc::channel::<LogMessage>(capacity);
        let handle = tokio::spawn(async move {
            let mut buffered_messages = Vec::with_capacity(capacity);
            let mut flush_interval = tokio::time::interval(Duration::from_millis(100));
            flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // interval's first tick is immediate; subsequent ticks flush quiet batches.
            flush_interval.tick().await;

            loop {
                tokio::select! {
                    message = receiver.recv() => {
                        match message {
                            Some(message) => {
                                buffered_messages.push(message);
                                if buffered_messages.len() >= capacity {
                                    Self::flush_messages(&mut buffered_messages);
                                }
                            }
                            None => break,
                        }
                    }
                    _ = flush_interval.tick() => {
                        Self::flush_messages(&mut buffered_messages);
                    }
                }
            }
            Self::flush_messages(&mut buffered_messages);
        });
        Self { sender, _handle: handle }
    }

    /// Never wait for log I/O or queue space on the audio producer thread.
    pub fn log(&self, level: Level, target: &str, message: String) {
        if !log::log_enabled!(target: target, level) {
            return;
        }
        let _ = self.sender.try_send(LogMessage {
            level,
            target: target.to_string(),
            message,
        });
    }

    fn flush_messages(messages: &mut Vec<LogMessage>) {
        for msg in messages.drain(..) {
            // Respect current filtering even if configuration changed while queued.
            if log::log_enabled!(target: &msg.target, msg.level) {
                log::logger().log(
                    &Record::builder()
                        .args(format_args!("{}", msg.message))
                        .level(msg.level)
                        .target(&msg.target)
                        .build(),
                );
            }
        }
    }
}

static ASYNC_LOGGER: once_cell::sync::OnceCell<Arc<AsyncLogger>> = once_cell::sync::OnceCell::new();

pub fn init_async_logger() {
    let _ = get_async_logger();
}

pub fn get_async_logger() -> Option<Arc<AsyncLogger>> {
    if let Some(logger) = ASYNC_LOGGER.get() {
        return Some(Arc::clone(logger));
    }
    if tokio::runtime::Handle::try_current().is_err() {
        return None;
    }
    Some(Arc::clone(ASYNC_LOGGER.get_or_init(|| Arc::new(AsyncLogger::new(1000)))))
}

// Check filtering before allocating formatted strings or initializing the logger.
#[macro_export]
macro_rules! async_debug {
    ($($arg:tt)*) => {
        if log::log_enabled!(log::Level::Debug) {
            if let Some(logger) = $crate::audio::async_logger::get_async_logger() {
                logger.log(log::Level::Debug, module_path!(), format!($($arg)*));
            }
        }
    };
}

#[macro_export]
macro_rules! async_info {
    ($($arg:tt)*) => {
        if log::log_enabled!(log::Level::Info) {
            if let Some(logger) = $crate::audio::async_logger::get_async_logger() {
                logger.log(log::Level::Info, module_path!(), format!($($arg)*));
            }
        }
    };
}

#[macro_export]
macro_rules! async_warn {
    ($($arg:tt)*) => {
        if log::log_enabled!(log::Level::Warn) {
            if let Some(logger) = $crate::audio::async_logger::get_async_logger() {
                logger.log(log::Level::Warn, module_path!(), format!($($arg)*));
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> LogMessage {
        LogMessage { level: Level::Info, target: "test".into(), message: "test".into() }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn logging_queue_is_bounded_and_never_waits_for_capacity() {
        // No await: the consumer cannot drain the queue before these assertions.
        let logger = AsyncLogger::new(2);
        assert!(logger.sender.try_send(message()).is_ok());
        assert!(logger.sender.try_send(message()).is_ok());
        assert!(matches!(logger.sender.try_send(message()), Err(mpsc::error::TrySendError::Full(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn zero_capacity_is_safe() {
        let logger = AsyncLogger::new(0);
        assert!(logger.sender.try_send(message()).is_ok());
        assert!(matches!(logger.sender.try_send(message()), Err(mpsc::error::TrySendError::Full(_))));
    }
}
