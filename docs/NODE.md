# Relay Node (relay-node)

`relay-node` is the forwarding daemon that runs on each relay server. It
listens on the ports you configured in the panel and forwards TCP/UDP traffic
to the target address. It also reports CPU / memory / disk / network / active
connections back to the panel over plain HTTP, so the panel can show live
node status.

This doc covers: **what the binaries are, how to install, how to configure,
how to update, how to uninstall, troubleshooting, and important notes.**

---

## Binaries

Each GitHub Release ships two pre-built static binaries (musl + rustls, no
glibc dependency, no external libraries needed):

| File | Architecture | Typical server |
|------|--------------|----------------|
| `relay-node-linux-amd64` | x86_64 | Most VPS / cloud / Intel / AMD servers |
| `relay-node-linux-arm64` | aarch64 | ARM VPS, Raspberry Pi 4, Apple Silicon (Linux VM) |

Pick the one matching your server's architecture:

```bash
uname -m
# x86_64   -> use relay-node-linux-amd64
# aarch64  -> use relay-node-linux-arm64
```

Both are downloaded automatically by the installer; you only need to choose
manually if you install by hand (see below).

> **Windows / macOS binaries are NOT provided.** The node runs on Linux only.
> The panel can run anywhere (Docker); the forwarding nodes must be Linux.

---

## Install

### Option A: One-line installer (recommended)

This is the supported path. It detects the architecture, downloads the right
binary from GitHub Releases, writes a systemd service, and starts it.

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
  -t <NODE_TOKEN> \
  -u https://your-panel.example.com
```

Get `<NODE_TOKEN>` from the panel UI: create an **inbound** device group,
copy its token.

Installer flags:

| Flag | Meaning | Default |
|------|---------|---------|
| `-t, --token` | Node token (required, from the panel UI) | - |
| `-u, --url` | Panel URL, e.g. `https://panel.example.com` (required) | - |
| `-s, --service-name` | systemd service name | `relay-node` |
| `-p, --proxy` | Proxy for the download, e.g. `socks5://127.0.0.1:10808` | none |

Run as **root** (or with `sudo`). The installer:
1. Detects arch (`uname -m`), picks `amd64` or `arm64`
2. Downloads `relay-node-linux-<arch>` into `/opt/relay-node/relay-node`
3. Writes `/opt/relay-node/start.sh` with your `PANEL_URL` + `NODE_TOKEN`
4. Writes `/etc/systemd/system/relay-node.service` and enables it
5. Starts the service

### Option B: Manual install

Use this if you cannot run the installer (no systemd, custom paths, air-gapped
server where you copy the binary over manually).

```bash
# 1. Download the right binary for your arch (replace with your release version)
ARCH=amd64   # or arm64
VERSION=1.0.3
curl -fL -o relay-node \
  "https://github.com/MoeShinX/relay-panel/releases/download/v${VERSION}/relay-node-linux-${ARCH}"

# 2. Make it executable and put it somewhere
chmod +x relay-node
sudo mkdir -p /opt/relay-node
sudo mv relay-node /opt/relay-node/relay-node
```

### Manual systemd setup

For production you should run it under systemd so it auto-starts on boot and
restarts on crash. Create these two files (mirror what the installer generates):

**`/opt/relay-node/start.sh`** — sets the env vars and launches the binary:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "/opt/relay-node"
export PANEL_URL="https://your-panel.example.com"   # <-- your panel URL
export NODE_TOKEN="your-node-token"                  # <-- token from the panel UI
export POLL_INTERVAL="${POLL_INTERVAL:-10}"
export RUST_LOG="${RUST_LOG:-info}"
exec ./relay-node
```

```bash
sudo chmod 700 /opt/relay-node/start.sh
```

**`/etc/systemd/system/relay-node.service`**:

```ini
[Unit]
Description=RelayNode forwarding service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/relay-node
ExecStart=/bin/bash /opt/relay-node/start.sh
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Then enable and start it:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now relay-node
systemctl status relay-node   # should show active (running)
```

---

## Configuration

The node is configured entirely via **environment variables** (no config file).
The installer writes them into `/opt/relay-node/start.sh`; if you run manually,
export them before launching.

| Variable | Meaning | Default |
|----------|---------|---------|
| `PANEL_URL` | Panel base URL, e.g. `https://panel.example.com` | `http://127.0.0.1:18888` |
| `NODE_TOKEN` | Node token from the panel's inbound group | `default-token` |
| `POLL_INTERVAL` | HTTP poll / status-report interval in seconds | `10` |
| `PUBLIC_IP_CHECK_URL` | URL used to detect the node's public egress IP | `https://api.ipify.org` |
| `NETWORK_INTERFACE` | NIC counted for whole-machine traffic stats; `auto` reads the default route | `auto` |
| `LISTEN_IPV4` | IPv4 inbound listen address for TCP/UDP rules; empty disables IPv4 | `0.0.0.0` |
| `LISTEN_IPV6` | IPv6 inbound listen address for TCP/UDP rules; empty disables IPv6 | `::` |
| `OUTBOUND_INTERFACE` | NIC for outbound IPv4 egress; `auto` = system routing (no source bind) | `auto` |
| `OUTBOUND_BIND_IPV4` | Exact IPv4 source for outbound TCP/UDP; overrides `OUTBOUND_INTERFACE` | (unset) |
| `RUST_LOG` | Log level: `error` / `warn` / `info` / `debug` | `info` |

