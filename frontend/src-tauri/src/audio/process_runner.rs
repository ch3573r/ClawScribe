//! Bounded synchronous child-process I/O, for dedicated/blocking workers only.
//! Drains diagnostics while feeding input, enforces a deadline, and reaps on error.
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct ProcessOutput { pub status: ExitStatus, pub stderr: Vec<u8> }
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

pub fn run(command: &mut Command, input: Option<&[u8]>, deadline: Duration) -> std::io::Result<ProcessOutput> {
    command.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let mut stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("Child stderr unavailable"))?;
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> std::io::Result<()> {
            if let (Some(mut stdin), Some(input)) = (stdin, input) { stdin.write_all(input)?; }
            Ok(())
        });
        let reader = scope.spawn(move || -> std::io::Result<Vec<u8>> {
            let mut output = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let count = stderr.read(&mut buffer)?;
                if count == 0 { break; }
                // Keep the tail, while always draining the pipe to avoid deadlock.
                let excess = output.len().saturating_add(count).saturating_sub(MAX_DIAGNOSTIC_BYTES);
                if excess > 0 { output.drain(..excess); }
                output.extend_from_slice(&buffer[..count]);
            }
            Ok(output)
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if start.elapsed() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = child.kill(); let _ = child.wait();
                    break Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Audio encoder exceeded its time limit; recovery files were retained"));
                }
                Err(error) => { let _ = child.kill(); let _ = child.wait(); break Err(error); }
            }
        };
        let written = writer.join().map_err(|_| std::io::Error::other("Encoder input worker panicked"))?;
        let diagnostics = reader.join().map_err(|_| std::io::Error::other("Encoder diagnostics worker panicked"))??;
        let status = status?;
        // Preserve the process diagnostic on a nonzero exit, even on broken pipe.
        if status.success() { written?; }
        Ok(ProcessOutput { status, stderr: diagnostics })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn drains_input_and_large_stderr_without_deadlock() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 200000 /dev/zero >&2; cat >/dev/null"]);
        let output = run(&mut command, Some(&vec![0; 1_000_000]), Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stderr.len(), MAX_DIAGNOSTIC_BYTES);
    }
    #[cfg(unix)]
    #[test]
    fn deadline_kills_and_reaps_a_stalled_encoder() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let start = Instant::now();
        let error = run(&mut command, Some(&vec![0; 1_000_000]), Duration::from_millis(60)).err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn spawn_failure_returns_an_error_not_a_panic() {
        assert!(run(&mut Command::new("nonexistent-clawscribe-encoder"), None, Duration::from_secs(1)).is_err());
    }
}
