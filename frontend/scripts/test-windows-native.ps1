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

    $listing = & $testExecutable --list
    if ($LASTEXITCODE -ne 0) { throw "Native library test discovery failed." }
    $tests = @($listing | Where-Object { $_ -match ': test$' })
    if ($tests.Count -eq 0 -or -not ($tests -like 'self_test::tests::*')) {
        throw "Expected the application library tests, including packaged self-test coverage."
    }
    Write-Host "Discovered $($tests.Count) native library tests. Running all non-ignored tests."
    & $testExecutable --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Native library tests failed." }

    $whisperFixture = "audio::transcription::cloud::whisper_audio::tests::ffmpeg_creates_compact_parts_and_reports_actual_boundaries"
    if ("${whisperFixture}: test" -notin $tests) {
        throw "The Hosted Whisper FFmpeg fixture is missing."
    }
    & $testExecutable $whisperFixture --exact --ignored
    if ($LASTEXITCODE -ne 0) { throw "Hosted Whisper FFmpeg validation failed." }

    if ($env:CLAWSCRIBE_MEASURE_MAI -eq "true") {
        $fixture = "audio::transcription::cloud::tests::mai_conversion_and_upload_stream_measurements"
        $listing = & $testExecutable $fixture --exact --ignored --list
        if ($LASTEXITCODE -ne 0 -or "${fixture}: test" -notin $listing) {
            throw "The synthetic MAI performance fixture is missing."
        }
        $previousDuration = $env:CLAWSCRIBE_MAI_PERF_SECONDS
        try {
            $env:CLAWSCRIBE_MAI_PERF_SECONDS = "600"
            & $testExecutable $fixture --exact --ignored --nocapture --test-threads=1
            if ($LASTEXITCODE -ne 0) { throw "Synthetic MAI audio measurements failed." }
        } finally {
            $env:CLAWSCRIBE_MAI_PERF_SECONDS = $previousDuration
        }
    }
} finally {
    $env:PATH = $previousPath
    Pop-Location
}
