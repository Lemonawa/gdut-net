@echo off
rem Show Windows system-proxy state. Expect ProxyEnable=0 (off).
powershell -NoProfile -Command "$p=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'; Write-Host ('ProxyEnable=' + $p.ProxyEnable + '  ProxyServer=' + $p.ProxyServer)"
pause
