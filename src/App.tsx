import { FormEvent, ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type ProcessSource = "tunnel" | "localMcp";
type ProcessState = "stopped" | "starting" | "running" | "stopping" | "failed";
type RuntimeKind = "wsl" | "nativeWindows";
type SaveButtonState = "idle" | "saving" | "success" | "failure";

type AppSettings = {
  runtimeKind: RuntimeKind;
  wslDistribution: string;
  profileName: string;
  tunnelId: string;
  sessionId: string;
  sampleName: string;
  tunnelClientExecutable: string;
  localMcpExecutable: string;
  mcpCommand: string;
  workingDirectory: string;
  controlPlaneApiKey: string;
  allowedDirectories: string[];
  permissionMode: "ask" | "yolo";
};

type LogEvent = {
  source: ProcessSource;
  stream: string;
  line: string;
  timestamp: string;
};

type StateEvent = {
  source: ProcessSource;
  state: ProcessState;
  pid?: number;
  error?: string;
};

const defaultSettings: AppSettings = {
  runtimeKind: "wsl",
  wslDistribution: "",
  profileName: "",
  tunnelId: "",
  sessionId: "local-mcp",
  sampleName: "sample_mcp_stdio_local",
  tunnelClientExecutable: "$HOME/.local/bin/tunnel-client",
  localMcpExecutable: "$HOME/.local/bin/local-mcp",
  mcpCommand: "$HOME/.local/bin/local-mcp mcp",
  workingDirectory: "",
  controlPlaneApiKey: "",
  allowedDirectories: [],
  permissionMode: "ask",
};

const OPENAI_PLATFORM_URL = "https://platform.openai.com/";

const sourceLabels: Record<ProcessSource, string> = {
  tunnel: "tunnel-client",
  localMcp: "local-mcp",
};

const stateLabels: Record<ProcessState, string> = {
  stopped: "Stopped",
  starting: "Starting",
  running: "Running",
  stopping: "Stopping",
  failed: "Failed",
};

function App() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [view, setView] = useState<"main" | "settings">("main");
  const [selectedLog, setSelectedLog] = useState<ProcessSource>("tunnel");
  const [logs, setLogs] = useState<Record<ProcessSource, string[]>>({
    tunnel: [],
    localMcp: [],
  });
  const [states, setStates] = useState<Record<ProcessSource, ProcessState>>({
    tunnel: "stopped",
    localMcp: "stopped",
  });
  const [runtimeAllowedDirectories, setRuntimeAllowedDirectories] = useState<string[]>([]);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [saveButtonState, setSaveButtonState] = useState<SaveButtonState>("idle");
  const [busy, setBusy] = useState(false);
  const [revokeMenuOpen, setRevokeMenuOpen] = useState(false);
  const terminalRef = useRef<HTMLDivElement>(null);
  const revokeMenuRef = useRef<HTMLDivElement>(null);
  const saveResetTimerRef = useRef<number | null>(null);

  const revokeDirectories = useMemo(
    () =>
      runtimeAllowedDirectories
        .filter((directory) => directory !== settings.workingDirectory)
        .sort((left, right) => left.localeCompare(right)),
    [runtimeAllowedDirectories, settings.workingDirectory],
  );
  const revokeDisabled = states.localMcp !== "running" || revokeDirectories.length === 0;

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    async function registerLogListener() {
      const unlisten = await listen<LogEvent>("process-log", ({ payload }) => {
        const label =
          payload.stream === "system"
            ? sourceLabels[payload.source]
            : `${sourceLabels[payload.source]}:${payload.stream}`;
        const line = `[${payload.timestamp}] [${label}] ${payload.line}`;
        setLogs((previous) => ({
          ...previous,
          [payload.source]: [...previous[payload.source], line],
        }));
      });

      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    }

    async function registerStateListener() {
      const unlisten = await listen<StateEvent>("process-state", ({ payload }) => {
        setStates((previous) => ({ ...previous, [payload.source]: payload.state }));
        if (
          payload.source === "localMcp" &&
          (payload.state === "stopped" || payload.state === "failed")
        ) {
          setRuntimeAllowedDirectories([]);
        }
        if (payload.error) setError(payload.error);
      });

      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    }

    async function initialize() {
      try {
        await Promise.all([registerLogListener(), registerStateListener()]);
        if (disposed) return;

        const [loaded, currentStates] = await Promise.all([
          invoke<AppSettings>("load_settings"),
          invoke<Record<ProcessSource, ProcessState>>("process_states"),
        ]);
        if (disposed) return;

        setSettings(loaded);
        setStates((previous) => ({ ...previous, ...currentStates }));
      } catch (reason) {
        if (!disposed) setError(toMessage(reason));
      }
    }

    void initialize();
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (terminal) terminal.scrollTop = terminal.scrollHeight;
  }, [logs, selectedLog]);

  useEffect(() => {
    return () => {
      if (saveResetTimerRef.current !== null) {
        window.clearTimeout(saveResetTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!revokeMenuOpen) return;

    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!revokeMenuRef.current?.contains(event.target as Node)) {
        setRevokeMenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setRevokeMenuOpen(false);
    };

    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [revokeMenuOpen]);

  useEffect(() => {
    if (revokeDisabled) setRevokeMenuOpen(false);
  }, [revokeDisabled]);

  const selectedLines = logs[selectedLog];
  const selectedState = states[selectedLog];
  const canSend = selectedState === "running";

  const validationMessage = useMemo(() => {
    if (!settings.profileName.trim()) return "Profile Name is required.";
    if (!settings.tunnelId.trim()) return "Tunnel ID is required.";
    if (!settings.sessionId.trim()) return "Session ID is required.";
    if (!settings.workingDirectory.trim()) return "Working Directory is required.";
    if (!settings.controlPlaneApiKey.trim()) return "CONTROL_PLANE_API_KEY is required.";
    return null;
  }, [settings]);

  async function run(action: () => Promise<void>) {
    setError(null);
    setBusy(true);
    try {
      await action();
    } catch (reason) {
      setError(toMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    if (saveButtonState === "saving") return;

    if (saveResetTimerRef.current !== null) {
      window.clearTimeout(saveResetTimerRef.current);
      saveResetTimerRef.current = null;
    }

    setError(null);
    setSaveMessage(null);
    setSaveButtonState("saving");

    const startedAt = performance.now();
    let didSave = false;
    let failure: unknown = null;

    try {
      await invoke("save_settings", { settings });
      didSave = true;
    } catch (reason) {
      failure = reason;
    }

    const remainingLoadingTime = Math.max(0, 500 - (performance.now() - startedAt));
    if (remainingLoadingTime > 0) {
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, remainingLoadingTime);
      });
    }

    if (didSave) {
      setSaveButtonState("success");
      setSaveMessage("Settings have been saved.");
    } else {
      const message = toMessage(failure);
      setSaveButtonState("failure");
      setSaveMessage(message);
    }

    saveResetTimerRef.current = window.setTimeout(() => {
      setSaveButtonState("idle");
      setSaveMessage(null);
      saveResetTimerRef.current = null;
    }, 3000);
  }

  async function start(command: "initialize_tunnel" | "start_tunnel" | "start_local_mcp") {
    if (validationMessage) {
      setError(validationMessage);
      return;
    }
    await run(async () => {
      await invoke(command, { settings });
      if (command === "start_local_mcp") {
        setRuntimeAllowedDirectories(
          Array.from(
            new Set(
              [settings.workingDirectory, ...settings.allowedDirectories].filter(Boolean),
            ),
          ),
        );
      }
      setSelectedLog(command === "start_local_mcp" ? "localMcp" : "tunnel");
    });
  }

  async function stop(source: ProcessSource) {
    await run(() => invoke("stop_process", { source }));
  }

  async function submitInput(event: FormEvent) {
    event.preventDefault();
    const normalized = input.trim();
    if (!normalized || !canSend) return;
    setInput("");
    await run(() => invoke("send_process_input", { source: selectedLog, input: normalized }));
  }

  async function copyLogs() {
    try {
      await navigator.clipboard.writeText(selectedLines.join("\n"));
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function sendPermission(action: string, directory?: string) {
    if (states.localMcp !== "running") {
      setError("Please start local-mcp first.");
      return;
    }
    await run(async () => {
      await invoke("send_permission", {
        settings,
        action,
        directory: directory ?? null,
      });
      if (action === "allow" && directory) {
        setRuntimeAllowedDirectories((previous) =>
          Array.from(new Set([...previous, directory])),
        );
      }
      if (action === "revoke" && directory) {
        setRuntimeAllowedDirectories((previous) =>
          previous.filter((value) => value !== directory),
        );
      }
      setSelectedLog("localMcp");
    });
  }

  async function chooseAndAllowDirectory() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      await sendPermission("allow", selected);
    }
  }

  async function setPermissionMode(mode: "ask" | "yolo") {
    const next = { ...settings, permissionMode: mode };
    setSettings(next);
    try {
      await invoke("save_settings", { settings: next });
      if (states.localMcp === "running") {
        await invoke("send_permission", {
          settings: next,
          action: mode,
          directory: null,
        });
      }
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function chooseDirectory(target: "working" | "allowed") {
    const selected = await open({ directory: true, multiple: target === "allowed" });
    if (!selected) return;
    const values = Array.isArray(selected) ? selected : [selected];
    if (target === "working") {
      setSettings((previous) => ({ ...previous, workingDirectory: values[0] }));
    } else {
      setSettings((previous) => ({
        ...previous,
        allowedDirectories: Array.from(new Set([...previous.allowedDirectories, ...values])),
      }));
    }
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <h1>{view === "main" ? "Local MCP Tunnel" : "Local MCP Tunnel Settings"}</h1>
        <button
          className="toolbar-button settings-button"
          onClick={() => setView(view === "main" ? "settings" : "main")}
        >
          {view === "main" ? <GearIcon /> : <BackIcon />}
          <span>{view === "main" ? "Settings" : "Back"}</span>
        </button>
      </header>

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button aria-label="Dismiss error" onClick={() => setError(null)}>×</button>
        </div>
      )}

      {view === "settings" ? (
        <SettingsView
          settings={settings}
          setSettings={setSettings}
          chooseDirectory={chooseDirectory}
          save={save}
          saveButtonState={saveButtonState}
          saveMessage={saveMessage}
        />
      ) : (
        <main className="main-page">
          <section className="process-grid">
            <ProcessCard
              title="Tunnel Client"
              state={states.tunnel}
              details={[
                ["Profile", settings.profileName || "—"],
                ["Tunnel ID", settings.tunnelId || "—"],
              ]}
              primaryLabel="Run"
              onPrimary={() => start("start_tunnel")}
              onSecondary={() => start("initialize_tunnel")}
              secondaryLabel="Init"
              onStop={() => stop("tunnel")}
              busy={busy}
            />

            <ProcessCard
              title="local-mcp"
              state={states.localMcp}
              details={[
                ["Session", settings.sessionId || "—"],
                ["Working Dir", settings.workingDirectory || "—"],
              ]}
              primaryLabel="Start"
              onPrimary={() => start("start_local_mcp")}
              onStop={() => stop("localMcp")}
              busy={busy}
            />
          </section>

          <section className="log-card">
            <div className="log-toolbar">
              <div className="log-toolbar-start">
                <strong>Log</strong>
                <div className="log-tabs" role="tablist" aria-label="Log source">
                  {(["tunnel", "localMcp"] as ProcessSource[]).map((source) => (
                    <button
                      key={source}
                      className={selectedLog === source ? "active" : ""}
                      onClick={() => setSelectedLog(source)}
                    >
                      {sourceLabels[source]}
                    </button>
                  ))}
                </div>
              </div>

              <div className="log-toolbar-actions">
                {selectedLog === "localMcp" && (
                  <div className="inline-permissions">
                    <div className="segmented compact-segmented">
                      <button
                        className={settings.permissionMode === "ask" ? "active" : ""}
                        onClick={() => void setPermissionMode("ask")}
                        disabled={states.localMcp !== "running"}
                      >
                        Ask
                      </button>
                      <button
                        className={settings.permissionMode === "yolo" ? "active danger" : ""}
                        onClick={() => void setPermissionMode("yolo")}
                        disabled={states.localMcp !== "running"}
                      >
                        Yolo
                      </button>
                    </div>
                    <button
                      onClick={() => void chooseAndAllowDirectory()}
                      disabled={states.localMcp !== "running"}
                    >
                      Allow…
                    </button>
                    <div className="revoke-dropdown" ref={revokeMenuRef}>
                      <button
                        type="button"
                        className="revoke-trigger"
                        aria-label="Revoke directory"
                        aria-haspopup="menu"
                        aria-expanded={revokeMenuOpen}
                        disabled={revokeDisabled}
                        onClick={() => setRevokeMenuOpen((open) => !open)}
                      >
                        <span>Revoke</span>
                        <ChevronDownIcon />
                      </button>
                      {revokeMenuOpen && (
                        <div className="revoke-menu" role="menu">
                          {revokeDirectories.map((directory) => (
                            <button
                              type="button"
                              role="menuitem"
                              className="revoke-menu-item"
                              key={directory}
                              title={directory}
                              onClick={() => {
                                setRevokeMenuOpen(false);
                                void sendPermission("revoke", directory);
                              }}
                            >
                              {directory}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                    <button onClick={() => void sendPermission("list")} disabled={states.localMcp !== "running"}>
                      List
                    </button>
                    <button onClick={() => void sendPermission("status")} disabled={states.localMcp !== "running"}>
                      Status
                    </button>
                  </div>
                )}
                <button className="toolbar-button compact-button" onClick={() => void copyLogs()}>
                  Copy
                </button>
                <button
                  className="toolbar-button compact-button"
                  onClick={() => setLogs((previous) => ({ ...previous, [selectedLog]: [] }))}
                >
                  Clear
                </button>
              </div>
            </div>

            <div className="terminal" ref={terminalRef}>
              {selectedLines.length === 0 ? (
                <div className="terminal-caret" aria-hidden="true" />
              ) : (
                selectedLines.map((line, index) => <div key={`${index}-${line}`}>{line}</div>)
              )}
            </div>

            <form className="terminal-input" onSubmit={submitInput}>
              <span>›</span>
              <input
                value={input}
                onChange={(event) => setInput(event.target.value)}
                placeholder={canSend ? `Send input to ${sourceLabels[selectedLog]}` : `${sourceLabels[selectedLog]} is not running`}
                disabled={!canSend}
                autoComplete="off"
              />
            </form>
          </section>
        </main>
      )}
    </div>
  );
}

function ProcessCard(props: {
  title: string;
  state: ProcessState;
  details: Array<[string, string]>;
  primaryLabel: string;
  secondaryLabel?: string;
  onPrimary: () => void;
  onSecondary?: () => void;
  onStop: () => void;
  busy: boolean;
}) {
  const running = ["starting", "running", "stopping"].includes(props.state);
  return (
    <section className="process-section">
      <h2>{props.title}</h2>
      <div className="process-card">
        <div className={`process-status ${props.state}`}>
          <span className="status-dot" />
          <strong>{stateLabels[props.state]}</strong>
        </div>

        <div className="process-details">
          {props.details.map(([label, value]) => (
            <div className="process-detail" key={label}>
              <span>{label} :</span>
              <strong title={value}>{value}</strong>
            </div>
          ))}
        </div>

        <div className="button-row">
          {props.onSecondary && !running && (
            <button className="secondary-button" onClick={props.onSecondary} disabled={props.busy}>
              {props.secondaryLabel}
            </button>
          )}
          {running ? (
            <button className="stop-button" onClick={props.onStop} disabled={props.state === "stopping"}>
              Stop
            </button>
          ) : (
            <button className="primary-button" onClick={props.onPrimary} disabled={props.busy}>
              {props.primaryLabel}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}

function SettingsView(props: {
  settings: AppSettings;
  setSettings: React.Dispatch<React.SetStateAction<AppSettings>>;
  chooseDirectory: (target: "working" | "allowed") => Promise<void>;
  save: () => Promise<void>;
  saveButtonState: SaveButtonState;
  saveMessage: string | null;
}) {
  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    props.setSettings((previous) => ({ ...previous, [key]: value }));

  const deleteAllDirectories = () => update("allowedDirectories", []);

  return (
    <main className="settings-page">
      <div className="settings-content">
        <SettingsSection title="Runtime Environment">
          <SettingsRow label="Working Directory">
            <div className="settings-row-control directory-control">
              <input value={props.settings.workingDirectory} readOnly title={props.settings.workingDirectory} />
              <button className="secondary-button select-button" onClick={() => void props.chooseDirectory("working")}>Select</button>
            </div>
          </SettingsRow>
          <SettingsRow label="Runtime">
            <div className="segmented runtime-segmented">
              <button
                className={props.settings.runtimeKind === "wsl" ? "active" : ""}
                onClick={() => update("runtimeKind", "wsl")}
              >
                WSL
              </button>
              <button
                className={props.settings.runtimeKind === "nativeWindows" ? "active" : ""}
                onClick={() => update("runtimeKind", "nativeWindows")}
              >
                Native Windows
              </button>
            </div>
          </SettingsRow>
          {props.settings.runtimeKind === "wsl" && (
            <SettingsRow label="WSL Distribution">
              <SettingsInput
                value={props.settings.wslDistribution}
                onChange={(value) => update("wslDistribution", value)}
                placeholder="Default distribution"
              />
            </SettingsRow>
          )}
        </SettingsSection>

        <SettingsSection
          title="Tunnel Client"
          titleAction={
            <button
              type="button"
              className="settings-section-link"
              aria-label="Open OpenAI Platform in the browser"
              title="Open OpenAI Platform"
              onClick={() => void openUrl(OPENAI_PLATFORM_URL)}
            >
              <ExternalLinkIcon />
            </button>
          }
        >
          <SettingsRow label="Profile Name">
            <SettingsInput value={props.settings.profileName} onChange={(value) => update("profileName", value)} />
          </SettingsRow>
          <SettingsRow label="Tunnel ID">
            <SettingsInput value={props.settings.tunnelId} onChange={(value) => update("tunnelId", value)} />
          </SettingsRow>
          <SettingsRow label="Sample Name">
            <SettingsInput value={props.settings.sampleName} onChange={(value) => update("sampleName", value)} />
          </SettingsRow>
          <SettingsRow label="tunnel-client">
            <SettingsInput
              value={props.settings.tunnelClientExecutable}
              onChange={(value) => update("tunnelClientExecutable", value)}
            />
          </SettingsRow>
          <SettingsRow label="CONTROL_PLANE_API_KEY">
            <SettingsInput
              type="password"
              value={props.settings.controlPlaneApiKey}
              onChange={(value) => update("controlPlaneApiKey", value)}
            />
          </SettingsRow>
        </SettingsSection>

        <SettingsSection title="local-mcp">
          <SettingsRow label="Session ID">
            <SettingsInput value={props.settings.sessionId} onChange={(value) => update("sessionId", value)} />
          </SettingsRow>
          <SettingsRow label="local-mcp">
            <SettingsInput
              value={props.settings.localMcpExecutable}
              onChange={(value) => update("localMcpExecutable", value)}
            />
          </SettingsRow>
          <SettingsRow label="MCP Command">
            <SettingsInput value={props.settings.mcpCommand} onChange={(value) => update("mcpCommand", value)} />
          </SettingsRow>
        </SettingsSection>

        <SettingsSection title="Auto-allow on startup" className="allow-section">
          {props.settings.allowedDirectories.length === 0 ? (
            <div className="empty-directory-row">No additional directories configured.</div>
          ) : (
            props.settings.allowedDirectories.map((directory) => (
              <div className="allow-directory-row" key={directory}>
                <FolderIcon />
                <span title={directory}>{directory}</span>
                <button
                  className="icon-only-button"
                  aria-label={`Remove ${directory}`}
                  onClick={() =>
                    update(
                      "allowedDirectories",
                      props.settings.allowedDirectories.filter((value) => value !== directory),
                    )
                  }
                >
                  <TrashIcon />
                </button>
              </div>
            ))
          )}
          <div className="allow-actions">
            <button className="secondary-button" onClick={() => void props.chooseDirectory("allowed")}>Add Directory…</button>
            <button
              className="secondary-button"
              onClick={deleteAllDirectories}
              disabled={props.settings.allowedDirectories.length === 0}
            >
              Delete All
            </button>
          </div>
          <p className="settings-help">
            Immediately after starting local-mcp, the app sends <code>/permission allow</code> to each registered directory.
          </p>
        </SettingsSection>
      </div>

      <div className="settings-footer">
        <span className={`save-message ${props.saveButtonState === "failure" ? "failure" : ""}`}>
          {props.saveMessage}
        </span>
        <button
          className={`primary-button save-button ${props.saveButtonState}`}
          onClick={() => void props.save()}
          disabled={props.saveButtonState === "saving"}
          aria-label={
            props.saveButtonState === "idle"
              ? "Save settings"
              : props.saveButtonState === "saving"
                ? "Saving settings"
                : props.saveButtonState === "success"
                  ? "Settings saved"
                  : "Failed to save settings. Retry"
          }
        >
          <span className={`save-button-face ${props.saveButtonState === "idle" ? "active" : ""}`}>
            Save
          </span>
          <span className={`save-button-face ${props.saveButtonState === "saving" ? "active" : ""}`}>
            <span className="save-spinner" aria-hidden="true" />
            Saving...
          </span>
          <span className={`save-button-face ${props.saveButtonState === "success" ? "active" : ""}`}>
            <SaveCheckIcon />
            Saved
          </span>
          <span className={`save-button-face ${props.saveButtonState === "failure" ? "active" : ""}`}>
            <SaveErrorIcon />
            Retry
          </span>
        </button>
      </div>
    </main>
  );
}

function SettingsSection(props: {
  title: string;
  children: ReactNode;
  className?: string;
  titleAction?: ReactNode;
}) {
  return (
    <section className={`settings-section ${props.className ?? ""}`}>
      <div className="settings-section-header">
        <h2>{props.title}</h2>
        {props.titleAction}
      </div>
      <div className="settings-group">{props.children}</div>
    </section>
  );
}

function SettingsRow(props: { label: string; children: ReactNode }) {
  return (
    <div className="settings-row">
      <label>{props.label}</label>
      {props.children}
    </div>
  );
}

function SettingsInput(props: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: "text" | "password";
}) {
  return (
    <input
      className="settings-input"
      type={props.type ?? "text"}
      value={props.value}
      placeholder={props.placeholder}
      onChange={(event) => props.onChange(event.target.value)}
      autoComplete="off"
    />
  );
}

function ExternalLinkIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M14 5h5v5M19 5l-8 8" />
      <path d="M19 13v5a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5" />
    </svg>
  );
}

function SaveCheckIcon() {
  return (
    <svg className="save-state-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path d="m3.5 8.2 2.8 2.8 6.2-6.2" />
    </svg>
  );
}

function SaveErrorIcon() {
  return (
    <svg className="save-state-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M8 3.2v5.4M8 11.8v.2" />
    </svg>
  );
}

function ChevronDownIcon() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="m4 6 4 4 4-4" />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M9.7 3.4 10.2 2h3.6l.5 1.4c.5.2 1 .4 1.5.7l1.4-.6 2.5 2.5-.6 1.4c.3.5.5 1 .7 1.5l1.4.5V13l-1.4.5c-.2.5-.4 1-.7 1.5l.6 1.4-2.5 2.5-1.4-.6c-.5.3-1 .5-1.5.7l-.5 1.4h-3.6L9.7 19c-.5-.2-1-.4-1.5-.7l-1.4.6-2.5-2.5.6-1.4c-.3-.5-.5-1-.7-1.5L2.8 13V9.4l1.4-.5c.2-.5.4-1 .7-1.5L4.3 6l2.5-2.5 1.4.6c.5-.3 1-.5 1.5-.7Z" />
      <circle cx="12" cy="11.2" r="3" />
    </svg>
  );
}

function BackIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="m14.5 5-6 7 6 7" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 6.5h6l2 2h10v9.5H3z" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 7h14M9 7V4h6v3m-8 0 1 13h8l1-13M10 10v7m4-7v7" />
    </svg>
  );
}

function toMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return JSON.stringify(reason);
}

export default App;
