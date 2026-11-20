//! Free-space checks for the volume the recordings are written to.
//!
//! While a recording runs, the saver spools the mixed capture to disk as raw
//! f32 mono chunks at 48 kHz (`recording_saver`, `.audio-spool`). When the
//! recording stops, that spool is encoded to AAC at 192 kb/s (`encode.rs`) and
//! the spool is only deleted after the encoded file is published, so both
//! exist at the same time. Per hour of meeting that is:
//!
//! * spool: 48 000 samples/s x 4 bytes x 3600 s = 691.2 MB
//! * encoded: 192 000 bit/s / 8 x 3600 s = 86.4 MB
//! * peak: 777.6 MB
//!
//! The transcription queue's VAD segments are staged separately and are not
//! counted here.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) const SPOOL_BYTES_PER_HOUR: u64 = 48_000 * 4 * 3600;
pub(crate) const ENCODED_BYTES_PER_HOUR: u64 = 192_000 / 8 * 3600;
pub(crate) const PEAK_BYTES_PER_HOUR: u64 = SPOOL_BYTES_PER_HOUR + ENCODED_BYTES_PER_HOUR;

/// A recording does not start with less than the peak need of a two-hour
/// meeting free (1.56 GB). Two hours covers an ordinary long meeting; a
/// longer one is caught by the warning below long before audio is lost.
pub(crate) const START_FLOOR_BYTES: u64 = 2 * PEAK_BYTES_PER_HOUR;

/// While recording, warn once when less than one more hour of capture
/// (including its encode) fits (777.6 MB), early enough to free space or stop.
pub(crate) const WARNING_BYTES: u64 = PEAK_BYTES_PER_HOUR;

/// How often a running recording looks at free space.
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Reading the volume list can stall on a disconnected network or card-reader
/// drive. Past this the free space is treated as unknown and nothing is blocked.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

fn format_size(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
}

/// The refusal shown when a recording would start on a nearly full volume.
/// Unknown free space (`None`) never blocks a recording.
pub(crate) fn start_refusal(free: Option<u64>, folder: &Path) -> Option<String> {
    let free = free?;
    (free < START_FLOOR_BYTES).then(|| {
        format!(
            "Not enough free disk space to record. {} is free where recordings are saved ({}); at least {} is needed. Free up space or choose another recordings folder in Settings.",
            format_size(free),
            folder.display(),
            format_size(START_FLOOR_BYTES)
        )
    })
}

/// Removes the Windows extended-length prefix that `canonicalize` adds, so the
/// path compares with volume mount points such as `C:\`. Network paths
/// (`\\?\UNC\...`) are returned unchanged and match no local volume.
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Free bytes of the volume with the longest mount point containing `path`.
fn pick_free_bytes(path: &Path, volumes: &[(PathBuf, u64)]) -> Option<u64> {
    volumes
        .iter()
        .filter(|(mount, _)| path.starts_with(mount))
        .max_by_key(|(mount, _)| mount.components().count())
        .map(|(_, free)| *free)
}

/// Free bytes where `folder` is (or would be created). Blocking. `None` when
/// the volume cannot be determined, for example a network share.
pub(crate) fn free_space(folder: &Path) -> Option<u64> {
    let existing = folder.ancestors().find(|ancestor| ancestor.exists())?;
    let resolved = strip_verbatim_prefix(&existing.canonicalize().ok()?);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let volumes: Vec<(PathBuf, u64)> = disks
        .iter()
        .map(|disk| (disk.mount_point().to_path_buf(), disk.available_space()))
        .collect();
    pick_free_bytes(&resolved, &volumes)
}

/// [`free_space`] off the async runtime and bounded in time.
pub(crate) async fn free_space_bounded(folder: &Path) -> Option<u64> {
    let folder = folder.to_path_buf();
    tokio::time::timeout(
        QUERY_TIMEOUT,
        tokio::task::spawn_blocking(move || free_space(&folder)),
    )
    .await
    .ok()?
    .ok()?
}

