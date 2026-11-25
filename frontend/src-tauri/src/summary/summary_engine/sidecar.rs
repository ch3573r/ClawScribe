// Sidecar process lifecycle management for llama-helper
// Handles spawning, health checking, keep-alive, and graceful shutdown

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::models;

// ============================================================================
// Sidecar State Management
// ============================================================================

/// Sidecar process manager with keep-alive and health monitoring
pub struct SidecarManager {
    /// Child process handle
    child_process: Arc<Mutex<Option<Child>>>,

    /// Stdin writer for sending requests
    stdin_writer: Arc<Mutex<Option<ChildStdin>>>,

    /// Stdout reader for receiving responses
    stdout_reader: Arc<Mutex<Option<BufReader<ChildStdout>>>>,

    /// Own both halves of one JSONL exchange; the protocol has no request IDs.
    request_lock: Arc<Mutex<()>>,

    /// Last activity timestamp
    last_activity: Arc<RwLock<Instant>>,

    /// Health status
    is_healthy: Arc<AtomicBool>,

    /// Cancels the health and idle tasks that belong to the current spawn.
    /// Each spawn installs a fresh token so tasks never outlive their process.
    background: Arc<std::sync::Mutex<CancellationToken>>,

    /// Serialize process and pipe replacement with teardown.
    lifecycle: Arc<Mutex<()>>,

    /// Once detached, this manager can finish existing work but cannot spawn.
    retired: Arc<AtomicBool>,

    /// Reject queued exchanges while the lifecycle owner is cleaning up.
    quarantined: Arc<AtomicBool>,

    #[cfg(windows)]
    job: Arc<crate::windows_job::Job>,

    /// Active request count (for graceful shutdown)
    active_request_count: Arc<AtomicUsize>,

    /// Path to llama-helper binary
    helper_binary_path: PathBuf,

    /// Current model path (if loaded)
    current_model_path: Arc<RwLock<Option<PathBuf>>>,

    /// Idle timeout in seconds (configurable via env var)
    idle_timeout_secs: u64,
}

/// RAII guard for tracking active requests
/// Decrements the active request count when dropped
struct RequestGuard {
    counter: Arc<AtomicUsize>,
}

impl RequestGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

fn bundled_helper_names() -> [String; 2] {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let platform = if cfg!(windows) {
        "pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-gnu"
    };
    [
        format!("llama-helper{suffix}"),
        format!("llama-helper-{}-{platform}{suffix}", std::env::consts::ARCH),
    ]
}

