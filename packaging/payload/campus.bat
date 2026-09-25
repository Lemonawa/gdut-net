@echo off
rem Campus mode: set the gdut-net service to Automatic, start it, wait for
rem dial success (max 180s), run a 30s stability check, then bring the tray
rem back (home mode exits it and removes its autostart entry) and re-enable the
rem Clash campus-DNS policies (home mode turns them off).
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
rem Clash Verge: campus resolvers matter here again (Tencent AAAA suppression,
rem campus-only records). Takes effect on the next Clash Verge restart.
if exist "%~dp0clash-campus-dns.ps1" powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0clash-campus-dns.ps1" on >nul 2>&1
rem Home mode removed the tray autostart; write the same value the installer
rem uses (tray::register_autostart = quoted exe path + " tray").
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v gdut-net-tray /t REG_SZ /d "\"%~dp0gdut-net.exe\" tray" /f >nul
rem Then bring the tray back. explorer de-elevates (this script is elevated)
rem so the tray stays a normal-user session process, not admin. No-arg start =
rem tray + daily window; an existing tray just wakes its window.
start "" explorer.exe "%~dp0gdut-net.exe"
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
