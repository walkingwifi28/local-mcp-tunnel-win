use super::{CliRuntime, CommandSpec};
use std::env;
use tokio::process::Command;

pub struct WslRuntime {
    distribution: String,
}

impl WslRuntime {
    pub fn new(distribution: String) -> Self {
        Self { distribution }
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new("wsl.exe");
        if !self.distribution.trim().is_empty() {
            command.arg("-d").arg(self.distribution.trim());
        }
        command
    }

    fn merge_wslenv(keys: impl Iterator<Item = String>) -> String {
        let mut entries: Vec<String> = env::var("WSLENV")
            .unwrap_or_default()
            .split(':')
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();

        for key in keys {
            if !entries
                .iter()
                .any(|entry| entry.split('/').next() == Some(key.as_str()))
            {
                entries.push(key);
            }
        }
        entries.join(":")
    }

    fn windows_to_wsl_path(path: &str) -> String {
        let trimmed = path.trim();
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
            let drive = (bytes[0] as char).to_ascii_lowercase();
            let rest = trimmed[3..].replace('\\', "/");
            return format!("/mnt/{drive}/{rest}");
        }
        trimmed.replace('\\', "/")
    }
}

impl CliRuntime for WslRuntime {
    fn build_command(&self, spec: &CommandSpec) -> Result<Command, String> {
        if spec.executable.trim().is_empty() {
            return Err("Executable path is empty.".into());
        }
        if spec.working_directory.trim().is_empty() {
            return Err("Working directory is empty.".into());
        }

        let working_directory = Self::windows_to_wsl_path(&spec.working_directory);
        let executable = Self::windows_to_wsl_path(&spec.executable);

        // Launch through WSL's direct --exec path. Arguments placed after `bash -lc`
        // can be lost by wsl.exe, while --exec preserves argv and imported WSLENV values.
        let script = r#"
set -e
executable="$1"
shift
case "$executable" in
  '$HOME'*) executable="$HOME${executable#'$HOME'}" ;;
  '~/'*) executable="$HOME/${executable#~/}" ;;
esac
expanded_arguments=()
for argument in "$@"; do
  case "$argument" in
    '$HOME'*) argument="$HOME${argument#'$HOME'}" ;;
    '~/'*) argument="$HOME/${argument#~/}" ;;
  esac
  expanded_arguments+=("$argument")
done
printf '__LOCAL_MCP_TUNNEL_PID__:%s\n' "$$"
exec "$executable" "${expanded_arguments[@]}"
"#;

        let mut command = self.base_command();
        command
            .arg("--cd")
            .arg(working_directory)
            .arg("--exec")
            .arg("bash")
            .arg("-c")
            .arg(script)
            .arg("local-mcp-tunnel")
            .arg(executable)
            .args(&spec.arguments);

        let imported_keys = spec.environment.keys().cloned();
        command.env("WSLENV", Self::merge_wslenv(imported_keys));
        for (key, value) in &spec.environment {
            command.env(key, value);
        }
        Ok(command)
    }

    fn build_stop_command(&self, runtime_pid: u32) -> Option<Command> {
        if runtime_pid == 0 {
            return None;
        }

        let mut command = self.base_command();
        command
            .arg("--exec")
            .arg("bash")
            .arg("-c")
            .arg("kill -INT -- \"$1\" 2>/dev/null || kill -TERM -- \"$1\" 2>/dev/null || true")
            .arg("local-mcp-tunnel-stop")
            .arg(runtime_pid.to_string());
        Some(command)
    }

    fn normalize_directory(&self, directory: &str) -> String {
        Self::windows_to_wsl_path(directory)
    }
}

#[cfg(test)]
mod tests {
    use super::WslRuntime;

    #[test]
    fn converts_windows_drive_path() {
        assert_eq!(
            WslRuntime::windows_to_wsl_path(r"C:\Users\atush\source\repos"),
            "/mnt/c/Users/atush/source/repos"
        );
    }

    #[test]
    fn preserves_wsl_path() {
        assert_eq!(
            WslRuntime::windows_to_wsl_path("/home/wsl/.local/bin/local-mcp"),
            "/home/wsl/.local/bin/local-mcp"
        );
    }

    #[test]
    fn preserves_home_variable() {
        assert_eq!(
            WslRuntime::windows_to_wsl_path("$HOME/.local/bin/local-mcp"),
            "$HOME/.local/bin/local-mcp"
        );
    }
}
