param([switch]$Fix)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $paths = @(git -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate repository files.' }
    $utf8 = [Text.UTF8Encoding]::new($false, $true)
    foreach ($relative in $paths | Sort-Object -Unique) {
        $path = Join-Path $root $relative
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) { continue }
        $bytes = [IO.File]::ReadAllBytes($path)
        if ($bytes -contains 0) { continue }
        try { $content = $utf8.GetString($bytes) } catch { throw "Text must be UTF-8: $relative" }
        $newline = if ($relative.StartsWith('.githooks/')) { "`n" } else { "`r`n" }
        $normalized = ($content -replace '\r\n|\r|\n', $newline)
        if ($normalized -cne $content) {
            if (!$Fix) { throw "Invalid line endings: $relative. Run scripts/check-line-endings.ps1 -Fix." }
            [IO.File]::WriteAllText($path, $normalized, $utf8)
        }
    }
} finally { Pop-Location }