fn resolve_helper_from(
    debug: bool,
    exe_dir: &std::path::Path,
    resource_dir: &std::path::Path,
    overrides: [Option<PathBuf>; 3],
) -> Result<PathBuf> {
    let names = bundled_helper_names();
    if debug {
        if let Some(path) = &overrides[0] {
            if path.is_file() {
                return Ok(path.clone());
            }
        }
    }
    let mut directories = vec![exe_dir.to_path_buf(), resource_dir.to_path_buf()];
    if debug {
        if let Some(path) = &overrides[1] {
            directories.push(path.clone());
        }
        if let Some(manifest) = &overrides[2] {
            directories.push(manifest.join("binaries"));
            if let Some(root) = manifest.parent().and_then(|dir| dir.parent()) {
                directories.push(root.join("target/release"));
                directories.push(root.join("target/debug"));
            }
        }
    }
    for directory in directories {
        for name in &names {
            let path = directory.join(name);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(anyhow!(
        "Bundled local summary engine is missing. Repair or reinstall ClawScribe."
    ))
}

impl SidecarManager {
    /// Create a new sidecar manager
    pub fn new(_app_data_dir: PathBuf) -> Result<Self> {
        let helper_binary_path = Self::resolve_helper_binary()?;

        // Get idle timeout from env var or use default
        let idle_timeout_secs = std::env::var("LLAMA_IDLE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(models::DEFAULT_IDLE_TIMEOUT_SECS);

        log::info!(
            "SidecarManager initialized with idle timeout: {}s",
            idle_timeout_secs
        );
        log::info!("Helper binary path: {}", helper_binary_path.display());

        Ok(Self {
            child_process: Arc::new(Mutex::new(None)),
            stdin_writer: Arc::new(Mutex::new(None)),
            stdout_reader: Arc::new(Mutex::new(None)),
            request_lock: Arc::new(Mutex::new(())),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            is_healthy: Arc::new(AtomicBool::new(false)),
            background: Arc::new(std::sync::Mutex::new(cancelled_token())),
            lifecycle: Arc::new(Mutex::new(())),
            retired: Arc::new(AtomicBool::new(false)),
            quarantined: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            helper_binary_path,
            #[cfg(windows)]
            job: Arc::new(
                crate::windows_job::Job::new().context("Could not create local helper job")?,
            ),
            current_model_path: Arc::new(RwLock::new(None)),
            idle_timeout_secs,
        })
    }

    /// Production accepts only the exact packaged helper names in app-owned locations.
    fn resolve_helper_binary() -> Result<PathBuf> {
        let exe = std::env::current_exe()?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| anyhow!("Could not locate application directory"))?;
        let resources = if cfg!(target_os = "macos") {
            exe_dir.join("../Resources")
        } else {
            exe_dir.join("resources")
        };
        #[cfg(debug_assertions)]
        let overrides = [
            std::env::var_os("MEETILY_LLAMA_HELPER").map(PathBuf::from),
            std::env::var_os("RESOURCE_DIR").map(PathBuf::from),
            Some(
                std::env::var_os("CARGO_MANIFEST_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
            ),
        ];
        #[cfg(not(debug_assertions))]
        let overrides = [None, None, None];
        resolve_helper_from(cfg!(debug_assertions), exe_dir, &resources, overrides)
    }

    /// Ensure sidecar is running, spawn if needed
    pub async fn ensure_running(&self, model_path: PathBuf) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        self.check_retired()?;
        // Check if already running with correct model
        {
            let current_model = self.current_model_path.read().await;
            if current_model.as_ref() == Some(&model_path)
                && self.is_healthy()
                && self.child_running().await
            {
                log::debug!("Sidecar already running with correct model");
                self.update_activity().await;
                return Ok(());
            }
        }

        // Need to spawn or restart
        self.spawn(model_path).await
    }

    /// Spawn the sidecar process
    async fn spawn(&self, model_path: PathBuf) -> Result<()> {
        // Shutdown existing process if running
        self.shutdown_locked(SHUTDOWN_DEADLINE).await?;
        self.check_retired()?;

        log::info!("Spawning llama-helper sidecar");
        log::info!("Model path: {}", model_path.display());

        #[cfg(unix)]
        let mut command = tokio::process::Command::new("nice");

        #[cfg(not(unix))]
        let mut command = tokio::process::Command::new(&self.helper_binary_path);

        #[cfg(unix)]
        command.arg("-n").arg("10").arg(&self.helper_binary_path);

        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // Log stderr to main process
            .env(
                "LLAMA_IDLE_TIMEOUT",
                helper_idle_backstop_secs(self.idle_timeout_secs).to_string(),
            );

        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;

            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }

        // Retain the manager slot before spawning; no await may lose a new child.
        let mut child_lock = self.child_process.lock().await;
        self.check_retired()?;
        let mut child = command.spawn().with_context(|| {
            format!(
                "Failed to spawn llama-helper at {:?}",
                self.helper_binary_path
            )
        })?;

        #[cfg(windows)]
        {
            use std::os::windows::io::BorrowedHandle;
            let handle = child
                .raw_handle()
                .ok_or_else(|| anyhow!("Local helper exited during startup"))?;
            // SAFETY: child owns this handle for the duration of assignment.
            self.job
                .assign(unsafe { BorrowedHandle::borrow_raw(handle) })
                .context("Could not contain the local helper process")?;
        }

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to get stdout"))?;

        // Store handles
        *child_lock = Some(child);
        drop(child_lock);

        {
            let mut stdin_lock = self.stdin_writer.lock().await;
            *stdin_lock = Some(stdin);
        }

        {
            let mut stdout_lock = self.stdout_reader.lock().await;
            *stdout_lock = Some(BufReader::new(stdout));
        }

        // Update state
        {
            let mut current_model = self.current_model_path.write().await;
            *current_model = Some(model_path);
        }

        self.is_healthy.store(true, Ordering::SeqCst);
        self.update_activity().await;

        log::info!("Sidecar spawned successfully");

        // Start background tasks bound to this spawn only. The preceding
        // shutdown() cancelled the previous spawn's tasks.
        let token = self.replace_background_token();
        self.start_health_check_loop(token.clone());
        self.start_idle_check_loop(token);

        Ok(())
    }

    /// Send a request to the sidecar and wait for response.
    pub async fn send_request(&self, request_json: String, timeout: Duration) -> Result<String> {
        self.send_request_cancellable(request_json, timeout, None, None)
            .await
    }

    pub async fn send_request_cancellable(
        &self,
        request_json: String,
        timeout: Duration,
        model: Option<PathBuf>,
        token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if self.quarantined.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "Local summary helper is quarantined until cleanup completes"
            ));
        }
        let _guard = RequestGuard::new(self.active_request_count.clone());
        let fallback_token = tokio_util::sync::CancellationToken::new();
        let token = token.unwrap_or(&fallback_token);
        // Waiting requests have no ownership of the helper and must never kill it.
        let _exchange = tokio::select! {
            biased;
            _ = token.cancelled() => return Err(anyhow!("Generation cancelled while queued")),
            lock = self.request_lock.lock() => lock,
        };
        self.check_retired()?;
        if self.quarantined.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "Local summary helper is quarantined until cleanup completes"
            ));
        }
        let result = tokio::select! {
            biased;
            _ = token.cancelled() => None,
            result = async {
                // Model switches and process startup belong to the exchange owner too.
                if let Some(path) = &model { self.ensure_running(path.clone()).await?; }
                let started = Instant::now();
                let result = self.exchange_request(&request_json, timeout).await;
                // A sleeping machine can miss the parent's idle timer. The
                // helper may already have written Goodbye or closed its pipes.
                // Retry only the idle handshake or an immediate empty disconnect.
                if let (Some(model), Err(error)) = (model, &result) {
                    if error.is::<RetryableHelperDisconnect>() {
                        self.ensure_running_after_disconnect(model).await?;
                        return self.exchange_request(
                            &request_json,
                            timeout.saturating_sub(started.elapsed()),
                        ).await;
                    }
                }
                result
            } => Some(result),
        };
        match result {
            Some(Ok(result)) => Ok(result),
            Some(Err(error)) => {
                self.shutdown().await?;
                Err(error)
            }
            None => {
                // Keep the exchange guard until the cancelled worker has stopped.
                self.shutdown().await?;
                Err(anyhow!("Generation cancelled"))
            }
        }
    }

    async fn ensure_running_after_disconnect(&self, model: PathBuf) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        self.check_retired()?;
        self.spawn(model).await
    }

    async fn exchange_request(&self, request_json: &str, timeout: Duration) -> Result<String> {
        if timeout.is_zero() {
            return Err(anyhow!("Request timed out"));
        }
        tokio::time::timeout(timeout, async {
            {
                let mut stdin_lock = self.stdin_writer.lock().await;
                let stdin = stdin_lock
                    .as_mut()
                    .ok_or_else(|| anyhow!("Sidecar not running"))?;
                stdin
                    .write_all(request_json.as_bytes())
                    .await
                    .context("Failed to write request to stdin")?;
                stdin
                    .write_all(b"\n")
                    .await
                    .context("Failed to write newline")?;
                stdin.flush().await.context("Failed to flush stdin")?;
            }

            let response = self.read_response(Some(Instant::now())).await?;
            if is_goodbye(&response) {
                return Err(RetryableHelperDisconnect("Local summary helper went idle").into());
            }
            self.update_activity().await;
            Ok(response)
        })
        .await
        .map_err(|_| anyhow!("Request timed out after {timeout:?}"))?
    }

    /// Read a single line response from stdout
    async fn read_response(&self, written_at: Option<Instant>) -> Result<String> {
        let mut stdout_lock = self.stdout_reader.lock().await;
        let reader = stdout_lock
            .as_mut()
            .ok_or_else(|| anyhow!("Sidecar not running"))?;

        read_response_line(reader, written_at).await
    }

    /// Send ping to keep sidecar alive
    async fn send_ping(&self) -> Result<()> {
        // A health check must neither race a generation's response nor queue
        // behind an expensive inference. The next interval can try again.
        let Ok(_exchange) = self.request_lock.try_lock() else {
            return Ok(());
        };
        let request = serde_json::json!({"type": "ping"}).to_string();
        let timeout = Duration::from_secs(5);

        // Note: We don't use send_request here to avoid incrementing active_request_count
        // for internal health checks, as that would prevent graceful shutdown

        let result: Result<()> = async {
            // Write request
            {
                let mut stdin_lock = self.stdin_writer.lock().await;
                if let Some(stdin) = stdin_lock.as_mut() {
                    stdin.write_all(request.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await?;
                } else {
                    return Err(anyhow!("Sidecar not running"));
                }
            }

            // Read response
            let response = tokio::time::timeout(timeout, self.read_response(None)).await??;

            let resp: serde_json::Value = serde_json::from_str(&response)?;
            if resp.get("type").and_then(|t| t.as_str()) == Some("pong") {
                Ok(())
            } else {
                Err(anyhow!(
                    "Unexpected response to the local summary engine health check"
                ))
            }
        }
        .await;
        if result.is_err() {
            // A late pong must never become the next generation's response.
            // shutdown does not acquire request_lock, so it can reset this
            // connection before ownership passes to a queued generation.
            let _ = self.shutdown().await;
        }
        result
    }

    pub(super) fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.cancel_background();
    }

    fn check_retired(&self) -> Result<()> {
        super::client::check_exit_admission()?;
        if self.retired.load(Ordering::SeqCst) {
            return Err(anyhow!("Local summary helper is stopping"));
        }
        Ok(())
    }

    /// Gracefully shutdown the sidecar
    /// Waits for active requests to complete before killing the process
    pub async fn shutdown_gracefully(&self) -> Result<()> {
        log::info!("Initiating graceful shutdown of sidecar");
        self.retire();

        // Wait for active requests to complete
        // We poll every 500ms
        let start = Instant::now();
        let max_wait = Duration::from_secs(600); // Wait up to 10 minutes for long generations

        loop {
            let count = self.active_request_count.load(Ordering::SeqCst);
            if count == 0 {
                log::info!("No active requests, proceeding with shutdown");
                break;
            }

            if start.elapsed() > max_wait {
                log::warn!(
                    "Timed out waiting for active requests ({} active), forcing shutdown",
                    count
                );
                break;
            }

            log::debug!("Waiting for {} active requests to complete...", count);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        self.shutdown().await
    }

    /// Force shutdown the sidecar
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_within(SHUTDOWN_DEADLINE).await
    }

    async fn shutdown_within(&self, deadline: Duration) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        self.shutdown_locked(deadline).await
    }

    async fn shutdown_locked(&self, deadline: Duration) -> Result<()> {
        self.cancel_background();
        self.quarantined.store(true, Ordering::SeqCst);
        self.is_healthy.store(false, Ordering::SeqCst);
        // Kill the process before attempting to acquire pipe locks. Keep its
        // handle in the manager until exit is confirmed, even on cancellation.
        let confirmed = {
            let mut slot = self.child_process.lock().await;
            let confirmed = match slot.as_mut() {
                Some(child) => kill_and_reap(child, deadline).await,
                None => true,
            };
            if confirmed {
                *slot = None;
            }
            confirmed
        };
        let cleanup = tokio::time::timeout(Duration::from_secs(2), async {
            *self.stdin_writer.lock().await = None;
            *self.stdout_reader.lock().await = None;
            *self.current_model_path.write().await = None;
        })
        .await;
        self.quarantined.store(false, Ordering::SeqCst);
        if !confirmed {
            return Err(anyhow!(
                "Local summary helper did not stop within {} seconds",
                deadline.as_secs()
            ));
        }
        cleanup.context("Local summary helper pipe cleanup timed out")
    }

    async fn child_running(&self) -> bool {
        match self.child_process.lock().await.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn replace_background_token(&self) -> CancellationToken {
        let token = CancellationToken::new();
        let previous = {
            let mut slot = self.background.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *slot, token.clone())
        };
        previous.cancel();
        token
    }

    fn cancel_background(&self) {
        self.background
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
    }

    /// A handle onto the same shared state for a background task.
    fn shared(&self) -> Self {
        Self {
            child_process: self.child_process.clone(),
            stdin_writer: self.stdin_writer.clone(),
            stdout_reader: self.stdout_reader.clone(),
            request_lock: self.request_lock.clone(),
            last_activity: self.last_activity.clone(),
            is_healthy: self.is_healthy.clone(),
            background: self.background.clone(),
            lifecycle: self.lifecycle.clone(),
            retired: self.retired.clone(),
            quarantined: self.quarantined.clone(),
            #[cfg(windows)]
            job: self.job.clone(),
            active_request_count: self.active_request_count.clone(),
            helper_binary_path: self.helper_binary_path.clone(),
            current_model_path: self.current_model_path.clone(),
            idle_timeout_secs: self.idle_timeout_secs,
        }
    }
    /// Check if sidecar is healthy
    pub fn is_healthy(&self) -> bool {
        self.is_healthy.load(Ordering::SeqCst)
    }

    /// Update last activity timestamp
    async fn update_activity(&self) {
        let mut last_activity = self.last_activity.write().await;
        *last_activity = Instant::now();
    }

    /// Get seconds since last activity
    async fn seconds_since_activity(&self) -> u64 {
        let last_activity = self.last_activity.read().await;
        last_activity.elapsed().as_secs()
    }

    /// Start health check loop (runs in background until `token` is cancelled)
    fn start_health_check_loop(&self, token: CancellationToken) {
        let manager = self.shared();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEALTH_CHECK_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = interval.tick() => {}
                }

                if !manager.is_healthy() {
                    log::debug!("Health check loop: sidecar unhealthy, skipping ping");
                    continue;
                }

                // Don't ping if we are busy with a request
                if manager.active_request_count.load(Ordering::SeqCst) > 0 {
                    continue;
                }

                log::debug!("Health check: sending ping");
                // Not raced against the token: dropping send_ping between its
                // write and read would leave a stray pong for the next
                // request. It is bounded by its own 5 s timeout.
                let ping = manager.send_ping().await;
                if token.is_cancelled() {
                    break;
                }
                if let Err(e) = ping {
                    log::warn!("Health check failed: {}", e);
                    manager.is_healthy.store(false, Ordering::SeqCst);
                }
            }

            log::debug!("Health check loop exited");
        });
    }

    /// Start idle check loop (runs in background until `token` is cancelled)
    fn start_idle_check_loop(&self, token: CancellationToken) {
        let manager = self.shared();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(IDLE_CHECK_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = interval.tick() => {}
                }

                // Don't shutdown if we are busy
                if manager.active_request_count.load(Ordering::SeqCst) > 0 {
                    // Update activity to prevent timeout immediately after request finishes
                    manager.update_activity().await;
                    continue;
                }

                let idle_secs = manager.seconds_since_activity().await;
                log::debug!("Idle check: {}s since last activity", idle_secs);

                if idle_secs > manager.idle_timeout_secs {
                    // Own the exchange so no request for this helper is
                    // mid-flight, then re-check: a newer spawn or a request
                    // may have arrived since the tick.
                    let Ok(_exchange) = manager.request_lock.try_lock() else {
                        continue;
                    };
                    if token.is_cancelled() {
                        break;
                    }
                    if manager.active_request_count.load(Ordering::SeqCst) > 0
                        || manager.seconds_since_activity().await <= manager.idle_timeout_secs
                    {
                        continue;
                    }
                    log::info!(
                        "Sidecar idle for {}s (timeout: {}s), shutting down",
                        idle_secs,
                        manager.idle_timeout_secs
                    );

                    if let Err(e) = manager.shutdown().await {
                        log::error!("Failed to shutdown idle sidecar: {}", e);
                    }

                    break;
                }
            }

            log::debug!("Idle check loop exited");
        });
    }
}

