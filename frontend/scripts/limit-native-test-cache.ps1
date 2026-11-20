<#
.SYNOPSIS
  Keeps the persistent runner's native test build output bounded.

.DESCRIPTION
  The runner keeps Cargo's target directory between runs so builds stay
  incremental. Every compiled commit leaves another library test executable
  (`app_lib-<hash>.exe` with its `.pdb` and `.d`) in `release/deps`, and a full
  volume has already made installer builds fail. This script:

    1. keeps the newest -KeepTestExecutables test executables (by time) and
       deletes only older `app_lib-<16 hex>.exe|pdb|d` files from the release
       dependency folder, after checking that the folder and its parents are
       not links and that each file is a regular file;
    2. reports the workspace volume's free space and, unless
       -NoFailOnLowSpace, fails when it is below -MinimumFreeGiB, so a full
       disk is reported plainly before a long build instead of as an installer
       error.

  Libraries, build scripts, models, sidecars, installers and source are never
  touched; later builds recreate anything removed here.

.PARAMETER KeepTestExecutables
  How many of the newest library test executables to keep. Default 2.

.PARAMETER MinimumFreeGiB
  Free space the workspace volume must have. Default 12.

.PARAMETER NoFailOnLowSpace
  Report free space but do not fail when it is low (used for the after-run step).
#>
param(
    [int]$KeepTestExecutables = 2,
    [int]$MinimumFreeGiB = 12,
    [switch]$NoFailOnLowSpace
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

if ($KeepTestExecutables -lt 1) { throw "Keep at least one test executable." }
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$manifest = Join-Path $repoRoot "frontend/src-tauri/Cargo.toml"

$metadataText = & cargo metadata --locked --format-version 1 --no-deps --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { throw "Cargo metadata failed." }
$targetDirectory = (($metadataText -join "`n") | ConvertFrom-Json).target_directory
if (-not $targetDirectory -or -not (Test-Path -LiteralPath $targetDirectory -PathType Container)) {
    Write-Host "No Cargo target directory yet; nothing to trim."
} else {
    $targetResolved = (Resolve-Path -LiteralPath $targetDirectory).Path
    $deps = Join-Path $targetResolved "release/deps"
    if (Test-Path -LiteralPath $deps -PathType Container) {
        $entry = Get-Item -LiteralPath $deps
        while ($null -ne $entry) {
            if ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw "The release dependency path contains a link." }
            $entry = $entry.Parent
        }
        $executables = @(Get-ChildItem -LiteralPath $deps -File -Filter "app_lib-*.exe" |
            Where-Object { $_.Name -match '^app_lib-[0-9a-f]{16}\.exe$' } |
            Sort-Object LastWriteTimeUtc -Descending)
        $stale = @($executables | Select-Object -Skip $KeepTestExecutables)
        $freed = 0L
        foreach ($executable in $stale) {
            $hash = $executable.BaseName
            foreach ($extension in @("exe", "pdb", "d")) {
                $path = Join-Path $deps "$hash.$extension"
                if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
                $file = Get-Item -LiteralPath $path
                if ($file.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { continue }
                $freed += $file.Length
                Remove-Item -LiteralPath $path -Force
            }
        }
        Write-Host ("Kept {0} newest library test executables, removed {1} older ones ({2:N1} MiB)." -f
            [Math]::Min($KeepTestExecutables, $executables.Count), $stale.Count, ($freed / 1MB))
    } else {
        Write-Host "No release dependency folder yet; nothing to trim."
    }
}

$root = [System.IO.Path]::GetPathRoot($repoRoot)
$volume = [System.IO.DriveInfo]::new($root)
$freeGiB = [Math]::Round($volume.AvailableFreeSpace / 1GB, 1)
Write-Host "Workspace volume free space: $freeGiB GiB (minimum $MinimumFreeGiB GiB)"
if ($env:GITHUB_STEP_SUMMARY) {
    "- Workspace volume free space after trimming: $freeGiB GiB" | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
}
if (-not $NoFailOnLowSpace -and $freeGiB -lt $MinimumFreeGiB) {
    throw "The workspace volume has $freeGiB GiB free; at least $MinimumFreeGiB GiB are needed. Free space on the runner (see docs/windows-release.md, GitHub Actions) before building."
}