Notes:
- `PANEL_URL` and `NODE_TOKEN` are **required** for the node to talk to your
  panel. Without them it falls back to defaults and won't authenticate.
- `PUBLIC_IP_CHECK_URL` is hit once at startup and then every 30 minutes (not
  every poll). Failure is non-fatal: the node just reports no public IP.
  Point this at your own echo service if you don't want to depend on ipify.
- `POLL_INTERVAL` controls both config polling AND status reporting. Lower =
  faster status updates but more HTTP traffic. 10s is a good default.
- `NETWORK_INTERFACE` selects which NIC the panel's "Machine Upload/Download"
  columns count. Default `auto` reads the default-route interface (typically
  eth0 / ens3 / venet0 / wg0), so only that NIC is counted instead of summing
  Docker bridges and veth pairs. For multi-NIC / policy-routing / special VPS,
  pin it explicitly, e.g. `NETWORK_INTERFACE=eth0`. The "NIC" column shows the
  selected interface. This is system-wide (includes SSH, system updates, …),
  NOT RelayPanel forwarded traffic.

### v1.0.5: IPv6 dual-stack + multi-NIC egress (TCP/UDP)

By default the node listens on **both** IPv4 (`0.0.0.0`) and IPv6 (`::`) for
every TCP/UDP rule, using separate sockets (the IPv6 socket is `IPV6_V6ONLY`
so it doesn't collide with the IPv4 socket on the same port). After startup,
`ss -lntup` shows both `0.0.0.0:<port>` and `[::]:<port>` for each rule.

- Disable one family by setting its variable empty: `LISTEN_IPV6=` → IPv4 only.
- If only one family binds (e.g. the host has no IPv6), the other keeps
  working and a warning is logged. The rule is only marked failed when **both**
  families fail to bind.

**Multi-NIC egress** — for servers where inbound (e.g. public IPv6 on `ens19`)
and outbound (e.g. IPv4 via `ens18`) use different interfaces, force the
outbound source so connections leave the right NIC:

```bash
# Option A: pick the egress NIC by name (node resolves its IPv4):
OUTBOUND_INTERFACE=ens18

# Option B: set the exact source IPv4 (use YOUR server's address):
OUTBOUND_BIND_IPV4=10.0.2.61
```

When neither is set (or `OUTBOUND_INTERFACE=auto`), the node does NOT bind a
source address and lets the OS route normally (backward compatible). A
misconfigured value (invalid IP, unknown NIC, non-local IP) is a **fatal
startup error** — the node refuses to boot rather than silently sending
traffic out the wrong interface. WS/TLS rules are IPv4-only and unaffected.


---

## Verify it's working

After install:

```bash
# 1. Version should print instantly and exit (does NOT start the service)
timeout 3 /opt/relay-node/relay-node --version
# expected: relay-node 1.0.3

# 2. Service status
systemctl status relay-node

# 3. Live logs
journalctl -u relay-node -f
```

In the logs you should see:
- `RelayNode 1.0.3 starting, panel=...`
- `websocket connected` (if your reverse proxy supports WS)
- `TCP listening on <port> (rule <id>)` / `UDP listening on ...` for each rule
- `report_traffic HTTP 200` (the per-report status line; the detailed per-cycle
  metrics are at `debug` level)

On the panel side, the node appears in **Node Status** with a green "online"
tag within ~30 seconds.

---

## Update

### Option A: one-click from the panel (recommended, no SSH)

**Panel → Node Status → find the node → click "Upgrade".**

The panel pushes an upgrade instruction to the node over WebSocket, and the node
fetches the new version from the official Release itself: sha256 verified, never
downgrades, backed up and atomically replaced, then it restarts itself. You never
log into the node.

When a node is behind, the node status page highlights that an upgrade is
available.

> **Upgrading drops the forwarding connections currently running on that node**
> — replacing the binary requires restarting the process. Pick a quiet window if
> the node is busy.

**It depends on how the node was installed** — self-upgrade relies on "the old
process exits, and something starts the new one":

| Install method | Node status page |
| --- | --- |
| **systemd** | Blue upgrade button; one-click when behind and online |
| **Docker** | Amber "update the image" hint — pull the latest `relay-panel-node` image and recreate the container |
| **Manual** (nohup / screen / running the binary directly) | Greyed out, "nothing restarts it after exit" |

For a node showing "manual run: one-click upgrade not supported", just re-run the
Option B install command on it — the script installs it as a systemd service, and
one-click works from then on.

### Option B: re-run the installer

Updating is the same one-line installer. **The simplest approach is to copy the
install command from the panel UI again and re-run it** — that command already
carries the correct `-t <NODE_TOKEN> -u <PANEL_URL>`, so you don't have to
remember them:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
  -t <NODE_TOKEN> \
  -u https://your-panel.example.com
```

> **You must pass `-t` and `-u` on every run** (including updates). The
> installer does NOT read the previous `start.sh`'s parameters — it requires
> them as arguments and regenerates `start.sh` from the values you pass. If
> either is missing the installer aborts with a clear error.

What happens on update:
1. Downloads the new binary to a temp file and verifies it
2. **Stops the running service** (so the old binary file can be replaced
   cleanly — avoids "Text file busy" on Linux)
3. Replaces the binary and rewrites `start.sh` with the `-t`/`-u` you passed
4. Restarts the service

Active forwarding rules are reloaded from the panel on the first sync after
restart (via WebSocket push or the 10s HTTP poll); if the panel is unreachable,
the node loads the last config from its local `config-cache.json` and keeps
forwarding.

To check the version after updating:

```bash
/opt/relay-node/relay-node --version
```

The version pulled is whatever `SCRIPT_VERSION` is compiled into the copy of
the script on `main`. To pin a specific older release instead, see
[Version pinning](#version-pinning) below.

To pin a specific older release instead of latest, download that version's
binary manually (see Option B) - the installer always pulls the version
compiled into it (`SCRIPT_VERSION` at the top of the script).

---

## Uninstall

```bash
systemctl disable --now relay-node
rm -f /etc/systemd/system/relay-node.service
systemctl daemon-reload
rm -rf /opt/relay-node
```

---

## Troubleshooting

### Node shows "offline" in the panel
- Check `systemctl status relay-node` is `active (running)`
- Check `PANEL_URL` is reachable from the node: `curl -sf $PANEL_URL/`
- Check `NODE_TOKEN` matches the panel's inbound group token
- Look for `report_status` errors in `journalctl -u relay-node`
- Online threshold: the panel marks a node offline if no status report arrives
  within 30 seconds (3x the default poll interval)

### `websocket error: ... sec-websocket-key ...`
Make sure you are on the latest version:
`/opt/relay-node/relay-node --version`. If it still happens, your reverse proxy
may not be passing the WebSocket Upgrade headers - but note WS is only the
control channel; forwarding + status reporting work over plain HTTP regardless.

### WebSocket keeps disconnecting / reconnecting every ~2 minutes
The node sends a 25s heartbeat Ping so the connection is not
treated as idle. If you still see periodic disconnects, your reverse proxy /
CDN may have an idle timeout shorter than 25s, or may not be forwarding
Pong frames. Occasional reconnects are harmless (config is re-synced), but if
it's frequent, check the proxy's WebSocket idle/timeout settings. Note: the
node keeps forwarding during any WS outage.

### Forwarding not working (can't connect to the listen port)
- Confirm the port is listening: `ss -tlnp | grep <port>` (TCP) /
  `ss -ulnp | grep <port>` (UDP)
- Check the server's firewall / cloud security group allows the port inbound
- Check the rule's target address is reachable from the node

### "Text file busy" when updating
This means the old binary was still running when the new one was moved into
place. The installer stops the service first to avoid this; if you hit it,
run `systemctl stop relay-node` manually, then re-run the installer.

### Connection count is always 0
This is normal if no clients are connected. TCP connections are counted on
accept/close; UDP sessions are counted per (client, rule) and expire after 60s
of inactivity. Generate real traffic and the count moves.

---

## Important notes

1. **Linux only.** No Windows/macOS node binary. The panel can run in Docker
   anywhere; forwarding nodes must be Linux.

2. **Run as root or via systemd.** Binding to ports below 1024 requires root
   or `CAP_NET_BIND_SERVICE`. The installer's systemd unit handles this.

3. **WebSocket is optional.** The node uses plain HTTP for both config polling
   and status reporting. WebSocket is only a real-time push channel for faster
   config updates. If your reverse proxy blocks WS, forwarding and status still
   work; you just lose instant config push (config syncs on the next poll,
   every `POLL_INTERVAL` seconds).

4. **Offline resilience.** If the panel is down, the node keeps forwarding
   using the last config it received (cached in `config-cache.json`). It does
   NOT stop existing listeners when the panel is unreachable. Status reports
   fail silently and resume when the panel returns.

5. **Don't edit the binary in place while running.** Linux will refuse with
   "Text file busy". Always stop the service first (the installer does this).

6. **Public IP detection uses an external service** (ipify by default). If
   your node has no outbound internet except to the panel, set
   `PUBLIC_IP_CHECK_URL` to your own IP-echo endpoint, or accept that public IP
   shows as "-" in the panel. This does NOT affect forwarding.

7. **Log levels.** The default `RUST_LOG=info` shows startup, connection,
   listener, and the per-report `report_traffic HTTP 200` status line. The
   detailed per-cycle metrics (`report_status: cpu=... mem=...`) and
   connection open/close events are at `debug`, so `info` does NOT flood the
   log on a healthy node. For an even quieter node set `RUST_LOG=warn`
   (only warnings/errors). Set `RUST_LOG=debug` only when diagnosing issues
   (it prints every status report + every connection open/close).

8. **Transport options.** Raw (plain TCP/UDP) is the default transport.

---

## Version pinning

The one-line installer (`scripts/relay-node-install.sh`) is always pulled from
the `main` branch, and it downloads the binary version compiled into itself
(the `SCRIPT_VERSION` at the top of the script). So `main`'s installer always
installs the **latest** release.

If you need to **pin a specific older version** (e.g. stay on a specific version while
testing), do NOT use the `main` installer — download that release's binary
directly and install by hand (see [Manual install](#option-b-manual-install)):

```bash
# Example: pin to a specific version on amd64
VERSION=1.0.3
ARCH=amd64   # or arm64
curl -fL -o relay-node \
  "https://github.com/MoeShinX/relay-panel/releases/download/v${VERSION}/relay-node-linux-${ARCH}"
```

Then follow the [Manual systemd setup](#manual-systemd-setup) to run it.

All released versions and their assets are listed on the
[Releases page](https://github.com/MoeShinX/relay-panel/releases).

---

## Token & connection security

`NODE_TOKEN` is a sensitive credential — anyone who obtains it can report traffic
and pull config as a node of that group. Protect it as follows.

### Strongly recommended: connect to the panel over HTTPS / WSS

In production, **always** point `PANEL_URL` at `https://` (behind a reverse
proxy; see [REVERSE-PROXY.md](./REVERSE-PROXY.md)).

