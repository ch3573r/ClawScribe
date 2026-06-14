param(
    [ValidateSet("cpu", "vulkan", "cuda", "openblas")]
    [string]$Feature = "vulkan",

    [switch]$CheckOnly,
    [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"

$frontendRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$tauriRoot = Join-Path $frontendRoot "src-tauri"

Set-Location $frontendRoot

$isWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if (-not $isWindowsHost) {
    throw "Windows release artifacts must be built on Windows."
}

$env:NEXT_TELEMETRY_DISABLED = "1"
$env:TAURI_BUNDLE_TARGETS = "msi,nsis"

if (-not $SkipInstall) {
    pnpm install --frozen-lockfile
}

$featureArgs = @()
if ($Feature -ne "cpu") {
    $featureArgs = @("--features", $Feature)
}

if ($CheckOnly) {
    Push-Location $tauriRoot
    try {
        cargo check @featureArgs
    } finally {
        Pop-Location
    }

    pnpm exec tsc --noEmit
    exit 0
}

pnpm build

if ($Feature -eq "cpu") {
    pnpm exec tauri build
} else {
    pnpm exec tauri build -- @featureArgs
}

$bundleRoot = Join-Path $tauriRoot "target\release\bundle"
$artifactPatterns = @(
    Join-Path $bundleRoot "msi\*.msi"
    Join-Path $bundleRoot "nsis\*.exe"
)

Write-Host ""
Write-Host "Windows release artifacts:"
foreach ($pattern in $artifactPatterns) {
    Get-ChildItem $pattern -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "  $($_.FullName)"
    }
}
