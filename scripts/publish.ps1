$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    cargo build --config src/.cargo/config.toml --manifest-path src/Cargo.toml --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    $null = New-Item -ItemType Directory -Path artifacts -Force
    Copy-Item -LiteralPath src/target/release/mixerplus.exe -Destination artifacts/mixerplus.exe -Force
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath artifacts/mixerplus.exe).Hash.ToLower()
    [IO.File]::WriteAllText("$PWD/artifacts/mixerplus.exe.sha256", $hash, [Text.Encoding]::ASCII)
    # artifacts/ holds exactly the GitHub release assets. The license and third-party
    # notices are embedded in the executable (tray menu > About).
    foreach ($file in @('install.ps1', 'uninstall.ps1')) {
        Copy-Item -LiteralPath "scripts/$file" -Destination artifacts -Force
    }
    $assets = @('mixerplus.exe', 'mixerplus.exe.sha256', 'install.ps1', 'uninstall.ps1')
    Get-ChildItem -LiteralPath artifacts -Force | Where-Object { $assets -notcontains $_.Name } |
        Remove-Item -Recurse -Force
    Write-Host 'Release assets are in artifacts/ (mixerplus.exe, its .sha256, install.ps1, uninstall.ps1). No .NET runtime or debug symbols are required.'
} finally { Pop-Location }
