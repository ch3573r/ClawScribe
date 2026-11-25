#[cfg(any(test, all(windows, feature = "directml")))]
use super::WorkerError;
use std::fs::{self, OpenOptions};
use std::io;
#[cfg(any(test, all(windows, feature = "directml")))]
use std::io::Write;
use std::path::Path;
#[cfg(any(test, all(windows, feature = "directml")))]
use std::sync::OnceLock;

const MARKER: &str = "parakeet-directml-probe-in-progress";
#[cfg(any(test, all(windows, feature = "directml")))]
const PENDING: &str = "parakeet-directml-probe-running";
#[cfg(any(test, all(windows, feature = "directml")))]
const TIMEOUTS: &str = "parakeet-directml-probe-timeouts";
#[cfg(any(test, all(windows, feature = "directml")))]
const UNFINISHED: &str = "parakeet-directml-probe-unfinished";
#[cfg(any(test, all(windows, feature = "directml")))]
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) fn record_runtime_failure(app_data: &Path) -> io::Result<()> {
    fs::create_dir_all(app_data)?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(app_data.join(MARKER))
    {
        Ok(marker) => marker.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(any(test, all(windows, feature = "directml")))]
fn remove_marker(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(any(test, all(windows, feature = "directml")))]
fn read_streak(path: &Path) -> io::Result<u8> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let Some(data) = data.strip_prefix(b"version=") else {
        return Ok(0); // Legacy counters were unversioned.
    };
    let mut fields = data.splitn(2, |&byte| byte == b'\n');
    let (Some(version), Some(count)) = (fields.next(), fields.next()) else {
        return Ok(2);
    };
    if version.is_empty() || std::str::from_utf8(version).is_err() {
        return Ok(2);
    }
    if version != APP_VERSION.as_bytes() {
        return Ok(0);
    }
    Ok(std::str::from_utf8(count)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(2))
}

#[cfg(any(test, all(windows, feature = "directml")))]
fn write_streak(app_data: &Path, path: &Path, count: u8) -> io::Result<()> {
    let mut counter = tempfile::NamedTempFile::new_in(app_data)?;
    writeln!(counter, "version={APP_VERSION}\n{count}")?;
    counter.as_file().sync_all()?;
    counter.persist(path)?;
    Ok(())
}

#[cfg(any(test, all(windows, feature = "directml")))]
fn run_probe(
    app_data: &Path,
    probe: impl FnOnce() -> Result<bool, WorkerError>,
) -> io::Result<bool> {
    // Keep the durable failure marker separate from pending/retry state: one
    // instance completing a probe must never clear another's runtime failure.
    let marker = app_data.join(MARKER);
    if marker.try_exists()? {
        return Ok(false);
    }
    fs::create_dir_all(app_data)?;
    // Keep a stable file to lock. The OS releases its lock on every exit, and
    // a pending file left by an older build can also be locked and reused.
    let pending = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(app_data.join(PENDING))?;
    pending.try_lock().map_err(io::Error::from)?;

    let timeouts = app_data.join(TIMEOUTS);
    let unfinished = app_data.join(UNFINISHED);
    let count = read_streak(&timeouts)?;
    let interrupted = read_streak(&unfinished)?;
    let available = if count >= 2 || interrupted >= 2 || marker.try_exists()? {
        false
    } else {
        // Publish the counter atomically while holding the stable probe lock.
        // A parent that never returns from the probe leaves this increment behind.
        write_streak(app_data, &unfinished, interrupted + 1)?;
        let available = match probe() {
            Ok(available) => {
                remove_marker(&timeouts)?;
                available
            }
            Err(error) => {
                match error {
                    // The short deadline includes starting the child process.
                    WorkerError::TimedOut => {
                        write_streak(app_data, &timeouts, count + 1)?;
                        log::warn!("Parakeet automatic DirectML probe timed out");
                    }
                    WorkerError::Crashed => {
                        record_runtime_failure(app_data)?;
                        log::warn!("Parakeet automatic DirectML probe worker crashed");
                    }
                    _ => {
                        remove_marker(&timeouts)?;
                        log::warn!("Parakeet automatic DirectML probe did not complete");
                    }
                }
                false
            }
        };
        remove_marker(&unfinished)?;
        available
    };
    Ok(available && !marker.try_exists()?)
}

