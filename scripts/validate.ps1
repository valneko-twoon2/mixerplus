param([switch]$Staged)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if ($Staged) {
        # A mixed working tree could validate different code than the commit.
        git diff --quiet
        if ($LASTEXITCODE -ne 0) { throw 'Stage intended edits before committing; unstaged tracked edits remain.' }
        $untracked = @(git ls-files --others --exclude-standard)
        if ($LASTEXITCODE -ne 0 -or $untracked.Count -gt 0) {
            throw 'Untracked files remain; stage task files and preserve unrelated user work before committing.'
        }
    }
    & "$PSScriptRoot/sync-harness.ps1" -Check
    & "$PSScriptRoot/check-line-endings.ps1"
    & "$PSScriptRoot/update-notices.ps1" -Check
    cargo fmt --manifest-path src/Cargo.toml --check
    if ($LASTEXITCODE -ne 0) { throw 'Rust formatting failed.' }
    cargo clippy --config src/.cargo/config.toml --manifest-path src/Cargo.toml --all-targets --locked -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    cargo test --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
    git diff --check
    if ($LASTEXITCODE -ne 0) { throw 'Whitespace validation failed.' }
    git diff --cached --check
    if ($LASTEXITCODE -ne 0) { throw 'Staged whitespace validation failed.' }
} finally { Pop-Location }
