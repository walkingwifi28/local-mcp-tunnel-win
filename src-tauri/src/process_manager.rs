use crate::runtime::{configure_stdio, runtime_for, CommandSpec, RuntimeOptions};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::{Mutex, Notify},
    time::sleep,
};

const PID_MARKER: &str = "__LOCAL_MCP_TUNNEL_PID__:";
const TUNNEL_RESTART_MAX_ATTEMPTS: u32 = 5;
const TUNNEL_RESTART_INITIAL_DELAY: Duration = Duration::from_secs(1);
const TUNNEL_RESTART_MAX_DELAY: Duration = Duration::from_secs(30);
const TUNNEL_RESTART_RESET_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSource {
    Tunnel,
    LocalMcp,
}

impl ProcessSource {
    fn label(self) -> &'static str {
        match self {
            Self::Tunnel => "tunnel-client",
            Self::LocalMcp => "local-mcp",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    max_attempts: u32,
    initial_delay: Duration,
    max_delay: Duration,
    reset_after: Duration,
}

impl RestartPolicy {
    pub const fn disabled() -> Self {
        Self {
            max_attempts: 0,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            reset_after: Duration::MAX,
        }
    }

    pub const fn tunnel() -> Self {
        Self {
            max_attempts: TUNNEL_RESTART_MAX_ATTEMPTS,
            initial_delay: TUNNEL_RESTART_INITIAL_DELAY,
            max_delay: TUNNEL_RESTART_MAX_DELAY,
            reset_after: TUNNEL_RESTART_RESET_AFTER,
        }
    }

    fn enabled(self) -> bool {
        self.max_attempts > 0
    }

    fn delay_for_attempt(self, attempt: u32) -> Option<Duration> {
        if attempt == 0 || attempt > self.max_attempts {
            return None;
        }

        let mut delay = self.initial_delay;
        for _ in 1..attempt {
            delay = delay.saturating_mul(2).min(self.max_delay);
        }
        Some(delay)
    }
}

#[derive(Debug, Default)]
struct RestartBudget {
    attempts: u32,
}

impl RestartBudget {
    fn next(
        &mut self,
        policy: RestartPolicy,
        previous_uptime: Duration,
    ) -> Option<(u32, Duration)> {
        if previous_uptime >= policy.reset_after {
            self.attempts = 0;
        }
        if self.attempts >= policy.max_attempts {
            return None;
        }

        self.attempts += 1;
        policy
            .delay_for_attempt(self.attempts)
            .map(|delay| (self.attempts, delay))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    source: ProcessSource,
    stream: String,
    line: String,
    timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEvent {
    source: ProcessSource,
    state: String,
    pid: Option<u32>,
    error: Option<String>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum Lifecycle {
    Starting = 0,
    Running = 1,
    Stopping = 2,
}

impl Lifecycle {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Starting,
            2 => Self::Stopping,
            _ => Self::Running,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
        }
    }
}

struct RunningProcess {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    runtime_pid: Arc<AtomicU32>,
    host_pid: Option<u32>,
}

struct SpawnedProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    runtime_pid: Arc<AtomicU32>,
    host_pid: Option<u32>,
}

enum InstallResult {
    Installed(Option<u32>),
    Cancelled,
}

#[derive(Clone)]
struct ManagedProcess {
    current: Arc<Mutex<Option<RunningProcess>>>,
    stop_requested: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
    lifecycle: Arc<AtomicU8>,
    spec: CommandSpec,
    runtime_options: RuntimeOptions,
    restart_policy: RestartPolicy,
    redacted_values: Arc<Vec<String>>,
}

#[derive(Clone, Default)]
pub struct ProcessRegistry {
    processes: Arc<Mutex<HashMap<ProcessSource, ManagedProcess>>>,
}

impl ProcessRegistry {
    pub async fn states(&self) -> HashMap<ProcessSource, String> {
        let processes = self.processes.lock().await;
        [ProcessSource::Tunnel, ProcessSource::LocalMcp]
            .into_iter()
            .map(|source| {
                let state = processes
                    .get(&source)
                    .map(|managed| {
                        Lifecycle::from_u8(managed.lifecycle.load(Ordering::SeqCst))
                            .as_str()
                            .to_owned()
                    })
                    .unwrap_or_else(|| "stopped".to_owned());
                (source, state)
            })
            .collect()
    }

