<#
.SYNOPSIS
  Packaged-binary acceptance for the designated local build runner.

.DESCRIPTION
  Unpacks the NSIS installer that was just built into a new temporary folder
  with 7-Zip (nothing is installed, no registry or Start menu entry is
  written, an installed copy is never touched), then runs the packaged
  executable's headless self-test twice, in two separate processes, against a
  synthetic profile inside that temporary folder:

    run     package layout (sidecars, runtime libraries, bundled templates,
            Codex runtime), packaged encoder, upgrade of a synthetic legacy
            database (including line-ending checksum drift) through an isolated
            open path that never touches a credential store, interrupted
            recording discovery and recovery, fixture import, a summary through
            the provider path against a loopback stub
    verify  reopens the same profile in a fresh process and checks that the
            data survived

  It also runs the executable against unsafe input and checks the exact refusal
  and that nothing was touched: a malformed command line, a relative path, a
  non-empty folder, a folder with only a stale marker, an unmarked folder for
  verify, a forged marker, and a profile that contains a link.

  It never reads or writes the user's own profile, settings, credentials,
  templates or providers, and it needs no network. It does not capture audio,
  load a speech model, drive the interface or run the installer. Those checks
  stay manual; see docs/windows-release.md.

  What reaches the Actions log is limited to fixed check names, pass or fail,
  exit codes and a few counts. Paths, error text and the application's own output
  are never printed. If the work folder cannot be removed, that is reported after
  the first failure and never replaces it.

.PARAMETER Installer
  Path to the NSIS setup executable. Defaults to the current package.json
  version in the local bundle folders.

.PARAMETER BundleRoot
  Extra bundle folder to search when -Installer is not given.

.PARAMETER WorkRoot
  Folder that receives the temporary work folder. Defaults to RUNNER_TEMP, then
  the user's temp folder. It must be on a local drive with enough free space.

.PARAMETER StepTimeoutSeconds
  Time allowed for each run of the executable. The process started here is
  stopped by its own process id when the time runs out.

.PARAMETER KeepWork
  Keep the work folder (extracted package, profile and reports) for inspection.
#>
param(
    [string]$Installer = "",
    [string]$BundleRoot = "",
    [string]$WorkRoot = "",
    [int]$StepTimeoutSeconds = 300,
    [switch]$KeepWork
)

$ErrorActionPreference = "Stop"
# Exit codes of the executable are inspected explicitly below.
$PSNativeCommandUseErrorActionPreference = $false

if ([string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER) -or
    $env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER -or
    $env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER) {
    throw "Packaged acceptance requires the designated local build runner."
}
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { throw "Packaged acceptance must run on Windows." }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path

# Check names the executable may report. Anything else is shown as "unknown".
$knownChecks = @(
    "profile_isolation", "package_layout", "packaged_encoder", "migration_upgrade",
    "recording_recovery", "import_fixture", "summary_stub", "state_saved",
    "persistence_reopen", "database_integrity", "recovered_audio_persisted"
)
$exitMeaning = @{ 0 = "passed"; 1 = "a check failed"; 2 = "unusable command line"; 3 = "runtime or report setup failed"; 4 = "profile folder refused" }

class AcceptanceFailure : System.Exception {
    AcceptanceFailure([string]$Message) : base($Message) { }
}

function Stop-Acceptance {
    param([string]$Message)
    throw [AcceptanceFailure]::new($Message)
}

function Format-ExitCode {
    param([int]$Code)
    $hex = [System.BitConverter]::ToUInt32([System.BitConverter]::GetBytes($Code), 0).ToString("X8")
    if ($exitMeaning.ContainsKey($Code)) { return "$Code ($($exitMeaning[$Code]))" }
    return "$Code (0x$hex, unexpected: crash, missing runtime library or forced stop)"
}

