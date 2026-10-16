@echo off
pwsh -NoProfile -File "%~dp0set-version.ps1" %*
exit /b %ERRORLEVEL%
