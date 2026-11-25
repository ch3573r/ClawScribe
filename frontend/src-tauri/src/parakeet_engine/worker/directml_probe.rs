use super::WorkerError;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::OnceLock;

const MARKER: &str = "parakeet-directml-probe-in-progress";

pub(super) fn resolve(
    cache: &OnceLock<bool>,
    app_data: &Path,
    probe: impl FnOnce() -> Result<bool, WorkerError>,
) -> bool {
    *cache.get_or_init(|| {
        let marker = app_data.join(MARKER);
        // Atomic creation also prevents two app instances from probing at
        // once. Flush before starting native code. Only a clean response can
        // clear the marker, so a crash, hang or interrupted app launch stays
        // on CPU by default next time.
        let pending = fs::create_dir_all(app_data).and_then(|()| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)?
                .sync_all()
        });
        if let Err(error) = pending {
            log::warn!(
                "Parakeet automatic DirectML probe skipped ({:?})",
                error.kind()
            );
            return false;
        }

        match probe() {
            Ok(available) => {
                if let Err(error) = fs::remove_file(&marker) {
                    log::warn!(
                        "Could not clear the Parakeet DirectML probe marker ({:?})",
                        error.kind()
                    );
                }
                available
            }
            Err(error) => {
                log::warn!("Parakeet automatic DirectML probe failed: {error}");
                false
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_probe_is_cached_and_clears_the_marker() {
        for available in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join(MARKER);
            let cache = OnceLock::new();
            assert_eq!(
                resolve(&cache, dir.path(), || {
                    assert!(marker.is_file());
                    Ok(available)
                }),
                available
            );
            assert!(!marker.exists());
            assert_eq!(
                resolve(&cache, dir.path(), || panic!("result should be cached")),
                available
            );
        }
    }

    #[test]
    fn interrupted_probe_leaves_a_marker_that_disables_the_next_launch() {
        for error in [WorkerError::TimedOut, WorkerError::Crashed] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join(MARKER);
            let cache = OnceLock::new();
            assert!(!resolve(&cache, dir.path(), || Err(error)));
            assert!(marker.is_file());
            assert!(!resolve(&cache, dir.path(), || panic!(
                "retry in this session"
            )));
            assert!(!resolve(&OnceLock::new(), dir.path(), || panic!(
                "retry next launch"
            )));
        }
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
