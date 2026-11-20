param(
    [string]$TauriRoot = (Join-Path $PSScriptRoot "..\src-tauri"),
    # Validation runs reuse an already staged runtime instead of downloading the
    # pinned package again. The staged files are still checked against the pinned
    # hashes; anything missing or different is re-staged from the pinned package.
    [switch]$SkipIfCurrent
)

$ErrorActionPreference = "Stop"

$runtimeVersion = "0.169.0"
$target = "x86_64-pc-windows-msvc"
$sourcePackage = "@openai/codex@$runtimeVersion-win32-x64"
$sourceUrl = "https://registry.npmjs.org/@openai/codex/-/codex-$runtimeVersion-win32-x64.tgz"
$sourceSha256 = "c48d07b46034d2b0a6c091227993c50e19c389680169244d0dcbb96c4ebc429f"
$runtimeSha256 = "ce1945eb440b5481ed1233ebaf32b7056ee7e4e452fa62ffe1cbfb26103cdee0"

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

if ($SkipIfCurrent) {
    $stagedRuntimeExe = Join-Path $runtimeDir "vendor\$target\bin\codex.exe"
    $isStaged = (Test-Path -LiteralPath $sidecarPath -PathType Leaf) -and
        (Test-Path -LiteralPath $metadataPath -PathType Leaf) -and
        (Test-Path -LiteralPath $stagedRuntimeExe -PathType Leaf)
    if ($isStaged) {
        $stagedSidecarSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $sidecarPath).Hash.ToLowerInvariant()
        $stagedRuntimeSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedRuntimeExe).Hash.ToLowerInvariant()
        $stagedMetadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
        if ($stagedSidecarSha -eq $runtimeSha256 -and $stagedRuntimeSha -eq $runtimeSha256 -and
            $stagedMetadata.runtime_version -eq $runtimeVersion) {
            Write-Host "Codex app-server runtime $runtimeVersion is already staged and matches the pinned hash; skipping the download."
            return
        }
    }
}

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
