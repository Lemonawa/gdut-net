@echo off
echo Rolling back to old gdut-net ...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0rollback-v4.ps1"
echo Done. Log: C:\ProgramData\gdut-net\logs\rollback-v4.log
pause
