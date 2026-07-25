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

- 🔀 **Forwarding rules** — TCP/UDP multi-target forwarding, failover / round-robin balancing, circuit breaker with auto-recovery, domain targets that follow DDNS
- 🚦 **Connection control** — per-rule concurrent-connection cap; restart one rule, a batch, or on a schedule — dropping old connections and rebuilding listeners
- 🛒 **Plans & billing** — self-service plan purchase and redeem-code top-ups; charged as `(upload + download) × line rate`; one plan per user, renewals stack and switching replaces
- 📊 **Traffic visibility** — per-rule and per-user metering, with 1 / 7 / 30-day charts stacked by line so you can see which one is consuming the quota
- 🖥️ **Node management** — live CPU / memory / connections, region detection, Telegram or email alerts on offline, one-click upgrade from the panel (no SSH)
- 👤 **Users & groups** — manage any user's rules and plan, reset traffic or password, ban; device groups can be hidden, and removing a node doesn't affect rules
- 🗄️ **Deployment-friendly** — SQLite (zero-config) or PostgreSQL; panel and node both support amd64 / arm64
- 🔒 **Security** — first login forces password change; node auth via Bearer token

Full feature reference and user docs: **[relaypanel.dev](https://relaypanel.dev)**

---

## 🚀 Quick start

**One command deploy:**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> 🔑 **Default login `admin` / `admin123` — first login forces a password change.**

📖 Full guide: **[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

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

## 🔄 Update

**Panel** (back up `.env` and your database first):

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

**Nodes**: Panel → Node Status → click "Upgrade". No SSH. systemd nodes only (Docker nodes update the image instead); upgrading drops that node's live forwarding connections. See the [node documentation](docs/NODE.md#update).

---

## 🛠️ Local dev

```bash
cargo build && cargo run -p relay-panel &   # API on :18888
cd frontend && npm install && npm run dev   # UI on :5173
python3 tests/e2e_test.py                   # end-to-end test
```

---

## 📦 Tech stack

Rust · Axum · Tokio · sqlx · SQLite/PostgreSQL · JWT · React 19 · TypeScript · Ant Design · Docker Compose

---

## 📄 License & Disclaimer

AGPL-3.0 — see [LICENSE](LICENSE).

Open-source traffic-forwarding tool for **personal study and research only**.
Use lawfully and at your own risk.

Full **[Disclaimer](docs/DISCLAIMER.md)**
