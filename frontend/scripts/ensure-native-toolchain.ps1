<#
.SYNOPSIS
  Verifies the pinned native build toolchain on the designated build machine.

.DESCRIPTION
  The persistent runner keeps its toolchain between runs, so a pull-request job
  must use what is provisioned and must not install, replace or downgrade global
  software. This script checks, without changing anything by default:

    Rust       the channel pinned in rust-toolchain.toml with the Windows MSVC target
    LLVM       libclang of the pinned version (bindgen 0.69 mis-parses whisper
               structs with clang 21 and newer)
    Vulkan SDK the pinned SDK, recognised by its headers, library and tools
               in any folder (see vulkan-sdk-common.ps1)
    Vulkan     a loadable vulkan-1.dll (the SDK supplies build tools, not the
    runtime    Windows runtime)

  Everything that is found is exported to later workflow steps (LIBCLANG_PATH,
  the Vulkan variables and PATH, through GITHUB_ENV and GITHUB_PATH). Anything
  missing or of another version is listed together with how to provision it, and
  the script fails.

  -Provision is for an explicit, owner-started manual run only. It installs the
  missing Rust toolchain with rustup and a missing or different LLVM with
  Chocolatey (a different LLVM version is replaced only here, never silently).
  A missing Vulkan SDK is then staged by the workflow itself (the vulkan-sdk
  action, into its own per-version folder), so -Provision also needs
  -AllowMissingVulkanSdk, which reports it in the step output instead of failing.

.PARAMETER Provision
  Install what is missing. Never use from a pull-request or push trigger.

.PARAMETER AllowMissingVulkanSdk
  Report a missing Vulkan SDK in the step output instead of failing, so that a
  provisioning run can install it in a later step.

.PARAMETER LlvmVersion
  Pinned LLVM version. Same pin as the release workflow.

.PARAMETER VulkanSdkVersion
  Pinned Vulkan SDK version. Same pin as the release workflow.
