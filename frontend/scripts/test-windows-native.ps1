param(
    [string]$Features = "windows-gpu"
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$manifest = Join-Path $repoRoot "frontend/src-tauri/Cargo.toml"
$runtime = Join-Path $repoRoot "frontend/src-tauri/binaries/sherpa-onnx"
$stagedFfmpeg = Join-Path (Split-Path -Parent $manifest) "binaries/ffmpeg-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path -LiteralPath $stagedFfmpeg -PathType Leaf)) {
    throw "Missing staged ffmpeg; build the app first."
}

if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { throw "Native Windows tests must run on Windows." }
. (Join-Path $PSScriptRoot "configure-windows-portability.ps1")

$featureList = @($Features.Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$allowedFeatures = @("windows-gpu", "vulkan", "directml", "cuda", "openblas")
foreach ($feature in $featureList) {
    if ($feature -notin $allowedFeatures) { throw "Unsupported test feature: $feature" }
}
$featureArgs = @()
if ($featureList.Count -gt 0) { $featureArgs = @("--features", ($featureList -join ",")) }

# Vulkan is a load-time dependency even when the selected tests do not use a
# GPU. Fail before compilation/discovery with the missing prerequisite named.
# Installation is explicit in CI; this helper never changes the local machine.
if ($featureList -contains "windows-gpu" -or $featureList -contains "vulkan") {
    & (Join-Path $PSScriptRoot "ensure-windows-vulkan-runtime.ps1")
}

$requiredDlls = @("onnxruntime.dll", "sherpa-onnx-c-api.dll", "sherpa-onnx-cxx-api.dll")
if ($featureList -contains "windows-gpu" -or $featureList -contains "directml") {
    $requiredDlls += "DirectML.dll"
}
foreach ($dll in $requiredDlls) {
    if (-not (Test-Path -LiteralPath (Join-Path $runtime $dll) -PathType Leaf)) {
        throw "Missing staged runtime DLL '$dll'. Run stage-sherpa-runtime.ps1 first."
    }
}

$previousPath = $env:PATH
Push-Location $repoRoot
try {
    # Match the DLL set shipped beside the installed application. A bare cargo
    # test executable lives in target/release/deps, outside Tauri's bundle layout.
    $env:PATH = "$runtime;$previousPath"
    & cargo test -p llama-helper --release --locked --target x86_64-pc-windows-msvc -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Local summary helper regression tests failed." }

    $metadataText = & cargo metadata --locked --format-version 1 --no-deps --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw "Cargo metadata failed." }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    if (-not $metadata.target_directory) { throw "Cargo returned no target directory." }

    $buildOutput = & cargo test --release --locked --manifest-path $manifest @featureArgs --lib --no-run --message-format=json-render-diagnostics
    if ($LASTEXITCODE -ne 0) { throw "Native release-profile test compilation failed." }
    $executables = @($buildOutput | ForEach-Object {
        if ($_ -match '^\s*\{') {
            $message = $_ | ConvertFrom-Json
            if ($message.reason -eq 'compiler-artifact' -and $message.profile.test -and $message.executable) {
                $message.executable
            }
        }
    } | Select-Object -Unique)
    if ($executables.Count -ne 1) { throw "Expected exactly one native library test executable." }
    $testExecutable = (Resolve-Path -LiteralPath $executables[0]).Path
    $targetPrefix = (Resolve-Path -LiteralPath $metadata.target_directory).Path.TrimEnd([char]92, [char]47) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $testExecutable.StartsWith($targetPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Test executable is outside the resolved Cargo target directory."
    }

    $testDirectory = Split-Path -Parent $testExecutable
    node (Join-Path $PSScriptRoot "verify-windows-portability.mjs") (Split-Path -Parent $testDirectory)
    if ($LASTEXITCODE -ne 0) { throw "Windows CPU portability validation failed." }
    Get-ChildItem -LiteralPath $runtime -Filter '*.dll' -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $testDirectory $_.Name) -Force
    }

    # Prefer the build-staged encoder over PATH and runtime download fallback.
    Copy-Item -LiteralPath $stagedFfmpeg -Destination (Join-Path $testDirectory "ffmpeg.exe") -Force

    foreach ($filter in @(
        "summary::",
        "model_download::tests",
        "audio::decoder::tests",
        "audio::vad::tests::flush_",
        "audio::async_logger::tests",
        "audio::hardware_detector::tests",
        "updates::tests",
        "audio::recording_mode::tests",
        "audio::pipeline::queue_failure_tests",
        "audio::recording_saver::snapshot_tests",
        "audio::audio_spool::tests",
        "audio::incremental_saver::tests",
        "audio::audio_processing::folder_tests",
        "audio::recording_state::tests",
        "audio::recording_commands::stop_tests",
        "audio::transcription::queue::tests",
        "audio::outcome::tests",
        "audio::retranscription::tests",
        "api::",
        "database::repositories::transcript::recording_save_tests",
        "whisper_engine::acceleration::tests",
        "database::transcript_edits::tests",
        "library::",
        "database::repositories::meeting::tests",
        "database::manager::tests",
        "credentials::tests",
        "exports::",
        "openclaw::tests",
        "openai::",
        "transcript_preservation_tests",
        "model_switch_tests",
        "audio::batch_audio::tests",
        "audio::common::tests",
        "audio::inference::tests",
        "audio::transcription::nemotron_provider::tests",
        "audio::transcription::cloud::"
    )) {
        $listing = & $testExecutable $filter --list
        if ($LASTEXITCODE -ne 0) { throw "Native test discovery failed for '$filter'." }
        $testCount = @($listing | Where-Object { $_ -match ': test$' }).Count
        if ($testCount -eq 0) { throw "No tests matched required filter '$filter'." }
        Write-Host "Running $testCount required native tests: $filter"
        & $testExecutable $filter --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw "Native tests failed for '$filter'." }
    }
} finally {
    $env:PATH = $previousPath
    Pop-Location
}
