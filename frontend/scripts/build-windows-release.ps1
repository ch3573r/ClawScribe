param(
    [ValidateSet("cpu", "vulkan", "directml", "windows-gpu", "cuda", "openblas")]
    [string]$Feature = "windows-gpu",

    [switch]$CheckOnly,
    [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"

$frontendRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$tauriRoot = Join-Path $frontendRoot "src-tauri"

Set-Location $frontendRoot

function Assert-Command {
    param(
        [Parameter(Mandatory=$true)]
        [string]$Name
    )

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing required command '$Name'. Install it before running the ClawScribe Windows release build."
    }
}

function Assert-File {
    param(
        [Parameter(Mandatory=$true)]
        [string]$Path,

        [Parameter(Mandatory=$true)]
        [string]$Hint
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing required file '$Path'. $Hint"
    }
}

function Assert-VulkanSdk {
    if ($Feature -notin @("vulkan", "windows-gpu")) {
        return
    }

    if (-not $env:VULKAN_SDK -or -not (Test-Path -LiteralPath $env:VULKAN_SDK -PathType Container)) {
        throw "Feature '$Feature' requires the Vulkan SDK. Install it and make sure VULKAN_SDK points to the SDK root."
    }

    foreach ($relativePath in @("Bin", "Lib", "Include")) {
        $path = Join-Path $env:VULKAN_SDK $relativePath
        if (-not (Test-Path -LiteralPath $path -PathType Container)) {
            throw "Feature '$Feature' requires Vulkan SDK path '$path'."
        }
    }
}

$isWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if (-not $isWindowsHost) {
    throw "Windows release artifacts must be built on Windows."
}

Assert-Command "node"
Assert-Command "pnpm"
Assert-Command "cargo"
Assert-VulkanSdk

& (Join-Path $PSScriptRoot "verify-brand-icons.ps1") -FrontendRoot $frontendRoot
& (Join-Path $PSScriptRoot "stage-sherpa-runtime.ps1") -TauriRoot $tauriRoot

$windowsTarget = "x86_64-pc-windows-msvc"
$llamaHelperBinary = Join-Path $tauriRoot "binaries\llama-helper-$windowsTarget.exe"
Assert-File $llamaHelperBinary "Build it from the repository root with 'cargo build -p llama-helper --release --target $windowsTarget', then copy 'target\$windowsTarget\release\llama-helper.exe' to this path."
$codexRuntimeBinary = Join-Path $tauriRoot "binaries\codex-app-server-$windowsTarget.exe"
if (-not $CheckOnly) {
    & (Join-Path $PSScriptRoot "stage-codex-runtime.ps1") -TauriRoot $tauriRoot
    Assert-File $codexRuntimeBinary "Run 'frontend\scripts\stage-codex-runtime.ps1' before bundling the Windows installers."
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

$bundleRoot = Join-Path $tauriRoot "target\release\bundle"
Remove-Item -LiteralPath (Join-Path $bundleRoot "msi") -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $bundleRoot "nsis") -Recurse -Force -ErrorAction SilentlyContinue

if ($Feature -eq "cpu") {
    pnpm exec tauri build
} else {
    pnpm exec tauri build -- @featureArgs
}

$sourceVersion = (node -p "require('./package.json').version").Trim()
$commit = (git -C (Join-Path $frontendRoot "..") rev-parse HEAD).Trim()
$shortCommit = (git -C (Join-Path $frontendRoot "..") rev-parse --short HEAD).Trim()
$buildDateUtc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$upstreamBaseVersion = "0.4.0"
$artifactPatterns = @(
    Join-Path $bundleRoot "msi\*.msi"
    Join-Path $bundleRoot "nsis\*.exe"
)

Write-Host ""
Write-Host "Windows release artifacts:"
$artifactFiles = @()
foreach ($pattern in $artifactPatterns) {
    Get-ChildItem $pattern -ErrorAction SilentlyContinue | ForEach-Object {
        $artifactFiles += $_
        Write-Host "  $($_.FullName)"
    }
}

if ($artifactFiles.Count -eq 0) {
    throw "No Windows release artifacts were found under '$bundleRoot'."
}

$checksumPath = Join-Path $bundleRoot "SHA256SUMS.txt"
$resolvedBundleRoot = (Resolve-Path -LiteralPath $bundleRoot).Path
$checksumLines = foreach ($artifact in $artifactFiles | Sort-Object FullName) {
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $artifact.FullName
    $relativePath = [System.IO.Path]::GetRelativePath($resolvedBundleRoot, $artifact.FullName).Replace("\", "/")
    "$($hash.Hash.ToLowerInvariant())  $relativePath"
}
$checksumLines | Set-Content -LiteralPath $checksumPath -Encoding ascii
$metadataPath = Join-Path $bundleRoot "BUILD-METADATA.txt"
$metadata = @(
    "product=ClawScribe",
    "version=$sourceVersion",
    "upstream_base_version=$upstreamBaseVersion",
    "build_commit=$commit",
    "build_commit_short=$shortCommit",
    "build_date_utc=$buildDateUtc",
    "codex_runtime_version=0.139.0",
    "codex_runtime_target=$windowsTarget",
    "codex_runtime_source_package=@openai/codex@0.139.0-win32-x64",
    "codex_runtime_source_url=https://registry.npmjs.org/@openai/codex/-/codex-0.139.0-win32-x64.tgz",
    "codex_runtime_sha256=77a84f8078400467ade4301d827b8bcea2d29b6838c9cd162bf3573b7ef97e10",
    "codex_runtime_license=Apache-2.0"
)
$metadata | Set-Content -LiteralPath $metadataPath -Encoding ascii

Write-Host ""
Write-Host "SHA-256 checksums:"
Get-Content -LiteralPath $checksumPath | ForEach-Object {
    Write-Host "  $_"
}
Write-Host "  $checksumPath"
Write-Host "  $metadataPath"
