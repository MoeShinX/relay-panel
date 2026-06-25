# Deployment

RelayPanel targets **Linux (Debian 11 / 12 / 13)** and is deployed via
**Docker Compose**. There is no bare-metal deployment path — Docker is the
only supported runtime.

> **Windows binaries cannot be used on Linux.** Always build/run inside Docker
> or on the target Linux host. The Docker images bundle their own glibc, so a
> single `docker compose up` works identically on Debian 11, 12, and 13.

---

## One-line install

The easiest path — a single command that installs `git`/`curl`/`ca-certificates`/`openssl`,
clones the repo into `/opt/relay-panel`, then runs `deploy.sh` (which installs
Docker, generates secrets, builds, and verifies):

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

Run as root (default on most VPS / cloud servers). If logged in as a non-root
user, prefix with `sudo`. Debian / Ubuntu only.
The script refuses to overwrite a non-git directory at `/opt/relay-panel`.

---

## Prerequisites

Install Docker Engine + Compose plugin on Debian:

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER   # log out/in to apply
docker --version                # verify
docker compose version          # verify
```

This works on Debian 11 (bullseye), 12 (bookworm), and 13 (trixie).

---

## Image modes

Two ways to run the stack:

| Mode | What it does | When to use |
|------|-------------|-------------|
| **Pre-built (default)** | Pulls images from GHCR (`ghcr.io/moeshinx/relay-panel-*:x.y.z`). No compilation on the server. | **Recommended** for production. Fast (~30s). |
| **Source build** | Builds images locally from source (`docker-compose.yaml`). | Development, offline servers, or custom modifications. |

`deploy.sh` picks the mode automatically:
- Default → pre-built (`docker-compose.release.yaml`)
- `RELAYPANEL_BUILD_LOCAL=1 ./deploy.sh` → source build (`docker-compose.yaml`)

---

## Bare-metal node (no Docker on the node)

The panel runs in Docker, but forwarding nodes can run as bare-metal binaries
via systemd. This is faster and lighter than Docker on the node side.

### 1. Create a device group in the panel UI

Go to Device Groups → Add Group (type: inbound) → copy the **Install Command**.

### 2. Run on the node server

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
  -t <token> -u http://<panel-ip>:18888
```

This downloads the static binary, writes a systemd service, and starts it.

### 3. Verify

```bash
journalctl -u relay-node -f        # live logs
systemctl status relay-node        # status
```

### 4. Uninstall

```bash
systemctl disable --now relay-node
rm -f /etc/systemd/system/relay-node.service
rm -rf /opt/relay-node
```

> Binaries are built with musl + rustls (fully static, no OpenSSL dependency).
> Works on Debian 11, 12, and 13. amd64 and arm64 supported.

---

## Deploy with Docker Compose

### 1. Clone

```bash
git clone https://github.com/MoeShinX/relay-panel.git
cd relay-panel
```

### 2. Generate secrets

The panel refuses to start without a real `JWT_SECRET`. Generate one:

```bash
    cat > .env <<EOF
JWT_SECRET=$(openssl rand -hex 32)
PANEL_KEY=$(openssl rand -hex 16)
NODE_TOKEN=<paste a node token from the UI later>
# Optional: set when the panel is behind a domain/reverse proxy so the
# "Copy Install Command" button emits the right URL for nodes.
PUBLIC_PANEL_URL=https://panel.example.com
# DATABASE_URL controls the database backend.  Defaults to SQLite.
# See "Database modes" section below.
DATABASE_URL=sqlite:/app/data/data.db?mode=rwc
EOF
    chmod 600 .env
    ```

    > `NODE_TOKEN` defaults to `default-token`. Create an **inbound device group**
    > in the panel UI after first login, copy its token, then update `.env` and
    > `docker compose restart node`.

    ### Database modes

    RelayPanel supports three database modes, controlled by `DATABASE_URL`:

    | Mode | DATABASE_URL | What happens |
    |------|-------------|-------------|
    | **SQLite** (default) | `sqlite:/app/data/data.db?mode=rwc` | Panel starts directly. No extra containers. |
    | **PostgreSQL (embedded)** | `postgres://user:pass@postgres:5432/db` | `deploy.sh` starts a `postgres` container (profile), waits for healthy, then starts panel. Set `RELAYPANEL_DB_MODE=embedded-postgres` + `POSTGRES_DB`/`POSTGRES_USER`/`POSTGRES_PASSWORD` in `.env`. |
    | **PostgreSQL (external)** | `postgres://user:pass@host:port/db` | `deploy.sh` verifies connectivity with a real `psql` query before starting the panel. Set `RELAYPANEL_DB_MODE=external-postgres`. |

    - `DATABASE_URL` is the **canonical** env var.
    - `DATABASE_PATH` is a **legacy fallback** — it still works, but new
      deployments should use `DATABASE_URL`. On upgrade, `deploy.sh` wraps a
      legacy `DATABASE_PATH` into a `sqlite:` URL **at the same location** (the
      data file is never moved).
    - **Fresh install only:** `deploy.sh` shows the interactive backend menu
      (SQLite / embedded PostgreSQL / external PostgreSQL) when it is the first
      run (no `.env` yet) on a terminal. Non-interactive installs (e.g.
      `curl \| bash`) default to SQLite.
    - **Upgrade:** when `.env` already exists, `deploy.sh` keeps the current
      backend untouched — no menu, no change to `DATABASE_URL` /
      `RELAYPANEL_DB_MODE`. It detects the backend and prints one concise line,
      e.g. `Database backend: SQLite (unchanged)`. Switching between SQLite and
      PostgreSQL is a manual operation: stop the panel, migrate your data, then
      edit `.env` yourself.
    - External PostgreSQL **must** be a full, URL-encoded connection string.
      Passwords with special characters (`@`, `:`, `#`, `%`, etc.) must be
      percent-encoded (e.g. `p@ss` → `p%40ss`).
    - Connection failure is **fatal** — the deploy script never falls back to
      SQLite when PostgreSQL was configured.

    #### `PUBLIC_PANEL_URL` (optional)

