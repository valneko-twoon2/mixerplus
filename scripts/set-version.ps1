# Sets the application version in Cargo.toml, Cargo.lock, and the Windows manifest.
# Usage: set-version.ps1 [X.Y.Z]. Without an argument it asks for the version.
param([string]$Version)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$toml = Join-Path $root 'src/Cargo.toml'
$lock = Join-Path $root 'src/Cargo.lock'
$manifest = Join-Path $root 'src/app.manifest'

function Read-Text($path) { [IO.File]::ReadAllText($path) }
function Write-Text($path, $text) { [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false)) }

$current = [regex]::Match((Read-Text $toml), '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $Version) { $Version = Read-Host "New version (current $current)" }
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must look like 1.2.3, not '$Version'." }
if ($Version -eq $current) { Write-Host "Already at $current."; exit 0 }

# Only the first `version =` line of Cargo.toml is the package version.
$text = Read-Text $toml
Write-Text $toml ([regex]::new('(?m)^version = "[^"]+"').Replace($text, "version = `"$Version`"", 1))

$text = Read-Text $lock
$pattern = '(?m)(^name = "mixerplus"\r?\nversion = ")[^"]+'
if ($text -notmatch $pattern) { throw 'mixerplus entry not found in Cargo.lock.' }
Write-Text $lock ([regex]::new($pattern).Replace($text, "`${1}$Version", 1))

$text = Read-Text $manifest
$pattern = '(<assemblyIdentity version=")[^"]+("[^>]*name="mixerplus\.Tray")'
if ($text -notmatch $pattern) { throw 'mixerplus.Tray identity not found in app.manifest.' }
Write-Text $manifest ([regex]::new($pattern).Replace($text, "`${1}$Version.0`${2}", 1))

Write-Host "Version $current -> $Version"
Write-Host 'Updated src/Cargo.toml, src/Cargo.lock, src/app.manifest. Review with git diff, then commit.'
