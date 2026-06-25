# RelayPanel

**English** | [中文](README.zh-CN.md)

[![CI](https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml/badge.svg)](https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml)
[![Debian Compat](https://github.com/MoeShinX/relay-panel/actions/workflows/debian-compat.yml/badge.svg)](https://github.com/MoeShinX/relay-panel/actions/workflows/debian-compat.yml)

A self-hosted **TCP/UDP forwarding management panel** built in Rust. Manage
port-forwarding rules, device groups, traffic quotas, and live node status
through a web UI — lightweight: one ~7 MB panel binary + a ~4 MB node binary
per forwarding host.

**Deploy:** Docker Compose · **Database:** SQLite (default) or PostgreSQL ·
**Version:** `1.0.1`

---

## Architecture

```
 ┌─────────────┐    WebSocket (config push) + HTTP (status/traffic)   ┌──────────────┐
 │  Browser    │◄──────┐                                          ┌───►│ relay-node   │
 │  (React UI) │       │                                          │    │ (Tokio TCP/  │
 └─────────────┘       │   ┌──────────────────┐                   │    │  UDP engine)  │
                       └──►│   relay-panel     │◄──────────────────┘    └──────────────┘
                           │  (Axum + SQLite/  │              │
                           │   PostgreSQL)     │              ▼
                           └──────────────────┘        forwards traffic
                                       ▲               to real targets
                                       │
                              ┌────────┴────────┐
                              │  SQLite / PG    │
                              └─────────────────┘
```

- **Panel** — Axum HTTP server: serves the React SPA + REST API. JWT auth,
  bcrypt passwords. SQLite (zero-config) or PostgreSQL.
- **Node** — runs on each forwarding host. Opens TCP/UDP listeners, forwards
  traffic, reports status + traffic back.
- **Config delivery** — WebSocket real-time push (25 s heartbeat) + HTTP poll
  every 10 s as fallback. WS failure never stops forwarding.

## Features

- **Forwarding rules** — TCP/UDP port forwarding with multi-target support,
  per-target circuit breaker (3 failures → 30 s cooldown), failover and
  round-robin load balancing.
- **Dashboard** — node status overview, traffic statistics, version update
  check.
- **Traffic & quotas** — per-rule and per-user traffic tracking with
  configurable limits (rule count, bandwidth, traffic cap).
- **Multi-plan registration** — admins configure allowed plans; users choose
  on sign-up.
- **User management** — admin can manage any user's rules, reset traffic,
  reset password, ban/unban.
- **Live node status** — CPU, memory, connections, version, GeoIP country
  flag per node.
- **GeoIP** — built-in providers (ipinfo.io Lite + ipwho.is fallback),
  enabled by default. Opt out with `GEOIP_ENABLED=false`.
- **Dual database** — SQLite (default, zero-config) or PostgreSQL.
- **Security** — first login forces password change; node auth via
  `Authorization: Bearer` header (never in query string).

## Quick start

**Production (one command):**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> **Default login `admin` / `admin123` — first login forces a password change.**

Full deployment guide: **[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)** ·
Reverse proxy: **[docs/REVERSE-PROXY.md](docs/REVERSE-PROXY.md)** ·
Node setup: **[docs/NODE.md](docs/NODE.md)**

**Local dev:**

```bash
cargo build && cargo run -p relay-panel &   # API on :18888
cd frontend && npm install && npm run dev   # UI on :5173 (proxies /api → :18888)
python3 tests/e2e_test.py                   # end-to-end TCP+UDP forwarding test
```

## Update

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

> ⚠️ **Back up before updating.** Copy `.env` and your database (`data/` for
> SQLite, `pg_dump` for PostgreSQL) first.

Forwarding nodes: **Device Groups → Copy Install Command** → paste on the
node. See [docs/NODE.md](docs/NODE.md#update).

## Tech stack

| Layer    | Choice                               |
|----------|--------------------------------------|
| Backend  | Rust, Axum 0.8, Tokio, sqlx          |
| Database | SQLite / PostgreSQL                  |
| Auth     | JWT (jsonwebtoken), bcrypt           |
| Forward  | Tokio async TCP + UDP                |
| Frontend | React 19, TypeScript, antd 6, Vite   |
| Deploy   | Docker multi-stage, docker-compose   |

## License & Disclaimer

AGPL-3.0 — see [LICENSE](LICENSE). Open-source traffic-forwarding tool for
**personal study and research only**; use lawfully and at your own risk. Full
**[Disclaimer](docs/DISCLAIMER.md)**.
