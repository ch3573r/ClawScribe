//! Hidden, supervised local parser process. This module has no Tauri dependency.
use super::{DocumentError, DocumentFormat, ExtractedDocument, EXTRACTION_TIMEOUT};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn dispatch() -> Option<i32> {
    None
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
    _command: Command,
    _cancel: CancellationToken,
    _preempt: Arc<AtomicBool>,
    _deadline: Duration,
    _guard: G,
) -> Result<ExtractedDocument, DocumentError> {
    Err(DocumentError::WorkerUnavailable)
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
    }
    #[tokio::test]
    async fn stalled_child_cancellation_and_recording_preemption_kill_and_reap() {
        for recording in [false, true] {
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
