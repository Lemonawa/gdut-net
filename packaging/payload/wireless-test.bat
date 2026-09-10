@echo off
rem Field-check the campus portal login (one-shot; auto-disconnects WLAN when done).
rem Needs admin: the temporary /32 route add fails otherwise.
rem Usage: right-click this file -> "Run as administrator".
setlocal
cd /d "%~dp0"

net session >nul 2>&1
if %errorlevel% neq 0 (
  echo [!] Not elevated - the /32 route cannot be added and the test will fail.
  echo     Right-click this file and choose "Run as administrator".
  pause
  exit /b 1
)

echo Running wireless test (joins gdut, one portal login, then disconnects)...
echo This takes about 20-40 seconds. Do not close this window.
echo.

rem Capture via PowerShell: a windows-subsystem exe prints nothing to a plain
rem cmd invocation, but PowerShell pipeline capture is verified to work.
powershell -NoProfile -Command "$o = & '.\gdut-net.exe' wireless test 2>&1 | Out-String; [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Write-Output $o"

echo.
echo Expected: "RESULT: SUCCESS". Anything else - see the README troubleshooting section.
pause
