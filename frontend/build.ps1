# ClawScribe Windows release wrapper.
# Prefer scripts/build-windows-release.ps1 for new automation.

Write-Host ""
Write-Host "========================================"
Write-Host "   ClawScribe Windows Release Build"
Write-Host "========================================"
Write-Host ""

& "$PSScriptRoot\scripts\build-windows-release.ps1" @args
exit $LASTEXITCODE
