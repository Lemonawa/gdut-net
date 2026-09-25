# clash-campus-dns.ps1 - toggle the campus-DNS block in Clash Verge's Merge.yaml.
#
# Why: the block points at campus resolvers (10.1.3.38, 222.200.115.x). Away from
# campus they are unreachable, and mihomo queries a nameserver-policy list
# SEQUENTIALLY: one dead server costs a full ~5s timeout per lookup before
# fallback (measured 2026-09-25 at home: www.qq.com 5.0s, SERVFAIL on raw UDP to
# 127.0.0.1:1053). So home mode turns the block off, campus mode turns it back on.
#
# Mechanics: the block sits between "# CAMPUS-DNS-BEGIN" / "# CAMPUS-DNS-END"
# markers in Merge.yaml. "off" prefixes every code line inside with #OFF# (YAML
# comment), "on" strips that prefix. Existing comments are left alone and the
# operation is idempotent. Verge only re-merges on a full restart, so the change
# takes effect the next time Clash Verge starts.
#
# Usage: clash-campus-dns.ps1 on|off [-Path <Merge.yaml>]
# Exit: 0 ok / no-op, 1 usage or marker-sanity failure (file left untouched).

param(
    [Parameter(Mandatory = $true)][ValidateSet('on', 'off')][string]$Mode,
    [string]$Path
)

if (-not $Path) {
    $Path = Join-Path $env:APPDATA 'io.github.clash-verge-rev.clash-verge-rev\profiles\Merge.yaml'
}
if (-not (Test-Path -LiteralPath $Path)) {
    exit 0   # Clash Verge not installed / no merge profile: nothing to do.
}

$raw = [IO.File]::ReadAllBytes($Path)
$hasBom = ($raw.Length -ge 3 -and $raw[0] -eq 0xEF -and $raw[1] -eq 0xBB -and $raw[2] -eq 0xBF)
# WinPS 5.1 Get-Content decodes UTF-8-without-BOM as ANSI (GBK on zh-CN) and
# destroys the Chinese comments - always read/write bytes with an explicit codec.
$text = [Text.Encoding]::UTF8.GetString($raw)
if ($hasBom) { $text = $text.Substring(1) }
$nl = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }

$out = New-Object 'System.Collections.Generic.List[string]'
$inside = $false
$changed = 0
foreach ($line in ($text -split "`r?`n")) {
    $isBegin = $line -match '^\s*#\s*CAMPUS-DNS-BEGIN'
    $isEnd = $line -match '^\s*#\s*CAMPUS-DNS-END'
    if ($isEnd) { $inside = $false }
    if ($inside -and -not $isBegin -and -not $isEnd) {
        $body = $line.TrimStart()
        if ($Mode -eq 'off' -and $body -ne '' -and -not $body.StartsWith('#')) {
            $line = '#OFF#' + $line
            $changed++
        } elseif ($Mode -eq 'on' -and $body.StartsWith('#OFF#')) {
            $line = $line.Replace('#OFF#', '')
            $changed++
        }
    }
    if ($isBegin) { $inside = $true }
    $out.Add($line)
}

$newText = ($out -join $nl)
if ($hasBom) { $newText = [string][char]0xFEFF + $newText }
if ($newText -eq $text) { exit 0 }   # already in the requested state
if ($newText -notmatch '(?m)^\s*#\s*CAMPUS-DNS-BEGIN' -or $newText -notmatch '(?m)^\s*#\s*CAMPUS-DNS-END') {
    Write-Error 'ABORT: CAMPUS-DNS markers lost, Merge.yaml untouched'
    exit 1
}
$tmp = "$Path.tmp-toggle"
[IO.File]::WriteAllText($tmp, $newText, (New-Object Text.UTF8Encoding $false))
Move-Item -Force $tmp $Path
exit 0