const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// The app ends an idle helper itself. The helper's own idle exit is only a
/// backstop for an orphaned helper, so it must fire after the app's idle task
/// would have acted; otherwise a request could write to an exited helper.
fn helper_idle_backstop_secs(app_idle_timeout_secs: u64) -> u64 {
    app_idle_timeout_secs
        .saturating_add(2 * IDLE_CHECK_INTERVAL.as_secs())
        .saturating_add(HEALTH_CHECK_INTERVAL.as_secs())
}
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);

const IDLE_DISCONNECT_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct RetryableHelperDisconnect(&'static str);

async fn read_response_line(
    reader: impl tokio::io::AsyncBufRead + Unpin,
    written_at: Option<Instant>,
) -> Result<String> {
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    const CLOSED: &str = "Sidecar closed stdout (process may have crashed)";
    let mut line = Vec::new();
    let read = reader
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .await;
    let closed = match &read {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
        ),
        Ok(_) => false,
    };
    if closed
        && line.is_empty()
        && written_at.is_some_and(|at| at.elapsed() <= IDLE_DISCONNECT_WINDOW)
    {
        return Err(RetryableHelperDisconnect(CLOSED).into());
    }
    read.context("Failed to read response from stdout")?;
    if line.len() > MAX_RESPONSE_BYTES {
        return Err(anyhow!("Local helper response exceeds its protocol limit"));
    }
    if !line.ends_with(b"\n") {
        return Err(anyhow!(CLOSED));
    }
    Ok(String::from_utf8(line)
        .map_err(|_| anyhow!("Local helper response is not UTF-8"))?
        .trim()
        .to_string())
}

