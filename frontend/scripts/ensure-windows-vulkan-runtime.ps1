#requires -Version 7.0
param(
    # Installing prerequisites is opt-in. Native test discovery only probes.
    [switch]$InstallFromSdk
)

$ErrorActionPreference = 'Stop'
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { throw 'The Vulkan runtime preflight must run on Windows.' }
if (-not [Environment]::Is64BitProcess) { throw 'Use x64 PowerShell for the Windows x64 Vulkan runtime.' }

function Test-VulkanLoader {
    $handle = [IntPtr]::Zero
    if (-not [System.Runtime.InteropServices.NativeLibrary]::TryLoad('vulkan-1.dll', [ref]$handle)) {
        return $false
    }
    try {
        # Check an actual loader export, without creating a GPU instance.
        $null = [System.Runtime.InteropServices.NativeLibrary]::GetExport($handle, 'vkGetInstanceProcAddr')
        return $true
    } finally {
        [System.Runtime.InteropServices.NativeLibrary]::Free($handle)
    }
}

if (Test-VulkanLoader) {
    Write-Host 'Vulkan loader is loadable; no runtime installation needed.'
    return
}
if (-not $InstallFromSdk) {
    throw "Cannot load vulkan-1.dll. Extracting the Vulkan SDK supplies build tools, not the Windows runtime. Install the official Vulkan runtime/GPU driver, or explicitly run ensure-windows-vulkan-runtime.ps1 -InstallFromSdk on the build machine after SDK staging."
}
if (-not $env:VULKAN_SDK) { throw 'VULKAN_SDK must identify the staged SDK before runtime installation.' }

# The pinned Windows SDK carries its signed runtime installer here. Do not
# fetch an arbitrary DLL, install a graphics driver, or bypass signature errors.
$installer = Join-Path $env:VULKAN_SDK 'Helpers/VulkanRT.exe'
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "The staged Vulkan SDK has no runtime installer at '$installer'."
}
$signature = Get-AuthenticodeSignature -LiteralPath $installer
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(?i)LunarG') {
    throw "Refusing the Vulkan runtime installer: expected a valid LunarG signature, got '$($signature.Status)'."
}
Write-Host "Installing the SDK's signed Vulkan runtime: $installer"
Write-Host "Runtime installer SHA-256: $((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash)"
$process = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru -NoNewWindow
if ($process.ExitCode -ne 0) { throw "Vulkan runtime installation failed with exit code $($process.ExitCode)." }
if (-not (Test-VulkanLoader)) { throw 'The Vulkan installer completed, but vulkan-1.dll still cannot be loaded.' }
Write-Host 'Vulkan loader startup verified. This is not a GPU-inference or live-capture test.'
