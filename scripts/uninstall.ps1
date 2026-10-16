# Removes mixerplus for the current user. Presets in %AppData%\mixerplus are kept
# unless -RemoveData is given. Also the command behind Settings > Apps.
param(
    [string]$Directory = (Join-Path $env:LOCALAPPDATA 'Programs\mixerplus'),
    [switch]$RemoveData
)
$ErrorActionPreference = 'Stop'
$Directory = [IO.Path]::GetFullPath($Directory)
$target = Join-Path $Directory 'mixerplus.exe'

# Refuse to delete a folder that is not a mixerplus install.
if (-not (Test-Path -LiteralPath $target)) {
    throw "mixerplus is not installed in $Directory."
}

Get-Process -Name mixerplus -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $target } | Stop-Process -Force
Start-Sleep -Milliseconds 300

Remove-Item -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\mixerplus' -Recurse -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name mixerplus -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $Directory -Recurse -Force
if ($RemoveData) {
    Remove-Item -LiteralPath (Join-Path $env:APPDATA 'mixerplus') -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'mixerplus and its presets were removed.'
} else {
    Write-Host 'mixerplus was removed. Presets are kept in %AppData%\mixerplus (use -RemoveData to delete them).'
}
