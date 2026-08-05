param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$utf8 = [System.Text.UTF8Encoding]::new($false)

function Set-RequiredVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Pattern,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    $content = [System.IO.File]::ReadAllText($Path)
    $regex = [regex]::new($Pattern, [System.Text.RegularExpressions.RegexOptions]::Multiline)
    if (-not $regex.IsMatch($content)) {
        throw "$Label のversionを検出できませんでした: $Path"
    }

    $updated = $regex.Replace(
        $content,
        [System.Text.RegularExpressions.MatchEvaluator]{
            param($match)
            return $match.Groups[1].Value + $Version + $match.Groups[2].Value
        },
        1
    )

    if ($updated -ne $content) {
        [System.IO.File]::WriteAllText($Path, $updated, $utf8)
    }
}

Set-RequiredVersion `
    -Path (Join-Path $root 'package.json') `
    -Pattern '^(\s*"version"\s*:\s*")[^"]+(".*)$' `
    -Label 'package.json'

Set-RequiredVersion `
    -Path (Join-Path $root 'src-tauri\tauri.conf.json') `
    -Pattern '^(\s*"version"\s*:\s*")[^"]+(".*)$' `
    -Label 'tauri.conf.json'

Set-RequiredVersion `
    -Path (Join-Path $root 'src-tauri\Cargo.toml') `
    -Pattern '(?s)(\[package\].*?^version\s*=\s*")[^"]+(".*)$' `
    -Label 'Cargo.toml'

$cargoLockPath = Join-Path $root 'src-tauri\Cargo.lock'
if (Test-Path $cargoLockPath) {
    Set-RequiredVersion `
        -Path $cargoLockPath `
        -Pattern '(?s)(\[\[package\]\]\s*name\s*=\s*"local-mcp-tunnel-win"\s*version\s*=\s*")[^"]+(".*)$' `
        -Label 'Cargo.lock'
}

Write-Host "アプリケーションバージョンを $Version に設定しました。"
