param(
    [string]$TauriRoot = (Join-Path $PSScriptRoot "../src-tauri"),
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"

function Find-VcRedistDirectory {
    if ($env:VCToolsRedistDir) {
        return $env:VCToolsRedistDir
    }

    $vswhere = Join-Path ([Environment]::GetEnvironmentVariable("ProgramFiles(x86)")) "Microsoft Visual Studio/Installer/vswhere.exe"
    if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
        $installations = & $vswhere -products "*" -version "[17.0,18.0)" -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($LASTEXITCODE -ne 0) { throw "vswhere could not locate Visual Studio 2022 C++ Build Tools." }
        foreach ($installation in $installations) {
            $versions = Get-ChildItem -LiteralPath (Join-Path $installation "VC/Redist/MSVC") -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } |
                Sort-Object { [version]$_.Name } -Descending
            foreach ($version in $versions) {
                if (Test-Path -LiteralPath (Join-Path $version.FullName "x64/Microsoft.VC143.CRT") -PathType Container) {
                    return $version.FullName
                }
            }
        }
    }
    throw "Visual Studio 2022 x64 VC143 redistribution directory not found. Install the C++ Build Tools redistributable files or set VCToolsRedistDir."
}

function Get-VerifiedFileVersion {
    param([string]$Path)
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne "Valid" -or
        $signature.SignerCertificate.Subject -notmatch "(^|,\s*)O=Microsoft Corporation(,|$)") {
        throw "Expected a valid Microsoft Authenticode signature for '$([IO.Path]::GetFileName($Path))'. Signature status: $($signature.Status)."
    }
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    if ($info.FileMajorPart -le 0) { throw "Missing file version for '$([IO.Path]::GetFileName($Path))'." }
    return "$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart).$($info.FilePrivatePart)"
}

$tauri = (Resolve-Path -LiteralPath $TauriRoot).Path
$config = Get-Content -LiteralPath (Join-Path $PSScriptRoot "../src-tauri/tauri.windows-release.conf.json") -Raw | ConvertFrom-Json
$redist = Find-VcRedistDirectory
if (-not (Test-Path -LiteralPath $redist -PathType Container)) {
    throw "VC143 redistribution directory is missing. Check VCToolsRedistDir or repair the Visual Studio C++ Build Tools installation."
}

$sources = foreach ($name in $config.bundle.resources.PSObject.Properties.Value) {
    $component = if ($name -eq "vcomp140.dll") { "OpenMP" } else { "CRT" }
    $source = Join-Path $redist "x64/Microsoft.VC143.$component/$name"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Missing required VC143 x64 redistributable '$name' in Microsoft.VC143.$component. Repair the Visual Studio C++ Build Tools installation or check VCToolsRedistDir."
    }
    Get-Item -LiteralPath $source
}

$versions = foreach ($source in $sources) {
    $version = Get-VerifiedFileVersion $source.FullName
    "vc_runtime_$($source.Name)=$version"
}
if (-not $VerifyOnly) {
    $destination = Join-Path $tauri "binaries/vc-runtime"
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($source in $sources) {
        $staged = Join-Path $destination $source.Name
        Copy-Item -LiteralPath $source.FullName -Destination $staged -Force
        $null = Get-VerifiedFileVersion $staged
    }
    $versions | Set-Content -LiteralPath (Join-Path $destination "versions.txt") -Encoding ascii
}
$versions | ForEach-Object { Write-Host $_ }
