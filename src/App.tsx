import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

type ProcessSource = "tunnel" | "localMcp";
type ProcessState = "stopped" | "starting" | "running" | "stopping" | "failed";
type RuntimeKind = "wsl" | "nativeWindows";

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

const sourceLabels: Record<ProcessSource, string> = {
  tunnel: "tunnel-client",
  localMcp: "local-mcp",
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
  const [busy, setBusy] = useState(false);
  const terminalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    async function initialize() {
      try {
        const loaded = await invoke<AppSettings>("load_settings");
        if (!disposed) setSettings(loaded);

        const currentStates = await invoke<Record<ProcessSource, ProcessState>>("process_states");
        if (!disposed) {
          setStates((previous) => ({ ...previous, ...currentStates }));
        }

        unlisteners.push(
          await listen<LogEvent>("process-log", ({ payload }) => {
            const label =
              payload.stream === "system"
                ? sourceLabels[payload.source]
                : `${sourceLabels[payload.source]}:${payload.stream}`;
            const line = `[${payload.timestamp}] [${label}] ${payload.line}`;
            setLogs((previous) => ({
              ...previous,
              [payload.source]: [...previous[payload.source], line],
            }));
          }),
        );

        unlisteners.push(
          await listen<StateEvent>("process-state", ({ payload }) => {
            setStates((previous) => ({ ...previous, [payload.source]: payload.state }));
            if (
              payload.source === "localMcp" &&
              (payload.state === "stopped" || payload.state === "failed")
            ) {
              setRuntimeAllowedDirectories([]);
            }
            if (payload.error) setError(payload.error);
          }),
        );
      } catch (reason) {
        setError(toMessage(reason));
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
    await run(async () => {
      await invoke("save_settings", { settings });
      setSaveMessage("Settings have been saved.");
      window.setTimeout(() => setSaveMessage(null), 3000);
    });
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
      <header className="titlebar">
        <div>
          <h1>Local MCP Tunnel</h1>
          <p>Windows</p>
        </div>
        <button className="icon-button" onClick={() => setView(view === "main" ? "settings" : "main")}>
          {view === "main" ? "Settings" : "Back"}
        </button>
      </header>

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}>×</button>
        </div>
      )}

      {view === "settings" ? (
        <SettingsView
          settings={settings}
          setSettings={setSettings}
          chooseDirectory={chooseDirectory}
          save={save}
          busy={busy}
          saveMessage={saveMessage}
        />
      ) : (
        <main className="main-grid">
          <section className="control-column">
            <ProcessCard
              title="Tunnel Client"
              subtitle="Initialize and run the tunnel-client profile."
              state={states.tunnel}
              primaryLabel="Start"
              onPrimary={() => start("start_tunnel")}
              onSecondary={() => start("initialize_tunnel")}
              secondaryLabel="Initialize"
              onStop={() => stop("tunnel")}
              busy={busy}
            />

            <ProcessCard
              title="Local MCP"
              subtitle="Start local-mcp and keep stdin connected."
              state={states.localMcp}
              primaryLabel="Start"
              onPrimary={() => start("start_local_mcp")}
              onStop={() => stop("localMcp")}
              busy={busy}
            />


          </section>

          <section className="panel log-panel">
            <div className="log-toolbar">
              <div className="log-tabs">
                {(["tunnel", "localMcp"] as ProcessSource[]).map((source) => (
                  <button
                    key={source}
                    className={selectedLog === source ? "active" : ""}
                    onClick={() => setSelectedLog(source)}
                  >
                    {sourceLabels[source]}
                    <span className={`mini-dot ${states[source]}`} />
                  </button>
                ))}
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
                    <select
                      aria-label="Revoke directory"
                      defaultValue=""
                      disabled={
                        states.localMcp !== "running" ||
                        runtimeAllowedDirectories.filter(
                          (directory) => directory !== settings.workingDirectory,
                        ).length === 0
                      }
                      onChange={(event) => {
                        const directory = event.target.value;
                        event.target.value = "";
                        if (directory) void sendPermission("revoke", directory);
                      }}
                    >
                      <option value="">Revoke</option>
                      {runtimeAllowedDirectories
                        .filter((directory) => directory !== settings.workingDirectory)
                        .sort((left, right) => left.localeCompare(right))
                        .map((directory) => (
                          <option key={directory} value={directory}>
                            {directory}
                          </option>
                        ))}
                    </select>
                    <button onClick={() => void sendPermission("list")} disabled={states.localMcp !== "running"}>
                      List
                    </button>
                    <button onClick={() => void sendPermission("status")} disabled={states.localMcp !== "running"}>
                      Status
                    </button>
                  </div>
                )}
                <button
                  className="clear-button"
                  onClick={() => setLogs((previous) => ({ ...previous, [selectedLog]: [] }))}
                >
                  Clear
                </button>
              </div>
            </div>

            <div className="terminal" ref={terminalRef}>
              {selectedLines.length === 0 ? (
                <div className="terminal-empty">No log output yet.</div>
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
  subtitle: string;
  state: ProcessState;
  primaryLabel: string;
  secondaryLabel?: string;
  onPrimary: () => void;
  onSecondary?: () => void;
  onStop: () => void;
  busy: boolean;
}) {
  const running = ["starting", "running", "stopping"].includes(props.state);
  return (
    <section className="panel process-card">
      <div className="process-copy">
        <div className={`status-icon ${props.state}`} />
        <div>
          <h2>{props.title}</h2>
          <p>{props.subtitle}</p>
        </div>
      </div>
      <div className="process-footer">
        <StatePill state={props.state} />
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

function StatePill({ state }: { state: ProcessState }) {
  const labels: Record<ProcessState, string> = {
    stopped: "Stopped",
    starting: "Starting",
    running: "Running",
    stopping: "Stopping",
    failed: "Failed",
  };
  return <span className={`state-pill ${state}`}>{labels[state]}</span>;
}

function SettingsView(props: {
  settings: AppSettings;
  setSettings: React.Dispatch<React.SetStateAction<AppSettings>>;
  chooseDirectory: (target: "working" | "allowed") => Promise<void>;
  save: () => Promise<void>;
  busy: boolean;
  saveMessage: string | null;
}) {
  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    props.setSettings((previous) => ({ ...previous, [key]: value }));

  return (
    <main className="settings-page">
      <section className="panel settings-panel">
        <div className="settings-heading">
          <div>
            <h2>Runtime</h2>
            <p>WSL is used now. Native Windows remains available for future .exe builds.</p>
          </div>
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
        </div>

        <div className="form-grid">
          {props.settings.runtimeKind === "wsl" && (
            <Field label="WSL Distribution" hint="Leave empty to use the default distribution.">
              <input
                value={props.settings.wslDistribution}
                onChange={(event) => update("wslDistribution", event.target.value)}
                placeholder="Ubuntu"
              />
            </Field>
          )}
          <Field label="Profile Name">
            <input value={props.settings.profileName} onChange={(event) => update("profileName", event.target.value)} />
          </Field>
          <Field label="Tunnel ID">
            <input value={props.settings.tunnelId} onChange={(event) => update("tunnelId", event.target.value)} />
          </Field>
          <Field label="Session ID">
            <input value={props.settings.sessionId} onChange={(event) => update("sessionId", event.target.value)} />
          </Field>
          <Field label="Sample Name">
            <input value={props.settings.sampleName} onChange={(event) => update("sampleName", event.target.value)} />
          </Field>
          <Field label="CONTROL_PLANE_API_KEY" hint="Stored in Windows Credential Manager, not settings.json.">
            <input
              type="password"
              value={props.settings.controlPlaneApiKey}
              onChange={(event) => update("controlPlaneApiKey", event.target.value)}
            />
          </Field>
          <Field label="tunnel-client Executable">
            <input
              value={props.settings.tunnelClientExecutable}
              onChange={(event) => update("tunnelClientExecutable", event.target.value)}
            />
          </Field>
          <Field label="local-mcp Executable">
            <input
              value={props.settings.localMcpExecutable}
              onChange={(event) => update("localMcpExecutable", event.target.value)}
            />
          </Field>
          <Field label="MCP Command">
            <input value={props.settings.mcpCommand} onChange={(event) => update("mcpCommand", event.target.value)} />
          </Field>
          <Field label="Working Directory" wide>
            <div className="browse-row">
              <input value={props.settings.workingDirectory} readOnly />
              <button onClick={() => void props.chooseDirectory("working")}>Browse</button>
            </div>
          </Field>
        </div>
      </section>

      <section className="panel settings-panel">
        <div className="settings-heading">
          <div>
            <h2>Automatic Allow Directories</h2>
            <p>These permissions are sent immediately after local-mcp starts.</p>
          </div>
          <button className="secondary-button" onClick={() => void props.chooseDirectory("allowed")}>Add Directories</button>
        </div>
        <div className="directory-list">
          {props.settings.allowedDirectories.length === 0 ? (
            <p className="empty-list">No additional directories configured.</p>
          ) : (
            props.settings.allowedDirectories.map((directory) => (
              <div className="directory-item" key={directory}>
                <code>{directory}</code>
                <button
                  onClick={() =>
                    update(
                      "allowedDirectories",
                      props.settings.allowedDirectories.filter((value) => value !== directory),
                    )
                  }
                >
                  Remove
                </button>
              </div>
            ))
          )}
        </div>
      </section>

      <div className="settings-footer">
        <span>{props.saveMessage}</span>
        <button className="primary-button save-button" onClick={() => void props.save()} disabled={props.busy}>
          Save Settings
        </button>
      </div>
    </main>
  );
}

function Field(props: { label: string; hint?: string; wide?: boolean; children: React.ReactNode }) {
  return (
    <label className={`field ${props.wide ? "wide" : ""}`}>
      <span>{props.label}</span>
      {props.children}
      {props.hint && <small>{props.hint}</small>}
    </label>
  );
}

function toMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return JSON.stringify(reason);
}

export default App;
