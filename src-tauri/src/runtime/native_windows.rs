use super::{CliRuntime, CommandSpec};
use tokio::process::Command;

pub struct NativeWindowsRuntime;

impl CliRuntime for NativeWindowsRuntime {
    fn build_command(&self, spec: &CommandSpec) -> Result<Command, String> {
        if spec.executable.trim().is_empty() {
            return Err("Executable path is empty.".into());
        }

        let mut command = Command::new(spec.executable.trim());
        command
            .args(&spec.arguments)
            .current_dir(spec.working_directory.trim());
        for (key, value) in &spec.environment {
            command.env(key, value);
        }
        Ok(command)
    }

    fn build_stop_command(&self, _runtime_pid: u32) -> Option<Command> {
        None
    }

    fn normalize_directory(&self, directory: &str) -> String {
        directory.trim().to_owned()
    }
}
