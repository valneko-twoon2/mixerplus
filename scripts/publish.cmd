@echo off
pwsh -NoProfile -File "%~dp0publish.ps1"
exit /b %ERRORLEVEL%