function Resolve-InstallerPath {
    if ($Installer) {
        if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) { Stop-Acceptance "The given installer was not found." }
        $resolved = (Resolve-Path -LiteralPath $Installer).Path
    } else {
        $version = (Get-Content -LiteralPath (Join-Path $repoRoot "frontend/package.json") -Raw | ConvertFrom-Json).version
        $roots = @()
        if ($BundleRoot) { $roots += $BundleRoot }
        $roots += (Join-Path $repoRoot "frontend/src-tauri/target/release/bundle")
        $roots += (Join-Path $repoRoot "target/release/bundle")
        $resolved = $null
        foreach ($root in $roots) {
            $candidate = Join-Path $root "nsis/ClawScribe_${version}_x64-setup.exe"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $resolved = (Resolve-Path -LiteralPath $candidate).Path
                break
            }
        }
        if (-not $resolved) { Stop-Acceptance "No ClawScribe $version setup executable was found. Build installers first or pass -Installer." }
    }
    $item = Get-Item -LiteralPath $resolved
    if ($item.Length -le 0 -or $item.Name -notlike "ClawScribe_*_x64-setup.exe") {
        Stop-Acceptance "The installer is empty or is not a ClawScribe setup executable."
    }
    return $resolved
}

function Resolve-SevenZip {
    foreach ($base in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if ($base) {
            $candidate = Join-Path $base "7-Zip/7z.exe"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    $command = Get-Command "7z.exe" -CommandType Application -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    Stop-Acceptance "7-Zip is required to unpack the installer without installing it. Install it on the runner; the release workflow already installs it."
}

# Runs the packaged executable once. Output is drained but never kept or printed.
function Invoke-SelfTest {
    param([string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory, [string]$TempFolder)
    $info = [System.Diagnostics.ProcessStartInfo]::new($Executable)
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    $info.WorkingDirectory = $WorkingDirectory
    # Stray temporary files land in the disposable work folder, not the runner's.
    $info.Environment["TEMP"] = $TempFolder
    $info.Environment["TMP"] = $TempFolder
    $process = [System.Diagnostics.Process]::Start($info)
    $null = $process.StandardOutput.ReadToEndAsync()
    $null = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($StepTimeoutSeconds * 1000)) {
        # Only the process started here is stopped, by its id.
        try { $process.Kill($true) } catch { }
        return [pscustomobject]@{ ExitCode = $null; TimedOut = $true }
    }
    $process.WaitForExit()
    return [pscustomobject]@{ ExitCode = $process.ExitCode; TimedOut = $false }
}

# The report is read only if it is a small regular file with the expected shape.
function Read-SelfTestReport {
    param([string]$Profile, [string]$Phase)
    $path = Join-Path $Profile "self-test-report-$Phase.json"
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if (-not $item -or $item.PSIsContainer -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -or $item.Length -gt 1MB) { return $null }
    try { $report = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json } catch { return $null }
    if ($report.schema -ne 1 -or $report.phase -ne $Phase -or -not $report.checks -or $report.checks.Count -lt 1) { return $null }
    return $report
}

# A detail is shown only when it is short and holds no path or drive letter.
function Get-SafeDetail {
    param($Detail)
    $text = [string]$Detail
    if ($text.Length -le 240 -and $text -match "^[A-Za-z0-9 .,;:()%+/'_-]*$" -and $text -notmatch "[A-Za-z]:[\\/]" -and $text -notmatch "\\\\") { return $text }
    return ""
}

function Get-FolderSnapshot {
    param([string]$Folder)
    $entries = Get-ChildItem -LiteralPath $Folder -Recurse -Force -ErrorAction SilentlyContinue | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($Folder.Length)
        if ($_.PSIsContainer) { "$relative|dir" }
        else { "$relative|$($_.Length)|$((Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash)" }
    }
    return (@($entries) -join "`n")
}

function Remove-Link {
    param([string]$Path)
    # Deletes only the link itself, never what it points to.
    if (Test-Path -LiteralPath $Path) { [System.IO.Directory]::Delete($Path) }
}

function Remove-OwnedWorkFolder {
    param([string]$Folder, [string]$Base)
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) { return }
    $resolved = (Resolve-Path -LiteralPath $Folder).Path
    $prefix = (Resolve-Path -LiteralPath $Base).Path.TrimEnd([char]92, [char]47) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Leaf $resolved) -notmatch '^clawscribe-packaged-acceptance-[0-9a-f]{32}$') {
        throw "Refusing to remove a folder this script did not create."
    }
    if (Get-ChildItem -LiteralPath $resolved -Recurse -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue | Select-Object -First 1) {
        throw "Refusing to remove a work folder that contains a link."
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

$failures = [System.Collections.Generic.List[string]]::new()
$summary = [System.Collections.Generic.List[string]]::new()
$firstFailure = $null
$cleanupFailure = $null
$work = $null
$workBase = $null

try {
    $installerPath = Resolve-InstallerPath
    $sevenZip = Resolve-SevenZip
    $workBase = if ($WorkRoot) { $WorkRoot } elseif ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
    New-Item -ItemType Directory -Force -Path $workBase | Out-Null
    $workBase = (Resolve-Path -LiteralPath $workBase).Path
    # The volume that really receives the unpacked package, not the checkout's.
    $volumeRoot = [System.IO.Path]::GetPathRoot($workBase)
    if ($volumeRoot -notmatch '^[A-Za-z]:\\$') { Stop-Acceptance "The work folder must be on a local drive with a drive letter." }
    $volume = [System.IO.DriveInfo]::new($volumeRoot)
    if (-not $volume.IsReady -or $volume.DriveType -ne [System.IO.DriveType]::Fixed -or $volume.AvailableFreeSpace -lt 3GB) {
        Stop-Acceptance "Packaged acceptance needs a ready fixed local volume with at least 3 GiB free for the unpacked package."
    }
    $work = Join-Path $workBase ("clawscribe-packaged-acceptance-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $work | Out-Null
    $tempFolder = Join-Path $work "temp"
    New-Item -ItemType Directory -Path $tempFolder | Out-Null

    $extract = Join-Path $work "package"
    Write-Host "Unpacking the installer without installing it"
    & $sevenZip x -y "-o$extract" -- $installerPath | Out-Null
    if ($LASTEXITCODE -ne 0) { Stop-Acceptance "7-Zip could not unpack the installer." }

    # The application executable is the one next to the packaged encoder.
    $encoder = Get-ChildItem -LiteralPath $extract -Recurse -File -Filter "ffmpeg.exe" | Select-Object -First 1
    if (-not $encoder) { Stop-Acceptance "The unpacked installer contains no packaged encoder." }
    $applicationFolder = $encoder.DirectoryName
    $sidecars = @("ffmpeg.exe", "llama-helper.exe", "codex-app-server.exe")
    $executables = @(Get-ChildItem -LiteralPath $applicationFolder -File -Filter "*.exe" |
        Where-Object { $_.Name -notin $sidecars -and $_.Name -notlike "uninstall*" })
    if ($executables.Count -ne 1) { Stop-Acceptance "Expected exactly one application executable beside the packaged encoder, found $($executables.Count)." }
    $application = $executables[0].FullName
    $applicationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $application).Hash.ToLowerInvariant()
    Write-Host "Application executable: $($executables[0].Name) sha256=$applicationHash"

    $profile = Join-Path $work "profile"

    function Add-Result {
        param([bool]$Passed, [string]$Line)
        $label = if ($Passed) { "PASS" } else { "FAIL" }
        Write-Host "$label $Line"
        $summary.Add("- $label $Line")
        if (-not $Passed) { $failures.Add($Line) }
    }

    function Invoke-Phase {
        param([string]$Phase)
        $result = Invoke-SelfTest -Executable $application -Arguments @("--self-test", $Phase, $profile) -WorkingDirectory $work -TempFolder $tempFolder
        if ($result.TimedOut) {
            Add-Result $false "[$Phase] no result: the executable did not finish within $StepTimeoutSeconds seconds and was stopped"
            return $null
        }
        Write-Host "[$Phase] exit code: $(Format-ExitCode $result.ExitCode)"
        $report = Read-SelfTestReport -Profile $profile -Phase $Phase
        if ($null -eq $report) {
            # No usable JSON: the exit code is the only evidence, and it is reported.
            Add-Result $false "[$Phase] no readable report; exit code $(Format-ExitCode $result.ExitCode)"
            return $result.ExitCode
        }
        foreach ($check in $report.checks) {
            $name = if ($knownChecks -contains [string]$check.name) { [string]$check.name } else { "unknown check" }
            $passed = ($check.status -eq "passed")
            if ($passed) {
                $detail = Get-SafeDetail $check.detail
                Add-Result $true ("[$Phase] ${name}" + $(if ($detail) { ": $detail" } else { "" }))
            } else {
                $notRun = ([string]$check.detail).StartsWith("Not run:")
                Add-Result $false ("[$Phase] ${name}: " + $(if ($notRun) { "not run" } else { "failed" }))
            }
        }
        if ($result.ExitCode -ne 0 -or -not $report.passed) { Add-Result $false "[$Phase] exit code $(Format-ExitCode $result.ExitCode)" }
        return $result.ExitCode
    }

    function Test-Refusal {
        param([string]$Label, [string[]]$Arguments, [int]$ExpectedExit, [string]$Untouched = "", [string]$WorkingDirectory = $work)
        $before = if ($Untouched) { Get-FolderSnapshot $Untouched } else { "" }
        $result = Invoke-SelfTest -Executable $application -Arguments $Arguments -WorkingDirectory $WorkingDirectory -TempFolder $tempFolder
        $after = if ($Untouched) { Get-FolderSnapshot $Untouched } else { "" }
        $ok = (-not $result.TimedOut) -and ($result.ExitCode -eq $ExpectedExit) -and ($before -ceq $after)
        $actual = if ($result.TimedOut) { "no exit" } else { "exit $($result.ExitCode)" }
        Add-Result $ok "[negative] $Label (expected exit $ExpectedExit, got $actual$(if ($before -cne $after) { ', folder changed' } else { '' }))"
    }

    Write-Host "Run phase (fresh process)"
    $runExit = Invoke-Phase "run"
    # Exit code 0 or 1 means the run phase marked a profile, so verify can inspect it
    # even when a run check failed. Any other code left no usable profile.
    $ran = ($runExit -eq 0 -or $runExit -eq 1)
    if ($ran) {
        Write-Host "Verify phase (second fresh process)"
        $null = Invoke-Phase "verify"
    } else {
        Add-Result $false "[verify] not run: the run phase left no usable profile"
    }

    # Unsafe input must be refused with the exact code and without touching anything.
    $outside = Join-Path $work "outside"
    New-Item -ItemType Directory -Path $outside | Out-Null
    $busy = Join-Path $work "busy-profile"
    New-Item -ItemType Directory -Path $busy | Out-Null
    Set-Content -LiteralPath (Join-Path $busy "keep.txt") -Value "untouched"
    $stale = Join-Path $work "stale-marker-profile"
    New-Item -ItemType Directory -Path $stale | Out-Null
    Set-Content -LiteralPath (Join-Path $stale ".clawscribe-self-test") -Value "stale"
    $unmarked = Join-Path $work "unmarked-profile"
    New-Item -ItemType Directory -Path $unmarked | Out-Null
    $forged = Join-Path $work "forged-marker-profile"
    New-Item -ItemType Directory -Path $forged | Out-Null
    Set-Content -LiteralPath (Join-Path $forged ".clawscribe-self-test") -Value "ClawScribe self-test profile. Safe to delete.`nrun-id: not-an-id"
    Set-Content -LiteralPath (Join-Path $forged "self-test-state.json") -Value '{"schema": 1, "run_id": "x", "recovered_audio": "..\\..\\outside.mp4"}'

    Test-Refusal "no phase" @("--self-test") 2
    Test-Refusal "no profile folder" @("--self-test", "run") 2
    Test-Refusal "unknown phase" @("--self-test", "frobnicate", $unmarked) 2 $unmarked
    Test-Refusal "extra argument" @("--self-test", "run", $unmarked, "extra") 2 $unmarked
    # Started in an empty folder that must stay empty, so a relative path is never created.
    $relativeCwd = Join-Path $work "relative-start"
    New-Item -ItemType Directory -Path $relativeCwd | Out-Null
    Test-Refusal "relative profile folder" @("--self-test", "run", "relative-profile") 4 $relativeCwd $relativeCwd
    Test-Refusal "non-empty profile folder" @("--self-test", "run", $busy) 4 $busy
    Test-Refusal "profile folder with only a stale marker" @("--self-test", "run", $stale) 4 $stale
    Test-Refusal "verify on an unmarked folder" @("--self-test", "verify", $unmarked) 4 $unmarked
    Test-Refusal "verify with a forged marker and state" @("--self-test", "verify", $forged) 4 $forged

    # A link inside an otherwise valid profile must make verify refuse, and the
    # link's target must stay empty. The link is removed without following it.
    if ($ran -and (Test-Path -LiteralPath $profile -PathType Container)) {
        $link = Join-Path $profile "leak"
        New-Item -ItemType Junction -Path $link -Target $outside | Out-Null
        try {
            Test-Refusal "verify on a profile that contains a link" @("--self-test", "verify", $profile) 4
            $leaked = @(Get-ChildItem -LiteralPath $outside -Force).Count
            Add-Result ($leaked -eq 0) "[negative] nothing was written through the link (files found: $leaked)"
        } finally {
            Remove-Link $link
        }
    }
} catch {
    # The first failure is kept. Cleanup below can add to it but never replace it.
    $firstFailure = if ($_.Exception -is [AcceptanceFailure]) { $_.Exception.Message } else { "Packaged-binary acceptance stopped unexpectedly ($($_.Exception.GetType().Name))." }
} finally {
    try {
        if ($env:GITHUB_STEP_SUMMARY) {
            $lines = @(
                "## Packaged-binary acceptance",
                "",
                "Unpacked installer, headless self-test on a synthetic profile in two processes, and refusal checks on unsafe input. Only fixed check names, results and exit codes are shown.",
                ""
            ) + $summary
            if ($firstFailure) { $lines += @("", "Stopped early: $firstFailure") }
            $lines += @(
                "",
                "Not covered: live microphone and system-audio capture, speech models, real providers, the graphical interface, installer interaction and long sessions. Those remain manual checks (docs/windows-release.md)."
            )
            $lines | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
        }
    } catch { }
    if ($work) {
        if ($KeepWork) {
            Write-Host $(if ($env:GITHUB_ACTIONS) { "Work folder kept on the runner." } else { "Work folder kept: $work" })
        } else {
            try { Remove-OwnedWorkFolder -Folder $work -Base $workBase } catch { $cleanupFailure = "The acceptance work folder could not be removed; free the runner's temp space manually." }
        }
    }
}

if ($firstFailure) {
    $message = $firstFailure
    if ($failures.Count -gt 0) { $message += " Failed checks: $($failures -join '; ')" }
    if ($cleanupFailure) { $message += " Also: $cleanupFailure" }
    throw $message
}
if ($failures.Count -gt 0) {
    $message = "Packaged-binary acceptance failed: $($failures -join '; ')"
    if ($cleanupFailure) { $message += " Also: $cleanupFailure" }
    throw $message
}
if ($cleanupFailure) { throw $cleanupFailure }
Write-Host "Packaged-binary acceptance passed. Live capture, speech models, real providers, the interface and the installer itself were not exercised."
