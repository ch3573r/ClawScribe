//! Hidden, supervised local parser process. This module has no Tauri dependency.
use super::{
    DocumentError, DocumentFormat, ExtractedDocument, EXTRACTION_TIMEOUT, INPUT_BYTES, TEXT_BYTES,
    WORKER_BYTES,
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--clawscribe-document-worker")) {
        return None;
    }
    let result = (|| {
        let path = args.next().ok_or(DocumentError::WorkerUnavailable)?;
        let format = args
            .next()
            .and_then(|v| v.into_string().ok())
            .ok_or(DocumentError::UnsupportedFormat)?;
        if args.next().is_some() {
            return Err(DocumentError::WorkerUnavailable);
        }
        // The child opens no input before the parent has assigned and verified its job.
        let mut handshake = [0u8; 3];
        std::io::stdin()
            .read_exact(&mut handshake)
            .map_err(|_| DocumentError::WorkerUnavailable)?;
        if handshake != *b"GO\n" || !job::current_is_capped() {
            return Err(DocumentError::WorkerUnavailable);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| DocumentError::FileUnavailable)?
            .take(INPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DocumentError::FileUnavailable)?;
        super::extract::extract(DocumentFormat::from_extension(&format)?, &bytes)
    })();
    let mut output = BoundedOutput(Vec::new());
    if serde_json::to_writer(&mut output, &result).is_err() {
        output.0.clear();
        let _ = serde_json::to_writer(
            &mut output,
            &Result::<ExtractedDocument, _>::Err(DocumentError::TextLimit),
        );
    }
    Some(if std::io::stdout().write_all(&output.0).is_ok() {
        0
    } else {
        2
    })
}

struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > TEXT_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Document IPC limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub async fn extract_file<G: Send + 'static>(
    path: PathBuf,
    format: DocumentFormat,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    guard: G,
) -> Result<ExtractedDocument, DocumentError> {
    let executable = std::env::current_exe().map_err(|_| DocumentError::WorkerUnavailable)?;
    let mut command = Command::new(executable);
    command
        .arg("--clawscribe-document-worker")
        .arg(path)
        .arg(format.extension());
    run_command(command, cancel, preempt, EXTRACTION_TIMEOUT, guard).await
}

async fn run_command<G: Send + 'static>(
    command: Command,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    deadline: Duration,
    guard: G,
) -> Result<ExtractedDocument, DocumentError> {
    let cancel = cancel.child_token();
    let on_drop = CancelOnDrop(cancel.clone());
    // This task, not the UI waiter, owns the process, job and recording admission lease.
    let supervisor = tokio::spawn(async move {
        let _guard = guard;
        supervise(command, cancel, preempt, deadline).await
    });
    let result = supervisor
        .await
        .map_err(|_| DocumentError::WorkerUnavailable)?;
    drop(on_drop);
    result
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn supervise(
    mut command: Command,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    deadline: Duration,
) -> Result<ExtractedDocument, DocumentError> {
    if cancel.is_cancelled() || preempt.load(Ordering::Acquire) {
        return Err(DocumentError::Cancelled);
    }
    let job = job::Job::new()?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .map_err(|_| DocumentError::WorkerUnavailable)?;
    let result = async {
        job.assign(child.id().ok_or(DocumentError::WorkerUnavailable)?)?;
        let mut input = child.stdin.take().ok_or(DocumentError::WorkerUnavailable)?;
        input.write_all(b"GO\n").await.map_err(|_| DocumentError::WorkerUnavailable)?;
        drop(input);
        let mut output = child.stdout.take().ok_or(DocumentError::WorkerUnavailable)?.take(TEXT_BYTES as u64 + 1);
        let completed = async {
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).await.map_err(|_| DocumentError::WorkerUnavailable)?;
            if bytes.len() > TEXT_BYTES { return Err(DocumentError::TextLimit); }
            let status = child.wait().await.map_err(|_| DocumentError::WorkerUnavailable)?;
            if !status.success() { return Err(DocumentError::WorkerLimit); }
            serde_json::from_slice::<Result<ExtractedDocument, DocumentError>>(&bytes)
                .map_err(|_| DocumentError::WorkerUnavailable)?
        };
        tokio::pin!(completed);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(DocumentError::Cancelled),
            _ = async { while !preempt.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(10)).await; } } => Err(DocumentError::Cancelled),
            _ = tokio::time::sleep(deadline) => Err(DocumentError::WorkerLimit),
            result = &mut completed => result,
        }
    }.await;
    // Dropping the job kills any descendant too. Always reap before releasing the lease.
    if result.is_err() {
        let _ = child.start_kill();
    }
    drop(job);
    let _ = child.wait().await;
    #[cfg(test)]
    REAPED.fetch_add(1, Ordering::SeqCst);
    result
}

