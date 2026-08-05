use crate::runtime::{configure_stdio, runtime_for, CommandSpec, RuntimeOptions};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    sync::Mutex,
    time::sleep,
};

const PID_MARKER: &str = "__LOCAL_MCP_TUNNEL_PID__:";

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

#[derive(Clone)]
struct ManagedProcess {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    runtime_pid: Arc<AtomicU32>,
    stop_requested: Arc<AtomicBool>,
    runtime_options: RuntimeOptions,
    host_pid: Option<u32>,
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
                let state = if processes.contains_key(&source) {
                    "running"
                } else {
                    "stopped"
                };
                (source, state.to_owned())
            })
            .collect()
    }

    pub async fn start(
        &self,
        app: AppHandle,
        source: ProcessSource,
        spec: CommandSpec,
        runtime_options: RuntimeOptions,
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
        let runtime = runtime_for(&runtime_options);
        let mut command = runtime.build_command(&spec)?;
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

        let managed = ManagedProcess {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            runtime_pid: Arc::new(AtomicU32::new(0)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            runtime_options,
            host_pid,
        };

        self.processes.lock().await.insert(source, managed.clone());
        emit_state(&app, source, "running", host_pid, None);

        spawn_output_reader(
            app.clone(),
            source,
            "stdout",
            stdout,
            managed.runtime_pid.clone(),
            redacted_values.clone(),
        );
        spawn_output_reader(
            app.clone(),
            source,
            "stderr",
            stderr,
            managed.runtime_pid.clone(),
            redacted_values,
        );
        self.spawn_monitor(app, source, managed);
        Ok(())
    }

    fn spawn_monitor(&self, app: AppHandle, source: ProcessSource, managed: ManagedProcess) {
        let registry = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let status = {
                    let mut child = managed.child.lock().await;
                    match child.try_wait() {
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
                    }
                };

                if let Some(status) = status {
                    registry.processes.lock().await.remove(&source);
                    let requested = managed.stop_requested.load(Ordering::SeqCst);
                    let code = status.code().unwrap_or(-1);
                    emit_log(
                        &app,
                        source,
                        "system",
                        &format!("{} finished (code: {code}).", source.label()),
                    );
                    if requested || status.success() {
                        emit_state(&app, source, "stopped", None, None);
                    } else {
                        emit_state(
                            &app,
                            source,
                            "failed",
                            None,
                            Some(format!("Exit code: {code}")),
                        );
                    }
                    break;
                }
                sleep(Duration::from_millis(200)).await;
            }
        });
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

        let mut stdin = managed.stdin.lock().await;
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
        emit_state(&app, source, "stopping", managed.host_pid, None);
        emit_log(&app, source, "system", "Stop request sent.");

        let runtime_pid = managed.runtime_pid.load(Ordering::SeqCst);
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

        let mut child = managed.child.lock().await;
        if child
            .try_wait()
            .map_err(|error| format!("Failed to read process status: {error}"))?
            .is_none()
        {
            child
                .start_kill()
                .map_err(|error| format!("Failed to stop {}: {error}", source.label()))?;
        }
        Ok(())
    }

    pub async fn stop_all(&self, app: AppHandle) {
        for source in [ProcessSource::Tunnel, ProcessSource::LocalMcp] {
            let _ = self.stop(app.clone(), source).await;
        }
    }
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
