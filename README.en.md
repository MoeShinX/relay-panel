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

- 🔀 **Forwarding rules** — TCP/UDP port forwarding with multi-target support, failover and round-robin load balancing
- 🛡️ **Circuit breaker** — 3 consecutive failures → skip target for 30 s; all-down triggers probe mode for auto-recovery
- 📊 **Dashboard** — node status overview, traffic statistics, version update check
- 📈 **Traffic & quotas** — per-rule and per-user tracking with configurable limits (rule count, bandwidth, traffic cap)
- 📋 **Multi-plan registration** — admins configure allowed plans; users choose on sign-up
- 👥 **User management** — manage any user's rules, reset traffic, reset password, ban/unban
- 🖥️ **Live node status** — CPU, memory, connections, version
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

📖 Full guide: **[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

---

## 🔄 Update

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

> ⚠️ Back up `.env` and your database before updating.

Forwarding nodes: **Device Groups → Copy Install Command** → paste on the node.

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
