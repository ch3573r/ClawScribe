param([switch]$Acceptance, [bool]$FullSuite = $true, [switch]$RetrievalAcceptance, [switch]$MutationAcceptance)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ([string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER) -or
    $env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER -or
    $env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER) { throw 'Designated build runner required.' }
. (Join-Path $PSScriptRoot 'configure-windows-portability.ps1')
$env:LIBCLANG_PATH = Join-Path $env:ProgramFiles 'LLVM/bin'
if ([string]::IsNullOrWhiteSpace($env:VULKAN_SDK)) { throw 'The pinned Vulkan SDK is required.' }
$header = Join-Path $env:VULKAN_SDK 'Include/vulkan/vulkan_core.h'
if (-not (Test-Path -LiteralPath $header) -or
    -not (Select-String -LiteralPath $header -Pattern '^#define VK_HEADER_VERSION 309$' -Quiet)) {
    throw 'The repository-pinned Vulkan SDK 1.4.309.0 is required.'
}
./frontend/scripts/ensure-windows-vulkan-runtime.ps1
node scripts/verify-public-repo-safety.mjs
cargo fmt --all -- --check
./frontend/scripts/stage-sherpa-runtime.ps1 -TauriRoot frontend/src-tauri -Runtime directml
rustc --version
cargo check -p clawscribe --locked --features windows-gpu
$runtime = (Resolve-Path 'frontend/src-tauri/binaries/sherpa-onnx').Path
$env:PATH = "$runtime;$env:PATH"
$build = & cargo test -p clawscribe --lib --release --locked --features windows-gpu --no-run --message-format=json-render-diagnostics
$executables = @($build | ForEach-Object {
    if ($_ -match '^\s*\{') {
        $item = $_ | ConvertFrom-Json
        if ($item.reason -eq 'compiler-artifact' -and $item.profile.test -and $item.executable) { $item.executable }
    }
} | Select-Object -Unique)
if ($executables.Count -ne 1) { throw 'Expected one library test executable.' }
$testDirectory = Split-Path -Parent $executables[0]
Get-ChildItem -LiteralPath $runtime -Filter '*.dll' -File | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $testDirectory $_.Name) -Force
}
$ffmpeg = 'frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe'
if (Test-Path $ffmpeg) { Copy-Item $ffmpeg (Join-Path $testDirectory 'ffmpeg.exe') -Force }
if ($MutationAcceptance) {
    & $executables[0] knowledge::store::tests::synthetic_fts_mutation_workload --ignored --exact --test-threads=1 --nocapture
}
$focusedFailures = 0
$PSNativeCommandUseErrorActionPreference = $false
foreach ($filter in @('knowledge::', 'summary::llm_client::response_tests::',
    'summary::openai_provider::tests::', 'summary::codex_provider::app_server_tests::',
    'summary::summary_engine::sidecar::protocol_tests::',
    'library::backup::tests::conversation_',
    'database::manager::tests::gapped_canonical_rows_keep_fts_alignment_through_snapshot_and_import')) {
    & $executables[0] $filter --test-threads=1 --show-output
    if ($LASTEXITCODE -ne 0) { $focusedFailures++ }
}
$PSNativeCommandUseErrorActionPreference = $true
if ($focusedFailures -ne 0) { throw "$focusedFailures focused native suites failed." }

if ($FullSuite) { & $executables[0] --test-threads=1 }
if ($RetrievalAcceptance) {
    $cache = Join-Path $env:RUNNER_TEMP 'clawscribe-knowledge-acceptance'
    New-Item -ItemType Directory -Force $cache | Out-Null
    $env:CLAWSCRIBE_KNOWLEDGE_MODEL = Join-Path $cache 'onnx'
    & $executables[0] knowledge::retrieval::tests::synthetic_retrieval_workload --ignored --exact --test-threads=1 --nocapture
}
if ($Acceptance) {
    $cache = Join-Path $env:RUNNER_TEMP 'clawscribe-knowledge-acceptance'
    New-Item -ItemType Directory -Force $cache | Out-Null
    $venv = Join-Path $cache 'reference-venv'
    $python = Join-Path $venv 'Scripts/python.exe'
    if (-not (Test-Path $python)) { python -m venv $venv }
    & $python -m pip install --disable-pip-version-check --index-url https://download.pytorch.org/whl/cpu torch==2.6.0
    & $python -m pip install --disable-pip-version-check transformers==4.51.3 tokenizers==0.21.4 numpy==2.2.6 safetensors==0.5.3 huggingface-hub==0.30.2
    $reference = Join-Path $cache 'reference.json'
    & $python frontend/scripts/knowledge-reference.py (Join-Path $cache 'pytorch') $reference
    $env:CLAWSCRIBE_KNOWLEDGE_MODEL = Join-Path $cache 'onnx'
    $env:CLAWSCRIBE_KNOWLEDGE_REFERENCE = $reference
    & $executables[0] knowledge::embedding::acceptance::pinned_onnx_reference_and_resource_gate --ignored --exact --test-threads=1 --nocapture
}
