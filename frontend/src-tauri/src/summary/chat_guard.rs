//! Only one send/clear mutation per meeting. The guard covers provider work,
//! but try_lock returns immediately rather than making another UI action wait.
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

static ACTIVE: Lazy<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn begin(meeting_id: &str) -> Result<OwnedMutexGuard<()>, String> {
    let lock = {
        let mut active = ACTIVE.lock().map_err(|_| "Chat operation registry unavailable")?;
        // Expired meetings do not accumulate in the registry across long sessions.
        active.retain(|_, lock| lock.strong_count() > 0);
        match active.get(meeting_id).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(AsyncMutex::new(()));
                active.insert(meeting_id.to_string(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.try_lock_owned().map_err(|_| "An answer or clear operation is already running for this meeting. Please wait for it to finish.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_meeting_cannot_send_or_clear_concurrently() {
        let guard = begin("guard-same").unwrap();
        assert!(begin("guard-same").is_err());
        drop(guard);
        assert!(begin("guard-same").is_ok());
    }
    #[test]
    fn different_meetings_do_not_block_each_other() {
        let _first = begin("guard-first").unwrap();
        let _second = begin("guard-second").unwrap();
    }
}
