param([switch]$Acceptance)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ([string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER) -or
    $env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER -or
    $env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER) { throw 'Designated build runner required.' }
. (Join-Path $PSScriptRoot 'configure-windows-portability.ps1')
$env:LIBCLANG_PATH = Join-Path $env:ProgramFiles 'LLVM/bin'
if (-not $env:VULKAN_SDK) {
    $sdk = Get-ChildItem 'C:/VulkanSDK' -Directory | Where-Object Name -eq '1.4.309.0' | Select-Object -First 1
    if ($sdk) { $env:VULKAN_SDK = $sdk.FullName }
}
./frontend/scripts/ensure-windows-vulkan-runtime.ps1
node scripts/verify-public-repo-safety.mjs
./frontend/scripts/stage-sherpa-runtime.ps1 -TauriRoot frontend/src-tauri -Runtime directml
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
& $executables[0] knowledge:: --test-threads=1
