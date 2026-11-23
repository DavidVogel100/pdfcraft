@echo off
rem Writes the mates of the assembly open in SOLIDWORKS to <assembly>.mates.json.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0export-mates.ps1" %*
echo.
pause
