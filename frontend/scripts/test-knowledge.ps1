param([switch]$Acceptance, [bool]$FullSuite = $true, [switch]$RetrievalAcceptance, [switch]$MutationAcceptance, [switch]$AnswerAcceptance)
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
if ($AnswerAcceptance) {
    if ([string]::IsNullOrWhiteSpace($env:CLAWSCRIBE_VALIDATION_ROOT)) { throw 'An explicit isolated validation root is required.' }
    $validationRoot = [System.IO.Path]::GetFullPath($env:CLAWSCRIBE_VALIDATION_ROOT)
    $rootVolume = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($validationRoot))
    if (-not $rootVolume.IsReady -or $rootVolume.DriveType -ne [IO.DriveType]::Fixed -or $rootVolume.AvailableFreeSpace -lt 4GB) { throw 'Isolated validation requires a fixed local volume with at least 4 GiB free.' }
    if (-not (Test-Path -LiteralPath $validationRoot)) {
        $rootParent = [IO.Directory]::GetParent($validationRoot)
        if ($null -eq $rootParent -or -not (Test-Path -LiteralPath $rootParent.FullName -PathType Container)) { throw 'The explicitly configured validation root requires an existing local parent.' }
        for ($ancestor = Get-Item -LiteralPath $rootParent.FullName; $null -ne $ancestor; $ancestor = $ancestor.Parent) {
            if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Validation root parent cannot traverse a reparse point.' }
        }
        New-Item -ItemType Directory -Path $validationRoot | Out-Null
    }
    if (-not (Test-Path -LiteralPath $validationRoot -PathType Container)) { throw 'Invalid isolated validation root.' }
    $validationItem = Get-Item -LiteralPath $validationRoot
    for ($ancestor = $validationItem; $null -ne $ancestor; $ancestor = $ancestor.Parent) {
        if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Validation root cannot traverse a reparse point.' }
    }
    $qaRoot = [System.IO.Path]::GetFullPath((Join-Path $validationRoot 'clawscribe-answer-qa'))
    if ([IO.Path]::GetDirectoryName($qaRoot) -ine $validationRoot.TrimEnd('\')) { throw 'Invalid isolated profile boundary.' }
    $volume = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($qaRoot))
    if (-not $volume.IsReady -or $volume.DriveType -ne [IO.DriveType]::Fixed -or $volume.AvailableFreeSpace -lt 4GB) { throw 'A fixed local volume with at least 4 GiB free is required for isolated answer validation.' }
    if (Test-Path -LiteralPath $qaRoot) {
        if ((Get-Item -LiteralPath $qaRoot).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Synthetic profile cannot be a reparse point.' }
        if (Get-ChildItem -LiteralPath $qaRoot -Force -Recurse -Attributes ReparsePoint | Select-Object -First 1) { throw 'Synthetic profile contains a reparse point.' }
    }
    New-Item -ItemType Directory -Force -Path $qaRoot | Out-Null
    $probe = Join-Path $qaRoot ([guid]::NewGuid().ToString() + '.probe')
    [IO.File]::WriteAllText($probe, 'isolated validation write probe')
    Remove-Item -LiteralPath $probe
    $env:CLAWSCRIBE_VALIDATION_ROOT = $validationRoot.TrimEnd('\')
    $env:CLAWSCRIBE_ANSWER_QA_ROOT = $qaRoot
    cargo build -p llama-helper --release --locked --target x86_64-pc-windows-msvc
    $helperSource = 'target/x86_64-pc-windows-msvc/release/llama-helper.exe'
    $helperDestination = Join-Path $testDirectory 'llama-helper.exe'
    Copy-Item -LiteralPath $helperSource -Destination $helperDestination -Force
    if ((Get-FileHash -LiteralPath $helperSource -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $helperDestination -Algorithm SHA256).Hash) { throw 'Packaged helper copy verification failed.' }
    $env:CLAWSCRIBE_ANSWER_QA_HELPER_SHA256 = (Get-FileHash -LiteralPath $helperDestination -Algorithm SHA256).Hash.ToLowerInvariant()
    $env:CLAWSCRIBE_ANSWER_QA_BUILD_SHA = (& git rev-parse HEAD).Trim()
    $PSNativeCommandUseErrorActionPreference = $false
    # Native helper stderr includes local model paths. Emit only this test's
    # explicitly sanitized outcomes; detailed invented text goes to step summary.
    & $executables[0] knowledge::answers::tests::fixed_actual_answer_acceptance --ignored --exact --test-threads=1 --nocapture 2>&1 | ForEach-Object {
        $line = $_.ToString()
        if ($line -match '(answer_qa case=\d{2} runtime=(completed|failed)( automated_gate=(true|false))?)$') { Write-Output $Matches[1] }
        elseif ($line -match '^test result:') { Write-Output $line }
    }
    $qaExit = $LASTEXITCODE
    $PSNativeCommandUseErrorActionPreference = $true
    if ($qaExit -ne 0) { throw 'Actual answer acceptance failed; inspect the bounded public synthetic evaluation summary.' }
}
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