#>
param(
    [switch]$Provision,
    [switch]$AllowMissingVulkanSdk,
    [string]$LlvmVersion = "20.1.8",
    [string]$VulkanSdkVersion = "1.4.309.0"
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { throw "The native toolchain check must run on Windows." }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$problems = [System.Collections.Generic.List[string]]::new()
$pathAdditions = [System.Collections.Generic.List[string]]::new()
$exports = [ordered]@{}
$vulkanSdkMissing = $false

function Add-Problem {
    param([string]$Text)
    Write-Host "MISSING $Text"
    $problems.Add($Text)
}

# ---- Rust ------------------------------------------------------------------------
$toolchainFile = Get-Content -LiteralPath (Join-Path $repoRoot "rust-toolchain.toml") -Raw
if ($toolchainFile -notmatch '(?m)^\s*channel\s*=\s*"([^"]+)"') { throw "rust-toolchain.toml pins no channel." }
$channel = $Matches[1]
$target = "x86_64-pc-windows-msvc"

function Test-RustToolchain {
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { return $false }
    $installed = @(& rustup toolchain list 2>$null)
    if (-not ($installed | Where-Object { $_ -match "^$([regex]::Escape($channel))-$([regex]::Escape($target))" })) { return $false }
    $targets = @(& rustup target list --toolchain $channel --installed 2>$null)
    return ($targets -contains $target)
}

if (-not (Test-RustToolchain)) {
    if ($Provision -and (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Rust $channel with the $target target (explicit provisioning)"
        & rustup toolchain install $channel --profile minimal --target $target
        if ($LASTEXITCODE -ne 0) { throw "rustup could not install Rust $channel." }
    }
    if (-not (Test-RustToolchain)) {
        Add-Problem "Rust $channel with target $target (install rustup, then run: rustup toolchain install $channel --profile minimal --target $target)"
    }
}
if (Test-RustToolchain) {
    Push-Location $repoRoot
    try { $rustc = (& rustc --version) -join " " } finally { Pop-Location }
    if ($rustc -notmatch "^rustc $([regex]::Escape($channel))[ .]") { Add-Problem "Rust $channel is not the active compiler for this checkout" }
    else { Write-Host "Rust $channel ready" }
}

# ---- LLVM ------------------------------------------------------------------------
function Find-Llvm {
    $candidates = @()
    if ($env:LIBCLANG_PATH) { $candidates += $env:LIBCLANG_PATH }
    $candidates += (Join-Path $env:ProgramFiles "LLVM/bin")
    foreach ($folder in $candidates) {
        $libclang = Join-Path $folder "libclang.dll"
        if (-not (Test-Path -LiteralPath $libclang -PathType Leaf)) { continue }
        $info = (Get-Item -LiteralPath $libclang).VersionInfo
        $versions = @($info.FileVersion, $info.ProductVersion)
        $clang = Join-Path $folder "clang.exe"
        if (Test-Path -LiteralPath $clang -PathType Leaf) { $versions += ((& $clang --version 2>$null) -join " ") }
        $pinned = [bool]($versions | Where-Object { $_ -match "(^|[^0-9.])$([regex]::Escape($LlvmVersion))([^0-9]|$)" })
        return [pscustomobject]@{ Folder = $folder; Pinned = $pinned; Found = (($versions | Where-Object { $_ }) | Select-Object -First 1) }
    }
    return $null
}

$llvm = Find-Llvm
if ($Provision -and ($null -eq $llvm -or -not $llvm.Pinned)) {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) { throw "Chocolatey is required to provision LLVM." }
    Write-Host "Installing LLVM $LlvmVersion (explicit provisioning)"
    # --allow-downgrade replaces a newer LLVM, which is why this runs only here.
    & choco install llvm --version=$LlvmVersion -y --no-progress --allow-downgrade
    if ($LASTEXITCODE -ne 0) { throw "Chocolatey could not install LLVM $LlvmVersion." }
    $llvm = Find-Llvm
}
if ($null -eq $llvm) {
    Add-Problem "LLVM $LlvmVersion (no libclang.dll found; provision the runner or run the workflow manually with provision-toolchain)"
} elseif (-not $llvm.Pinned) {
    Add-Problem "LLVM $LlvmVersion (found $($llvm.Found); a different LLVM is never replaced from a pull request or push)"
} else {
    Write-Host "LLVM $LlvmVersion ready"
    $exports["LIBCLANG_PATH"] = $llvm.Folder
    $pathAdditions.Add($llvm.Folder)
}

# ---- Vulkan SDK ------------------------------------------------------------------
# Identified by content (headers, library, tools), not by the folder's name; see
# vulkan-sdk-common.ps1. Staging a missing SDK is the vulkan-sdk composite action's job.
. (Join-Path $PSScriptRoot "vulkan-sdk-common.ps1")
$sdkSearch = Find-VulkanSdk -Version $VulkanSdkVersion
Write-VulkanSdkCandidates -Checked $sdkSearch.Checked
if ($sdkSearch.Match) {
    $sdk = $sdkSearch.Match.Path
    Write-Host "Vulkan SDK $VulkanSdkVersion ready"
    $exports["VULKAN_SDK"] = $sdk
    $exports["Vulkan_LIBRARY"] = ConvertTo-FullVulkanPath (Join-Path $sdk "Lib/vulkan-1.lib")
    $exports["Vulkan_INCLUDE_DIR"] = ConvertTo-FullVulkanPath (Join-Path $sdk "Include")
    $pathAdditions.Add((ConvertTo-FullVulkanPath (Join-Path $sdk "Bin")))
} elseif ($AllowMissingVulkanSdk) {
    $vulkanSdkMissing = $true
    Write-Host "Vulkan SDK $VulkanSdkVersion is not provisioned; the workflow stages it in its own folder in a later step."
} else {
    Add-Problem "Vulkan SDK $VulkanSdkVersion (no candidate holds its headers, library and tools; keep it in a per-version tool folder, or run the workflow manually with provision-toolchain)"
}

# ---- results -------------------------------------------------------------------------
if ($problems.Count -gt 0) {
    throw "The pinned native toolchain is not provisioned on this runner: $($problems -join '; ')."
}
if ($env:GITHUB_ENV) {
    foreach ($name in $exports.Keys) { "$name=$($exports[$name])" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
}
if ($env:GITHUB_PATH) {
    foreach ($folder in $pathAdditions) { $folder | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append }
}
if ($env:GITHUB_OUTPUT) {
    "vulkan_sdk=$(if ($vulkanSdkMissing) { 'missing' } else { 'ready' })" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
}
Write-Host "Pinned native toolchain verified."
