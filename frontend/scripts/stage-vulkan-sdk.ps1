<#
.SYNOPSIS
  Stages the pinned Vulkan SDK in its own per-version folder, never over another SDK.

.DESCRIPTION
  The SDK extraction action unpacks into whatever VULKAN_SDK already names, which
  on a persistent runner is an unrelated system SDK. This script brackets that
  action so it can only write to a fresh folder that belongs to the build tooling:

    Prepare  Refuses to continue if the per-version folder exists with other
             content. Records the inherited VULKAN_SDK and a fingerprint of its
             identifying files, creates a new staging folder next to the final
             one and points VULKAN_SDK (and VULKAN_SDK_VERSION, which the action
             prefers over its own input) at it for the following steps.
    Promote  Requires that the action really used the staging folder and that
             its content is the pinned SDK (headers, library, tools), that the
             inherited SDK is unchanged, then renames the staging folder to the
             per-version folder and exports it. An existing matching folder
             (a concurrent run) is reused. Nothing is ever deleted except the
             staging folder.
    Clean    Removes a staging folder that is still there, after checking its
             name, its parent and that it holds no link. Warns instead of failing.

  The per-version folder is <toolchain root>/vulkan-sdk/<version>, where the
  toolchain root is CLAWSCRIBE_TOOLCHAIN_ROOT, else the runner tool cache, else
  ProgramData. Later runs find it by content like any other candidate.

.PARAMETER Phase
  Prepare, Promote or Clean.

.PARAMETER Version
  Pinned SDK version (four parts).
#>
param(
    [Parameter(Mandatory = $true)][ValidateSet("Prepare", "Promote", "Clean")][string]$Phase,
    [string]$Version = "1.4.309.0"
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "vulkan-sdk-common.ps1")

if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw "The SDK version must have four numeric parts." }
$stagingPattern = '^\.staging-[0-9a-f]{32}$'

