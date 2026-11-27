use std::{
    any::Any,
    backtrace::Backtrace,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CRASH_LOG: &str = "crash.log";
const PREVIOUS_LOG: &str = "crash.previous.log";
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const REPORT_TAIL_BYTES: u64 = 128 * 1024;

pub fn install_panic_hook() {
    let argument = std::env::args_os().nth(1);
    let mode = match argument.as_deref().and_then(|arg| arg.to_str()) {
        Some(crate::parakeet_engine::worker::WORKER_FLAG) => "parakeet",
        Some("--clawscribe-document-worker") => "document",
        Some("--mcp-stdio") => "mcp",
        Some("--self-test") => "self-test",
        _ => "desktop",
    };
    // The packaged self-test owns a disposable profile and reports via its
    // launcher. It must not write to the user's normal profile.
    install(
        mode,
        (mode != "self-test").then(app_log_directory).flatten(),
    );
}

fn install(mode: &'static str, log_dir: Option<PathBuf>) {
    std::panic::set_hook(Box::new(move |panic| {
        let message = safe_message(panic.payload());
        let location = panic.location().map_or_else(
            || "unknown location".to_owned(),
            |location| {
                let file = location
                    .file()
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("unknown");
                format!("{file}:{}:{}", location.line(), location.column())
            },
        );
        let backtrace = Backtrace::force_capture().to_string();
        // Keep function frames and omit source paths and their machine-local directories.
        let frames = backtrace
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.len() <= 256
                    && line
                        .split_once(':')
                        .is_some_and(|(index, _)| index.parse::<usize>().is_ok())
            })
            .take(40)
            .collect::<Vec<_>>()
            .join(" | ");
        let stack = if frames.is_empty() {
            "unavailable"
        } else {
            &frames
        };
        let detail = format!("{mode} panic at {location}: {message} | backtrace: {stack}");
        if mode == "desktop" && log::log_enabled!(target: "clawscribe::panic", log::Level::Error) {
            log::error!(target: "clawscribe::panic", "{detail}");
        } else if !log_dir
            .as_deref()
            .is_some_and(|dir| append_report(dir, &detail).is_ok())
        {
            let _ = writeln!(io::stderr().lock(), "{detail}");
        }
    }));
}

fn safe_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        // Literal Rust panic messages are code-authored. Reject strings that
        // look like raw data even if a caller passed them as a static string.
        let lower = message.to_ascii_lowercase();
        if !message.is_empty()
            && message.len() <= 180
            && message.chars().all(|character| {
                character.is_ascii_alphanumeric() || " .,:_()-".contains(character)
            })
            && !["token", "secret", "password", "authorization", "api key"]
                .iter()
                .any(|word| lower.contains(word))
        {
            return message;
        }
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        // Formatted messages may include transcript text or provider errors.
        for reason in [
            "called `Result::unwrap()` on an `Err` value",
            "called `Option::unwrap()` on a `None` value",
            "index out of bounds",
            "assertion failed",
        ] {
            if message.starts_with(reason) {
                return reason;
            }
        }
    }
    "Panic message redacted"
}

fn app_log_directory() -> Option<PathBuf> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).ok()?;
    let identifier = config["identifier"].as_str()?;
    let base = dirs::data_dir()?;
    let selected = crate::database::recovery::selected_data_dir(&base, identifier).ok();
    let profile = selected.as_deref().and_then(Path::file_name);
    let profile = profile.unwrap_or_else(|| std::ffi::OsStr::new(identifier));
    // Match Tauri's desktop PathResolver::app_log_dir before it is available.
    #[cfg(target_os = "macos")]
    return Some(dirs::home_dir()?.join("Library/Logs").join(profile));
    #[cfg(not(target_os = "macos"))]
    return Some(dirs::data_local_dir()?.join(profile).join("logs"));
}

fn append_report(dir: &Path, detail: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(CRASH_LOG);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || crate::local_path::is_link(&metadata) => {
            return Err(io::Error::other("Crash log must be a regular file"));
        }
        Ok(metadata) if metadata.len() >= MAX_LOG_BYTES => {
            let _ = fs::rename(&path, dir.join(PREVIOUS_LOG));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let record = format!(
        "{} {} {detail}\n",
        std::process::id(),
        epoch_ms(SystemTime::now())
    );
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(record.as_bytes())
}

fn epoch_ms(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn has_child_report(path: &Path, pid: u32, started: SystemTime) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || crate::local_path::is_link(&metadata) {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let offset = metadata.len().saturating_sub(REPORT_TAIL_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut tail = Vec::new();
    file.take(REPORT_TAIL_BYTES).read_to_end(&mut tail)?;
    let prefix = format!("{pid} ");
    let tail = String::from_utf8_lossy(&tail);
    Ok(tail.lines().skip(usize::from(offset > 0)).any(|line| {
        line.strip_prefix(&prefix)
            .and_then(|rest| rest.split_once(' '))
            .and_then(|(timestamp, _)| timestamp.parse::<u128>().ok())
            .is_some_and(|when| when >= epoch_ms(started))
    }))
}

/// Confirm a specific child wrote a report. Keep its contents in the crash
/// log and surface the location in the main log.
pub(crate) fn report_child_panic(worker: &'static str, pid: u32, started: SystemTime) {
    let Some(dir) = app_log_directory() else {
        return;
    };
    if let Some(file) = [CRASH_LOG, PREVIOUS_LOG]
        .into_iter()
        .find(|file| has_child_report(&dir.join(file), pid, started).unwrap_or(false))
    {
        log::error!("{worker} panic recorded in {file} (process {pid})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    const TEST_LOG_DIR: &str = "CLAWSCRIBE_TEST_PANIC_LOG_DIR";

    #[test]
    fn worker_panic_is_persisted_with_stderr_discarded() {
        if let Some(dir) = std::env::var_os(TEST_LOG_DIR) {
            install("parakeet", Some(dir.into()));
            let _ = std::panic::catch_unwind(|| panic!("Synthetic worker panic"));
            let private = std::env::var(TEST_LOG_DIR).unwrap();
            panic!("Untrusted content: {private}");
        }

        let dir = tempfile::tempdir().unwrap();
        let started = SystemTime::now();
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_report::tests::worker_panic_is_persisted_with_stderr_discarded",
            ])
            .env(TEST_LOG_DIR, dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(!worker.wait().unwrap().success());
        let path = dir.path().join(CRASH_LOG);
        let log = fs::read_to_string(&path).unwrap();
        assert!(log.contains("parakeet panic at crash_report.rs:"));
        assert!(log.contains("Synthetic worker panic | backtrace:"));
        assert!(log.contains("Panic message redacted | backtrace:"));
        assert!(!log.contains(dir.path().to_str().unwrap()));
        assert!(has_child_report(&path, worker.id(), started).unwrap());
        assert!(!has_child_report(
            &path,
            worker.id(),
            SystemTime::now() + std::time::Duration::from_secs(1)
        )
        .unwrap());
    }
}
