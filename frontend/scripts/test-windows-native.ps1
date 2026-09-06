param(
    [string]$Features = "windows-gpu"
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$manifest = Join-Path $repoRoot "frontend/src-tauri/Cargo.toml"
$runtime = Join-Path $repoRoot "frontend/src-tauri/binaries/sherpa-onnx"

if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { throw "Native Windows tests must run on Windows." }

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
    Get-ChildItem -LiteralPath $runtime -Filter '*.dll' -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $testDirectory $_.Name) -Force
    }

    foreach ($filter in @("summary::processor::tests", "summary::chunking::tests", "audio::async_logger::tests", "audio::hardware_detector::tests", "updates::tests")) {
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
