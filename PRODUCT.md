# Product

<!-- impeccable:product-schema 1 -->

## Platform

windows-desktop

## Users

Primary users are Guangdong University of Technology students on the Higher Education Mega Center campus — starting with the author — who need the dorm wired network (PPPoE, 统一身份认证 / Dr.COM account) to stay authenticated and online without babysitting it. They run Windows with a proxy TUN (Clash / Mihomo) and/or Tailscale (wintun) stack active, and treat the campus connection as one tenant of that machine, not the whole of it. A second audience is other GDUT students installing from the public README and release zip on their own machines, with no access to the author's personal deployment scripts.

## Product Purpose

gdut-net replaces the official Dr.COM client with a single Rust binary running as a Windows service: it dials the campus PPPoE session, keeps it alive, redials drops with exponential backoff, and — when the wired link goes down — takes over the campus WiFi (eportal web auth). An optional Dr.COM heartbeat compat mode covers servers that validate keepalives. Success means the user never has to think about the network: no manual redial, no client-induced drops, no interference with the rest of the network stack, and a truthful status surface (tray / CLI) whenever something is wrong.

## Positioning

Mechanisms a neighboring client could not truthfully copy:

- One Rust binary split into a SYSTEM service (dial, probe, wireless) plus a user-session tray process communicating JSON-line over a named pipe (ADR-0001) — the daemon needs no UI and no desktop session.
- Every dial, probe, heartbeat, and portal packet is explicitly bound to the physical NIC, so sessions coexist with TUN/wintun virtual adapters (Clash, Tailscale) and survive route shifts. No LSP, driver, or WinPcap injection.
- Drops are judged by traffic probes (gateway ICMP, then HTTP re-check; two consecutive failures), not by `RASCS_Connected` (ADR-0003), so sessions that look connected but are kicked get caught.
- Wireless takeover is a managed state machine with two modes (`wired_exclusive` / `wired_plus_standby`), eportal login bound to the WLAN source IP, /32 host routes, and metric suppression for a clean release (ADR-0005).
- Clean-room Dr.COM heartbeat implemented from a capture spec; no code from the GPL/AGPL community clients (ADR-0002).

## Operating Context

- Windows 10/11. Install is an admin PowerShell step (`gdut-net.exe install`), which prompts for the campus password; the README and release zip are the distribution channel.
- The service auto-starts with Windows and runs headless (SYSTEM, Session 0); the tray process is registered per-user at logon and can be quit independently of the service.
- Configuration is TOML at `C:\ProgramData\gdut-net\config.toml`; logs rotate by size in the same directory; `log.event_log = true` mirrors warn/error into Event Viewer.
- Real-device acceptance is a documented ritual: `docs/acceptance.md` covers install, TUN coexistence, memory, 72-hour soak, clean uninstall, and wireless takeover. Field findings are recorded in `CONTEXT.md` (2026-09-10 real-machine session).
- Scope: the Windows client (service + tray + CLI) is the product. The personal desktop kit (one-click switch scripts, `docs/desktop-kit.md`) is supporting ops, not a product surface.

## Capabilities and Constraints

Capabilities: idempotent install / uninstall (`--purge` guarded); RAS phone-book entry `gdut`; redial backoff 1s → 300s cap, reset after 300s stable, auth failure 691 pinned at 600s; optional heartbeat (UDP 61440) default off; wireless takeover in exclusive or standby mode with eportal auth and automatic release; CLI `status` / `tray` / `wireless` commands; system toast notifications; named-pipe state snapshots feeding the tray UI.

Binding constraints:

- Never scan or touch virtual adapters. Dial/probe/heartbeat/portal traffic binds the physical NIC (or the WLAN source IP during takeover).
- Never dial while the link is down: link gate, plug-in redial trigger, and RasMan recovery after repeated 756/813.
- Heartbeat stays clean-room from captures and off by default; if the official client owns UDP 61440, fail loudly rather than degrade silently.
- The password is DPAPI machine-scope ciphertext (`password_blob`); portal URLs containing the password never reach logs (host + path only).
- Every operation that changes networking ships a self-contained automatic rollback; no AI reachability is assumed during the user's outage window.
- All user-facing output (CLI, logs, scripts, `.bat` / `.ps1` echo) is English: Chinese text garbles in GBK Windows consoles.
- Domain vocabulary and live field rules live in `CONTEXT.md`; decisions live in `docs/adr/`.

## Brand Commitments

- Name: `gdut-net`; toast / AUMID display name `GDUT Net`.
- Identity: third-party replacement for the official client; contains no official client code; WTFPL license.
- Voice: factual, English-only user-facing text; errors name the failure and never degrade silently.

## Evidence on Hand

- `README.md` (product, install, config, CLI docs), `docs/acceptance.md` (on-device checklist), `docs/adr/0001`–`0006` (decision records), `CONTEXT.md` (domain vocabulary + field-tested rules), `docs/superpowers/{plans,specs}` (design history).
- CI (`.github/workflows/ci.yml`, `release.yml`) and GitHub releases through v0.3.0.
- Absent (do not fabricate): product screenshots or marketing imagery, logo / brand asset files (the tray icon is generated in code), testimonials, benchmarks, telemetry, third-party user data.

## Product Principles

1. Unattended reliability first — the connection must stay up while nobody watches: self-healing redial, probe-based truth, wireless fallback.
2. Coexist with the machine's network stack — proxy TUNs and VPNs are part of the environment, not enemies; nothing may destabilize adapters or routes outside the campus link.
3. Truthful, visible state — probe-based drop detection, semantic tray state, logged errors; risky features stay opt-in and failures are never silent.
4. Safety by construction — credentials encrypted, secrets never logged, network-changing actions self-contained with rollback, uninstall leaves no residue.
5. Stranger-installable — a GDUT student with only the README and release zip can install, run, and recover without reading source or touching the author's machine.
