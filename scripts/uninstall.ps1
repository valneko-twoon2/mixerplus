# Removes MixerPlus for the current user. Presets in %AppData%\MixerPlus are kept
# unless -RemoveData is given. Also the command behind Settings > Apps.
param(
    [string]$Directory = (Join-Path $env:LOCALAPPDATA 'Programs\MixerPlus'),
    [switch]$RemoveData
)
$ErrorActionPreference = 'Stop'
$Directory = [IO.Path]::GetFullPath($Directory)
$target = Join-Path $Directory 'mixerplus.exe'

# Refuse to delete a folder that is not a MixerPlus install.
if (-not (Test-Path -LiteralPath $target)) {
    throw "MixerPlus is not installed in $Directory."
}

Get-Process -Name mixerplus -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $target } | Stop-Process -Force
Start-Sleep -Milliseconds 300

Remove-Item -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MixerPlus' -Recurse -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name MixerPlus -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $Directory -Recurse -Force
if ($RemoveData) {
    Remove-Item -LiteralPath (Join-Path $env:APPDATA 'MixerPlus') -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'MixerPlus and its presets were removed.'
} else {
    Write-Host 'MixerPlus was removed. Presets are kept in %AppData%\MixerPlus (use -RemoveData to delete them).'
}
