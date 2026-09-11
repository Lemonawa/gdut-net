# gdut-net switch v5 -- migrate Desktop kit into Program Files + full switch.
# Runs elevated via the pre-authorized scheduled task gdut-switch.
# Phases: A. silent install (keep password) + personal scripts;
#         B. repoint task to the installed script; C. dial + 75s stability;
#         D. any failure -> service back to the Desktop kit.

$ErrorActionPreference = 'Continue'
$desktop = 'C:\Users\Lemonawa\Desktop\gdut-net'
$install = 'C:\Program Files\gdut-net'
$log     = 'C:\ProgramData\gdut-net\logs\switch-v4.log'
$gdutLog = 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log'
function Log($m) { "$(Get-Date -Format 'MM-dd HH:mm:ss') $m" | Out-File $log -Append }
function RollbackToDesktop() {
  Log ">>> Rollback: restoring the Desktop gdut-net build"
  Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
  Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  # The service may point at the install dir (setup got that far) or still at the
  # Desktop path. In both cases putting the Desktop exe at the install path and
  # starting the service restores the previous working build.
  if (Test-Path "$install\gdut-net.exe") {
    Copy-Item "$desktop\gdut-net.exe" "$install\gdut-net.exe" -Force
    Log "Copied the Desktop build over the installed exe"
  }
  sc.exe start gdut-net | Out-Null
  Log "Rollback done: service started with the Desktop build"
}

"=== migrate+switch $(Get-Date) ===" | Out-File $log

Log "A. Stop service + tray"
Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Log "A. Silent install into $install (keep existing password)"
$setup = Join-Path $desktop 'gdut-net-setup.exe'
if (-not (Test-Path $setup)) { Log "missing $setup"; exit 1 }
# GUI-subsystem exe: plain cmd redirect loses output; PowerShell pipeline capture is
# the field-verified pattern (see wireless-test.bat in the desktop kit).
$setupOut = & $setup --silent --keep-password 2>&1 | Out-String
$setupCode = $LASTEXITCODE
Log "setup output: $($setupOut.Trim())"
Log "setup exit=$setupCode"
if ($setupCode -ne 0) { Log "install failed"; RollbackToDesktop; exit 1 }

Log "A. Copy personal ops scripts + backup exe"
$personal = Join-Path $desktop 'personal'
if (Test-Path $personal) { Copy-Item (Join-Path $personal '*') $install -Force }
if (Test-Path (Join-Path $desktop 'gdut-net-bak.exe')) { Copy-Item (Join-Path $desktop 'gdut-net-bak.exe') (Join-Path $install 'gdut-net-bak.exe') -Force }

Log "B. Repoint the gdut-switch task to the installed script"
$taskArg = '-NoProfile -ExecutionPolicy Bypass -File "' + $install + '\switch-v4.ps1"'
Set-ScheduledTask -TaskName 'gdut-switch' -Action (New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $taskArg) | Out-Null
$q = schtasks /Query /TN gdut-switch /V /FO LIST | Out-String
if ($q -notmatch [regex]::Escape("$install\switch-v4.ps1")) { Log "WARN: task repoint verify failed (check schtasks /Query /TN gdut-switch)" }
else { Log "Task repointed to $install\switch-v4.ps1" }

Log "C. Wait for dial success (max 180s)"
$ok = $false
for ($i = 0; $i -lt 180; $i++) {
  Start-Sleep -Seconds 1
  $recent = Get-Content $gdutLog -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'Dial succeeded') { $ok = $true; break }
}
if (-not $ok) { Log "no dial in 180s"; Get-Content $gdutLog -Tail 5 -Encoding UTF8 | Out-File $log -Append; RollbackToDesktop; exit 1 }
Log "Dial succeeded"

Log "C. Stability check 75s"
for ($i = 0; $i -lt 25; $i++) {
  Start-Sleep -Seconds 3
  $recent = Get-Content $gdutLog -Tail 4 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'considered dropped|Probe failed') { Log "unstable"; RollbackToDesktop; exit 1 }
}
$http = curl.exe -s -m 8 -o NUL -w '%{http_code}' http://www.gstatic.com/generate_204 2>&1
Log "SUCCESS: migrated to $install, service stable (http=$http)"
exit 0