/// Refuses a recording that cannot fit a minimum meeting.
pub(crate) async fn ensure_space_to_record(folder: &Path) -> Result<(), String> {
    match start_refusal(free_space_bounded(folder).await, folder) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// Throttles the free-space check of one recording and reports low space once.
pub(crate) struct DiskWatch {
    last_check: Instant,
    warned: bool,
}

impl DiskWatch {
    /// The start check has just run, so the first periodic check is one
    /// interval away.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            last_check: now,
            warned: false,
        }
    }

    /// True when a check is due; records it.
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last_check) >= CHECK_INTERVAL {
            self.last_check = now;
            true
        } else {
            false
        }
    }

    /// Some(free bytes) exactly once per recording, when free space is below
    /// the warning threshold. Unknown free space never warns.
    pub(crate) fn evaluate(&mut self, free: Option<u64>) -> Option<u64> {
        let free = free?;
        if self.warned || free >= WARNING_BYTES {
            return None;
        }
        self.warned = true;
        Some(free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_follow_from_the_formats() {
        assert_eq!(SPOOL_BYTES_PER_HOUR, 691_200_000);
        assert_eq!(ENCODED_BYTES_PER_HOUR, 86_400_000);
        assert_eq!(PEAK_BYTES_PER_HOUR, 777_600_000);
        assert_eq!(START_FLOOR_BYTES, 1_555_200_000);
        assert!(WARNING_BYTES < START_FLOOR_BYTES);
    }

    #[test]
    fn a_nearly_full_volume_refuses_the_start_and_names_the_space_and_folder() {
        let folder = Path::new("D:/Recordings");
        let message = start_refusal(Some(500_000_000), folder).unwrap();
        assert!(message.contains("0.5 GB is free"));
        assert!(message.contains("D:/Recordings"));
        assert!(message.contains("at least 1.6 GB is needed"));
        assert!(start_refusal(Some(START_FLOOR_BYTES - 1), folder).is_some());
    }

    #[test]
    fn enough_or_unknown_space_never_blocks_a_recording() {
        let folder = Path::new("D:/Recordings");
        assert!(start_refusal(Some(START_FLOOR_BYTES), folder).is_none());
        assert!(start_refusal(Some(u64::MAX), folder).is_none());
        assert!(start_refusal(None, folder).is_none());
    }

    #[test]
    fn the_volume_with_the_longest_matching_mount_point_wins() {
        let volumes = vec![
            (PathBuf::from("/"), 10),
            (PathBuf::from("/data"), 20),
            (PathBuf::from("/data/recordings"), 30),
            (PathBuf::from("/other"), 40),
        ];
        assert_eq!(pick_free_bytes(Path::new("/data/recordings/a"), &volumes), Some(30));
        assert_eq!(pick_free_bytes(Path::new("/data/x"), &volumes), Some(20));
        assert_eq!(pick_free_bytes(Path::new("/home"), &volumes), Some(10));
        assert_eq!(pick_free_bytes(Path::new("/data/recordings-old"), &volumes), Some(20));
    }

    #[test]
    fn an_unmatched_path_is_unknown_not_zero() {
        let volumes = vec![(PathBuf::from("/data"), 20)];
        assert_eq!(pick_free_bytes(Path::new("/elsewhere"), &volumes), None);
    }

    #[test]
    fn the_extended_length_prefix_is_removed_for_local_drives_only() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\?\UNC\server\share\x")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"C:\plain")),
            PathBuf::from(r"C:\plain")
        );
    }

    #[test]
    fn the_periodic_check_is_throttled() {
        let start = Instant::now();
        let mut watch = DiskWatch::new(start);
        assert!(!watch.due(start + Duration::from_secs(15)));
        assert!(!watch.due(start + Duration::from_secs(59)));
        assert!(watch.due(start + CHECK_INTERVAL));
        // The next interval counts from the check that ran.
        assert!(!watch.due(start + CHECK_INTERVAL + Duration::from_secs(30)));
        assert!(watch.due(start + CHECK_INTERVAL * 2));
    }

    #[test]
    fn low_space_is_reported_once_per_recording() {
        let mut watch = DiskWatch::new(Instant::now());
        assert_eq!(watch.evaluate(Some(WARNING_BYTES)), None);
        assert_eq!(watch.evaluate(Some(WARNING_BYTES + 1)), None);
        assert_eq!(watch.evaluate(None), None);
        assert_eq!(watch.evaluate(Some(WARNING_BYTES - 1)), Some(WARNING_BYTES - 1));
        assert_eq!(watch.evaluate(Some(1)), None, "only once");
        assert_eq!(watch.evaluate(Some(WARNING_BYTES - 1)), None);
    }

    #[test]
    fn the_real_volume_query_returns_a_plausible_value_for_the_temp_folder() {
        // Read-only query of the machine running the test; no assertion on size.
        let folder = std::env::temp_dir();
        if let Some(free) = free_space(&folder) {
            assert!(free > 0);
        }
    }
}
