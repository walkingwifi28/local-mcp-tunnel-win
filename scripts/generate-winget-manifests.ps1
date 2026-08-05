param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Fa-f0-9]{64}$')]
    [string]$InstallerSha256,

    [string]$Repository = 'walkingwifi28/local-mcp-tunnel-win',

    [string]$InstallerName = "Local-MCP-Tunnel-$Version-win-x64-setup.exe",

    [string]$OutputRoot = 'manifests',

    [datetime]$ReleaseDate = (Get-Date)
)

$ErrorActionPreference = 'Stop'
$packageIdentifier = 'WalkingWiFi.LocalMCPTunnel'
$manifestVersion = '1.9.0'
$installerUrl = "https://github.com/$Repository/releases/download/v$Version/$InstallerName"
$manifestDirectory = Join-Path $OutputRoot "w\WalkingWiFi\LocalMCPTunnel\$Version"

New-Item -ItemType Directory -Path $manifestDirectory -Force | Out-Null

$versionManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$manifestVersion.schema.json
PackageIdentifier: $packageIdentifier
PackageVersion: $Version
DefaultLocale: ja-JP
ManifestType: version
ManifestVersion: $manifestVersion
"@

$installerManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$manifestVersion.schema.json
PackageIdentifier: $packageIdentifier
PackageVersion: $Version
InstallerType: nullsoft
Scope: user
InstallModes:
  - interactive
  - silent
  - silentWithProgress
UpgradeBehavior: install
ReleaseDate: $($ReleaseDate.ToString('yyyy-MM-dd'))
Installers:
  - Architecture: x64
    InstallerUrl: $installerUrl
    InstallerSha256: $($InstallerSha256.ToUpperInvariant())
ManifestType: installer
ManifestVersion: $manifestVersion
"@

$localeManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$manifestVersion.schema.json
PackageIdentifier: $packageIdentifier
PackageVersion: $Version
PackageLocale: ja-JP
Publisher: WalkingWiFi
PublisherUrl: https://github.com/walkingwifi28
PublisherSupportUrl: https://github.com/$Repository/issues
Author: WalkingWiFi
PackageName: Local MCP Tunnel
PackageUrl: https://github.com/$Repository
License: Proprietary
ShortDescription: tunnel-clientとlocal-mcpをWindowsから操作するデスクトップアプリです。
Description: Tauri、Rust、Reactで構築されたWindowsデスクトップアプリです。WSL内またはWindowsネイティブのtunnel-clientとlocal-mcpを起動し、リアルタイムログ、標準入力、Permission操作、設定保存を提供します。
Tags:
  - mcp
  - tauri
  - wsl
  - developer-tools
ManifestType: defaultLocale
ManifestVersion: $manifestVersion
"@

$utf8 = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText(
    (Join-Path $manifestDirectory "$packageIdentifier.yaml"),
    ($versionManifest.TrimStart() + "`n"),
    $utf8
)
[System.IO.File]::WriteAllText(
    (Join-Path $manifestDirectory "$packageIdentifier.installer.yaml"),
    ($installerManifest.TrimStart() + "`n"),
    $utf8
)
[System.IO.File]::WriteAllText(
    (Join-Path $manifestDirectory "$packageIdentifier.locale.ja-JP.yaml"),
    ($localeManifest.TrimStart() + "`n"),
    $utf8
)

Write-Host "wingetマニフェストを生成しました: $manifestDirectory"
Get-ChildItem $manifestDirectory -File | Select-Object -ExpandProperty FullName
