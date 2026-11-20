param(
    [ValidateSet("cpu", "vulkan", "directml", "windows-gpu", "cuda", "openblas")]
    [string]$Feature = "windows-gpu",

    [switch]$CheckOnly,
    [switch]$SkipInstall,

    # With -CheckOnly: run only the native Rust validation and native tests.
    # Frontend typecheck and tests are covered by their own job. The frontend
    # assets the native build embeds are still rebuilt from the reviewed sources.
    [switch]$NativeOnly
)

$ErrorActionPreference = "Stop"
if ($NativeOnly -and -not $CheckOnly) {
    throw "-NativeOnly applies only to -CheckOnly validation."
}

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
Assert-Command "git"
Assert-VulkanSdk
. (Join-Path $PSScriptRoot "configure-windows-portability.ps1")

$repoRoot = Resolve-Path (Join-Path $frontendRoot "..")
node (Join-Path $repoRoot "scripts\verify-public-repo-safety.mjs")
if ($LASTEXITCODE -ne 0) {
    throw "Public-repository safety check failed. Remove private paths or secrets before building."
}

& (Join-Path $PSScriptRoot "verify-brand-icons.ps1") -FrontendRoot $frontendRoot
$sherpaRuntime = if ($Feature -in @("directml", "windows-gpu")) { "directml" } else { "cpu" }
& (Join-Path $PSScriptRoot "stage-sherpa-runtime.ps1") -TauriRoot $tauriRoot -Runtime $sherpaRuntime

$windowsTarget = "x86_64-pc-windows-msvc"
$llamaHelperBinary = Join-Path $tauriRoot "binaries\llama-helper-$windowsTarget.exe"
Assert-File $llamaHelperBinary "Build it from the repository root with 'cargo build -p llama-helper --release --target $windowsTarget', then copy 'target\$windowsTarget\release\llama-helper.exe' to this path."
$codexRuntimeBinary = Join-Path $tauriRoot "binaries\codex-app-server-$windowsTarget.exe"
& (Join-Path $PSScriptRoot "stage-codex-runtime.ps1") -TauriRoot $tauriRoot -SkipIfCurrent:$CheckOnly
Assert-File $codexRuntimeBinary "Run 'frontend\scripts\stage-codex-runtime.ps1' before bundling the Windows installers."
node (Join-Path $PSScriptRoot "test-codex-runtime.mjs")
if ($LASTEXITCODE -ne 0) { throw "Bundled Codex runtime smoke failed." }

$env:NEXT_TELEMETRY_DISABLED = "1"
$env:TAURI_BUNDLE_TARGETS = "msi,nsis"

if (-not $SkipInstall) {
    pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "Frontend dependency installation failed." }
}

$featureArgs = @()
if ($Feature -ne "cpu") {
    $featureArgs = @("--features", $Feature)
}

# Reused runner checkouts keep generated output between runs. Remove only folders
# inside the frontend workspace that hold no link.
function Remove-GeneratedFrontendFolder {
    param([Parameter(Mandatory=$true)][ValidateSet(".next/types", "out")][string]$RelativePath)
    $folder = Join-Path $frontendRoot $RelativePath
    if (-not (Test-Path -LiteralPath $folder)) { return }
    $resolved = (Resolve-Path -LiteralPath $folder).Path
    $frontendPrefix = $frontendRoot.Path.TrimEnd([char]92, [char]47) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($frontendPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Generated frontend output '$RelativePath' is outside the frontend workspace."
    }
    $directory = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
    while ($directory) {
        if ($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            throw "Generated frontend output '$RelativePath' is inside a linked path."
        }
        $directory = $directory.Parent
    }
    if (Get-ChildItem -LiteralPath $resolved -Recurse -Force -Attributes ReparsePoint -ErrorAction Stop | Select-Object -First 1) {
        throw "Generated frontend output '$RelativePath' contains a link."
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

# Generated types for deleted routes would break the typecheck.
Remove-GeneratedFrontendFolder ".next/types"

# The native build embeds the exported frontend (frontend/out). Rebuild it from
# the reviewed sources every time, so a stale folder left on the persistent
# runner is never compiled into the tested or shipped application.
function Build-FrontendAssets {
    Remove-GeneratedFrontendFolder "out"
    pnpm build
    if ($LASTEXITCODE -ne 0) { throw "Frontend production build failed." }
    Assert-File (Join-Path $frontendRoot "out/index.html") "The frontend build produced no exported assets."
}

if ($CheckOnly) {
    Build-FrontendAssets
    Push-Location $tauriRoot
    try {
        cargo check --locked @featureArgs
        if ($LASTEXITCODE -ne 0) { throw "Native Rust validation failed." }
    } finally {
        Pop-Location
    }

    if (-not $NativeOnly) {
        pnpm exec tsc --noEmit
        if ($LASTEXITCODE -ne 0) { throw "Frontend typecheck failed." }
        pnpm test
        if ($LASTEXITCODE -ne 0) { throw "Frontend regression tests failed." }
    }
    $nativeFeatures =if ($Feature -eq "cpu") { "" } else { $Feature }
    & (Join-Path $PSScriptRoot "test-windows-native.ps1") -Features $nativeFeatures
    exit 0
}

pnpm exec tsc --noEmit
if ($LASTEXITCODE -ne 0) { throw "Frontend typecheck failed." }
pnpm test
if ($LASTEXITCODE -ne 0) { throw "Frontend regression tests failed." }
Build-FrontendAssets

$cargoMetadata = cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $tauriRoot "Cargo.toml") |
    ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $cargoMetadata.target_directory) {
    throw "Failed to resolve the Cargo workspace target directory."
}
$bundleRoot = Join-Path $cargoMetadata.target_directory "release\bundle"
Remove-Item -LiteralPath (Join-Path $bundleRoot "msi") -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $bundleRoot "nsis") -Recurse -Force -ErrorAction SilentlyContinue

if ($Feature -eq "cpu") {
    pnpm exec tauri build -- --locked
} else {
    pnpm exec tauri build -- @featureArgs --locked
}

if ($LASTEXITCODE -ne 0) { throw "Windows installer build failed." }

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

$expectedMsi = "ClawScribe_${sourceVersion}_x64_en-US.msi"
$expectedNsis = "ClawScribe_${sourceVersion}_x64-setup.exe"
foreach ($expected in @($expectedMsi, $expectedNsis)) {
    if (-not ($artifactFiles | Where-Object { $_.Name -eq $expected -and $_.Length -gt 0 })) {
        throw "Required current-version installer '$expected' is missing or empty."
    }
}

$checksumPath = Join-Path $bundleRoot "SHA256SUMS.txt"
$resolvedBundleRoot = (Resolve-Path -LiteralPath $bundleRoot).Path
$checksumLines = foreach ($artifact in $artifactFiles | Sort-Object FullName) {
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $artifact.FullName
    $relativePath = $artifact.FullName.Substring($resolvedBundleRoot.Length).TrimStart([char]92, [char]47).Replace("\", "/")
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
    "codex_runtime_version=0.169.0",
    "codex_runtime_target=$windowsTarget",
    "codex_runtime_source_package=@openai/codex@0.169.0-win32-x64",
    "codex_runtime_source_url=https://registry.npmjs.org/@openai/codex/-/codex-0.169.0-win32-x64.tgz",
    "codex_runtime_sha256=ce1945eb440b5481ed1233ebaf32b7056ee7e4e452fa62ffe1cbfb26103cdee0",
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
