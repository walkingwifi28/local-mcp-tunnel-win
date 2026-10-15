use crate::runtime::RuntimeKind;
use keyring::v1::Entry;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

const CREDENTIAL_SERVICE: &str = "Local MCP Tunnel";
const CREDENTIAL_USER: &str = "CONTROL_PLANE_API_KEY";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub runtime_kind: RuntimeKind,
    #[serde(default)]
    pub wsl_distribution: String,
    #[serde(default)]
    pub profile_name: String,
    #[serde(default)]
    pub tunnel_id: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    #[serde(default = "default_sample_name")]
    pub sample_name: String,
    #[serde(default = "default_tunnel_client")]
    pub tunnel_client_executable: String,
    #[serde(default = "default_local_mcp")]
    pub local_mcp_executable: String,
    #[serde(default = "default_mcp_command")]
    pub mcp_command: String,
    #[serde(default = "default_working_directory")]
    pub working_directory: String,
    #[serde(default)]
    pub control_plane_api_key: String,
    #[serde(default)]
    pub allowed_directories: Vec<String>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            runtime_kind: RuntimeKind::Wsl,
            wsl_distribution: String::new(),
            profile_name: String::new(),
            tunnel_id: String::new(),
            session_id: default_session_id(),
            sample_name: default_sample_name(),
            tunnel_client_executable: default_tunnel_client(),
            local_mcp_executable: default_local_mcp(),
            mcp_command: default_mcp_command(),
            working_directory: default_working_directory(),
            control_plane_api_key: String::new(),
            allowed_directories: Vec::new(),
            permission_mode: default_permission_mode(),
        }
    }
}

