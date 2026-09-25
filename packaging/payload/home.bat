@echo off
rem Home mode: campus PPPoE does not exist at home, so stop gdut-net
rem (avoids endless redial + toast spam) and set the service to Manual.
rem The tray process is stopped and its autostart entry (HKCU Run) removed,
rem otherwise it would pop back up at the next logon. Campus mode restores
rem both. The Clash campus-DNS policies are disabled too (unreachable here,
rem ~5s per lookup); campus mode re-enables them.
rem Proxy is left untouched (you may need Clash at home).
rem Run as Administrator. Self-contained rollback at the bottom.

net session >nul 2>&1
if errorlevel 1 (
  echo NOT ADMIN: right-click and run as Administrator.
  pause
  exit /b 1
)

net stop gdut-net >nul 2>&1
sc config gdut-net start=demand >nul
taskkill /F /IM gdut-net.exe >nul 2>&1
rem Keep the tray from coming back at next logon (campus mode re-adds it).
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v gdut-net-tray /f >nul 2>&1
rem Clash Verge: stop pointing DNS at campus resolvers (takes effect on next Verge start).
if exist "%~dp0clash-campus-dns.ps1" powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0clash-campus-dns.ps1" off >nul 2>&1

sc query gdut-net | findstr /C:"STOPPED" >nul
if errorlevel 1 goto :rollback
echo HOME MODE OK: service stopped and set to Manual.
echo Tray stopped and autostart removed (campus mode restores both).
echo Clash campus-DNS policies disabled (restart Clash Verge to apply).
echo At home your normal network works, gdut-net stays quiet.
pause
exit /b 0

:rollback
echo HOME SWITCH FAILED, rolling back to campus state...
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v gdut-net-tray /t REG_SZ /d "\"%~dp0gdut-net.exe\" tray" /f >nul
sc config gdut-net start=auto >nul
net start gdut-net >nul 2>&1
echo ROLLBACK DONE: service restored to Automatic and started.
pause
exit /b 1