- If `PANEL_URL` is `http://`, the node's traffic reports, config fetches, and
  the WebSocket control channel are **all in cleartext**. The `NODE_TOKEN`
  travels in the `Authorization: Bearer ...` header **in plaintext over the
  network** and any man-in-the-middle (compromised router, ISP, public Wi-Fi,
  packet capture) can grab it.
- Over HTTPS / WSS the token and data are encrypted in transit — this is the
  minimum bar.
- Whether the node's forwarded listener traffic is encrypted depends on the
  rule's transport (raw/ws/tls), NOT on the panel connection. This section is
  about the **node ↔ panel** control channel only.

### Handling the token

- Don't put the token in a URL — it would leak into access logs, shell history,
  browser history, and screenshots. This project reads the token only from the
  `Authorization` header, but when *you* run the install command the
  `-t <TOKEN>` argument lands in your shell history — avoid pasting it in
  untrusted environments, or clear that history afterward.
- Don't screenshot or paste the install command or `start.sh` (they contain the
  token) into tickets, chats, or issues.
- If your reverse proxy / panel access logs record full request headers, they
  also record `Authorization` — disable or redact that field's logging in
  production.

### What to do if a token leaks

- **Rotate immediately**: regenerate the group's token in the panel's "Device
  Groups" page (the old token becomes invalid).
- **Shared within a group**: today all nodes in one inbound group share a single
  token, so rotation means **every node in that group** must be reconfigured
  with the new token (re-run the one-line script, or edit `start.sh` and
  `systemctl restart`). This is why a leak has a wide blast radius — consider
  one group per physical node so rotation stays minimal.
- Rotation only invalidates the old token; traffic already reported is not
  rolled back.

### Trust model (read this)

- The panel **trusts metering data reported by nodes it controls** (traffic,
  connections, status). The panel cannot independently verify that a given
  connection actually happened — it can only record numbers reported by a node
  holding a valid token. A compromised node can under-report, over-report, or
  fabricate traffic figures.
- So protecting the token is equivalent to "who can influence your billing and
  quotas." Don't hand it to untrusted people or deploy it on untrusted machines.
  Quotas are a **soft limit** that relies on honest node reporting.

---

## See also
- [NODE.zh-CN.md](./NODE.zh-CN.md) - 中文版本文档 (Chinese version of this doc)
- [DEPLOYMENT.md](./DEPLOYMENT.md) - deploying the panel itself (Docker Compose)
- [../README.md](../README.md) - project overview
- [../CHANGELOG.md](../CHANGELOG.md) - version history
