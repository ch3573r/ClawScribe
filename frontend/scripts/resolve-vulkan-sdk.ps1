<#
.SYNOPSIS
  Finds the pinned Vulkan SDK by its content and makes it the one later steps use.

.DESCRIPTION
  Read-only: it inspects candidate folders (per-version tool folders, VULKAN_SDK
  and the default LunarG folder) and accepts the first one whose headers, library
  and tools are the pinned version, whatever the folder is called. Every
  candidate and the reason it was accepted or rejected is printed. On success it
  exports VULKAN_SDK, Vulkan_LIBRARY, Vulkan_INCLUDE_DIR and the Bin folder to
  later workflow steps and sets the step outputs found=true and path. Nothing is
  installed, moved or deleted.

  With -AllowMissing a missing SDK sets found=false and succeeds, so that an
  explicit provisioning step can stage it; without it a missing SDK fails.

.PARAMETER Version
  Pinned SDK version (four parts). Same pin as the release workflow.

.PARAMETER AllowMissing
  Report a missing SDK in the step output instead of failing.
#>
param(
    [string]$Version = "1.4.309.0",
    [switch]$AllowMissing
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "vulkan-sdk-common.ps1")

$result = Find-VulkanSdk -Version $Version
Write-VulkanSdkCandidates -Checked $result.Checked
if ($result.Match) {
    Write-Host "Vulkan SDK $Version found by content."
    Export-VulkanSdk -Path $result.Match.Path
    if ($env:GITHUB_OUTPUT) {
        "found=true" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
        "path=$($result.Match.Path)" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
    }
    return
}
if ($env:GITHUB_OUTPUT) { "found=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append }
if ($AllowMissing) {
    Write-Host "Vulkan SDK $Version is not provisioned; it will be staged in its own folder."
    return
}
throw "Vulkan SDK $Version is not provisioned on this runner: no candidate folder holds its headers, library and tools. Install it in a per-version tool folder, or run a manual provisioning workflow."
