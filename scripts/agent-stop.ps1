$ErrorActionPreference = 'Stop'
try {
    # Hook payloads are UTF-8; the console default code page would garble replies.
    [Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
    $inputText = [Console]::In.ReadToEnd()
    $event = if ($inputText.Trim()) { $inputText | ConvertFrom-Json } else { $null }
    Push-Location (Split-Path $PSScriptRoot -Parent)
    try {
        $status = @(git status --porcelain)
        if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect Git status.' }
        if ($status.Count -gt 0 -and !$event.stop_hook_active) {
            @{ decision = 'block'; reason = 'Uncommitted changes remain. Validate and commit only task changes with an English Conventional Commit. Preserve pre-existing user changes and report blockers instead of committing unrelated work.' } | ConvertTo-Json -Compress
        } else {
            # Avoid endless continuations when unrelated user changes remain.
            '{}'
        }
    } finally { Pop-Location }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 2
}
