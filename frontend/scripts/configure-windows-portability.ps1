$ErrorActionPreference = "Stop"
if ($env:CARGO_ENCODED_RUSTFLAGS) {
    throw "CARGO_ENCODED_RUSTFLAGS overrides the portable Windows profile. Clear it before building."
}
if ($env:RUSTFLAGS -match 'target-cpu\s*=\s*(?!x86-64-v2(?:\s|$))\S+' -or $env:RUSTFLAGS -match 'target-feature') {
    throw "RUSTFLAGS overrides the portable Windows CPU baseline. Remove the CPU/feature override before building."
}
if ($env:RUSTFLAGS -notmatch 'target-cpu=x86-64-v2') {
    $env:RUSTFLAGS = (($env:RUSTFLAGS + " -C target-cpu=x86-64-v2").Trim())
}
$env:CMAKE_PROJECT_INCLUDE = (Join-Path $PSScriptRoot "portable-ggml.cmake").Replace('\', '/')