#[cfg(any(test, all(windows, feature = "directml")))]
pub(super) fn resolve(
    cache: &OnceLock<bool>,
    app_data: &Path,
    probe: impl FnOnce() -> Result<bool, WorkerError>,
) -> bool {
    *cache.get_or_init(|| {
        run_probe(app_data, probe).unwrap_or_else(|error| {
            log::warn!(
                "Parakeet automatic DirectML probe skipped ({:?})",
                error.kind()
            );
            false
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_ignores_legacy_and_other_version_streaks_but_keeps_crash_marker() {
        for old in ["", "2\n", "version=0.0.0\n2\n"] {
            let dir = tempfile::tempdir().unwrap();
            for name in [TIMEOUTS, UNFINISHED] {
                fs::write(dir.path().join(name), old).unwrap();
            }
            assert!(run_probe(dir.path(), || Ok(true)).unwrap());
            record_runtime_failure(dir.path()).unwrap();
            assert!(
                !run_probe(dir.path(), || panic!("durable failure must block probes")).unwrap()
            );
        }
    }

    #[test]
    fn current_version_streaks_replace_atomically_and_corruption_blocks_probing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TIMEOUTS);
        assert!(!run_probe(dir.path(), || Err(WorkerError::TimedOut)).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("version={}\n1\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(!run_probe(dir.path(), || Err(WorkerError::TimedOut)).unwrap());
        assert_eq!(read_streak(&path).unwrap(), 2);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2); // Stable lock and timeout counter.

        for corrupt in [
            format!("version={APP_VERSION}\nbroken\n"),
            format!("version={APP_VERSION}"),
        ] {
            fs::write(&path, corrupt).unwrap();
            assert!(!run_probe(dir.path(), || panic!(
                "corrupt current state must block probes"
            ))
            .unwrap());
        }
    }

    #[test]
    fn completed_probe_is_cached_and_clears_the_marker() {
        for available in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join(PENDING);
            let cache = OnceLock::new();
            assert_eq!(
                resolve(&cache, dir.path(), || {
                    assert!(marker.is_file());
                    Ok(available)
                }),
                available
            );
            assert!(resolve(&OnceLock::new(), dir.path(), || Ok(true)));
            assert_eq!(
                resolve(&cache, dir.path(), || panic!("result should be cached")),
                available
            );
        }
    }

    #[test]
    fn crashed_probe_blocks_automatic_probing_on_later_launches() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OnceLock::new();
        assert!(!resolve(&cache, dir.path(), || Err(WorkerError::Crashed)));
        assert!(dir.path().join(MARKER).is_file());
        assert!(!resolve(&OnceLock::new(), dir.path(), || panic!(
            "retry next launch"
        )));
    }

    #[test]
    fn pending_probe_blocks_a_concurrent_probe_and_preserves_a_runtime_failure() {
        let dir = tempfile::tempdir().unwrap();
        let app_data = dir.path();
        assert!(!resolve(&OnceLock::new(), app_data, || {
            assert!(!resolve(&OnceLock::new(), app_data, || panic!(
                "concurrent probe"
            )));
            record_runtime_failure(app_data).unwrap();
            Ok(true)
        }));
        assert!(!resolve(&OnceLock::new(), app_data, || panic!(
            "runtime failure persisted"
        )));
    }

    #[test]
    fn only_two_consecutive_timeouts_block_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        let probe = |result| resolve(&OnceLock::new(), dir.path(), || result);
        assert!(!probe(Err(WorkerError::TimedOut)));
        assert!(probe(Ok(true)));
        for _ in 0..2 {
            assert!(!resolve(&OnceLock::new(), dir.path(), || {
                Err(WorkerError::TimedOut)
            }));
        }
        assert!(!resolve(&OnceLock::new(), dir.path(), || panic!(
            "a third automatic probe should be blocked"
        )));
    }

    #[test]
    fn spawn_failure_clears_pending_probe_and_resets_timeout_streak() {
        let dir = tempfile::tempdir().unwrap();
        let probe = |result| resolve(&OnceLock::new(), dir.path(), || result);
        assert!(!probe(Err(WorkerError::TimedOut)));
        assert!(!resolve(&OnceLock::new(), dir.path(), || {
            assert!(dir.path().join(PENDING).is_file());
            Err(WorkerError::Spawn)
        }));
        assert!(!dir.path().join(MARKER).exists());
        assert!(!probe(Err(WorkerError::TimedOut)));
        assert!(probe(Ok(true)));
    }

    #[test]
    fn two_unfinished_attempts_block_automatic_probing_on_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let result = std::panic::catch_unwind(|| {
                run_probe(dir.path(), || panic!("parent stopped during probe"))
            });
            assert!(result.is_err());
        }
        assert!(!run_probe(dir.path(), || panic!("third probe should be blocked")).unwrap());
    }

    #[test]
    fn a_finished_attempt_clears_the_unfinished_attempt_streak() {
        let dir = tempfile::tempdir().unwrap();
        for completed in [
            Ok(true),
            Err(WorkerError::TimedOut),
            Err(WorkerError::Spawn),
        ] {
            let result = std::panic::catch_unwind(|| {
                run_probe(dir.path(), || panic!("parent stopped during probe"))
            });
            assert!(result.is_err());
            let available = completed == Ok(true);
            assert_eq!(run_probe(dir.path(), || completed).unwrap(), available);
            assert!(!dir.path().join(UNFINISHED).exists());
        }
        assert!(run_probe(dir.path(), || Ok(true)).unwrap());
    }

    #[test]
    fn a_stale_pending_file_does_not_disable_later_probes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(PENDING), []).unwrap();
        assert!(resolve(&OnceLock::new(), dir.path(), || Ok(true)));
    }

    #[test]
    fn io_failure_does_not_prevent_a_later_probe() {
        let dir = tempfile::tempdir().unwrap();
        let timeouts = dir.path().join(TIMEOUTS);
        fs::create_dir(&timeouts).unwrap();
        assert!(!resolve(&OnceLock::new(), dir.path(), || panic!(
            "unreadable retry state"
        )));
        fs::remove_dir(timeouts).unwrap();
        assert!(resolve(&OnceLock::new(), dir.path(), || Ok(true)));
    }

    #[test]
    fn existing_marker_or_unwritable_path_prevents_starting_a_probe() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MARKER);
        std::fs::write(&marker, []).unwrap();
        for path in [dir.path(), marker.as_path()] {
            assert!(!resolve(&OnceLock::new(), path, || panic!(
                "unsafe to probe"
            )));
        }
    }
}
