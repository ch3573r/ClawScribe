<#
.SYNOPSIS
  Shared Vulkan SDK detection for the build scripts. Dot-source it; it defines
  functions only and changes nothing when loaded.

.DESCRIPTION
  An SDK is identified by what it contains, never by the name of the folder that
  holds it. A folder is the pinned SDK when

    headers   Include/vulkan/vulkan_core.h defines the pinned major, minor and
              patch (VK_HEADER_VERSION_COMPLETE and VK_HEADER_VERSION)
    library   Lib/vulkan-1.lib exists and is not empty
    tools     Bin exists, and if Bin/vulkaninfoSDK.exe is present its file version
              equals the pinned four-part version

  A matching SDK under any folder name is accepted; a wrong one is rejected even
  inside a folder that carries the pinned name. Persistent per-version folders
  that the build tooling itself maintains are searched along with VULKAN_SDK and
  the default SDK folder (C:\VulkanSDK, or CLAWSCRIBE_VULKAN_SDK_ROOT).
#>

function ConvertTo-FullVulkanPath {
    param([string]$Path)
    return [System.IO.Path]::GetFullPath($Path).TrimEnd([char]92, [char]47)
}

function Get-VulkanSdkEvidence {
    param([Parameter(Mandatory = $true)][string]$Path)
    $evidence = [ordered]@{ HeaderVersion = $null; HasLibrary = $false; HasBin = $false; ToolPresent = $false; ToolVersion = $null }
    $header = Join-Path $Path "Include/vulkan/vulkan_core.h"
    if (Test-Path -LiteralPath $header -PathType Leaf) {
        $patch = $null; $major = $null; $minor = $null
        $seen = 0
        # Closed explicitly: an open handle would make the later folder rename fail.
        $reader = [System.IO.StreamReader]::new($header)
        try {
            while ($null -ne ($line = $reader.ReadLine())) {
                if (++$seen -gt 5000) { break }
                if ($null -eq $patch -and $line -match '^\s*#define\s+VK_HEADER_VERSION\s+(\d+)\s*$') { $patch = $Matches[1] }
                elseif ($null -eq $major -and $line -match '^\s*#define\s+VK_HEADER_VERSION_COMPLETE\s+VK_MAKE_API_VERSION\(\s*\d+\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*VK_HEADER_VERSION\s*\)') {
                    $major = $Matches[1]; $minor = $Matches[2]
                }
                if ($null -ne $patch -and $null -ne $major) { break }
            }
        } finally { $reader.Dispose() }
        if ($null -ne $patch -and $null -ne $major) { $evidence.HeaderVersion = "$major.$minor.$patch" }
    }
    $library = Join-Path $Path "Lib/vulkan-1.lib"
    $evidence.HasLibrary = (Test-Path -LiteralPath $library -PathType Leaf) -and ((Get-Item -LiteralPath $library).Length -gt 0)
    $evidence.HasBin = Test-Path -LiteralPath (Join-Path $Path "Bin") -PathType Container
    $tool = Join-Path $Path "Bin/vulkaninfoSDK.exe"
    if (Test-Path -LiteralPath $tool -PathType Leaf) {
        $evidence.ToolPresent = $true
        $parsed = $null
        # File versions can carry trailing text ("10.0.1 (WinBuild...)"); only the leading numbers count.
        $fileVersion = [string](Get-Item -LiteralPath $tool).VersionInfo.FileVersion
        if ($fileVersion -match '^\s*(\d+(?:\.\d+){1,3})' -and [version]::TryParse($Matches[1], [ref]$parsed)) { $evidence.ToolVersion = $parsed }
    }
    return [pscustomobject]$evidence
}

# Result: Matches, Reason (empty when it matches) and the evidence that was found.
function Test-VulkanSdk {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Version)
    $pin = [version]$Version
    $short = "$($pin.Major).$($pin.Minor).$($pin.Build)"
    $result = [ordered]@{ Path = $Path; Matches = $false; Reason = ""; HeaderVersion = $null; ToolVersion = $null }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { $result.Reason = "not a folder"; return [pscustomobject]$result }
    $evidence = Get-VulkanSdkEvidence -Path $Path
    $result.HeaderVersion = $evidence.HeaderVersion
    $result.ToolVersion = $evidence.ToolVersion
    if (-not $evidence.HeaderVersion) { $result.Reason = "no readable Vulkan headers" }
    elseif ($evidence.HeaderVersion -ne $short) { $result.Reason = "headers are $($evidence.HeaderVersion), not $short" }
    elseif (-not $evidence.HasLibrary) { $result.Reason = "no vulkan-1.lib" }
    elseif (-not $evidence.HasBin) { $result.Reason = "no Bin folder" }
    elseif ($evidence.ToolVersion -and $evidence.ToolVersion -ne $pin) { $result.Reason = "tool version is $($evidence.ToolVersion), not $Version" }
    else { $result.Matches = $true }
    return [pscustomobject]$result
}

# Folders under which the build tooling keeps per-version tools, in search order.
function Get-ToolchainRoots {
    $roots = @()
    if ($env:CLAWSCRIBE_TOOLCHAIN_ROOT) { $roots += $env:CLAWSCRIBE_TOOLCHAIN_ROOT }
    if ($env:RUNNER_TOOL_CACHE) { $roots += (Join-Path $env:RUNNER_TOOL_CACHE "clawscribe") }
    if ($env:ProgramData) { $roots += (Join-Path $env:ProgramData "ClawScribe/toolchain") }
    return @($roots | ForEach-Object { ConvertTo-FullVulkanPath $_ } | Select-Object -Unique)
}

