# Local MCP Tunnel for Windows

RustとTauri 2で実装した、`tunnel-client`と`local-mcp`を操作するWindowsデスクトップアプリです。

現時点ではWSL内のLinux版CLIを使用します。将来Windowsネイティブ版CLIが提供された場合に切り替えられるよう、Rustバックエンドを`CliRuntime`で抽象化しています。

## インストール

wingetからインストールします。

```powershell
winget install WalkingWiFi.LocalMCPTunnel
```

アップデート:

```powershell
winget upgrade WalkingWiFi.LocalMCPTunnel
```

アンインストール:

```powershell
winget uninstall WalkingWiFi.LocalMCPTunnel
```

wingetへの初回登録がMicrosoftの審査中の場合は、GitHub ReleasesのNSISインストーラーを使用してください。

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

## 利用時の前提条件

- Windows 10またはWindows 11
- WSL2
- WSL内に`local-mcp`と`tunnel-client`がインストール済み
- Microsoft Edge WebView2 Runtime

## 開発環境の前提条件

- Node.js / pnpm
- Rust MSVC toolchain
- Visual Studio 2022 Build Tools

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

wingetマニフェストの検証:

```powershell
winget validate manifests\w\WalkingWiFi\LocalMCPTunnel\<version>
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

## Releaseとwinget公開フロー

`vX.Y.Z`形式のタグをpushすると、`.github/workflows/release.yml`が次の処理を実行します。

1. Node.js、pnpm、Rustをセットアップ
2. タグからアプリケーションバージョンを反映
3. TypeScriptビルド、Rustテスト、rustfmt、Clippyを実行
4. Windows x64向けNSISインストーラーを作成
5. 配布ファイル名を`Local-MCP-Tunnel-X.Y.Z-win-x64-setup.exe`へ統一
6. SHA-256ファイルを生成
7. 実際のSHA-256を含むwinget分割マニフェストを生成
8. インストーラー、SHA-256、wingetマニフェストZIPをGitHub Releaseへ添付

公開:

```powershell
git tag v0.1.0
git push origin v0.1.0
```

生成されるReleaseファイル:

```text
Local-MCP-Tunnel-0.1.0-win-x64-setup.exe
Local-MCP-Tunnel-0.1.0-win-x64-setup.exe.sha256
WalkingWiFi.LocalMCPTunnel-0.1.0-winget-manifests.zip
```

初回登録では、Releaseで生成されたマニフェストをMicrosoftの`winget-pkgs`リポジトリへPull Requestとして提出します。Microsoftによる検証・審査・マージ後、`winget install WalkingWiFi.LocalMCPTunnel`が利用可能になります。

以降のバージョンは、同じPackage Identifierの新しいバージョンとしてマニフェストを提出します。

### ローカルでwingetマニフェストを生成

```powershell
$version = "0.1.0"
$installer = "src-tauri\target\release\bundle\nsis\Local MCP Tunnel_0.1.0_x64-setup.exe"
$sha256 = (Get-FileHash $installer -Algorithm SHA256).Hash

.\scripts\generate-winget-manifests.ps1 `
  -Version $version `
  -InstallerSha256 $sha256 `
  -Repository "walkingwifi28/local-mcp-tunnel-win"
```

### コード署名

現在のインストーラーは未署名です。Windows Defender SmartScreenの警告が表示される可能性があります。詳細は[CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md)を参照してください。

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
