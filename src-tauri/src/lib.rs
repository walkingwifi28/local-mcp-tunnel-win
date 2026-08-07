mod process_manager;
mod runtime;
mod settings;

use process_manager::{emit_log, ProcessRegistry, ProcessSource, RestartPolicy};
use runtime::{runtime_for, CommandSpec, RuntimeOptions};
use settings::AppSettings;
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
struct AppState {
    processes: ProcessRegistry,
}

fn runtime_options(settings: &AppSettings) -> RuntimeOptions {
    RuntimeOptions {
        kind: settings.runtime_kind,
        wsl_distribution: settings.wsl_distribution.clone(),
    }
}

fn environment(settings: &AppSettings) -> HashMap<String, String> {
    HashMap::from([(
        "CONTROL_PLANE_API_KEY".to_owned(),
        settings.control_plane_api_key.clone(),
    )])
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<AppSettings, String> {
    settings::load(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    settings::save(&app, &settings)
}

#[tauri::command]
async fn process_states(
    state: State<'_, AppState>,
) -> Result<HashMap<ProcessSource, String>, String> {
    Ok(state.processes.states().await)
}

#[tauri::command]
async fn initialize_tunnel(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    settings.validate()?;
    let spec = CommandSpec {
        executable: settings.tunnel_client_executable.clone(),
        arguments: vec![
            "init".into(),
            "--sample".into(),
            settings.sample_name.clone(),
            "--profile".into(),
            settings.profile_name.clone(),
            "--tunnel-id".into(),
            settings.tunnel_id.clone(),
            "--mcp-command".into(),
            settings.mcp_command.clone(),
            "--force".into(),
        ],
        environment: environment(&settings),
        working_directory: settings.working_directory.clone(),
    };
    state
        .processes
        .start(
            app,
            ProcessSource::Tunnel,
            spec,
            runtime_options(&settings),
            RestartPolicy::disabled(),
        )
        .await
}

#[tauri::command]
async fn start_tunnel(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    settings.validate()?;
    let spec = CommandSpec {
        executable: settings.tunnel_client_executable.clone(),
        arguments: vec![
            "run".into(),
            "--profile".into(),
            settings.profile_name.clone(),
        ],
        environment: environment(&settings),
        working_directory: settings.working_directory.clone(),
    };
    state
        .processes
        .start(
            app,
            ProcessSource::Tunnel,
            spec,
            runtime_options(&settings),
            RestartPolicy::tunnel(),
        )
        .await
}

#[tauri::command]
async fn start_local_mcp(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    settings.validate()?;
    let options = runtime_options(&settings);
    let spec = CommandSpec {
        executable: settings.local_mcp_executable.clone(),
        arguments: vec!["start".into(), settings.session_id.clone()],
        environment: environment(&settings),
        working_directory: settings.working_directory.clone(),
    };

    state
        .processes
        .start(
            app.clone(),
            ProcessSource::LocalMcp,
            spec,
            options.clone(),
            RestartPolicy::disabled(),
        )
        .await?;

    let permission = if settings.permission_mode == "yolo" {
        "/permission yolo"
    } else {
        "/permission ask"
    };
    emit_log(
        &app,
        ProcessSource::LocalMcp,
        "system",
        &format!(
            "Applying saved Permission Mode ({}).",
            settings.permission_mode
        ),
    );
    state
        .processes
        .send(ProcessSource::LocalMcp, permission)
        .await?;
    emit_log(&app, ProcessSource::LocalMcp, "stdin", permission);

    let runtime = runtime_for(&options);
    let directories = settings.normalized_allowed_directories();
    if !directories.is_empty() {
        emit_log(
            &app,
            ProcessSource::LocalMcp,
            "system",
            &format!(
                "Applying automatic allow at startup ({} items).",
                directories.len()
            ),
        );
    }
    for directory in directories {
        let normalized = runtime.normalize_directory(&directory);
        let command = format!("/permission allow {normalized}");
        state
            .processes
            .send(ProcessSource::LocalMcp, &command)
            .await?;
        emit_log(&app, ProcessSource::LocalMcp, "stdin", &command);
    }
    Ok(())
}

#[tauri::command]
async fn stop_process(
    app: AppHandle,
    state: State<'_, AppState>,
    source: ProcessSource,
) -> Result<(), String> {
    state.processes.stop(app, source).await
}

#[tauri::command]
async fn send_permission(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
    action: String,
    directory: Option<String>,
) -> Result<String, String> {
    let command = match action.as_str() {
        "ask" | "yolo" | "list" | "status" => format!("/permission {action}"),
        "allow" | "revoke" => {
            let directory = directory
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("A directory is required for permission {action}."))?;
            let runtime = runtime_for(&runtime_options(&settings));
            let normalized = runtime.normalize_directory(&directory);
            format!("/permission {action} {normalized}")
        }
        _ => return Err(format!("Unsupported permission action: {action}")),
    };

    state
        .processes
        .send(ProcessSource::LocalMcp, &command)
        .await?;
    emit_log(&app, ProcessSource::LocalMcp, "stdin", &command);
    Ok(command)
}

#[tauri::command]
async fn send_process_input(
    app: AppHandle,
    state: State<'_, AppState>,
    source: ProcessSource,
    input: String,
) -> Result<(), String> {
    state.processes.send(source, &input).await?;
    emit_log(&app, source, "stdin", &input);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            process_states,
            initialize_tunnel,
            start_tunnel,
            start_local_mcp,
            stop_process,
            send_permission,
            send_process_input
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let app = window.app_handle().clone();
                let processes = app.state::<AppState>().processes.clone();
                tauri::async_runtime::spawn(async move {
                    processes.stop_all(app).await;
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Local MCP Tunnel");
}