function Set-StepEnvironment {
    param([string]$Name, [string]$Value)
    Set-Item -LiteralPath "Env:$Name" -Value $Value
    if ($env:GITHUB_ENV) { "$Name=$Value" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
}

# True when the inner path is the outer path or lies inside it.
function Test-PathInside {
    param([string]$Inner, [string]$Outer)
    if (-not $Inner -or -not $Outer) { return $false }
    $a = (ConvertTo-FullVulkanPath $Inner) + [System.IO.Path]::DirectorySeparatorChar
    $b = (ConvertTo-FullVulkanPath $Outer) + [System.IO.Path]::DirectorySeparatorChar
    return $a.StartsWith($b, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoLinkInPath {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    while ($item) {
        if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw "The SDK staging path passes through a link." }
        $item = $item.Parent
    }
}

$parent = Get-VulkanSdkStagingParent
$final = Join-Path $parent $Version

switch ($Phase) {
    "Prepare" {
        if ($parent -match '\s') { throw "The SDK staging path must not contain spaces (the extraction action does not quote it)." }
        if ([System.IO.Path]::GetPathRoot($parent) -notmatch '^[A-Za-z]:\\$') { throw "The SDK staging folder must be on a local drive with a drive letter." }
        $inherited = $env:VULKAN_SDK
        if ($inherited -and (Test-PathInside -Inner $parent -Outer $inherited)) { throw "The staging folder lies inside the inherited VULKAN_SDK folder." }
        if (Test-Path -LiteralPath $final) {
            $existing = Test-VulkanSdk -Path $final -Version $Version
            if (-not $existing.Matches) { throw "A folder already exists at the pinned per-version SDK path but is not the pinned SDK ($($existing.Reason)). Remove it manually; it is never overwritten or deleted automatically." }
            Write-Host "The pinned SDK is already in its per-version folder; nothing to stage."
            Set-StepEnvironment "CLAWSCRIBE_VULKAN_STAGING" ""
            return
        }
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
        Assert-NoLinkInPath $parent
        $staging = Join-Path $parent (".staging-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $staging | Out-Null
        Write-Host "Inherited VULKAN_SDK is left untouched: $(if ($inherited) { $inherited } else { '(not set)' })"
        Set-StepEnvironment "CLAWSCRIBE_INHERITED_VULKAN_SDK" $(if ($inherited) { $inherited } else { "" })
        Set-StepEnvironment "CLAWSCRIBE_INHERITED_VULKAN_FINGERPRINT" (Get-VulkanSdkFingerprint $inherited)
        Set-StepEnvironment "CLAWSCRIBE_VULKAN_STAGING" $staging
        # The action unpacks into VULKAN_SDK and reads VULKAN_SDK_VERSION before its input.
        Set-StepEnvironment "VULKAN_SDK" ($staging.Replace([char]92, [char]47))
        Set-StepEnvironment "VULKAN_SDK_VERSION" $Version
        Write-Host "SDK staging folder prepared next to the per-version folder."
    }
    "Promote" {
        $staging = $env:CLAWSCRIBE_VULKAN_STAGING
        if (-not $staging) {
            # Prepare found the pinned SDK in place.
            $existing = Test-VulkanSdk -Path $final -Version $Version
            if (-not $existing.Matches) { throw "No staged SDK and no pinned SDK in its per-version folder ($($existing.Reason))." }
            Export-VulkanSdk -Path $final
            return
        }
        $stagingFull = ConvertTo-FullVulkanPath $staging
        if ((Split-Path -Leaf $stagingFull) -notmatch $stagingPattern -or (Split-Path -Parent $stagingFull) -ine $parent) { throw "The staging folder is not one this tooling created." }
        if ((ConvertTo-FullVulkanPath $env:VULKAN_SDK) -ine $stagingFull) {
            throw "The SDK installer did not use the isolated staging folder, so an unrelated SDK may have been written. Stop and inspect the runner."
        }
        $staged = Test-VulkanSdk -Path $stagingFull -Version $Version
        if (-not $staged.Matches) { throw "The staged SDK is not the pinned SDK $Version ($($staged.Reason))." }
        $before = $env:CLAWSCRIBE_INHERITED_VULKAN_FINGERPRINT
        $inheritedPath = $env:CLAWSCRIBE_INHERITED_VULKAN_SDK
        if ($before -and (Get-VulkanSdkFingerprint $inheritedPath) -ne $before) { throw "The inherited SDK changed while the pinned SDK was staged. Stop and inspect the runner." }
        if (Test-Path -LiteralPath $final) {
            $existing = Test-VulkanSdk -Path $final -Version $Version
            if (-not $existing.Matches) { throw "A folder appeared at the pinned per-version SDK path with other content ($($existing.Reason))." }
            Write-Host "A concurrent run already provided the pinned SDK; it is reused."
        } else {
            Move-Item -LiteralPath $stagingFull -Destination $final
            $promoted = Test-VulkanSdk -Path $final -Version $Version
            if (-not $promoted.Matches) { throw "The SDK did not verify after it was moved into place ($($promoted.Reason))." }
            Write-Host "The pinned SDK $Version is now in its per-version folder."
        }
        Set-StepEnvironment "CLAWSCRIBE_VULKAN_STAGING" ""
        Export-VulkanSdk -Path $final
    }
    "Clean" {
        $staging = $env:CLAWSCRIBE_VULKAN_STAGING
        if (-not $staging -or -not (Test-Path -LiteralPath $staging)) { return }
        try {
            $stagingFull = ConvertTo-FullVulkanPath $staging
            if ((Split-Path -Leaf $stagingFull) -notmatch $stagingPattern -or (Split-Path -Parent $stagingFull) -ine $parent) { throw "not an owned staging folder" }
            Assert-NoLinkInPath $stagingFull
            if (Get-ChildItem -LiteralPath $stagingFull -Recurse -Force -Attributes ReparsePoint -ErrorAction Stop | Select-Object -First 1) { throw "contains a link" }
            Remove-Item -LiteralPath $stagingFull -Recurse -Force
            Write-Host "Removed the unused SDK staging folder."
        } catch {
            Write-Warning "The SDK staging folder could not be removed ($($_.Exception.Message)); remove it manually."
        }
    }
}
