# Fallback plan: self-hosted derper on SG VPS (region 900 "sg-selfhost")

> Fallback for `docs/tailscale-p2p.md`: campus↔Mac stuck on DERP den 440ms. SG VPS
> `workload-speedypage-sg` (45.139.226.189, linux/amd64, root) has ~195/250ms to both peers.
> The VPS's own tailscaled (UDP 41641) is never touched — derper uses 443/tcp + 3478/udp.

## 0. Coexistence & ports (verified vs `cmd/derper` flags, 2026)
- derper defaults: `-a :443` (DERP/HTTPS), `-http-port 80` (LE redirects only; `-1` disables),
  `-stun-port 3478/udp`. tailscaled binds 41641/udp → zero overlap. `--verify-clients` needs
  the local tailscaled socket (`-socket` if non-default) — it exists on this VPS.
- TLS is served iff port==443 **or** `-certmode manual`. Bare IP + no DNS name → no Let's
  Encrypt: run `manual` with a self-signed cert, pinned client-side by SHA-256 (below).
- If 443 busy: `-a :8443` + `"DERPPort": 8443` in the node (manual mode = TLS on any port).

## 1. Prerequisites (VPS, read-only)
```bash
ss -tulpn | grep -E ':(80|443|3478)[[:space:]]'   # free? 443 busy → use 8443 variant
command -v docker && docker info >/dev/null && echo docker-ok   # else Option B
systemctl is-active tailscaled && ls -l /var/run/tailscale/tailscaled.sock
```
Cloud panel: also open 443/tcp + 3478/udp in the provider security group (if any).

## 2. Cert (self-signed, 10y, SAN = IP)
```bash
mkdir -p /var/lib/derper/certs && cd /var/lib/derper
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
  -keyout certs/45.139.226.189.key -out certs/45.139.226.189.crt \
  -subj "/CN=45.139.226.189" -addext "subjectAltName=IP:45.139.226.189"
openssl x509 -in certs/45.139.226.189.crt -noout -fingerprint -sha256 | cut -d= -f2 | tr -d : > cert.sha256
```

## 3. Run — Option A: docker (self-built, no third-party image)
```bash
cat > Dockerfile <<'EOF'
FROM golang:1.25-alpine AS build
RUN apk add --no-cache git && go install tailscale.com/cmd/derper@v1.90.0
FROM alpine
COPY --from=build /go/bin/derper /usr/local/bin/derper
ENTRYPOINT ["/usr/local/bin/derper"]
EOF
# pin latest stable tag from github.com/tailscale/tailscale/releases
docker build -t derper:local . && docker run -d --name derper --restart unless-stopped \
  -p 443:443/tcp -p 3478:3478/udp -v /var/lib/derper:/var/lib/derper \
  -v /var/run/tailscale:/var/run/tailscale \
  derper:local -a :443 -stun -stun-port 3478 -http-port -1 -certmode manual \
  -certdir /var/lib/derper/certs -hostname 45.139.226.189 \
  -c /var/lib/derper/derper.key -verify-clients
docker logs derper   # expect "serving on :443 with TLS", no cert errors
```

## 3'. Run — Option B: bare binary (no docker)
```bash
# install Go >= 1.25 from go.dev/dl, then:
GOBIN=/usr/local/bin go install tailscale.com/cmd/derper@v1.90.0
# systemd unit ExecStart (same flags as Option A). Persist:
#   [Unit] After=network-online.target
#   [Service] ExecStart=/usr/local/bin/derper -a :443 -stun -http-port -1 -certmode manual \
#     -certdir /var/lib/derper/certs -hostname 45.139.226.189 \
#     -c /var/lib/derper/derper.key -verify-clients ; Restart=always
#   [Install] WantedBy=multi-user.target   → systemctl enable --now derper
```

## 4. Firewall & exposure (public workload)
```bash
ufw allow 443/tcp comment derper-derp && ufw allow 3478/udp comment derper-stun
```
- `-verify-clients` = only OUR tailnet's keys pass (checked via local tailscaled); without it
  anyone who finds the IP can relay through the VPS (bandwidth abuse). Keep it on.
- Content stays WireGuard-encrypted E2E — derper sees ciphertext only. No new inbound surface
  for the existing workload; 443/3478 are fresh, narrow, reversible rules.

## 5. DERPMap — tailnet policy file (admin console → Policies; HuJSON ok, all plans incl. free)
`OmitDefaultRegions` stays **false** (official DERPs remain as fallback). `HostName` must not
be a bare IP; use a `.invalid` name (no DNS lookup happens — `IPv4` forces the dial addr) and
pin the self-signed cert. `CERTSHA256` = contents of `cert.sha256` (lowercase hex, no colons):
```jsonc
"derpMap": { "Regions": { "900": {
  "RegionID": 900, "RegionCode": "sg-selfhost", "RegionName": "Singapore selfhost",
  "Nodes": [{ "Name": "900a", "RegionID": 900, "HostName": "sg.invalid",
    "IPv4": "45.139.226.189", "CertName": "sha256-raw:CERTSHA256", "STUNPort": 3478 }]
}}}
```
Clients pick it up automatically on policy save (no restart/re-login). Both peers measure
~200-250ms < den's 440ms → both home to 900 with no score tuning. Caveat: custom DERPs don't
do device sharing/cross-tailnet features (irrelevant here).

## 6. Verify (campus + Mac)
```bash
tailscale netcheck                       # region 900 row with sane latency
tailscale ping <peer-ts-dns-name>        # expect: via DERP(sg-selfhost) in ~200-300ms
tailscale status --json | jq -r '.Peer[] | "\(.HostName) relay=\(.Relay)"'
# on VPS: curl -sk https://127.0.0.1/derp/probe -o /dev/null -w '%{http_code}\n'  → 200
```

## 7. Rollback (self-contained)
1. Admin console → policy file: delete the whole `"derpMap"` block → Save. Clients fall back
   to official DERP map within minutes (relay goes back to den). No client action needed.
2. VPS: `docker rm -f derper` (or `systemctl disable --now derper`); close ports:
   `ufw delete allow 443/tcp; ufw delete allow 3478/udp` (+ cloud group). `rm -rf /var/lib/derper`.
3. VPS tailscaled + workload: untouched throughout — nothing to restore.