The URL nodes use to reach the panel. The "Copy Install Command" button in
**Device Groups** reads this from the `/api/v1/system/version` endpoint and
embeds it into the one-line install command.

- **Leave empty** when admins and nodes reach the panel at the same address
  you type into the browser (e.g. `http://203.0.113.10:18888`). The frontend
  falls back to `window.location.origin`.
- **Set it** when the panel is behind a reverse proxy or domain — e.g.
  `PUBLIC_PANEL_URL=https://panel.example.com` — so nodes get the
  TLS-terminated address instead of the raw `http://ip:port` the browser bar
  shows. Also used to derive the WebSocket URL (`wss://...`).
- A `localhost` / `127.0.0.1` / `0.0.0.0` value triggers a warning in the
  install-command modal, since nodes on other hosts cannot connect to those.

    #### GeoIP — node region resolution

The panel can display a country flag + name next to each node's public IP on the
node-status board. GeoIP is **enabled by default** with built-in providers.
flips the default to ON**, so a fresh install shows country flags without any
extra configuration. To opt out, set `GEOIP_ENABLED=false`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `GEOIP_ENABLED` | `true` | Master switch. Set to `false` / `0` to disable region resolution entirely. |
| `GEOIP_CACHE_TTL` | `604800` (7 days) | How long a resolved country is cached (seconds) before a re-lookup. |

**The GeoIP provider URLs are built-in and no longer
user-configurable.** The panel uses **ipinfo.io Lite** as primary, with
automatic fallback to **ipwho.is** if the primary fails (timeout, error, or
missing `country_code`). The old `GEOIP_API_URL` env var is ignored;
self-hosted / custom providers are no longer supported — if neither built-in
provider works, the lookup degrades to "unknown" without affecting node
status or forwarding.

Privacy / safety notes:

- The lookup runs **server-side on the panel only** — never from the browser.
- Only a node's **public IP** is sent to the GeoIP providers. Private,
  loopback, and link-local addresses are never queried.
- Failures degrade gracefully to "unknown" and **never** affect node status,
  online state, or forwarding. The third-party response body is not logged.
- To disable entirely, set `GEOIP_ENABLED=false`.
  remove that line — it is no longer read. The panel always uses its built-in
  primary + fallback pair.

### 3. Start

The recommended way is `deploy.sh`, which by default uses
`docker-compose.release.yaml` (pre-built GHCR images — no compilation on the
server). On a fresh install, `deploy.sh` offers an interactive **web access
mode** menu:

| Mode | What it does | When to use |
|------|-------------|-------------|
| **Direct** (default) | Panel publishes `0.0.0.0:18888` publicly. | Simple single-server setups; firewall rules control access. |
| **Reverse proxy** | Panel binds `127.0.0.1:18888` for same-host proxies, or `0.0.0.0:18888` with `REVERSE_PROXY_EXTERNAL=1` for a separate proxy host. | Existing reverse proxy infrastructure; domain + TLS termination external. |
| **Caddy (Compose)** | Compose-managed Caddy on ports 80/443 (plus 443/udp for HTTP/3) with auto-TLS. Panel localhost only. | No existing proxy; want automatic Let's Encrypt via Compose. Requires a domain. |