    pub async fn start(
        &self,
        app: AppHandle,
        source: ProcessSource,
        spec: CommandSpec,
        runtime_options: RuntimeOptions,
        restart_policy: RestartPolicy,
    ) -> Result<(), String> {
        {
            let processes = self.processes.lock().await;
            if processes.contains_key(&source) {
                return Err(format!("{} is already running.", source.label()));
            }
        }

        emit_state(&app, source, "starting", None, None);
        emit_log(
            &app,
            source,
            "system",
            &format!("Starting {}: {}", source.label(), display_command(&spec)),
        );

        let redacted_values = Arc::new(
            spec.environment
                .values()
                .filter(|value| !value.is_empty())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let spawned = match spawn_process(source, &spec, &runtime_options) {
            Ok(spawned) => spawned,
            Err(error) => {
                emit_state(&app, source, "failed", None, Some(error.clone()));
                return Err(error);
            }
        };
        let host_pid = spawned.host_pid;
        let runtime_pid = spawned.runtime_pid.clone();
        let reader_runtime_pid = runtime_pid.clone();
        let stdout = spawned.stdout;
        let stderr = spawned.stderr;
        let running = RunningProcess {
            child: spawned.child,
            stdin: Arc::new(Mutex::new(spawned.stdin)),
            runtime_pid,
            host_pid,
        };
        let managed = ManagedProcess {
            current: Arc::new(Mutex::new(Some(running))),
            stop_requested: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(Notify::new()),
            lifecycle: Arc::new(AtomicU8::new(Lifecycle::Running as u8)),
            spec,
            runtime_options,
            restart_policy,
            redacted_values,
        };

        self.processes.lock().await.insert(source, managed.clone());
        emit_state(&app, source, "running", host_pid, None);
        spawn_output_reader(
            app.clone(),
            source,
            "stdout",
            stdout,
            reader_runtime_pid.clone(),
            managed.redacted_values.clone(),
        );
        spawn_output_reader(
            app.clone(),
            source,
            "stderr",
            stderr,
            reader_runtime_pid,
            managed.redacted_values.clone(),
        );
        self.spawn_monitor(app, source, managed, Instant::now());
        Ok(())
    }

    fn spawn_monitor(
        &self,
        app: AppHandle,
        source: ProcessSource,
        managed: ManagedProcess,
        initial_started_at: Instant,
    ) {
        let registry = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut restart_budget = RestartBudget::default();
            let mut started_at = initial_started_at;

            'monitor: loop {
                let status = loop {
                    let status = {
                        let mut current = managed.current.lock().await;
                        match current.as_mut() {
                            Some(running) => match running.child.try_wait() {
                                Ok(status) => status,
                                Err(error) => {
                                    emit_log(
                                        &app,
                                        source,
                                        "system",
                                        &format!("Failed to read process status: {error}"),
                                    );
                                    None
                                }
                            },
                            None => None,
                        }
                    };

                    if let Some(status) = status {
                        break status;
                    }
                    sleep(Duration::from_millis(200)).await;
                };

                let uptime = started_at.elapsed();
                *managed.current.lock().await = None;
                let code = status.code().unwrap_or(-1);
                emit_log(
                    &app,
                    source,
                    "system",
                    &format!("{} finished (code: {code}).", source.label()),
                );

                let stop_requested = managed.stop_requested.load(Ordering::SeqCst);
                if stop_requested {
                    registry.finish(&app, source, "stopped", None).await;
                    break;
                }

                if !restart_allowed(managed.restart_policy, stop_requested) {
                    if status.success() {
                        registry.finish(&app, source, "stopped", None).await;
                    } else {
                        registry
                            .finish(&app, source, "failed", Some(format!("Exit code: {code}")))
                            .await;
                    }
                    break;
                }

                let mut previous_uptime = uptime;
                let mut last_error = format!("Exit code: {code}");
                loop {
                    let Some((attempt, delay)) =
                        restart_budget.next(managed.restart_policy, previous_uptime)
                    else {
                        let error = format!(
                            "{last_error}; automatic restart limit reached ({} attempts)",
                            managed.restart_policy.max_attempts
                        );
                        emit_log(&app, source, "system", &error);
                        registry.finish(&app, source, "failed", Some(error)).await;
                        break 'monitor;
                    };
                    previous_uptime = Duration::ZERO;

                    managed
                        .lifecycle
                        .store(Lifecycle::Starting as u8, Ordering::SeqCst);
                    emit_state(&app, source, "starting", None, None);
                    emit_log(
                        &app,
                        source,
                        "system",
                        &format!(
                            "Unexpected exit. Restart attempt {attempt}/{} in {} ms.",
                            managed.restart_policy.max_attempts,
                            delay.as_millis()
                        ),
                    );

                    tokio::select! {
                        _ = sleep(delay) => {}
                        _ = managed.stop_notify.notified() => {}
                    }
                    if managed.stop_requested.load(Ordering::SeqCst) {
                        registry.finish(&app, source, "stopped", None).await;
                        break 'monitor;
                    }

                    emit_log(
                        &app,
                        source,
                        "system",
                        &format!(
                            "Restarting {} (attempt {attempt}/{}): {}",
                            source.label(),
                            managed.restart_policy.max_attempts,
                            display_command(&managed.spec)
                        ),
                    );
                    let spawned =
                        match spawn_process(source, &managed.spec, &managed.runtime_options) {
                            Ok(spawned) => spawned,
                            Err(error) => {
                                last_error = error.clone();
                                emit_log(
                                    &app,
                                    source,
                                    "system",
                                    &format!("Restart attempt {attempt} failed: {error}"),
                                );
                                continue;
                            }
                        };

                    match install_spawned_process(&app, source, &managed, spawned).await {
                        Ok(InstallResult::Installed(host_pid)) => {
                            managed
                                .lifecycle
                                .store(Lifecycle::Running as u8, Ordering::SeqCst);
                            emit_state(&app, source, "running", host_pid, None);
                            emit_log(
                                &app,
                                source,
                                "system",
                                &format!("{} restarted successfully.", source.label()),
                            );
                            started_at = Instant::now();
                            continue 'monitor;
                        }
                        Ok(InstallResult::Cancelled) => {
                            registry.finish(&app, source, "stopped", None).await;
                            break 'monitor;
                        }
                        Err(error) => {
                            last_error = error.clone();
                            emit_log(
                                &app,
                                source,
                                "system",
                                &format!("Restart attempt {attempt} failed: {error}"),
                            );
                        }
                    }
                }
            }
        });
    }

    async fn finish(
        &self,
        app: &AppHandle,
        source: ProcessSource,
        state: &str,
        error: Option<String>,
    ) {
        self.processes.lock().await.remove(&source);
        emit_state(app, source, state, None, error);
    }

    pub async fn send(&self, source: ProcessSource, input: &str) -> Result<(), String> {
        let normalized = input.trim_end_matches(['\r', '\n']);
        if normalized.trim().is_empty() {
            return Ok(());
        }

        let managed = self
            .processes
            .lock()
            .await
            .get(&source)
            .cloned()
            .ok_or_else(|| format!("{} is not running.", source.label()))?;
        let stdin = managed
            .current
            .lock()
            .await
            .as_ref()
            .map(|running| running.stdin.clone())
            .ok_or_else(|| format!("{} is restarting.", source.label()))?;

        let mut stdin = stdin.lock().await;
        stdin
            .write_all(format!("{normalized}\n").as_bytes())
            .await
            .map_err(|error| format!("Failed to write to {}: {error}", source.label()))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Failed to flush {} stdin: {error}", source.label()))?;
        Ok(())
    }

    pub async fn stop(&self, app: AppHandle, source: ProcessSource) -> Result<(), String> {
        let managed = self
            .processes
            .lock()
            .await
            .get(&source)
            .cloned()
            .ok_or_else(|| format!("{} is not running.", source.label()))?;

        managed.stop_requested.store(true, Ordering::SeqCst);
        managed.stop_notify.notify_one();
        managed
            .lifecycle
            .store(Lifecycle::Stopping as u8, Ordering::SeqCst);

        let (runtime_pid, host_pid) = managed
            .current
            .lock()
            .await
            .as_ref()
            .map(|running| (running.runtime_pid.load(Ordering::SeqCst), running.host_pid))
            .unwrap_or((0, None));
        emit_state(&app, source, "stopping", host_pid, None);
        emit_log(&app, source, "system", "Manual stop request sent.");

        if runtime_pid > 0 {
            let runtime = runtime_for(&managed.runtime_options);
            if let Some(mut stop_command) = runtime.build_stop_command(runtime_pid) {
                #[cfg(windows)]
                {
                    stop_command.creation_flags(0x08000000);
                }
                let _ = stop_command.status().await;
                sleep(Duration::from_millis(700)).await;
            }
        }

        let mut current = managed.current.lock().await;
        if let Some(running) = current.as_mut() {
            if running
                .child
                .try_wait()
                .map_err(|error| format!("Failed to read process status: {error}"))?
                .is_none()
            {
                running
                    .child
                    .start_kill()
                    .map_err(|error| format!("Failed to stop {}: {error}", source.label()))?;
            }
        }
        Ok(())
    }

    pub async fn stop_all(&self, app: AppHandle) {
        for source in [ProcessSource::Tunnel, ProcessSource::LocalMcp] {
            let _ = self.stop(app.clone(), source).await;
        }
    }
}

