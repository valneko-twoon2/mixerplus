$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
& "$PSScriptRoot/sync-harness.ps1"
git config --local core.hooksPath .githooks
if ($LASTEXITCODE -ne 0) { throw 'Could not configure Git hooks.' }
git config --local core.autocrlf false
git config --local core.safecrlf true
Write-Host 'Git hooks installed. Review and trust the Codex Stop hook with /hooks.'
