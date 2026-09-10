@echo off
rem Home mode: campus PPPoE does not exist at home, so stop gdut-net
rem (avoids endless redial + toast spam) and set the service to Manual.
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

sc query gdut-net | findstr /C:"STOPPED" >nul
if errorlevel 1 goto :rollback
echo HOME MODE OK: service stopped and set to Manual.
echo At home your normal network works, gdut-net stays quiet.
pause
exit /b 0

:rollback
echo HOME SWITCH FAILED, rolling back to campus state...
sc config gdut-net start=auto >nul
net start gdut-net >nul 2>&1
echo ROLLBACK DONE: service restored to Automatic and started.
pause
exit /b 1