fn restart_allowed(policy: RestartPolicy, stop_requested: bool) -> bool {
    policy.enabled() && !stop_requested
}

fn spawn_process(
    source: ProcessSource,
    spec: &CommandSpec,
    runtime_options: &RuntimeOptions,
) -> Result<SpawnedProcess, String> {
    let runtime = runtime_for(runtime_options);
    let mut command = runtime.build_command(spec)?;
    configure_stdio(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start {}: {error}", source.label()))?;

    let host_pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open process stdin.".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open process stdout.".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to open process stderr.".to_owned())?;

    Ok(SpawnedProcess {
        child,
        stdin,
        stdout,
        stderr,
        runtime_pid: Arc::new(AtomicU32::new(0)),
        host_pid,
    })
}

async fn install_spawned_process(
    app: &AppHandle,
    source: ProcessSource,
    managed: &ManagedProcess,
    mut spawned: SpawnedProcess,
) -> Result<InstallResult, String> {
    let host_pid = spawned.host_pid;
    let runtime_pid = spawned.runtime_pid.clone();
    let stdin = Arc::new(Mutex::new(spawned.stdin));

    {
        let mut current = managed.current.lock().await;
        if managed.stop_requested.load(Ordering::SeqCst) {
            spawned
                .child
                .start_kill()
                .map_err(|error| format!("Failed to cancel restarted process: {error}"))?;
            return Ok(InstallResult::Cancelled);
        }
        *current = Some(RunningProcess {
            child: spawned.child,
            stdin,
            runtime_pid: runtime_pid.clone(),
            host_pid,
        });
    }

    spawn_output_reader(
        app.clone(),
        source,
        "stdout",
        spawned.stdout,
        runtime_pid.clone(),
        managed.redacted_values.clone(),
    );
    spawn_output_reader(
        app.clone(),
        source,
        "stderr",
        spawned.stderr,
        runtime_pid,
        managed.redacted_values.clone(),
    );
    Ok(InstallResult::Installed(host_pid))
}

fn spawn_output_reader<R>(
    app: AppHandle,
    source: ProcessSource,
    stream: &'static str,
    reader: R,
    runtime_pid: Arc<AtomicU32>,
    redacted_values: Arc<Vec<String>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(value) = line.strip_prefix(PID_MARKER) {
                if let Ok(pid) = value.trim().parse::<u32>() {
                    runtime_pid.store(pid, Ordering::SeqCst);
                }
                continue;
            }
            let output_stream = if source == ProcessSource::LocalMcp {
                "stdout"
            } else {
                stream
            };
            let sanitized = redacted_values
                .iter()
                .fold(line, |text, secret| text.replace(secret, "••••••••"));
            emit_log(&app, source, output_stream, &sanitized);
        }
    });
}

