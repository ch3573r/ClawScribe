use super::super::model::TimestampedResult;
use super::protocol::{read_frame, write_frame, Request, Response};
use std::ffi::OsString;
#[cfg(windows)]
use std::io::Write;
use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const LOAD_TIMEOUT: Duration = Duration::from_secs(180);
const WAIT_SLICE: Duration = Duration::from_millis(20);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
static DIRECTML_DISABLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub fn directml_session_disabled() -> bool {
    DIRECTML_DISABLED
        .get()
        .is_some_and(|disabled| disabled.load(Ordering::Acquire))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    TimedOut,
    Cancelled,
    Crashed,
    Protocol,
    Spawn,
    Engine(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => f.write_str("Parakeet worker stopped responding"),
            Self::Cancelled => f.write_str("Parakeet transcription cancelled"),
            Self::Crashed => f.write_str("Parakeet worker exited unexpectedly"),
            Self::Protocol => f.write_str("Invalid Parakeet worker response"),
            Self::Spawn => f.write_str("Parakeet worker could not be started"),
            Self::Engine(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for WorkerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSpec {
    pub model_dir: PathBuf,
    pub precision_suffix: Option<String>,
    pub use_directml: bool,
}

pub fn transcription_timeout(sample_count: usize) -> Duration {
    // A 12 s live segment holds admission for at most 2 x 48 s plus child
    // cleanup; each attempt includes any reload, and cancellation skips retry.
    Duration::from_secs_f64((sample_count as f64 / 16_000.0 * 4.0).max(20.0))
}

struct Exchange {
    request: Request,
    samples: Arc<Vec<f32>>,
    reply: mpsc::Sender<Result<Response, WorkerError>>,
}

struct RunningChild {
    process: Child,
    job: Option<job::Job>,
    requests: Option<mpsc::Sender<Exchange>>,
    io_thread: Option<JoinHandle<()>>,
}

impl RunningChild {
    fn spawn(program: &PathBuf, args: &[OsString]) -> Result<Self, WorkerError> {
        let job = job::Job::new()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let process = command.spawn().map_err(|_| WorkerError::Spawn)?;
        // Install ownership immediately so every subsequent error kills and reaps.
        let mut running = Self {
            process,
            job: Some(job),
            requests: None,
            io_thread: None,
        };
        let assigned = running
            .job
            .as_ref()
            .ok_or(WorkerError::Spawn)?
            .assign(running.process.id());
        if assigned.is_err() {
            return Err(if matches!(running.process.try_wait(), Ok(Some(_))) {
                WorkerError::Crashed
            } else {
                WorkerError::Spawn
            });
        }
        let stdin = running.process.stdin.take().ok_or(WorkerError::Spawn)?;
        let stdout = running.process.stdout.take().ok_or(WorkerError::Spawn)?;
        let (requests, receiver) = mpsc::channel::<Exchange>();
        // The same thread owns writes and reads. A child that stops reading a
        // large PCM frame cannot wedge the supervisor in a blocking pipe write.
        running.io_thread = Some(
            thread::Builder::new()
                .name("parakeet-worker-io".into())
                .spawn(move || {
                    let mut input = BufWriter::new(stdin);
                    let mut output = BufReader::new(stdout);
                    #[cfg(windows)]
                    if input
                        .write_all(b"GO\n")
                        .and_then(|_| input.flush())
                        .is_err()
                    {
                        return;
                    }
                    while let Ok(exchange) = receiver.recv() {
                        let result = (|| {
                            write_frame(&mut input, &exchange.request, &exchange.samples)
                                .map_err(pipe_error)?;
                            match read_frame::<Response>(&mut output).map_err(pipe_error)? {
                                Some((response, pcm)) if pcm.is_empty() => Ok(response),
                                Some(_) => Err(WorkerError::Protocol),
                                None => Err(WorkerError::Crashed),
                            }
                        })();
                        let failed = result.is_err();
                        if exchange.reply.send(result).is_err() || failed {
                            break;
                        }
                    }
                })
                .map_err(|_| WorkerError::Spawn)?,
        );
        running.requests = Some(requests);
        Ok(running)
    }

    /// Kill and wait before releasing inference admission. Closing the job also
    /// terminates any Windows descendants that inherited a pipe handle.
    fn stop(&mut self) -> bool {
        self.requests.take();
        let _ = self.process.kill();
        self.job.take();
        let reaped = self.process.wait().is_ok();
        if let Some(thread) = self.io_thread.take() {
            // Usually the killed child's pipes wake immediately. Do not turn an
            // inherited pipe on another platform into an unbounded join.
            let until = Instant::now() + SHUTDOWN_GRACE;
            while !thread.is_finished() && Instant::now() < until {
                thread::sleep(Duration::from_millis(2));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
        reaped
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pipe_error(error: io::Error) -> WorkerError {
    match error.kind() {
        io::ErrorKind::InvalidData => WorkerError::Protocol,
        _ => WorkerError::Crashed,
    }
}

/// Synchronous, single-request owner. The engine holds its model write lock
/// across the entire exchange, including cleanup and the one recovery attempt.
pub struct TranscriptionWorker {
    program: PathBuf,
    args: Vec<OsString>,
    child: Option<RunningChild>,
    last_load: Option<LoadSpec>,
    directml_disabled: Arc<AtomicBool>,
    load_timeout: Duration,
    #[cfg(test)]
    spawned: usize,
    #[cfg(test)]
    reaped: usize,
}

impl TranscriptionWorker {
    pub fn new() -> Result<Self, WorkerError> {
        Ok(Self::with_program(
            std::env::current_exe().map_err(|_| WorkerError::Spawn)?,
            vec![OsString::from(super::WORKER_FLAG)],
        ))
    }

    /// Injectable executable/arguments for model-free supervisor tests.
    pub fn with_program(program: PathBuf, args: Vec<OsString>) -> Self {
        Self {
            program,
            args,
            child: None,
            last_load: None,
            directml_disabled: DIRECTML_DISABLED
                .get_or_init(|| Arc::new(AtomicBool::new(false)))
                .clone(),
            load_timeout: LOAD_TIMEOUT,
            #[cfg(test)]
            spawned: 0,
            #[cfg(test)]
            reaped: 0,
        }
    }

    pub fn load(
        &mut self,
        spec: LoadSpec,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(), WorkerError> {
        let using_directml = spec.use_directml && !self.directml_disabled.load(Ordering::Acquire);
        let first = self.load_once(spec.clone(), self.load_timeout, is_cancelled);
        match first {
            Err(WorkerError::Crashed | WorkerError::TimedOut) if using_directml => {
                self.disable_directml();
                if is_cancelled() {
                    return Err(WorkerError::Cancelled);
                }
                // One CPU retry for an explicit GPU load, never nested inside
                // the transcribe retry's reload budget.
                self.load_once(spec, self.load_timeout, is_cancelled)
            }
            result => result,
        }
    }

    fn disable_directml(&self) {
        if !self.directml_disabled.swap(true, Ordering::AcqRel) {
            log::warn!("Parakeet GPU path disabled for this session after a worker failure");
        }
    }

    fn load_once(
        &mut self,
        mut spec: LoadSpec,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(), WorkerError> {
        if self.directml_disabled.load(Ordering::Acquire) {
            spec.use_directml = false;
        }
        let response = self.exchange(
            Request::Load {
                model_dir: spec.model_dir.clone(),
                precision_suffix: spec.precision_suffix.clone(),
                use_directml: spec.use_directml,
            },
            Arc::new(vec![]),
            timeout,
            is_cancelled,
        )?;
        match response {
            Response::Loaded => {
                self.last_load = Some(spec);
                Ok(())
            }
            Response::Error(message) => {
                // A failed replacement Load discarded the old sessions. The
                // last successful spec remains recoverable on a fresh child.
                self.stop_child();
                Err(WorkerError::Engine(message))
            }
            _ => {
                self.stop_child();
                Err(WorkerError::Protocol)
            }
        }
    }

    pub fn transcribe(
        &mut self,
        samples: Vec<f32>,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TimestampedResult, WorkerError> {
        // Share one immutable segment with the IO thread and its possible retry.
        let samples = Arc::new(samples);
        let first = self.transcribe_once(samples.clone(), timeout, is_cancelled);
        match first {
            Err(WorkerError::Crashed | WorkerError::TimedOut) => {
                if self
                    .last_load
                    .as_ref()
                    .is_some_and(|spec| spec.use_directml)
                {
                    self.disable_directml();
                }
                if is_cancelled() {
                    return Err(WorkerError::Cancelled);
                }
                // The failed exchange already killed/reaped the process. An
                // absent child is reloaded by transcribe_once, with CPU forced
                // for this and all later loads after a DirectML worker failure.
                self.transcribe_once(samples, timeout, is_cancelled)
            }
            result => result,
        }
    }

    fn transcribe_once(
        &mut self,
        samples: Arc<Vec<f32>>,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TimestampedResult, WorkerError> {
        if is_cancelled() {
            self.stop_child();
            return Err(WorkerError::Cancelled);
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(WorkerError::Protocol)?;
        if self.child.is_none() {
            let spec = self
                .last_load
                .clone()
                .ok_or_else(|| WorkerError::Engine("No Parakeet model loaded".into()))?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Err(error) = self.load_once(spec, remaining.min(self.load_timeout), is_cancelled)
            {
                // No provider response or transcript content in this diagnostic.
                log::warn!("Parakeet worker lazy respawn/model reload failed");
                return Err(error);
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.exchange(Request::Transcribe, samples, remaining, is_cancelled)? {
            Response::Transcript {
                text,
                timestamps,
                tokens,
            } => Ok(TimestampedResult {
                text,
                timestamps,
                tokens,
            }),
            Response::Error(message) => Err(WorkerError::Engine(message)),
            _ => {
                self.stop_child();
                Err(WorkerError::Protocol)
            }
        }
    }

    fn exchange(
        &mut self,
        request: Request,
        samples: Arc<Vec<f32>>,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Response, WorkerError> {
        let result = (|| {
            if is_cancelled() {
                return Err(WorkerError::Cancelled);
            }
            if timeout.is_zero() {
                return Err(WorkerError::TimedOut);
            }
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or(WorkerError::Protocol)?;
            if self.child.is_none() {
                self.child = Some(RunningChild::spawn(&self.program, &self.args)?);
                #[cfg(test)]
                {
                    self.spawned += 1;
                }
            }
            let (reply, receiver) = mpsc::channel();
            self.child
                .as_ref()
                .and_then(|child| child.requests.as_ref())
                .ok_or(WorkerError::Crashed)?
                .send(Exchange {
                    request,
                    samples,
                    reply,
                })
                .map_err(|_| WorkerError::Crashed)?;
            loop {
                if is_cancelled() {
                    return Err(WorkerError::Cancelled);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(WorkerError::TimedOut);
                }
                match receiver.recv_timeout(remaining.min(WAIT_SLICE)) {
                    Ok(response) => {
                        if is_cancelled() {
                            return Err(WorkerError::Cancelled);
                        }
                        if Instant::now() >= deadline {
                            return Err(WorkerError::TimedOut);
                        }
                        return response;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err(WorkerError::Crashed),
                }
            }
        })();
        if result.is_err() {
            self.stop_child();
        }
        result
    }

    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let reaped = child.stop();
            #[cfg(test)]
            if reaped {
                self.reaped += 1;
            }
            #[cfg(not(test))]
            let _ = reaped;
        }
    }

    /// Fault-injection hook for the bench. Retains the child entry and load
    /// spec so the next request observes the crash and exercises normal retry.
    pub fn kill_child(&mut self) -> Result<(), WorkerError> {
        if let Some(child) = self.child.as_mut() {
            child.process.kill().map_err(|_| WorkerError::Crashed)?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Some(requests) = child.requests.as_ref() {
                let (reply, _receiver) = mpsc::channel();
                let _ = requests.send(Exchange {
                    request: Request::Shutdown,
                    samples: Arc::new(vec![]),
                    reply,
                });
            }
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            while Instant::now() < deadline {
                match child.process.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(5)),
                }
            }
        }
        self.stop_child();
        self.last_load = None;
    }
}

impl Drop for TranscriptionWorker {
    fn drop(&mut self) {
        self.stop_child();
    }
}

#[cfg(windows)]
mod job {
    use super::WorkerError;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                    SetInformationJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                },
                Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
            },
        },
    };

    pub struct Job(HANDLE);
    // Owned, thread-independent kernel handle. Assignment is thread-safe;
    // destruction requires exclusive ownership through the engine model lock.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Result<Self, WorkerError> {
            let job = Self(
                unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                    .map_err(|_| WorkerError::Spawn)?,
            );
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                // No model memory cap: Parakeet can legitimately exceed 1 GiB.
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
            .map_err(|_| WorkerError::Spawn)?;
            Ok(job)
        }

        pub fn assign(&self, pid: u32) -> Result<(), WorkerError> {
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(|_| WorkerError::Spawn)?;
            let assigned = unsafe { AssignProcessToJobObject(self.0, process) };
            let _ = unsafe { CloseHandle(process) };
            assigned.map_err(|_| WorkerError::Spawn)
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod job {
    use super::WorkerError;
    pub struct Job;
    impl Job {
        pub fn new() -> Result<Self, WorkerError> {
            Ok(Self)
        }
        pub fn assign(&self, _: u32) -> Result<(), WorkerError> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(use_directml: bool) -> LoadSpec {
        LoadSpec {
            model_dir: "models/parakeet".into(),
            precision_suffix: Some("int8".into()),
            use_directml,
        }
    }

    fn isolated(mut worker: TranscriptionWorker) -> TranscriptionWorker {
        worker.directml_disabled = Arc::new(AtomicBool::new(false));
        worker
    }

    #[cfg(windows)]
    fn script(source: &str) -> TranscriptionWorker {
        isolated(TranscriptionWorker::with_program(
            "powershell.exe".into(),
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                source,
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
        ))
    }

    #[cfg(unix)]
    fn script(source: &str) -> TranscriptionWorker {
        isolated(TranscriptionWorker::with_program(
            "/bin/sh".into(),
            ["-c", source].into_iter().map(OsString::from).collect(),
        ))
    }

    #[cfg(any(windows, unix))]
    fn stalled() -> TranscriptionWorker {
        #[cfg(windows)]
        let mut worker = script("[Console]::ReadLine() | Out-Null; Start-Sleep -Seconds 60");
        #[cfg(unix)]
        let mut worker = script("exec sleep 60");
        worker.load_timeout = Duration::from_millis(250);
        worker
    }

    #[test]
    fn duration_based_deadline_has_a_twenty_second_floor() {
        assert_eq!(transcription_timeout(0), Duration::from_secs(20));
        assert_eq!(transcription_timeout(16_000), Duration::from_secs(20));
        assert_eq!(transcription_timeout(160_000), Duration::from_secs(40));
        assert_eq!(transcription_timeout(192_000), Duration::from_secs(48));
    }

    #[test]
    fn supervisor_can_share_the_engine_model_lock() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TranscriptionWorker>();
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn stalled_child_times_out_and_is_reaped() {
        let mut worker = stalled();
        let started = Instant::now();
        assert_eq!(
            worker.load(spec(false), &|| false),
            Err(WorkerError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(worker.child.is_none());
        assert_eq!(worker.reaped, 1);
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn immediate_exit_is_a_crash_and_is_reaped() {
        #[cfg(windows)]
        let mut worker = script("[Console]::ReadLine() | Out-Null; exit 7");
        #[cfg(unix)]
        let mut worker = script("exit 7");
        worker.load_timeout = Duration::from_secs(10);
        assert_eq!(
            worker.load(spec(false), &|| false),
            Err(WorkerError::Crashed)
        );
        assert!(worker.child.is_none());
        assert_eq!(worker.reaped, 1);
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn cancelled_load_kills_and_reaps() {
        let mut worker = stalled();
        worker.load_timeout = Duration::from_secs(10);
        let started = Instant::now();
        assert_eq!(
            worker.load(spec(false), &|| started.elapsed()
                >= Duration::from_millis(100)),
            Err(WorkerError::Cancelled)
        );
        assert!(worker.child.is_none());
        assert_eq!(worker.spawned, 1);
        assert_eq!(worker.reaped, 1);
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn next_load_respawns_after_a_failure() {
        let mut worker = stalled();
        for count in 1..=2 {
            assert_eq!(
                worker.load(spec(false), &|| false),
                Err(WorkerError::TimedOut)
            );
            assert_eq!(worker.spawned, count);
            assert_eq!(worker.reaped, count);
        }
    }

    #[test]
    fn spawn_error_is_typed() {
        let directory = tempfile::tempdir().unwrap();
        let mut worker = isolated(TranscriptionWorker::with_program(
            directory.path().join("missing-worker"),
            vec![],
        ));
        assert_eq!(worker.load(spec(false), &|| false), Err(WorkerError::Spawn));
        assert!(worker.child.is_none());
    }

    // A real protocol peer, requiring no model or test-only production dispatch.
    #[cfg(windows)]
    fn fake(mode: &str, state: &std::path::Path) -> TranscriptionWorker {
        let state = state.to_string_lossy().replace('\'', "''");
        script(&format!(
            r#"
$ErrorActionPreference = 'Stop'
$mode = '{mode}'
$state = '{state}'
$inputStream = [Console]::OpenStandardInput()
$reader = [System.IO.BinaryReader]::new($inputStream)
$writer = [System.IO.BinaryWriter]::new([Console]::OpenStandardOutput())
if ([Text.Encoding]::ASCII.GetString($reader.ReadBytes(3)) -ne "GO`n") {{ exit 2 }}
while ($true) {{
    try {{ $length = $reader.ReadUInt32() }} catch {{ exit 0 }}
    $json = [Text.Encoding]::UTF8.GetString($reader.ReadBytes($length))
    $request = ConvertFrom-Json -InputObject $json
    $pcmLength = $reader.ReadUInt32()
    $pcm = $reader.ReadBytes($pcmLength)
    if ($request -eq 'Shutdown') {{ exit 0 }}
    if ($null -ne $request.Load) {{
        Add-Content -LiteralPath ($state + '.loads') -Value ([string]$request.Load.use_directml)
        if ($request.Load.use_directml) {{
            if ($mode -eq 'load-timeout') {{ Start-Sleep -Seconds 60 }}
            if ($mode -eq 'load-crash') {{ exit 9 }}
        }}
        if ($mode -eq 'reload-stall' -and [IO.File]::Exists($state)) {{ Start-Sleep -Seconds 60 }}
        $response = '"Loaded"'
    }} else {{
        $encodedPcm = if ($pcmLength -gt 0) {{ [Convert]::ToBase64String($pcm) }} else {{ '' }}
        Add-Content -LiteralPath ($state + '.pcm') -Value $encodedPcm
        if ($mode -eq 'stall') {{ Start-Sleep -Seconds 60 }}
        if ($mode -eq 'crash-always') {{ exit 9 }}
        if (($mode -eq 'crash-once' -or $mode -eq 'timeout-once' -or $mode -eq 'reload-stall') -and -not [IO.File]::Exists($state)) {{
            [IO.File]::WriteAllText($state, 'failed')
            if ($mode -eq 'timeout-once') {{ Start-Sleep -Seconds 60 }}
            exit 9
        }}
        if ($mode -eq 'protocol') {{ $writer.Write([uint32]268435457); $writer.Flush(); continue }}
        if ($mode -eq 'garbage') {{ $writer.Write([Text.Encoding]::ASCII.GetBytes('garbage on stdout')); $writer.Flush(); Start-Sleep -Seconds 60 }}
        if ($mode -eq 'engine') {{ $response = '{{"Error":"Decode failed"}}' }}
        else {{ $response = '{{"Transcript":{{"text":"test","timestamps":[0.0],"tokens":["test"]}}}}' }}
    }}
    $bytes = [Text.Encoding]::UTF8.GetBytes($response)
    $writer.Write([uint32]$bytes.Length)
    $writer.Write($bytes)
    $writer.Write([uint32]0)
    $writer.Flush()
    if ($mode -eq 'not-reading') {{ Start-Sleep -Seconds 60 }}
}}
"#
        ))
    }

    #[test]
    #[cfg(windows)]
    fn crash_or_timeout_retries_identical_pcm_once_and_disables_directml_for_session() {
        for mode in ["crash-once", "timeout-once"] {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state");
            let mut worker = fake(mode, &state);
            worker.load(spec(true), &|| false).unwrap();
            let result = worker
                .transcribe(vec![0.0, 0.5], Duration::from_secs(8), &|| false)
                .unwrap();
            assert_eq!(result.text, "test");
            assert_eq!(result.tokens, ["test"]);
            assert_eq!(worker.spawned, 2);
            assert_eq!(worker.reaped, 1);
            let pcm = std::fs::read_to_string(state.with_extension("pcm")).unwrap();
            let attempts = pcm.lines().collect::<Vec<_>>();
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[0], attempts[1]);
            assert!(!worker.last_load.as_ref().unwrap().use_directml);
            worker.load(spec(true), &|| false).unwrap();
            worker.shutdown();
            let loads = std::fs::read_to_string(state.with_extension("loads")).unwrap();
            assert_eq!(
                loads.lines().collect::<Vec<_>>(),
                ["True", "False", "False"]
            );
            assert_eq!(worker.reaped, 2);

            // New supervisors share the process-wide downgrade in production.
            let mut next = fake("ok", &directory.path().join("next"));
            next.directml_disabled = worker.directml_disabled.clone();
            next.load(spec(true), &|| false).unwrap();
            assert!(!next.last_load.as_ref().unwrap().use_directml);
        }
    }

    #[test]
    #[cfg(windows)]
    fn cancelled_transcribe_is_not_retried() {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("state");
        let mut worker = fake("stall", &state);
        worker.load(spec(true), &|| false).unwrap();
        assert_eq!(
            worker
                .transcribe(vec![0.5], Duration::from_secs(10), &|| {
                    state.with_extension("pcm").exists()
                })
                .unwrap_err(),
            WorkerError::Cancelled
        );
        assert_eq!(worker.spawned, 1);
        assert_eq!(worker.reaped, 1);
        assert!(worker.child.is_none());
        assert!(!worker.directml_disabled.load(Ordering::Acquire));
    }

    #[test]
    #[cfg(windows)]
    fn directml_load_crash_or_timeout_retries_once_on_cpu() {
        for mode in ["load-crash", "load-timeout"] {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state");
            let mut worker = fake(mode, &state);
            worker.load_timeout = Duration::from_secs(8);
            worker.load(spec(true), &|| false).unwrap();
            assert!(worker.directml_disabled.load(Ordering::Acquire));
            assert!(!worker.last_load.as_ref().unwrap().use_directml);
            assert_eq!(worker.spawned, 2);
            assert_eq!(worker.reaped, 1);
            worker.shutdown();
            assert_eq!(worker.reaped, 2);
            let loads = std::fs::read_to_string(state.with_extension("loads")).unwrap();
            assert_eq!(loads.lines().collect::<Vec<_>>(), ["True", "False"]);
        }
    }

    #[test]
    #[cfg(windows)]
    fn retry_reload_shares_the_transcribe_attempt_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let mut worker = fake("reload-stall", &directory.path().join("state"));
        worker.load(spec(false), &|| false).unwrap();
        // The normal 180 s load budget must not be stacked on the retry.
        let started = Instant::now();
        assert_eq!(
            worker
                .transcribe(vec![], Duration::from_secs(8), &|| false)
                .unwrap_err(),
            WorkerError::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(25));
        assert_eq!(worker.spawned, 2);
        assert_eq!(worker.reaped, 2);
        assert!(worker.child.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn garbage_stdout_is_protocol_error_and_stops_child() {
        let directory = tempfile::tempdir().unwrap();
        let mut worker = fake("garbage", &directory.path().join("state"));
        worker.load(spec(false), &|| false).unwrap();
        assert_eq!(
            worker
                .transcribe(vec![], Duration::from_secs(10), &|| false)
                .unwrap_err(),
            WorkerError::Protocol
        );
        assert_eq!(worker.spawned, 1);
        assert_eq!(worker.reaped, 1);
        assert!(worker.child.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn second_crash_returns_error_without_a_third_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let mut worker = fake("crash-always", &directory.path().join("state"));
        worker.load(spec(true), &|| false).unwrap();
        assert!(matches!(
            worker.transcribe(vec![], Duration::from_secs(10), &|| false),
            Err(WorkerError::Crashed)
        ));
        assert_eq!(worker.spawned, 2);
        assert_eq!(worker.reaped, 2);
        assert!(worker.child.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn killed_or_absent_child_is_reloaded_on_next_segment() {
        let directory = tempfile::tempdir().unwrap();
        let mut worker = fake("ok", &directory.path().join("state"));
        worker.load(spec(false), &|| false).unwrap();
        worker.kill_child().unwrap();
        assert_eq!(
            worker
                .transcribe(vec![], Duration::from_secs(10), &|| false)
                .unwrap()
                .text,
            "test"
        );
        worker.stop_child();
        assert_eq!(
            worker
                .transcribe(vec![], Duration::from_secs(10), &|| false)
                .unwrap()
                .text,
            "test"
        );
        assert_eq!(worker.spawned, 3);
    }

    #[test]
    #[cfg(windows)]
    fn protocol_and_engine_errors_are_not_retried() {
        for (mode, expected) in [
            ("protocol", WorkerError::Protocol),
            ("engine", WorkerError::Engine("Decode failed".into())),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut worker = fake(mode, &directory.path().join("state"));
            worker.load(spec(false), &|| false).unwrap();
            assert_eq!(
                worker
                    .transcribe(vec![], Duration::from_secs(10), &|| false)
                    .unwrap_err(),
                expected
            );
            assert_eq!(worker.spawned, 1);
            if mode == "protocol" {
                assert_eq!(worker.reaped, 1);
                assert!(worker.child.is_none());
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn blocked_pcm_write_obeys_timeout_and_cancellation() {
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut worker = fake("not-reading", &directory.path().join("state"));
            worker.load(spec(false), &|| false).unwrap();
            let started = Instant::now();
            let result = worker.transcribe(vec![0.0; 262_144], Duration::from_millis(250), &|| {
                cancel && started.elapsed() >= Duration::from_millis(100)
            });
            assert_eq!(
                result.unwrap_err(),
                if cancel {
                    WorkerError::Cancelled
                } else {
                    WorkerError::TimedOut
                }
            );
            assert!(started.elapsed() < Duration::from_secs(15));
            assert_eq!(worker.spawned, if cancel { 1 } else { 2 });
            assert_eq!(worker.reaped, worker.spawned);
            assert!(worker.child.is_none());
        }
    }
}
