<#
.SYNOPSIS
  Packaged-binary acceptance for the designated local build runner.

.DESCRIPTION
  Unpacks the NSIS installer that was just built into a new temporary folder
  with 7-Zip (nothing is installed, no registry or Start menu entry is
  written, an installed copy is never touched), then runs the packaged
  executable's headless self-test twice, in two separate processes, against a
  synthetic profile inside that temporary folder:

    run     package layout, packaged encoder, upgrade of a synthetic legacy
            database (including line-ending checksum drift), interrupted
            recording discovery and recovery, fixture import, a summary through
            the provider path against a loopback stub
    verify  reopens the same profile in a fresh process and checks that the
            data survived

  It also checks that the executable refuses a non-empty profile folder and a
  malformed command line. Everything is deleted afterwards unless -KeepWork.

  It never reads or writes the user's own profile, settings, credentials,
  templates or providers, and it needs no network. It does not capture audio,
  load a speech model, drive the interface or run the installer. Those checks
  stay manual; see docs/windows-release.md.

.PARAMETER Installer
  Path to the NSIS setup executable. Defaults to the current package.json
  version in the local bundle folders.

.PARAMETER BundleRoot
  Extra bundle folder to search when -Installer is not given.

.PARAMETER WorkRoot
  Folder that receives the temporary work folder. Defaults to RUNNER_TEMP, then
  the user's temp folder.

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

function Resolve-InstallerPath {
    if ($Installer) {
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
        if (-not $resolved) { throw "No ClawScribe $version setup executable was found. Build installers first or pass -Installer." }
    }
    $item = Get-Item -LiteralPath $resolved
    if ($item.Length -le 0 -or $item.Name -notlike "ClawScribe_*_x64-setup.exe") {
        throw "The installer is empty or is not a ClawScribe setup executable."
    }
    return $resolved
}