fn is_goodbye(response: &str) -> bool {
    // The helper emits a short JSONL control frame. Skip parsing large answers here.
    response.len() <= 32
        && serde_json::from_str::<serde_json::Value>(response)
            .is_ok_and(|value| value == serde_json::json!({"type": "goodbye"}))
}

fn cancelled_token() -> CancellationToken {
    let token = CancellationToken::new();
    token.cancel();
    token
}

/// Kills `child` and waits for it, retrying until `deadline` elapses.
/// Returns whether the exit was confirmed.
async fn kill_and_reap(child: &mut Child, deadline: Duration) -> bool {
    let started = Instant::now();
    loop {
        let _ = child.start_kill();
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        match tokio::time::timeout(remaining.min(Duration::from_secs(1)), child.wait()).await {
            Ok(Ok(_)) => return true,
            Ok(Err(error)) => {
                log::warn!("Waiting for the local summary helper failed: {}", error);
            }
            Err(_) => {}
        }
        if started.elapsed() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn only_an_immediate_disconnect_without_response_bytes_is_retryable() {
        let recent = Instant::now();
        let old = recent - IDLE_DISCONNECT_WINDOW - Duration::from_secs(1);
        for (response, written_at, retryable) in [
            (&b""[..], recent, true),
            (&b""[..], old, false),
            (&b"{\"type\":"[..], recent, false),
        ] {
            let error = read_response_line(response, Some(written_at))
                .await
                .unwrap_err();
            assert_eq!(error.is::<RetryableHelperDisconnect>(), retryable);
        }
    }

    #[test]
    fn helper_idle_exit_is_a_backstop_after_the_app_idle_task() {
        let app = 300;
        let helper = helper_idle_backstop_secs(app);
        assert!(helper >= app + IDLE_CHECK_INTERVAL.as_secs() + HEALTH_CHECK_INTERVAL.as_secs());
        assert_eq!(helper_idle_backstop_secs(u64::MAX), u64::MAX);
    }

    #[test]
    fn replacing_the_background_token_cancels_the_previous_spawn() {
        let background = Arc::new(std::sync::Mutex::new(cancelled_token()));
        let first = CancellationToken::new();
        *background.lock().unwrap() = first.clone();
        let next = CancellationToken::new();
        let previous = std::mem::replace(&mut *background.lock().unwrap(), next.clone());
        previous.cancel();
        assert!(first.is_cancelled());
        assert!(!next.is_cancelled());
    }

    #[tokio::test]
    async fn kill_and_reap_confirms_a_killable_child() {
        #[cfg(windows)]
        let mut command = tokio::process::Command::new("cmd.exe");
        #[cfg(windows)]
        command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
        #[cfg(not(windows))]
        let mut command = tokio::process::Command::new("sleep");
        #[cfg(not(windows))]
        command.arg("30");
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert!(kill_and_reap(&mut child, Duration::from_secs(10)).await);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn kill_and_reap_gives_up_at_a_zero_deadline() {
        #[cfg(windows)]
        let mut command = tokio::process::Command::new("cmd.exe");
        #[cfg(windows)]
        command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
        #[cfg(not(windows))]
        let mut command = tokio::process::Command::new("sleep");
        #[cfg(not(windows))]
        command.arg("30");
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        // Bounded: returns instead of looping forever.
        assert!(!kill_and_reap(&mut child, Duration::ZERO).await);
    }
}

#[cfg(all(test, windows))]
pub(super) mod protocol_tests {
    use super::*;
    use tokio::process::{ChildStderr, Command};

    pub(in crate::summary::summary_engine) async fn fake_sidecar(
        script: &str,
    ) -> (Arc<SidecarManager>, BufReader<ChildStderr>) {
        // Separate process startup from the protocol deadlines being tested.
        let script = format!("[Console]::Error.WriteLine('started');\n{script}");
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script.as_str(),
            ])
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut started = String::new();
        tokio::time::timeout(Duration::from_secs(15), stderr.read_line(&mut started))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(started.trim(), "started");
        let manager = Arc::new(SidecarManager {
            child_process: Arc::new(Mutex::new(Some(child))),
            stdin_writer: Arc::new(Mutex::new(Some(stdin))),
            stdout_reader: Arc::new(Mutex::new(Some(BufReader::new(stdout)))),
            request_lock: Arc::new(Mutex::new(())),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            is_healthy: Arc::new(AtomicBool::new(true)),
            background: Arc::new(std::sync::Mutex::new(cancelled_token())),
            lifecycle: Arc::new(Mutex::new(())),
            retired: Arc::new(AtomicBool::new(false)),
            quarantined: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            helper_binary_path: PathBuf::new(),
            job: Arc::new(crate::windows_job::Job::new().unwrap()),
            current_model_path: Arc::new(RwLock::new(None)),
            idle_timeout_secs: 600,
        });
        (manager, stderr)
    }

    #[tokio::test]
    async fn retired_manager_cannot_start_another_helper() {
        let (manager, _) = fake_sidecar("Start-Sleep -Seconds 60").await;
        let lifecycle = manager.lifecycle.lock().await;
        let requester = manager.clone();
        let queued =
            tokio::spawn(async move { requester.ensure_running("synthetic".into()).await });
        manager.retire();
        drop(lifecycle);
        assert!(queued
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("stopping"));
        manager.shutdown().await.unwrap();
        assert!(manager.child_process.lock().await.is_none());
    }

    #[tokio::test]
    async fn unsolicited_idle_goodbye_is_handled_as_a_closed_helper() {
        let (manager, _) = fake_sidecar(
            "[Console]::In.ReadLine() | Out-Null; [Console]::Out.WriteLine('{\"type\":\"goodbye\"}')",
        ).await;
        let error = manager
            .send_request("{}".into(), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("went idle"));
        assert!(error.is::<RetryableHelperDisconnect>());
        assert!(manager.child_process.lock().await.is_none());
    }

    #[tokio::test]
    async fn queued_cancellation_leaves_the_running_exchange_intact() {
        let (manager, mut stderr) = fake_sidecar(
            r#"
[Console]::In.ReadLine() | Out-Null
[Console]::Error.WriteLine('ready')
Start-Sleep -Milliseconds 700
[Console]::Out.WriteLine('{"type":"response","text":"Running answer","error":null}')
[Console]::In.ReadLine() | Out-Null
"#,
        )
        .await;
        let running = manager.clone();
        let task = tokio::spawn(async move {
            running
                .send_request("{}".into(), Duration::from_secs(5))
                .await
        });
        let mut ready = String::new();
        stderr.read_line(&mut ready).await.unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let queued_manager = manager.clone();
        let queued_token = token.clone();
        let queued = tokio::spawn(async move {
            queued_manager
                .send_request_cancellable(
                    "{}".into(),
                    Duration::from_secs(5),
                    None,
                    Some(&queued_token),
                )
                .await
        });
        while manager.active_request_count.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        token.cancel();
        assert!(queued
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("queued"));
        assert!(manager.is_healthy());
        assert!(task.await.unwrap().unwrap().contains("Running answer"));
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_the_owner_stops_the_helper_before_unlocking() {
        let (manager, mut stderr) = fake_sidecar("[Console]::In.ReadLine() | Out-Null; [Console]::Error.WriteLine('ready'); Start-Sleep -Seconds 60").await;
        let token = tokio_util::sync::CancellationToken::new();
        let running = manager.clone();
        let running_token = token.clone();
        let task = tokio::spawn(async move {
            running
                .send_request_cancellable(
                    "{}".into(),
                    Duration::from_secs(60),
                    None,
                    Some(&running_token),
                )
                .await
        });
        let mut ready = String::new();
        stderr.read_line(&mut ready).await.unwrap();
        token.cancel();
        let error = tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(!manager.is_healthy());
        assert!(manager.request_lock.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cancellation_reaps_a_helper_that_does_not_read_stdin() {
        let (manager, _stderr) = fake_sidecar("Start-Sleep -Seconds 60").await;
        let token = tokio_util::sync::CancellationToken::new();
        let running = manager.clone();
        let running_token = token.clone();
        let task = tokio::spawn(async move {
            running
                .send_request_cancellable(
                    "x".repeat(2 * 1024 * 1024),
                    Duration::from_secs(60),
                    None,
                    Some(&running_token),
                )
                .await
        });
        // A full pipe makes the generation write pending, independently of inference.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(manager.request_lock.try_lock().is_err());
        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("Cancellation cleanup must not perform an unbounded polite stdin write")
            .unwrap();
        assert!(result.is_err());
        assert!(
            manager.child_process.lock().await.is_none(),
            "Only an actually reaped child may release the exchange"
        );
        assert_eq!(manager.active_request_count.load(Ordering::SeqCst), 0);
        assert!(manager.request_lock.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cleanup_quarantine_retains_exchange_and_job_admission_until_actual_reap() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let (manager, _stderr) =
            fake_sidecar("[Console]::In.ReadLine() | Out-Null; Start-Sleep -Seconds 60").await;
        // Simulate an unavailable reap path while retaining the real child handle.
        let child_guard = manager.child_process.lock().await;
        let running = manager.clone();
        let (completed, done) = tokio::sync::oneshot::channel();
        let result = crate::summary::llm_client::supervise(
            &tokio_util::sync::CancellationToken::new(),
            tokio::time::Instant::now() + Duration::from_millis(100),
            Duration::from_millis(25),
            move |token| async move {
                let _job = crate::audio::inference::claim_job()?;
                let result = running
                    .send_request_cancellable(
                        "{}".into(),
                        Duration::from_secs(60),
                        None,
                        Some(&token),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let _ = completed.send(());
                result
            },
        )
        .await;
        let retained = manager.quarantined.load(Ordering::SeqCst)
            && manager.request_lock.try_lock().is_err()
            && crate::audio::inference::claim_job().is_err();
        let rejected = manager
            .send_request("{}".into(), Duration::from_millis(10))
            .await
            .unwrap_err()
            .to_string();
        drop(child_guard);
        tokio::time::timeout(Duration::from_secs(5), done)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while manager.active_request_count.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(result.unwrap_err().contains("quarantined"));
        assert!(
            retained,
            "Cleanup timeout must retain child/exchange/JOBS ownership"
        );
        assert!(rejected.contains("quarantined"));
        assert!(manager.child_process.lock().await.is_none());
        assert!(!manager.quarantined.load(Ordering::SeqCst));
        let _next_job = crate::audio::inference::claim_job().unwrap();
        let (replacement,_)=fake_sidecar("[Console]::In.ReadLine() | Out-Null; [Console]::Out.WriteLine('{\"type\":\"response\",\"text\":\"Next answer\",\"error\":null}')").await;
        let next = tokio::time::timeout(
            Duration::from_secs(5),
            replacement.send_request("{}".into(), Duration::from_secs(2)),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), replacement.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(next.unwrap().unwrap().contains("Next answer"));
    }

    #[tokio::test]
    async fn helper_response_retention_is_bounded_before_json_parsing() {
        let (manager,_)=fake_sidecar("[Console]::In.ReadLine() | Out-Null; [Console]::Out.WriteLine(('x' * 2097152)); Start-Sleep -Seconds 60").await;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            manager.send_request("{}".into(), Duration::from_secs(3)),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), manager.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.unwrap().is_err(),
            "Oversized protocol line must fail without retaining unbounded text"
        );
    }

    #[tokio::test]
    async fn health_check_and_generation_cannot_overlap_jsonl_exchanges() {
        let script = r#"
$reader = [System.IO.StreamReader]::new([Console]::OpenStandardInput())
$first = $reader.ReadLine()
[Console]::Error.WriteLine('ready')
$next = $reader.ReadLineAsync()
if ($next.Wait(300)) {
    [Console]::Out.WriteLine('{"type":"error","message":"overlapping requests"}')
    [Console]::Out.WriteLine('{"type":"error","message":"overlapping requests"}')
    exit
}
[Console]::Out.WriteLine('{"type":"pong"}')
$line = $next.GetAwaiter().GetResult()
[Console]::Out.WriteLine('{"type":"response","text":"Complete answer","error":null}')
"#;
        let (manager, mut stderr) = fake_sidecar(script).await;
        let ping_manager = manager.clone();
        let ping = tokio::spawn(async move { ping_manager.send_ping().await });
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let result = manager
            .send_request(
                r#"{"type":"generate","prompt":"Question"}"#.into(),
                Duration::from_secs(5),
            )
            .await;
        let ping_result = ping.await.unwrap();
        manager.shutdown().await.unwrap();
        ping_result.unwrap();
        let response: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(response["text"], "Complete answer");
    }

    #[tokio::test]
    async fn failed_health_exchange_is_closed_before_the_next_request() {
        let (manager, _stderr) = fake_sidecar(
            r#"
[Console]::In.ReadLine() | Out-Null
[Console]::Out.WriteLine('{"type":"unexpected"}')
Start-Sleep -Seconds 60
"#,
        )
        .await;
        let result = tokio::time::timeout(Duration::from_secs(10), manager.send_ping())
            .await
            .unwrap();
        assert!(result.is_err());
        assert!(!manager.is_healthy());
        assert!(manager
            .send_request(
                r#"{"type":"generate","prompt":"Question"}"#.into(),
                Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("not running"));
    }

    #[tokio::test]
    async fn stalled_exchange_times_out_without_blocking_force_shutdown() {
        let (manager, _stderr) =
            fake_sidecar("[Console]::In.ReadLine() | Out-Null; Start-Sleep -Seconds 60").await;
        let result = tokio::time::timeout(
            Duration::from_secs(6),
            manager.send_request(
                r#"{"type":"generate","prompt":"Question"}"#.into(),
                Duration::from_millis(100),
            ),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert!(!manager.is_healthy());
        assert_eq!(manager.active_request_count.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    #[test]
    fn release_ignores_all_environment_candidates_and_rejects_fuzzy_names() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("app");
        let resources = exe.join("resources");
        let external = temp.path().join("external");
        let manifest = temp.path().join("project/frontend/src-tauri");
        for dir in [&exe, &resources, &external, &manifest.join("binaries")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let names = bundled_helper_names();
        let override_file = external.join(&names[0]);
        std::fs::write(&override_file, "synthetic executable placeholder").unwrap();
        std::fs::write(manifest.join("binaries").join(&names[1]), "placeholder").unwrap();
        std::fs::write(exe.join("llama-helper-untrusted.exe"), "placeholder").unwrap();
        let overrides = [Some(override_file.clone()), Some(external), Some(manifest)];
        assert!(resolve_helper_from(false, &exe, &resources, overrides.clone()).is_err());
        assert_eq!(
            resolve_helper_from(true, &exe, &resources, overrides.clone()).unwrap(),
            override_file
        );
        let bundled = resources.join(&names[0]);
        std::fs::write(&bundled, "placeholder").unwrap();
        assert_eq!(
            resolve_helper_from(false, &exe, &resources, overrides).unwrap(),
            bundled
        );
    }
}