# The one place where staging puts an SDK that is not provisioned yet.
function Get-VulkanSdkStagingParent {
    $roots = Get-ToolchainRoots
    if ($roots.Count -eq 0) { throw "No toolchain folder is available: set CLAWSCRIBE_TOOLCHAIN_ROOT." }
    return (Join-Path $roots[0] "vulkan-sdk")
}

function Get-VulkanSdkCandidates {
    param([Parameter(Mandatory = $true)][string]$Version)
    $found = [System.Collections.Generic.List[object]]::new()
    $add = {
        param($Label, $Path)
        if (-not $Path) { return }
        $full = ConvertTo-FullVulkanPath $Path
        if (-not ($found | Where-Object { $_.Path -ieq $full })) { $found.Add([pscustomobject]@{ Label = $Label; Path = $full }) }
    }
    foreach ($root in (Get-ToolchainRoots)) { & $add "per-version tool folder" (Join-Path $root "vulkan-sdk/$Version") }
    & $add "VULKAN_SDK (process)" $env:VULKAN_SDK
    & $add "VULKAN_SDK (machine)" ([Environment]::GetEnvironmentVariable("VULKAN_SDK", "Machine"))
    & $add "VULKAN_SDK (user)" ([Environment]::GetEnvironmentVariable("VULKAN_SDK", "User"))
    # LunarG's default install folder, or the folder named by CLAWSCRIBE_VULKAN_SDK_ROOT.
    $sdkRoot = if ($env:CLAWSCRIBE_VULKAN_SDK_ROOT) { $env:CLAWSCRIBE_VULKAN_SDK_ROOT } elseif ($env:SystemDrive) { Join-Path "$($env:SystemDrive)\" "VulkanSDK" } else { $null }
    if ($sdkRoot) { & $add "default SDK folder" (Join-Path $sdkRoot $Version) }
    return $found
}

# Result: Match (the first candidate that is the pinned SDK, or $null) and Checked.
function Find-VulkanSdk {
    param([Parameter(Mandatory = $true)][string]$Version)
    $checked = @()
    foreach ($candidate in (Get-VulkanSdkCandidates -Version $Version)) {
        $test = Test-VulkanSdk -Path $candidate.Path -Version $Version
        $checked += [pscustomobject]@{ Label = $candidate.Label; Path = $candidate.Path; Matches = $test.Matches; Reason = $test.Reason }
        if ($test.Matches) { return [pscustomobject]@{ Match = $checked[-1]; Checked = $checked } }
    }
    return [pscustomobject]@{ Match = $null; Checked = $checked }
}

function Write-VulkanSdkCandidates {
    param($Checked)
    $lines = @()
    foreach ($entry in $Checked) {
        $verdict = if ($entry.Matches) { "ACCEPT" } else { "REJECT" }
        $why = if ($entry.Matches) { "" } else { " ($($entry.Reason))" }
        $lines += "$verdict Vulkan SDK candidate [$($entry.Label)]$why"
    }
    if ($lines.Count -eq 0) { $lines = @("No Vulkan SDK candidate folder exists.") }
    $lines | ForEach-Object { Write-Host $_ }
    # The same verdicts go to the job summary, so the evidence outlives the log.
    if ($env:GITHUB_STEP_SUMMARY) {
        (@("", "### Vulkan SDK candidates", "") + ($lines | ForEach-Object { "- ``$_``" })) | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
    }
}

# A cheap fingerprint of an SDK's identifying files, to show that a folder was not rewritten.
function Get-VulkanSdkFingerprint {
    param([string]$Path)
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Container)) { return "absent" }
    $parts = @()
    foreach ($relative in @("Include/vulkan/vulkan_core.h", "Lib/vulkan-1.lib", "Bin/vulkaninfoSDK.exe")) {
        $file = Join-Path $Path $relative
        if (Test-Path -LiteralPath $file -PathType Leaf) {
            $item = Get-Item -LiteralPath $file
            $hash = if ($item.Length -lt 4MB) { (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash } else { "large" }
            $parts += "$relative|$($item.Length)|$($item.LastWriteTimeUtc.Ticks)|$hash"
        } else { $parts += "$relative|none" }
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($parts -join "`n"))
    return [System.BitConverter]::ToString([System.Security.Cryptography.SHA256]::HashData($bytes)).Replace("-", "").ToLowerInvariant()
}

# Makes a verified SDK the one later steps use (GITHUB_ENV, GITHUB_PATH and the
# step outputs; the current process too).
function Export-VulkanSdk {
    param([Parameter(Mandatory = $true)][string]$Path)
    $values = [ordered]@{
        VULKAN_SDK         = $Path
        Vulkan_LIBRARY     = ConvertTo-FullVulkanPath (Join-Path $Path "Lib/vulkan-1.lib")
        Vulkan_INCLUDE_DIR = ConvertTo-FullVulkanPath (Join-Path $Path "Include")
    }
    foreach ($name in $values.Keys) {
        Set-Item -LiteralPath "Env:$name" -Value $values[$name]
        if ($env:GITHUB_ENV) { "$name=$($values[$name])" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
    }
    if ($env:GITHUB_PATH) { (ConvertTo-FullVulkanPath (Join-Path $Path "Bin")) | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append }
}
