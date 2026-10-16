# Installs or updates MixerPlus for the current user. No administrator rights needed.
#   irm https://github.com/<owner>/<repo>/releases/latest/download/install.ps1 | iex
# Parameters (when run as a file): -Repo owner/name, -Directory target folder, and
# -Source, a local mixerplus.exe to install instead of downloading a release.
param(
    [string]$Repo = 'valneko-twoon2/MixerPlus',
    [string]$Directory = (Join-Path $env:LOCALAPPDATA 'Programs\MixerPlus'),
    [string]$Source
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.ServicePointManager]::SecurityProtocol

$work = Join-Path ([IO.Path]::GetTempPath()) ("mixerplus-" + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $work
try {
    $download = Join-Path $work 'mixerplus.exe'
    $remover = Join-Path $work 'uninstall.ps1'
    if ($Source) {
        Copy-Item -LiteralPath $Source -Destination $download
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'uninstall.ps1') -Destination $remover
    } else {
        $base = "https://github.com/$Repo/releases/latest/download"
        Invoke-WebRequest -Uri "$base/mixerplus.exe" -OutFile $download -UseBasicParsing
        # GitHub serves release assets as application/octet-stream, so Content can be bytes.
        $content = (Invoke-WebRequest -Uri "$base/mixerplus.exe.sha256" -UseBasicParsing).Content
        if ($content -is [byte[]]) { $content = [Text.Encoding]::ASCII.GetString($content) }
        $expected = $content.Trim().ToLower()
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLower()
        if ($expected -ne $actual) { throw 'Checksum mismatch; nothing was installed.' }
        Invoke-WebRequest -Uri "$base/uninstall.ps1" -OutFile $remover -UseBasicParsing
    }

    $target = Join-Path $Directory 'mixerplus.exe'
    # Replace a running copy: the executable is locked while it runs.
    Get-Process -Name mixerplus -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $target } | Stop-Process -Force
    Start-Sleep -Milliseconds 300
    $null = New-Item -ItemType Directory -Path $Directory -Force
    Copy-Item -LiteralPath $download -Destination $target -Force
    Copy-Item -LiteralPath $remover -Destination (Join-Path $Directory 'uninstall.ps1') -Force
    # List the app in Settings > Apps; this only touches the current user's hive.
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MixerPlus'
    $null = New-Item -Path $key -Force
    $version = (Get-Item -LiteralPath $target).VersionInfo.ProductVersion
    $command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Directory 'uninstall.ps1')`" -Directory `"$Directory`""
    $values = @{
        DisplayName = 'MixerPlus'; DisplayVersion = $version; Publisher = 'MixerPlus'
        InstallLocation = $Directory; DisplayIcon = $target; UninstallString = $command
    }
    foreach ($name in $values.Keys) {
        if ($values[$name]) { Set-ItemProperty -Path $key -Name $name -Value $values[$name] }
    }
    Set-ItemProperty -Path $key -Name NoModify -Value 1 -Type DWord
    Set-ItemProperty -Path $key -Name NoRepair -Value 1 -Type DWord
    # Start at sign-in, so MixerPlus stays in the tray. Presets never apply by themselves.
    $run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    Set-ItemProperty -Path $run -Name MixerPlus -Value "`"$target`""
    # The tray icon is promoted next to the system icons by MixerPlus itself, through
    # Explorer's per-icon registry entry, which is keyed by this fixed path.
    Start-Process -FilePath $target
    Write-Host "MixerPlus installed to $target"
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
