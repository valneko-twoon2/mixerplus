param([switch]$Check)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$utf8 = [Text.UTF8Encoding]::new($false)
$target = Join-Path $root 'THIRD-PARTY-NOTICES.txt'

# The notices cover every crate that ships in the release executable: the normal
# (non-build, non-dev) dependency closure of the application for the Windows target.
# The application embeds this file and lists it under Credits, so it must track Cargo.lock.
$json = cargo metadata --manifest-path (Join-Path $root 'src/Cargo.toml') --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
$metadata = $json | ConvertFrom-Json
$packages = @{}
foreach ($package in $metadata.packages) { $packages[$package.id] = $package }
$nodes = @{}
foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }

$seen = [Collections.Generic.HashSet[string]]::new()
$queue = [Collections.Generic.Queue[string]]::new()
$queue.Enqueue($metadata.resolve.root)
while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    foreach ($dep in $nodes[$id].deps) {
        if (!($dep.dep_kinds | Where-Object { $null -eq $_.kind })) { continue }
        if ($seen.Add($dep.pkg)) { $queue.Enqueue($dep.pkg) }
    }
}

$text = [Text.StringBuilder]::new()
$null = $text.Append("THIRD-PARTY SOFTWARE NOTICES`n`n")
$entries = $seen | ForEach-Object { $packages[$_] } | Sort-Object { $_.name }, { $_.version }
foreach ($package in $entries) {
    $license = if ($package.license) { $package.license } else { throw "Crate has no SPDX license: $($package.name)" }
    $null = $text.Append("$($package.name) $($package.version) - $license`n")
    $directory = Split-Path $package.manifest_path -Parent
    $files = Get-ChildItem -LiteralPath $directory -File |
        Where-Object { $_.Name -match '^(license|licence|copying|unlicense)([-._]|$)' } |
        Sort-Object { $_.Name.ToLowerInvariant() }
    if (!$files) { throw "No license file found for $($package.name) $($package.version)." }
    foreach ($file in $files) {
        $null = $text.Append("--- $($file.Name) ---`n")
        $body = [IO.File]::ReadAllText($file.FullName) -replace '\r\n|\r', "`n"
        $null = $text.Append($body.TrimEnd("`n")).Append("`n`n")
    }
    $null = $text.Append("`n")
}
$content = ($text.ToString().TrimEnd("`n") -replace '\n', "`r`n") + "`r`n"

if ($Check) {
    if (!(Test-Path -LiteralPath $target) -or [IO.File]::ReadAllText($target) -cne $content) {
        throw 'THIRD-PARTY-NOTICES.txt is stale. Run scripts/update-notices.ps1 after changing dependencies.'
    }
} else {
    [IO.File]::WriteAllText($target, $content, $utf8)
}
