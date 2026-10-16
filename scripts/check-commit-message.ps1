param([Parameter(Mandatory)][string]$MessageFile)
$ErrorActionPreference = 'Stop'
$message = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $MessageFile))
$lines = @($message -split '\r?\n' | Where-Object { $_ -notmatch '^#' })
$subject = $lines[0]
if ($message -match '[^\x09\x0A\x0D\x20-\x7E]') { throw 'Commit messages must use English ASCII text.' }
if ($message -match '(?im)^\s*co-authored-by\s*:') { throw 'Do not add Co-Authored-By trailers to commits.' }
if ($subject -notmatch '^(feat|fix|refactor|chore|docs|test|build|ci|perf|revert)(\([a-z0-9-]+\))?!?: [A-Za-z].+' -or $subject.Length -gt 100) {
    throw 'Use an English Conventional Commit subject of at most 100 characters.'
}
