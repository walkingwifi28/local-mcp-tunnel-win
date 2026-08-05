# Local MCP Tunnel for Windows

RustとTauri 2で実装した、`tunnel-client`と`local-mcp`を操作するWindowsデスクトップアプリです。

現時点ではWSL内のLinux版CLIを使用します。将来Windowsネイティブ版CLIが提供された場合に切り替えられるよう、Rustバックエンドを`CliRuntime`で抽象化しています。

## 構成

- Tauri 2
- Rust
- React 19 + TypeScript
- pnpm
- Windows Credential Manager
- NSIS / MSI installer

```text
src-tauri/src/runtime/
├── mod.rs                 # CliRuntime trait / shared command model
├── wsl.rs                 # wsl.exe integration
└── native_windows.rs      # native .exe runtime
```

## 実装済み機能

- `tunnel-client init`
- `tunnel-client run`
- `local-mcp start`
- プロセス開始・停止
- stdout / stderrのリアルタイム表示
- ログ欄からの標準入力送信
- `/permission ask|yolo|allow|revoke|list|status`
- 起動時のPermission Mode適用
- 起動時の複数ディレクトリ自動allow
- WindowsパスからWSLパスへの変換
- WSLディストリビューション選択
- 将来のWindowsネイティブCLI切り替え
- APIキーのWindows Credential Manager保存
- 通常設定のJSON保存

## 前提条件

- Windows 10またはWindows 11
- WSL2
- WSL内に`local-mcp`と`tunnel-client`がインストール済み
- Node.js / pnpm
- Rust MSVC toolchain
- Visual Studio 2022 Build Tools
- Microsoft Edge WebView2 Runtime

Rustの確認:

```powershell
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"
rustc -Vv
rustup show active-toolchain
```

`host: x86_64-pc-windows-msvc`および`stable-x86_64-pc-windows-msvc`になっていることを確認します。

## 開発

```powershell
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"
pnpm install
pnpm tauri dev
```

## 検証

```powershell
pnpm build

cd src-tauri
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo check
```

## Windowsインストーラー作成

```powershell
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"
pnpm tauri build
```

生成先:

```text
src-tauri/target/release/bundle/nsis/
src-tauri/target/release/bundle/msi/
```

## 設定保存先

通常設定はTauriのアプリ設定ディレクトリ内の`settings.json`へ保存します。

`CONTROL_PLANE_API_KEY`はJSONへ保存せず、Windows Credential Managerの次のエントリへ保存します。

```text
Service: Local MCP Tunnel
Username: CONTROL_PLANE_API_KEY
```

## Runtime

### WSL

`wsl.exe`から指定ディストリビューションの`bash`を起動します。ディストリビューション名が空の場合は、Windowsで設定されたデフォルトのWSLディストリビューションを使用します。

Windowsパスは自動変換されます。

```text
C:\Users\atush\source\repos
→ /mnt/c/Users/atush/source/repos
```

`$HOME`または`~/`で始まる実行ファイルと引数は、WSL内のホームディレクトリへ展開されます。

### Native Windows

設定画面で`Native Windows`へ切り替えると、指定した`.exe`を直接起動します。現在の環境ではWindows版CLIがないため、通常はWSLを使用してください。