function Resolve-SevenZip {
    $command = Get-Command "7z.exe" -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    foreach ($base in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if ($base) {
            $candidate = Join-Path $base "7-Zip/7z.exe"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    throw "7-Zip is required to unpack the installer without installing it. Install it on the runner; the release workflow already installs it."
}

function Invoke-SelfTest {
    param([string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory)
    $info = [System.Diagnostics.ProcessStartInfo]::new($Executable)
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    $info.WorkingDirectory = $WorkingDirectory
    $process = [System.Diagnostics.Process]::Start($info)
    $output = $process.StandardOutput.ReadToEndAsync()
    $errors = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($StepTimeoutSeconds * 1000)) {
        # Only the process started here is stopped, by its id.
        try { $process.Kill($true) } catch { }
        throw "The packaged executable did not finish within $StepTimeoutSeconds seconds."
    }
    $process.WaitForExit()
    return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $output.Result; Errors = $errors.Result }
}

function Read-SelfTestReport {
    param([string]$Profile, [string]$Phase)
    $path = Join-Path $Profile "self-test-report-$Phase.json"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "The $Phase phase wrote no report." }
    return (Get-Content -LiteralPath $path -Raw | ConvertFrom-Json)
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

$installerPath = Resolve-InstallerPath
$sevenZip = Resolve-SevenZip
$workBase = if ($WorkRoot) { $WorkRoot } elseif ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
New-Item -ItemType Directory -Force -Path $workBase | Out-Null
$volume = [System.IO.DriveInfo]::new([System.IO.Path]::GetPathRoot((Resolve-Path -LiteralPath $workBase).Path))
if (-not $volume.IsReady -or $volume.AvailableFreeSpace -lt 3GB) {
    throw "Packaged acceptance needs a ready local volume with at least 3 GiB free for the unpacked package."
}
$work = Join-Path (Resolve-Path -LiteralPath $workBase).Path ("clawscribe-packaged-acceptance-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null

$failures = [System.Collections.Generic.List[string]]::new()
$summary = [System.Collections.Generic.List[string]]::new()
try {
    $extract = Join-Path $work "package"
    Write-Host "Unpacking the installer without installing it"
    & $sevenZip x -y "-o$extract" -- $installerPath | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip could not unpack the installer." }

    # The application executable is the one next to the packaged encoder.
    $encoder = Get-ChildItem -LiteralPath $extract -Recurse -File -Filter "ffmpeg.exe" | Select-Object -First 1
    if (-not $encoder) { throw "The unpacked installer contains no packaged encoder." }
    $applicationFolder = $encoder.DirectoryName
    $sidecars = @("ffmpeg.exe", "llama-helper.exe", "codex-app-server.exe")
    $executables = @(Get-ChildItem -LiteralPath $applicationFolder -File -Filter "*.exe" |
        Where-Object { $_.Name -notin $sidecars -and $_.Name -notlike "uninstall*" })
    if ($executables.Count -ne 1) { throw "Expected exactly one application executable beside the packaged encoder, found $($executables.Count)." }
    $application = $executables[0].FullName
    $applicationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $application).Hash.ToLowerInvariant()
    Write-Host "Application executable: $($executables[0].Name) sha256=$applicationHash"

    $profile = Join-Path $work "profile"

    Write-Host "Run phase (fresh process)"
    $run = Invoke-SelfTest -Executable $application -Arguments @("--self-test", "run", $profile) -WorkingDirectory $work
    $runReport = Read-SelfTestReport -Profile $profile -Phase "run"
    Write-Host "Verify phase (second fresh process)"
    $verify = Invoke-SelfTest -Executable $application -Arguments @("--self-test", "verify", $profile) -WorkingDirectory $work
    $verifyReport = Read-SelfTestReport -Profile $profile -Phase "verify"

    foreach ($pair in @(@("run", $run, $runReport), @("verify", $verify, $verifyReport))) {
        $phase = $pair[0]; $result = $pair[1]; $report = $pair[2]
        foreach ($check in $report.checks) {
            $label = if ($check.status -eq "passed") { "PASS" } else { "FAIL" }
            $line = "$label [$phase] $($check.name): $($check.detail)"
            Write-Host $line
            $summary.Add("- $line")
            if ($check.status -ne "passed") { $failures.Add("[$phase] $($check.name)") }
        }
        if ($result.ExitCode -ne 0 -or -not $report.passed) { $failures.Add("[$phase] exit code $($result.ExitCode)") }
    }

    # The executable must refuse unsafe input without touching it.
    $busy = Join-Path $work "busy-profile"
    New-Item -ItemType Directory -Path $busy | Out-Null
    Set-Content -LiteralPath (Join-Path $busy "keep.txt") -Value "untouched"
    $refused = Invoke-SelfTest -Executable $application -Arguments @("--self-test", "run", $busy) -WorkingDirectory $work
    $afterRefusal = @(Get-ChildItem -LiteralPath $busy -Force)
    if ($refused.ExitCode -eq 0 -or $afterRefusal.Count -ne 1 -or $afterRefusal[0].Name -ne "keep.txt") {
        $failures.Add("a non-empty profile folder was not refused untouched")
        $summary.Add("- FAIL [negative] non-empty profile folder was not refused untouched")
    } else {
        $summary.Add("- PASS [negative] a non-empty profile folder is refused untouched")
    }
    $malformed = Invoke-SelfTest -Executable $application -Arguments @("--self-test", "run") -WorkingDirectory $work
    if ($malformed.ExitCode -ne 2) {
        $failures.Add("a malformed self-test command line was not refused with exit code 2")
        $summary.Add("- FAIL [negative] a malformed command line was not refused")
    } else {
        $summary.Add("- PASS [negative] a malformed command line is refused")
    }
} finally {
    if ($env:GITHUB_STEP_SUMMARY) {
        $lines = @(
            "## Packaged-binary acceptance",
            "",
            "Unpacked installer, headless self-test on a synthetic profile, two processes.",
            ""
        ) + $summary + @(
            "",
            "Not covered: live microphone and system-audio capture, speech models, real providers, the graphical interface, installer interaction and long sessions. Those remain manual checks (docs/windows-release.md)."
        )
        $lines | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
    }
    if ($KeepWork) { Write-Host "Work folder kept: $work" } else { Remove-OwnedWorkFolder -Folder $work -Base $workBase }
}

if ($failures.Count -gt 0) {
    throw "Packaged-binary acceptance failed: $($failures -join '; ')"
}
Write-Host "Packaged-binary acceptance passed. Live capture, speech models, real providers, the interface and the installer itself were not exercised."