All three modes can be set via `RELAYPANEL_WEB_MODE` (non-interactive / CI).
See **[docs/REVERSE-PROXY.md](REVERSE-PROXY.md)** for detailed configuration.

```bash
./deploy.sh
```

This pulls the images, starts the `panel` container (and optionally Caddy / node
via Compose profiles), and verifies the panel is reachable. For a source build
instead, see [Source build (fallback)](#source-build-fallback) below.

### 4. Verify

```bash
# panel HTTP reachable?
curl http://127.0.0.1:18888/

# login works?
curl -X POST http://127.0.0.1:18888/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin123"}'
```

Open `http://<server-ip>:18888` in a browser. Login `admin` / `admin123`.
The panel forces a password change on this first login — pick a strong one.

### 5. View logs

```bash
docker compose logs -f panel
docker compose logs -f node
```

### 6. Upgrade panel

The recommended upgrade flow. Do NOT use `docker compose up -d --build` as the
default — that compiles from source on every update. The default below pulls
pre-built GHCR images and only restarts the containers (minimal downtime, no
`down` needed).

**0. Back up data** (at least the database + `.env`):

```bash
cd /opt/relay-panel
# SQLite default lives at data.db; back it up with a timestamp.
cp -a data.db data.db.bak.$(date +%Y%m%d-%H%M%S) 2>/dev/null || true
cp -a .env .env.bak.$(date +%Y%m%d-%H%M%S)
```

> If `DATABASE_URL` (or legacy `DATABASE_PATH`) points elsewhere, back up that file instead.

**1. Pull latest code:**

```bash
git pull
```

**2. Run the upgrade** — `deploy.sh` defaults to `docker-compose.release.yaml`,
pulls the new GHCR images, and restarts the `panel`/`node` containers:

```bash
./deploy.sh
```

> **Note on the admin password:** `deploy.sh` decides success by the container
> state + port reachability + the `GET /api/v1/health` endpoint (a real JSON
> health probe — status:"ok" + version). It does **not** log in as
> `admin`/`admin123` at all (that probe was removed — it was a
> needless login that could trip rate-limiting). So an upgrade on a deployment
> where you've already changed the default password reports success exactly
> like a first deploy.

**3. Verify:**

```bash
docker compose -f docker-compose.release.yaml ps   # both services Up
curl -I http://127.0.0.1:18888/login                # 200/302 = healthy
```

#### Manual upgrade (without deploy.sh)

If you prefer raw `docker compose` (skipping `deploy.sh`'s checks):

```bash
cd /opt/relay-panel
git pull
docker compose -f docker-compose.release.yaml pull
docker compose -f docker-compose.release.yaml up -d
```

`up -d` recreates only containers whose image/config changed — no need for a
separate `docker compose down`.

#### Source build (fallback)

Only if you cannot use the pre-built images (offline server, custom
modifications):

```bash
RELAYPANEL_BUILD_LOCAL=1 ./deploy.sh
```

This builds from `docker-compose.yaml` on the server (slower — compiles
Rust + builds the frontend locally).

#### Fully stop the stack

Only for troubleshooting or a full shutdown — not part of a normal upgrade:

```bash
docker compose -f docker-compose.release.yaml down
```

---

## Update notifications (dashboard)

The dashboard's "new version" banner is a **passive pull**, not a push:

- The panel only checks GitHub Releases when an admin **opens the
  Dashboard page**, and then re-checks every **30 minutes** while it stays
  open. There is no background service and no email/webhook.
- Each check pulls `/releases` (not `/releases/latest`) and picks the
  highest semver tag — pre-releases are included during the pre-release
  phase (toggle `ALLOW_PRERELEASE_UPDATES` in `crates/panel/src/api/system.rs`
  to restrict to stable only).
- Results are cached on the server for 30 minutes. Click **Check for
  updates** on the Dashboard header to bypass the cache (`?refresh=1`).
- If GitHub is unreachable (rate-limit, no outbound network, bad token),
  the response carries `check_failed: true` and the Dashboard shows a
  yellow "update check failed" banner with the error message — it will
  **not** silently pretend there is no update. Every failure is also
  logged server-side with the URL, HTTP status, and body for diagnostics.

For automatic update notifications, point an external monitor (e.g.
UptimeRobot, healthchecks.io) at the panel and watch the Docker image tag
in your registry instead.

## Optional: manage Docker Compose with systemd

If you want `docker compose` to auto-start on boot and restart on crash,
create a systemd service that manages the compose stack:

```ini
# /etc/systemd/system/relaypanel.service
[Unit]
Description=RelayPanel (Docker Compose)
Requires=docker.service
After=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=/opt/relay-panel
# .env is read automatically by docker compose
ExecStart=/usr/bin/docker compose up -d
ExecStop=/usr/bin/docker compose down
TimeoutStartSec=0

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now relaypanel
sudo systemctl status relaypanel
```

This starts the entire stack (panel + node) as a single unit. There is **no**
per-binary systemd service — both run inside containers.

---

## Networking notes

- **panel** container publishes port `18888` to the host (binding depends on
  `RELAYPANEL_WEB_MODE`: `0.0.0.0` for direct, `127.0.0.1` for reverse-proxy
  and Caddy modes).
- **node** container uses `network_mode: host` so it can bind the forwarding
  listen ports directly on the host. If you run panel and node on **separate
  machines**, set `PANEL_URL` to the panel's address and remove host networking
  (bind the forwarding ports explicitly instead).
- **Caddy** container (profile `caddy`) publishes ports `80`, `443/tcp`, and `443/udp` to the
  host for TLS termination and HTTP/3. The `Caddyfile` proxies to `panel:18888` on the
  Compose network.
- The SQLite database persists in the `panel_data` Docker volume.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `JWT_SECRET must be set` | You didn't create `.env` or it's empty. Run step 2. |
| Panel unreachable | `docker compose logs panel` — check for binding errors. |
| Node not forwarding | Verify `NODE_TOKEN` matches a group token from the UI. |
| Port already in use | Another process holds the listen port; pick a different one in the rule. |
| Users kicked to login page repeatedly | The panel's DB is temporarily unreachable (lock contention / disk full). Check `docker compose logs panel` for `auth db lookup failed`. This is a 500, not a token problem — the page recovers once the DB is healthy. |
| Node shows "离线" but process is running | The node can't reach the panel over HTTP (firewall / `PUBLIC_PANEL_URL`). Check `journalctl -u relay-node` for `report_status` errors. |
| Traffic keeps growing without bounds | A rule's target is unreachable and connections are piling up. Check the node's listener errors column on the Nodes page. |
| PostgreSQL connection failed | `deploy.sh` fails fast with "Cannot connect to external PostgreSQL". Check host, port, user, password, database name, and firewall. For embedded PG: `docker compose logs postgres`. |

### Logging

Both the panel and the node write structured logs to stdout (captured by
`docker compose logs` / `journalctl`). The verbosity is controlled by the
`RUST_LOG` environment variable:

| Level | What you see | When to use |
|-------|--------------|-------------|
| `error` | Only failures that stop a component | Production baseline (quiet) |
| `warn` | Destructive admin ops (delete/ban/rotate), recoverable errors | Recommended default |
| `info` | Startup, listener binds, each `report_traffic HTTP 200` | Default |
| `debug` | Per-report details, every connection open/close | Troubleshooting only — noisy |

Set it per-service in `docker-compose.yml`:

```yaml
services:
  panel:
    environment:
      RUST_LOG: warn,relay_panel=info
  node:
    environment:
      RUST_LOG: info
```

The **panel** defaults to `info`; set `RUST_LOG=warn` to suppress the per-cycle
`report_traffic` lines while keeping destructive-op audit lines. The **node**
defaults to `info`; see `docs/NODE.md` for node-side tuning.

---

## Reverse proxy & WebSocket support

RelayPanel nodes connect to the panel via WebSocket (`/api/v1/node/ws`) for
real-time config push. If you run a reverse proxy (Nginx, Caddy, Cloudflare),
you **must** enable WebSocket Upgrade support.

For a complete guide covering Nginx, Caddy, Cloudflare, `PUBLIC_PANEL_URL`,
and the difference between panel admin HTTPS and node-side TLS Simple, see
**[docs/REVERSE-PROXY.md](REVERSE-PROXY.md)**.

### Quick examples

```nginx
location /api/v1/node/ws {
    proxy_pass http://127.0.0.1:18888;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_read_timeout 3600s;
}

location / {
    proxy_pass http://127.0.0.1:18888;
    proxy_set_header Host $host;
}
```

### Caddy

```caddyfile
panel.example.com {
    reverse_proxy 127.0.0.1:18888
}
```

Caddy handles WebSocket automatically.

### Cloudflare / CDN

- Go to **Network** settings -> enable **WebSockets**.
- If WS fails, check if CDN strips Upgrade/Connection headers.

### WS Troubleshooting

If node logs show `websocket error` or keeps reconnecting:
1. Panel unreachable: verify PANEL_URL from node perspective.
2. Invalid token: verify NODE_TOKEN matches panel UI.
3. Reverse proxy missing Upgrade header.
4. CDN blocking WS: enable WebSocket in CDN dashboard.
5. Firewall: ensure port open for HTTP and WS.

> Even if WebSocket fails, the node continues forwarding with cached config
> and falls back to HTTP polling every 10s. The node never exits due to WS failure.
