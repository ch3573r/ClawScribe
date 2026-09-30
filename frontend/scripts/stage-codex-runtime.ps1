param(
    [string]$TauriRoot = (Join-Path $PSScriptRoot "..\src-tauri")
)

$ErrorActionPreference = "Stop"

$runtimeVersion = "0.159.2"
$target = "x86_64-pc-windows-msvc"
$sourcePackage = "@openai/codex@$runtimeVersion-win32-x64"
$sourceUrl = "https://registry.npmjs.org/@openai/codex/-/codex-$runtimeVersion-win32-x64.tgz"
$sourceSha256 = "a71d5560d56189969350cf42c0121b57d69ae88405fcdab89fc3fa3d704f6bb6"
$runtimeSha256 = "52f75c649bebb8001102a1dd129c1ea6d02b0940321e6d7e82ee0526753bd58a"

function Assert-Command {
    param([Parameter(Mandatory=$true)][string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing required command '$Name'."
    }
}

function Assert-Sha256 {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)][string]$Expected
    )
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "SHA256 mismatch for '$Path'. Expected $Expected but got $actual."
    }
}

Assert-Command "npm"
Assert-Command "tar"

$resolvedTauriRoot = (Resolve-Path -LiteralPath $TauriRoot).Path
$binariesDir = Join-Path $resolvedTauriRoot "binaries"
$runtimeDir = Join-Path $binariesDir "codex-app-server-runtime"
$sidecarPath = Join-Path $binariesDir "codex-app-server-$target.exe"
$metadataPath = Join-Path $binariesDir "codex-app-server-runtime.json"
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) "clawscribe-codex-runtime-$runtimeVersion-$target"
$extractDir = Join-Path $tempRoot "extract"

New-Item -ItemType Directory -Force -Path $binariesDir | Out-Null
Remove-Item -Recurse -Force -LiteralPath $tempRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $extractDir | Out-Null

Push-Location $tempRoot
try {
    npm pack $sourcePackage --pack-destination $tempRoot | Out-Host
} finally {
    Pop-Location
}

$tarball = Join-Path $tempRoot "openai-codex-$runtimeVersion-win32-x64.tgz"
if (-not (Test-Path -LiteralPath $tarball -PathType Leaf)) {
    throw "Codex runtime package was not downloaded: $sourcePackage"
}
Assert-Sha256 -Path $tarball -Expected $sourceSha256

tar -xzf $tarball -C $extractDir
$packageRoot = Join-Path $extractDir "package"
$vendorRoot = Join-Path $packageRoot "vendor\$target"
$sourceExe = Join-Path $vendorRoot "bin\codex.exe"
Assert-Sha256 -Path $sourceExe -Expected $runtimeSha256

Remove-Item -Recurse -Force -LiteralPath $runtimeDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
Copy-Item -Recurse -Force -LiteralPath (Join-Path $packageRoot "vendor") -Destination $runtimeDir
# ClawScribe drives text-only app-server turns. The optional voice host carries
# separately licensed GStreamer/GLib and Microsoft VC++ runtime DLLs, so it is
# not redistributed with the installer.
$voiceDir = Join-Path $runtimeDir "vendor\$target\codex-resources\voice"
if (Test-Path -LiteralPath $voiceDir) {
    Remove-Item -Recurse -Force -LiteralPath $voiceDir
}
New-Item -ItemType File -Force -Path (Join-Path $runtimeDir ".gitkeep") | Out-Null
Copy-Item -Force -LiteralPath $sourceExe -Destination $sidecarPath

$metadata = [ordered]@{
    name = "Codex app-server runtime"
    runtime_version = $runtimeVersion
    target = $target
    source_package = $sourcePackage
    source_url = $sourceUrl
    source_sha256 = $sourceSha256
    runtime_sha256 = $runtimeSha256
    license = "Apache-2.0"
    notice = "Codex is distributed by OpenAI under the Apache-2.0 license. ClawScribe bundles the pinned Windows x64 runtime only for the Advanced: Codex app-server provider."
    build_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd")
    entrypoint = "binaries/codex-app-server-$target.exe"
}
$metadata | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $metadataPath -Encoding utf8

Write-Host "Staged Codex app-server runtime:"
Write-Host "  package: $sourcePackage"
Write-Host "  sidecar: $sidecarPath"
Write-Host "  sha256: $runtimeSha256"
Write-Host "  metadata: $metadataPath"
