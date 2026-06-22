param(
    [string]$TauriRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\src-tauri")).Path,
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

function Get-SherpaVersion {
    param([string]$CargoToml)

    $content = Get-Content -LiteralPath $CargoToml -Raw
    $matches = [regex]::Matches($content, 'sherpa-onnx\s*=\s*(?:\{\s*)?(?:version\s*=\s*)?"([^"]+)"')
    if ($matches.Count -eq 0) {
        throw "Could not find sherpa-onnx version in $CargoToml"
    }

    return $matches[$matches.Count - 1].Groups[1].Value
}

function Test-RequiredDlls {
    param([string]$Directory)

    $dlls = @(
        "onnxruntime.dll",
        "onnxruntime_providers_shared.dll",
        "sherpa-onnx-c-api.dll",
        "sherpa-onnx-cxx-api.dll"
    )

    foreach ($dll in $dlls) {
        if (-not (Test-Path -LiteralPath (Join-Path $Directory $dll) -PathType Leaf)) {
            return $false
        }
    }

    return $true
}

$tauriRootPath = (Resolve-Path -LiteralPath $TauriRoot).Path
$repoRoot = (Resolve-Path (Join-Path $tauriRootPath "..\..")).Path
$cargoToml = Join-Path $tauriRootPath "Cargo.toml"

if (-not $Version) {
    $Version = Get-SherpaVersion -CargoToml $cargoToml
}

$archiveStem = "sherpa-onnx-v$Version-win-x64-shared-MT-Release-lib"
$cacheRoot = Join-Path $repoRoot "target\sherpa-onnx-prebuilt"
$libDir = Join-Path $cacheRoot "$archiveStem\lib"
$destDir = Join-Path $tauriRootPath "binaries\sherpa-onnx"

if (-not (Test-RequiredDlls -Directory $libDir)) {
    New-Item -ItemType Directory -Force -Path $cacheRoot | Out-Null

    $archiveName = "$archiveStem.tar.bz2"
    $archivePath = Join-Path $cacheRoot $archiveName
    $url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$Version/$archiveName"

    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        Write-Host "Downloading sherpa-onnx Windows shared runtime from $url"
        Invoke-WebRequest -Uri $url -OutFile $archivePath
    }

    $tar = Get-Command tar -ErrorAction SilentlyContinue
    if (-not $tar) {
        throw "Missing required command 'tar'. Install tar or run a Cargo build once so sherpa-onnx-sys extracts the shared runtime."
    }

    if (Test-Path -LiteralPath (Join-Path $cacheRoot $archiveStem)) {
        Remove-Item -LiteralPath (Join-Path $cacheRoot $archiveStem) -Recurse -Force
    }

    & $tar.Source -xjf $archivePath -C $cacheRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to extract $archivePath"
    }
}

if (-not (Test-RequiredDlls -Directory $libDir)) {
    throw "Sherpa runtime DLLs are incomplete in $libDir"
}

New-Item -ItemType Directory -Force -Path $destDir | Out-Null
Get-ChildItem -LiteralPath $libDir -Filter "*.dll" | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $destDir $_.Name) -Force
}

if (-not (Test-RequiredDlls -Directory $destDir)) {
    throw "Failed to stage complete sherpa runtime DLL set in $destDir"
}

Write-Host "Staged sherpa-onnx runtime DLLs:"
Get-ChildItem -LiteralPath $destDir -Filter "*.dll" | ForEach-Object {
    Write-Host "  $($_.Name) ($($_.Length) bytes)"
}
