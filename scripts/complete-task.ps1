$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $status = @(git status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect Git status.' }
    if ($status.Count -gt 0) { throw 'Uncommitted changes remain. Commit task changes; preserve and report pre-existing user changes.' }
    Write-Host 'Task changes are committed; working tree is clean.'
} finally { Pop-Location }
