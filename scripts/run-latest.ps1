# Ends every running mixerplus (including the installed copy) and starts the
# latest release build, so the tray runs the code just checked. Agents run this
# after diagnostics instead of restarting the installed copy, which may be older.
$ErrorActionPreference = 'Stop'
$exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'src/target/release/mixerplus.exe'
if (-not (Test-Path $exe)) {
    throw "No release build at $exe. Build it with: cargo build --config src/.cargo/config.toml --manifest-path src/Cargo.toml --release --locked"
}
$running = @(Get-Process mixerplus -ErrorAction SilentlyContinue)
foreach ($process in $running) {
    Stop-Process -Id $process.Id -Force -Confirm:$false
}
foreach ($process in $running) {
    $null = $process.WaitForExit(5000)
}
$started = Start-Process $exe -PassThru
Write-Host "Started the latest build ($((Get-Item $exe).LastWriteTime)), process $($started.Id)."