#[cfg(test)]
static REAPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(windows)]
mod job {
    use super::*;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                    QueryInformationJobObject, SetInformationJobObject,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOB_OBJECT_LIMIT_PROCESS_MEMORY,
                },
                Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
            },
        },
    };
    pub struct Job(HANDLE);
    // Exclusive owned kernel handle, used only by the supervisor; Windows job handles are thread-independent.
    unsafe impl Send for Job {}
    impl Job {
        pub fn new() -> Result<Self, DocumentError> {
            let job = Self(
                unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                    .map_err(|_| DocumentError::WorkerUnavailable)?,
            );
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation:
                    windows::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION {
                        LimitFlags: JOB_OBJECT_LIMIT_PROCESS_MEMORY
                            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                        ..Default::default()
                    },
                ProcessMemoryLimit: WORKER_BYTES,
                ..Default::default()
            };
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                )
            }
            .map_err(|_| DocumentError::WorkerUnavailable)?;
            Ok(job)
        }
        pub fn assign(&self, pid: u32) -> Result<(), DocumentError> {
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(|_| DocumentError::WorkerUnavailable)?;
            let assigned = unsafe { AssignProcessToJobObject(self.0, process) };
            let _ = unsafe { CloseHandle(process) };
            assigned.map_err(|_| DocumentError::WorkerUnavailable)
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    pub fn current_is_capped() -> bool {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                HANDLE::default(),
                JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as _,
                std::mem::size_of_val(&limits) as u32,
                None,
            )
        }
        .is_ok()
            && limits
                .BasicLimitInformation
                .LimitFlags
                .contains(JOB_OBJECT_LIMIT_PROCESS_MEMORY)
            && limits.ProcessMemoryLimit > 0
            && limits.ProcessMemoryLimit <= WORKER_BYTES
    }
}
#[cfg(not(windows))]
mod job {
    use super::*;
    pub struct Job;
    impl Job {
        pub fn new() -> Result<Self, DocumentError> {
            Err(DocumentError::WorkerUnavailable)
        }
        pub fn assign(&self, _: u32) -> Result<(), DocumentError> {
            Err(DocumentError::WorkerUnavailable)
        }
    }
    pub fn current_is_capped() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn stalled() -> Command {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::ReadLine() | Out-Null; Start-Sleep -Seconds 60",
        ]);
        command
    }
    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn stalled_child_timeout_kills_and_reaps_before_releasing_lease() {
        let before = REAPED.load(Ordering::SeqCst);
        let releases = Arc::new(AtomicUsize::new(0));
        let result = run_command(
            stalled(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(250),
            Lease(releases.clone()),
        )
        .await;
        assert_eq!(result, Err(DocumentError::WorkerLimit));
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert!(REAPED.load(Ordering::SeqCst) > before);
    }
    #[tokio::test]
    async fn stalled_child_cancellation_and_recording_preemption_kill_and_reap() {
        for recording in [false, true] {
            let before = REAPED.load(Ordering::SeqCst);
            let cancel = CancellationToken::new();
            let preempt = Arc::new(AtomicBool::new(false));
            let releases = Arc::new(AtomicUsize::new(0));
            let task = tokio::spawn(run_command(
                stalled(),
                cancel.clone(),
                preempt.clone(),
                Duration::from_secs(3),
                Lease(releases.clone()),
            ));
            tokio::time::sleep(Duration::from_millis(200)).await;
            if recording {
                preempt.store(true, Ordering::Release);
            } else {
                cancel.cancel();
            }
            assert_eq!(task.await.unwrap(), Err(DocumentError::Cancelled));
            assert_eq!(releases.load(Ordering::SeqCst), 1);
            assert!(REAPED.load(Ordering::SeqCst) > before);
        }
    }
    #[tokio::test]
    async fn bounded_child_extracts_real_pdf_pages() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("reference.pdf");
        std::fs::write(&path, super::super::fixtures::pdf(24, true, false)).unwrap();
        let executable = std::env::var_os("CLAWSCRIBE_DOCUMENT_TEST_WORKER").unwrap();
        let mut command = Command::new(executable);
        command
            .arg("--clawscribe-document-worker")
            .arg(&path)
            .arg("pdf");
        let result = run_command(
            command,
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            EXTRACTION_TIMEOUT,
            (),
        )
        .await
        .unwrap();
        assert_eq!(result.blocks.last().unwrap().page, Some(24));
    }
}