impl AppSettings {
    pub fn tunnel_mcp_command(&self) -> String {
        if self.runtime_kind != RuntimeKind::NativeWindows {
            return self.mcp_command.clone();
        }

        // tunnel-client parses this string using shell quoting, which consumes
        // Windows path backslashes. Normalize only the executable, not its args.
        let command = self.mcp_command.trim_start();
        let leading_length = self.mcp_command.len() - command.len();
        let executable_end = match command.chars().next() {
            Some(quote @ ('"' | '\'')) => command[1..]
                .find(quote)
                .map(|end| end + 2)
                .unwrap_or(command.len()),
            _ => command.find(char::is_whitespace).unwrap_or(command.len()),
        };
        format!(
            "{}{}{}",
            &self.mcp_command[..leading_length],
            command[..executable_end].replace('\\', "/"),
            &command[executable_end..]
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.profile_name.trim().is_empty() {
            return Err("Please enter a Profile Name.".into());
        }
        if self.tunnel_id.trim().is_empty() {
            return Err("Please enter a Tunnel ID.".into());
        }
        if self.session_id.trim().is_empty() {
            return Err("Please enter a Session ID.".into());
        }
        if self.working_directory.trim().is_empty() {
            return Err("Please select a Working Directory.".into());
        }
        if self.control_plane_api_key.trim().is_empty() {
            return Err("Please set CONTROL_PLANE_API_KEY.".into());
        }
        Ok(())
    }

    pub fn normalized_allowed_directories(&self) -> Vec<String> {
        let mut result = Vec::new();
        for value in &self.allowed_directories {
            let trimmed = value.trim();
            if !trimmed.is_empty() && !result.iter().any(|existing| existing == trimmed) {
                result.push(trimmed.to_owned());
            }
        }
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSettings {
    runtime_kind: RuntimeKind,
    wsl_distribution: String,
    profile_name: String,
    tunnel_id: String,
    session_id: String,
    sample_name: String,
    tunnel_client_executable: String,
    local_mcp_executable: String,
    mcp_command: String,
    working_directory: String,
    allowed_directories: Vec<String>,
    permission_mode: String,
}

impl From<&AppSettings> for PersistedSettings {
    fn from(settings: &AppSettings) -> Self {
        Self {
            runtime_kind: settings.runtime_kind,
            wsl_distribution: settings.wsl_distribution.clone(),
            profile_name: settings.profile_name.clone(),
            tunnel_id: settings.tunnel_id.clone(),
            session_id: settings.session_id.clone(),
            sample_name: settings.sample_name.clone(),
            tunnel_client_executable: settings.tunnel_client_executable.clone(),
            local_mcp_executable: settings.local_mcp_executable.clone(),
            mcp_command: settings.mcp_command.clone(),
            working_directory: settings.working_directory.clone(),
            allowed_directories: settings.normalized_allowed_directories(),
            permission_mode: settings.permission_mode.clone(),
        }
    }
}

impl From<PersistedSettings> for AppSettings {
    fn from(settings: PersistedSettings) -> Self {
        Self {
            runtime_kind: settings.runtime_kind,
            wsl_distribution: settings.wsl_distribution,
            profile_name: settings.profile_name,
            tunnel_id: settings.tunnel_id,
            session_id: settings.session_id,
            sample_name: settings.sample_name,
            tunnel_client_executable: settings.tunnel_client_executable,
            local_mcp_executable: settings.local_mcp_executable,
            mcp_command: settings.mcp_command,
            working_directory: settings.working_directory,
            control_plane_api_key: String::new(),
            allowed_directories: settings.allowed_directories,
            permission_mode: settings.permission_mode,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config directory: {error}"))?;
    Ok(directory.join("settings.json"))
}

fn credential_entry() -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(|error| format!("Failed to access Windows Credential Manager: {error}"))
}

pub fn load(app: &AppHandle) -> Result<AppSettings, String> {
    let path = settings_path(app)?;
    let mut settings = if path.exists() {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
        let persisted: PersistedSettings = serde_json::from_str(&text)
            .map_err(|error| format!("Failed to parse {}: {error}", path.display()))?;
        AppSettings::from(persisted)
    } else {
        AppSettings::default()
    };

    settings.control_plane_api_key = credential_entry()
        .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
        .unwrap_or_default();
    Ok(settings)
}

pub fn save(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }

    let persisted = PersistedSettings::from(settings);
    let json = serde_json::to_string_pretty(&persisted)
        .map_err(|error| format!("Failed to serialize settings: {error}"))?;
    fs::write(&path, json)
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))?;

    let entry = credential_entry()?;
    if settings.control_plane_api_key.trim().is_empty() {
        let _ = entry.delete_credential();
    } else {
        entry
            .set_password(settings.control_plane_api_key.trim())
            .map_err(|error| format!("Failed to save API key: {error}"))?;
    }
    Ok(())
}

fn default_session_id() -> String {
    "local-mcp".into()
}

fn default_sample_name() -> String {
    "sample_mcp_stdio_local".into()
}

fn default_tunnel_client() -> String {
    "$HOME/.local/bin/tunnel-client".into()
}

fn default_local_mcp() -> String {
    "$HOME/.local/bin/local-mcp".into()
}

fn default_mcp_command() -> String {
    "$HOME/.local/bin/local-mcp mcp".into()
}

fn default_working_directory() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
}

fn default_permission_mode() -> String {
    "ask".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mcp_command_accepts_both_path_separators() {
        let mut settings = AppSettings::default();
        settings.runtime_kind = RuntimeKind::NativeWindows;
        for input in [r"C:\tools\local-mcp.exe mcp", "C:/tools/local-mcp.exe mcp"] {
            settings.mcp_command = input.into();
            assert_eq!(settings.tunnel_mcp_command(), "C:/tools/local-mcp.exe mcp");
            assert_eq!(settings.mcp_command, input);
        }
    }

    #[test]
    fn native_mcp_command_preserves_quotes_and_arguments() {
        let mut settings = AppSettings::default();
        settings.runtime_kind = RuntimeKind::NativeWindows;
        for quote in ['\"', '\''] {
            settings.mcp_command =
                format!(r#"  {quote}C:\Program Files\local-mcp.exe{quote} mcp --value 'a\b'"#);
            assert_eq!(
                settings.tunnel_mcp_command(),
                format!(r#"  {quote}C:/Program Files/local-mcp.exe{quote} mcp --value 'a\b'"#)
            );
        }
    }

    #[test]
    fn wsl_mcp_command_is_unchanged() {
        let mut settings = AppSettings::default();
        settings.mcp_command = r"/home/user/local-mcp mcp --value a\b".into();
        assert_eq!(settings.tunnel_mcp_command(), settings.mcp_command);
    }
}
