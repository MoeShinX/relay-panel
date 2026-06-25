# RelayPanel

[English](README.md) | **中文**

[![CI](https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml/badge.svg)](https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml)
[![Debian Compat](https://github.com/MoeShinX/relay-panel/actions/workflows/debian-compat.yml/badge.svg)](https://github.com/MoeShinX/relay-panel/actions/workflows/debian-compat.yml)

自托管的 **TCP/UDP 端口转发管理面板**，用 Rust 编写。通过 Web UI 管理端口转发规则、
设备分组、流量配额和实时节点状态 —— 轻量：单个约 7 MB 的 panel 二进制 + 约 4 MB
的 node 二进制。

**部署：** Docker Compose · **数据库：** SQLite（默认）或 PostgreSQL ·
**当前版本：** `1.0.1`

---

## 架构

```
 ┌─────────────┐    WebSocket（配置推送）+ HTTP（状态/流量上报）   ┌──────────────┐
 │  浏览器     │◄──────┐                                          ┌───►│ relay-node  │
 │  (React UI) │       │                                          │    │ (Tokio TCP/ │
 └─────────────┘       │   ┌──────────────────┐                   │    │  UDP 引擎)  │
                       └──►│   relay-panel     │◄──────────────────┘    └──────────────┘
                           │ (Axum + SQLite/   │              │
                           │  PostgreSQL)      │              ▼
                           └──────────────────┘       转发流量到真实目标
                                       ▲
                                       │
                              ┌────────┴────────┐
                              │  SQLite / PG    │
                              └─────────────────┘
```

- **Panel** — Axum HTTP 服务器：提供 React SPA + REST API。JWT 鉴权，bcrypt 密码。
  支持 SQLite（零配置）或 PostgreSQL。
- **Node** — 运行在每个转发主机上，开启 TCP/UDP 监听器转发流量，回报状态与流量。
- **配置下发** — WebSocket 实时推送（25 秒心跳）+ HTTP 每 10 秒轮询兜底。WS 失败
  绝不中断转发。

## 功能亮点

- **转发规则** — TCP/UDP 端口转发，多目标支持，单目标熔断（3 次失败 → 30 秒冷却），
  故障转移与轮询负载均衡。
- **仪表盘** — 节点状态总览、流量统计、版本更新检查。
- **流量与配额** — 按规则、按用户计量流量，可设规则数/带宽/流量上限。
- **多套餐注册** — 管理员配置允许注册的套餐，用户注册时自行选择。
- **用户管理** — 管理员可从用户管理页直接管理任意用户的规则、重置流量、重置密码、
  封禁/解封。
- **实时节点状态** — CPU、内存、连接数、版本、GeoIP 国旗。
- **GeoIP** — 内置主源（ipinfo.io Lite）+ 备用源（ipwho.is），默认开启。
  `GEOIP_ENABLED=false` 关闭。
- **双数据库** — SQLite（默认，零配置）或 PostgreSQL。
- **安全** — 首次登录强制改密码；节点鉴权走 `Authorization: Bearer` 头（不走
  查询字符串，不泄露到日志）。

## 快速开始

**生产部署（一条命令）：**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> **默认账号 `admin` / `admin123`，首次登录强制修改密码。**

完整指南：**[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

**本地开发：**

```bash
cargo build && cargo run -p relay-panel &   # API 在 :18888
cd frontend && npm install && npm run dev   # UI 在 :5173（代理 /api → :18888）
python3 tests/e2e_test.py                   # 端到端 TCP+UDP 转发测试
```

## 更新

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

> ⚠️ **更新前请备份数据。** 先把 `.env` 和数据库（SQLite 为 `data/` 目录，
> PostgreSQL 用 `pg_dump`）复制到安全位置。

转发节点更新：**设备分组 → 复制对接命令** → 粘贴到节点执行。
详见 [docs/NODE.zh-CN.md](docs/NODE.zh-CN.md#更新)。

## 技术栈

| 层级     | 选型                                 |
|----------|--------------------------------------|
| 后端     | Rust, Axum 0.8, Tokio, sqlx          |
| 数据库   | SQLite / PostgreSQL                  |
| 鉴权     | JWT (jsonwebtoken), bcrypt           |
| 转发     | Tokio 异步 TCP + UDP                 |
| 前端     | React 19, TypeScript, antd 6, Vite   |
| 部署     | Docker 多阶段构建，docker-compose    |

## 许可证与免责声明

AGPL-3.0 —— 详见 [LICENSE](LICENSE)。开源流量转发工具，**仅供个人学习与研究使用**；
请在合法合规前提下使用，风险自负。完整 **[免责声明](docs/DISCLAIMER.md)**。