pub fn emit_log(app: &AppHandle, source: ProcessSource, stream: &str, line: &str) {
    let payload = LogEvent {
        source,
        stream: stream.to_owned(),
        line: line.to_owned(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
    };
    let _ = app.emit("process-log", payload);
}

fn emit_state(
    app: &AppHandle,
    source: ProcessSource,
    state: &str,
    pid: Option<u32>,
    error: Option<String>,
) {
    let _ = app.emit(
        "process-state",
        StateEvent {
            source,
            state: state.to_owned(),
            pid,
            error,
        },
    );
}

fn display_command(spec: &CommandSpec) -> String {
    std::iter::once(spec.executable.as_str())
        .chain(spec.arguments.iter().map(String::as_str))
        .map(|value| {
            if value.contains(char::is_whitespace) {
                format!("\"{value}\"")
            } else {
                value.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy(max_attempts: u32) -> RestartPolicy {
        RestartPolicy {
            max_attempts,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(350),
            reset_after: Duration::from_secs(10),
        }
    }

    #[test]
    fn restart_delay_uses_exponential_backoff_and_cap() {
        let policy = test_policy(5);
        assert_eq!(
            policy.delay_for_attempt(1),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            policy.delay_for_attempt(2),
            Some(Duration::from_millis(200))
        );
        assert_eq!(
            policy.delay_for_attempt(3),
            Some(Duration::from_millis(350))
        );
        assert_eq!(
            policy.delay_for_attempt(4),
            Some(Duration::from_millis(350))
        );
    }

    #[test]
    fn restart_budget_stops_at_configured_limit() {
        let policy = test_policy(2);
        let mut budget = RestartBudget::default();
        assert_eq!(
            budget.next(policy, Duration::ZERO),
            Some((1, Duration::from_millis(100)))
        );
        assert_eq!(
            budget.next(policy, Duration::ZERO),
            Some((2, Duration::from_millis(200)))
        );
        assert_eq!(budget.next(policy, Duration::ZERO), None);
    }

    #[test]
    fn stable_runtime_resets_consecutive_failure_budget() {
        let policy = test_policy(2);
        let mut budget = RestartBudget::default();
        assert_eq!(
            budget.next(policy, Duration::ZERO).map(|value| value.0),
            Some(1)
        );
        assert_eq!(
            budget.next(policy, Duration::ZERO).map(|value| value.0),
            Some(2)
        );
        assert_eq!(
            budget
                .next(policy, Duration::from_secs(10))
                .map(|value| value.0),
            Some(1)
        );
    }

    #[test]
    fn disabled_policy_never_allocates_a_restart() {
        let mut budget = RestartBudget::default();
        assert_eq!(budget.next(RestartPolicy::disabled(), Duration::ZERO), None);
    }

    #[test]
    fn manual_stop_suppresses_automatic_restart() {
        assert!(!restart_allowed(RestartPolicy::tunnel(), true));
        assert!(restart_allowed(RestartPolicy::tunnel(), false));
    }
}
