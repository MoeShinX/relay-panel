<p align="center">
  <img src="frontend/public/favicon.svg" width="80" height="80" alt="RelayPanel Logo" />
</p>

<h1 align="center">RelayPanel</h1>

<p align="center">
  ⚡ Self-hosted TCP/UDP Forwarding Management Panel ⚡
</p>

<p align="center">
  <strong>English</strong> | <a href="README.md">中文</a>
</p>

<p align="center">
  <a href="https://github.com/MoeShinX/relay-panel/releases/latest"><img src="https://img.shields.io/github/v/release/MoeShinX/relay-panel?style=flat-square&label=Release&color=blue" alt="Release" /></a>
  <a href="https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/MoeShinX/relay-panel/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/MoeShinX/relay-panel?style=flat-square&label=License&color=red" alt="License" /></a>
</p>

<p align="center">
  Built with Rust. Manage forwarding rules, device groups, traffic quotas, and<br/>
  live node status via web UI. Lightweight: Panel ~7 MB + Node ~4 MB.<br/>
  Deploy: Docker Compose. Database: SQLite / PostgreSQL.
</p>

---

## ✨ Features

- 🔀 **Forwarding rules** — TCP/UDP port forwarding with multi-target, failover and round-robin load balancing; low latency and jitter on long chains
- 🛡️ **Circuit breaker** — a target that keeps failing is skipped for a while; all-down triggers probe mode for auto-recovery
- 🌐 **Domain targets & DDNS following** — targets can be domain names; a DDNS target that changes IP is followed automatically, with no manual rule/node restart
- ♻️ **High-concurrency stability** — dead connections left by dropped links are reclaimed automatically, sustaining long-running high-connection load
- 🚦 **Per-rule connection cap** — each rule can cap its own concurrent connections; anything beyond it is refused (TCP)
- 🔄 **Rule restart** — restart one rule, batch-restart a selection, or set an automatic interval; a restart drops the connections established on that rule and rebuilds its listeners
- 🛒 **Plan shop & billing** — self-service purchase (balance charge) with order history; admin plan CRUD, plans grant lines and auto-authorize on purchase
- 💳 **Redeem codes** — admins generate codes in batches; users top up their own balance from the account page or the shop, with no payment gateway involved
- 💰 **Up + down billing + per-line rate** — charged as `(upload + download) × line rate (0.1–100)` against the plan quota
- 🔁 **Single current plan** — one plan per user: buying the **same** plan renews it (stack traffic / extend a time plan), buying a **different** plan switches (full replace, with a confirm); rules on lost lines auto-pause and auto-resume once re-authorized
- 📈 **Traffic & quotas** — per-rule and per-user tracking with configurable limits (rule count, bandwidth, traffic cap)
- 📊 **Traffic charts** — usage over the last 1 / 7 / 30 days, stacked by line in distinct colours so you can see which line is consuming the quota
- 🔔 **Node offline alerts** — Telegram or email notification when a node stays unreachable past a threshold, and again when it recovers
- 📋 **Multi-plan registration** — admins configure allowed plans; users choose on sign-up
- 👤 **User management** — manage any user's rules, plan (assign / renew / switch / expiry / remove), reset traffic, reset password, ban/unban
- 🖥️ **Device group management** — expandable groups with node listings; a "hidden" toggle hides a group from regular users' Node Status page only (rules keep working); node removal does not affect groups or rules
- ⬆️ **One-click node upgrade** — trigger from the panel; the node self-updates from the official release, upgrade-only and never downgrading, without logging into the node
- 🖱️ **Minimal rule import/export** — batch import / batch pause-resume with automatic node distribution
- 🖥️ **Live node status** — CPU, memory, connections, node version (highlighted when an upgrade is available)
- 🌍 **Node region detection** — automatically identifies each node's country/region with flag display
- 🗄️ **Dual database** — SQLite (default, zero-config) or PostgreSQL
- 🔒 **Security** — first login forces password change; node auth via Bearer token

---

## 🏗️ Architecture

```
  Browser (React UI)          relay-node (Tokio TCP/UDP)
       │                          ▲
       ▼                          │
   relay-panel  ◄─── WebSocket config push + HTTP status report
   (Axum API)                     │
       │                          ▼
   SQLite / PG              forwards traffic to targets
```

---

## 🚀 Quick start

**One command deploy:**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> 🔑 **Default login `admin` / `admin123` — first login forces a password change.**

> 🖥️ **Platform**: both the panel image and the node support **amd64 / arm64**, so ARM servers can deploy directly. The panel image is a multi-arch manifest (`docker pull` picks the right arch automatically) and the node install script auto-detects the arch via `uname -m` — no manual selection needed.

📖 Full guide: **[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

---

## 🔄 Update

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

> ⚠️ Back up `.env` and your database before updating.

**Forwarding nodes: Panel → Node Status → click "Upgrade" on each node.** No SSH
required. The panel pushes the upgrade to the node, which pulls the new version
from the official Release itself (sha256 verified, never downgrades). Upgrading a
node drops the forwarding connections currently running on it.

> **One-click upgrade is available for systemd nodes only** — self-upgrade relies
> on "old process exits, supervisor starts the new one". Docker nodes show an
> "update the image" hint instead (pull the latest image and recreate the
> container); a node run manually in the foreground has nothing to restart it, so
> the button is greyed out.

Manual upgrade (when one-click isn't available): **Device Groups → Copy Install
Command** → paste on the node. See the [node documentation](docs/NODE.md#update).

---

## 🛠️ Local dev

```bash
cargo build && cargo run -p relay-panel &   # API on :18888
cd frontend && npm install && npm run dev   # UI on :5173
python3 tests/e2e_test.py                   # end-to-end test
```

---

## 📦 Tech stack

| Layer | Choice |
|-------|--------|
| Backend | Rust · Axum 0.8 · Tokio · sqlx |
| Database | SQLite / PostgreSQL |
| Auth | JWT · bcrypt |
| Forward | Tokio async TCP + UDP |
| Frontend | React 19 · TypeScript · Ant Design |
| Deploy | Docker multi-stage · Compose |

---

## 📄 License & Disclaimer

AGPL-3.0 — see [LICENSE](LICENSE).

Open-source traffic-forwarding tool for **personal study and research only**.
Use lawfully and at your own risk.

Full **[Disclaimer](docs/DISCLAIMER.md)**
