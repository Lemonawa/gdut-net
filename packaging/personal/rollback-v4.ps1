# One-click rollback inside Program Files: restore the OLD gdut-net exe
# (not Dr.COM) + restart the service. Self-contained, works offline.
$ErrorActionPreference = 'Continue'
$dir = 'C:\Program Files\gdut-net'
$log = 'C:\ProgramData\gdut-net\logs\rollback-v4.log'
function Log($m) { "$(Get-Date -Format 'MM-dd HH:mm:ss') $m" | Out-File $log -Append }
"=== rollback to old gdut-net $(Get-Date) ===" | Out-File $log

Log "Stopping gdut-net + killing Dr.COM (release PPP port)"
Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name DrMain,DrClient,DrUpdate,DrTray -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 800
rasdial 'Dr.COM' /d 2>$null
rasdial 'gdut' /d 2>$null
Start-Sleep -Seconds 2

Log "Restoring gdut-net-bak.exe -> gdut-net.exe"
Copy-Item (Join-Path $dir 'gdut-net-bak.exe') (Join-Path $dir 'gdut-net.exe') -Force

Log "Starting service"
sc.exe start gdut-net | Out-Null
$ok = $false
for ($i = 0; $i -lt 90; $i++) {
  Start-Sleep -Seconds 2
  $recent = Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'Dial succeeded') { $ok = $true; break }
}
if ($ok) { Log "ROLLBACK OK: old gdut-net online" } else { Log "No dial in 180s - last resort: C:\Drcom\DrUpdateClient\DrMain.exe" }
