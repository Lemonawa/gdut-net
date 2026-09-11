@echo off
schtasks /Run /TN gdut-switch
echo Triggered. Check switch-v4.log for progress.
echo.
echo NOTE: a second window will appear while the script runs.
echo       Do NOT close it - closing kills the script mid-way.
echo.
timeout /t 3 /nobreak >nul
powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\switch-v4.log' -Tail 20"
pause
