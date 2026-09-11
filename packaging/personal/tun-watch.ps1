# tun-watch.ps1 - TUN blackout flight recorder (read-only, logs to tun-watch.log).
# Usage: open Verge + TUN, run this in PowerShell, wait until net dies,
# close Verge, paste the TAIL of tun-watch.log.
$log = "$PSScriptRoot\tun-watch.log"
"=== tun-watch started $(Get-Date -Format 'HH:mm:ss') ===" | Out-File $log
for ($i = 0; $i -lt 40; $i++) {
  $t = Get-Date -Format 'HH:mm:ss'
  $proxy = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings').ProxyEnable
  $core = (Get-Process verge-mihomo -EA SilentlyContinue | Select-Object -ExpandProperty Id) -join ','
  if (-not $core) { $core = 'dead' }
  $port = (Test-NetConnection -ComputerName 127.0.0.1 -Port 7890 -WarningAction SilentlyContinue).TcpTestSucceeded
  $tun = (Get-NetAdapter | Where-Object { $_.Name -match 'Mihomo' } | Select-Object -ExpandProperty Status) -join ','
  if (-not $tun) { $tun = 'gone' }
  $def = (Get-NetRoute -DestinationPrefix '0.0.0.0/0' -EA SilentlyContinue | ForEach-Object { "$($_.ifIndex):$($_.NextHop)/$($_.RouteMetric)" }) -join ' '
  $dns = (Resolve-DnsName mirrors.gdut.edu.cn -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty IPAddress)
  $web = 'skip'
  try { $web = (Invoke-WebRequest -Uri http://example.com -UseBasicParsing -TimeoutSec 8).StatusCode } catch { $web = 'FAIL' }
  "$t proxy=$proxy core=$core port7890=$port tun=$tun dns_mirror=$dns web=$web routes=[$def]" | Out-File $log -Append
  Start-Sleep 15
}
"=== tun-watch finished ===" | Out-File $log -Append
