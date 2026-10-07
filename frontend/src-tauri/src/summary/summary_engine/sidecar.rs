// Sidecar process lifecycle management for llama-helper
// Handles spawning, health checking, keep-alive, and graceful shutdown

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, RwLock};

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

    /// Shutdown flag
    should_shutdown: Arc<AtomicBool>,

    /// A resource whose cleanup has not actually completed cannot be reused.
    quarantined: Arc<AtomicBool>,

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
            should_shutdown: Arc::new(AtomicBool::new(false)),
            quarantined: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            helper_binary_path,
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
        // Check if already running with correct model
        {
            let current_model = self.current_model_path.read().await;
            if current_model.as_ref() == Some(&model_path) && self.is_healthy() {
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
        self.shutdown().await?;

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
            .env("LLAMA_IDLE_TIMEOUT", self.idle_timeout_secs.to_string());

        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;

            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }

        // Retain the manager slot before spawning; no await may lose a new child.
        let mut child_lock = self.child_process.lock().await;
        let mut child = command.spawn().with_context(|| {
            format!(
                "Failed to spawn llama-helper at {:?}",
                self.helper_binary_path
            )
        })?;

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
        self.should_shutdown.store(false, Ordering::SeqCst);
        self.update_activity().await;

        log::info!("Sidecar spawned successfully");

        // Start background tasks
        self.start_health_check_loop();
        self.start_idle_check_loop();

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
                if let Some(model) = model { self.ensure_running(model).await?; }
                self.exchange_request(request_json, timeout).await
            } => Some(result),
        };
        match result {
            Some(result) => result,
            None => {
                // Keep the exchange guard until the cancelled worker has stopped.
                self.shutdown().await?;
                Err(anyhow!("Generation cancelled"))
            }
        }
    }

    async fn exchange_request(&self, request_json: String, timeout: Duration) -> Result<String> {
        // Write request to stdin
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

        // Read response from stdout with timeout
        match tokio::time::timeout(timeout, self.read_response()).await {
            Ok(Ok(response)) => {
                self.update_activity().await;
                Ok(response)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout reached - shutdown sidecar to stop generation
                log::error!("Request timeout after {:?}, shutting down sidecar", timeout);
                if let Err(shutdown_err) = self.shutdown().await {
                    log::error!("Failed to shutdown sidecar after timeout: {}", shutdown_err);
                }
                Err(anyhow!("Request timed out after {:?}", timeout))
            }
        }
    }

    /// Read a single line response from stdout
    async fn read_response(&self) -> Result<String> {
        let mut stdout_lock = self.stdout_reader.lock().await;
        let reader = stdout_lock
            .as_mut()
            .ok_or_else(|| anyhow!("Sidecar not running"))?;

        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .context("Failed to read response from stdout")?;

        if line.is_empty() {
            return Err(anyhow!("Sidecar closed stdout (process may have crashed)"));
        }

        Ok(line.trim().to_string())
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
            let response = tokio::time::timeout(timeout, self.read_response()).await??;

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

    /// Gracefully shutdown the sidecar
    /// Waits for active requests to complete before killing the process
    pub async fn shutdown_gracefully(&self) -> Result<()> {
        log::info!("Initiating graceful shutdown of sidecar");

        // Set shutdown flag to prevent new internal tasks
        self.should_shutdown.store(true, Ordering::SeqCst);

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
        self.should_shutdown.store(true, Ordering::SeqCst);
        self.quarantined.store(true, Ordering::SeqCst);
        self.is_healthy.store(false, Ordering::SeqCst);
        // Do not write a polite message into a potentially full stdin pipe.
        // Keep the child in its manager slot across every await. If startup's
        // waiter is cancelled, the next cleanup owner still has the actual child.
        {
            let mut child_lock = self.child_process.lock().await;
            if let Some(child) = child_lock.as_mut() {
                loop {
                    let _ = child.start_kill();
                    match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                        Ok(Ok(_)) => break,
                        _ => tokio::time::sleep(Duration::from_millis(50)).await,
                    }
                }
            }
            *child_lock = None;
        }
        *self.stdin_writer.lock().await = None;
        *self.stdout_reader.lock().await = None;
        *self.current_model_path.write().await = None;
        self.quarantined.store(false, Ordering::SeqCst);
        Ok(())
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

    /// Start health check loop (runs in background)
    fn start_health_check_loop(&self) {
        let manager = Self {
            child_process: self.child_process.clone(),
            stdin_writer: self.stdin_writer.clone(),
            stdout_reader: self.stdout_reader.clone(),
            request_lock: self.request_lock.clone(),
            last_activity: self.last_activity.clone(),
            is_healthy: self.is_healthy.clone(),
            should_shutdown: self.should_shutdown.clone(),
            quarantined: self.quarantined.clone(),
            active_request_count: self.active_request_count.clone(),
            helper_binary_path: self.helper_binary_path.clone(),
            current_model_path: self.current_model_path.clone(),
            idle_timeout_secs: self.idle_timeout_secs,
        };

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                if manager.should_shutdown.load(Ordering::SeqCst) {
                    log::debug!("Health check loop: shutdown flag set, exiting");
                    break;
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
                if let Err(e) = manager.send_ping().await {
                    log::warn!("Health check failed: {}", e);
                    manager.is_healthy.store(false, Ordering::SeqCst);
                }
            }

            log::debug!("Health check loop exited");
        });
    }

    /// Start idle check loop (runs in background)
    fn start_idle_check_loop(&self) {
        let manager = Self {
            child_process: self.child_process.clone(),
            stdin_writer: self.stdin_writer.clone(),
            stdout_reader: self.stdout_reader.clone(),
            request_lock: self.request_lock.clone(),
            last_activity: self.last_activity.clone(),
            is_healthy: self.is_healthy.clone(),
            should_shutdown: self.should_shutdown.clone(),
            quarantined: self.quarantined.clone(),
            active_request_count: self.active_request_count.clone(),
            helper_binary_path: self.helper_binary_path.clone(),
            current_model_path: self.current_model_path.clone(),
            idle_timeout_secs: self.idle_timeout_secs,
        };

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                if manager.should_shutdown.load(Ordering::SeqCst) {
                    log::debug!("Idle check loop: shutdown flag set, exiting");
                    break;
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

impl Drop for SidecarManager {
    fn drop(&mut self) {
        // Set shutdown flag
        self.should_shutdown.store(true, Ordering::SeqCst);

        // Note: Actual cleanup happens in shutdown() method
        // We can't do async work in Drop, so this is best-effort
        log::debug!("SidecarManager dropped");
    }
}

#[cfg(all(test, windows))]
mod protocol_tests {
    use super::*;
    use tokio::process::{ChildStderr, Command};

    async fn fake_sidecar(script: &str) -> (Arc<SidecarManager>, BufReader<ChildStderr>) {
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
            should_shutdown: Arc::new(AtomicBool::new(false)),
            quarantined: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            helper_binary_path: PathBuf::new(),
            current_model_path: Arc::new(RwLock::new(None)),
            idle_timeout_secs: 600,
        });
        (manager, stderr)
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
