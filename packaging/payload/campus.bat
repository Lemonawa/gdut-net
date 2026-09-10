@echo off
rem Campus mode: set the gdut-net service to Automatic, start it, wait for
rem dial success (max 180s), then run a 30s stability check.
rem Run as Administrator. Self-contained rollback at the bottom.

net session >nul 2>&1
if errorlevel 1 (
  echo NOT ADMIN: right-click and run as Administrator.
  pause
  exit /b 1
)

sc config gdut-net start=auto >nul
net start gdut-net >nul 2>&1

echo Waiting for dial success (max 180s)...
for /L %%i in (1,1,90) do (
  timeout /t 2 /nobreak >nul
  powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 5 -Encoding UTF8 | Select-String 'Dial succeeded'" | findstr Dial >nul
  if not errorlevel 1 goto :dialed
)
goto :rollback

:dialed
echo DIAL OK, stability check 30s...
timeout /t 30 /nobreak >nul
powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 10 -Encoding UTF8 | Select-String 'considered dropped|Probe failed'" | findstr "dropped failed" >nul
if not errorlevel 1 goto :rollback
echo CAMPUS MODE OK:
"%~dp0gdut-net.exe" status
pause
exit /b 0

:rollback
echo DIAL FAILED, rolling back...
net stop gdut-net >nul 2>&1
sc config gdut-net start=demand >nul
echo ROLLBACK DONE: service stopped and set to Manual.
echo If the campus wire needs the official client, use the full switch as Administrator.
pause
exit /b 1
