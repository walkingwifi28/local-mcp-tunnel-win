mod native_windows;
mod wsl;

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, process::Stdio};
use tokio::process::Command;

pub use native_windows::NativeWindowsRuntime;
pub use wsl::WslRuntime;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeKind {
    #[default]
    Wsl,
    NativeWindows,
}

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub kind: RuntimeKind,
    pub wsl_distribution: String,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub executable: String,
    pub arguments: Vec<String>,
    pub environment: HashMap<String, String>,
    pub working_directory: String,
}

pub trait CliRuntime: Send + Sync {
    fn build_command(&self, spec: &CommandSpec) -> Result<Command, String>;
    fn build_stop_command(&self, runtime_pid: u32) -> Option<Command>;
    fn normalize_directory(&self, directory: &str) -> String;
}

pub fn runtime_for(options: &RuntimeOptions) -> Box<dyn CliRuntime> {
    match options.kind {
        RuntimeKind::Wsl => Box::new(WslRuntime::new(options.wsl_distribution.clone())),
        RuntimeKind::NativeWindows => Box::new(NativeWindowsRuntime),
    }
}

pub fn configure_stdio(command: &mut Command) {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}
