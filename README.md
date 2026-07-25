<p align="center">
  <img src="frontend/public/favicon.svg" width="80" height="80" alt="RelayPanel Logo" />
</p>

<h1 align="center">RelayPanel</h1>

<p align="center">
  ⚡ 自托管 TCP/UDP 端口转发管理面板 ⚡
</p>

<p align="center">
  <a href="README.en.md">English</a> | <strong>中文</strong>
</p>

<p align="center">
  <a href="https://github.com/MoeShinX/relay-panel/releases/latest"><img src="https://img.shields.io/github/v/release/MoeShinX/relay-panel?style=flat-square&label=Release&color=blue" alt="Release" /></a>
  <a href="https://github.com/MoeShinX/relay-panel/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/MoeShinX/relay-panel/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/MoeShinX/relay-panel?style=flat-square&label=License&color=red" alt="License" /></a>
</p>

<p align="center">
  用 Rust 编写,通过 Web UI 管理转发规则、设备分组、流量配额和实时节点状态。<br/>
  轻量：Panel ~7 MB + Node ~4 MB。部署方式：Docker Compose。数据库：SQLite / PostgreSQL。
</p>

---

## ✨ 功能亮点

- 🔀 **转发规则** — TCP/UDP 多目标转发，故障转移 / 轮询负载均衡，目标熔断自动恢复，域名目标跟随 DDNS
- 🚦 **连接治理** — 每条规则可设并发连接数上限；支持单条 / 批量 / 定时重启，重启会掐断旧连接并重建监听
- 🛒 **套餐与计费** — 用户自助买套餐、卡密充值余额；按「(上行 + 下行) × 线路倍率」扣额度；一人一个当前套餐，续费叠加、换套餐整体替换
- 📊 **流量可视** — 按规则 / 用户计量，近 1 / 7 / 30 天趋势图按线路分色堆叠，看得出是哪条线路在吃额度
- 🖥️ **节点管理** — 实时 CPU / 内存 / 连接数、地区识别、掉线推送 Telegram 或邮件、面板一键升级节点（免 SSH）
- 👤 **用户与分组** — 管理任意用户的规则与套餐、重置流量密码、封禁；设备分组可隐藏，节点卸载不影响规则
- 🗄️ **部署友好** — SQLite（零配置）或 PostgreSQL；面板与节点均支持 amd64 / arm64
- 🔒 **安全** — 首次登录强制改密码，节点 Bearer Token 鉴权

完整功能说明与使用文档：**[relaypanel.dev](https://relaypanel.dev)**

---

## 🚀 快速开始

**一条命令部署：**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> 🔑 **默认账号 `admin` / `admin123`，首次登录强制修改密码。**

📖 完整指南：**[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

---

## 🏗️ 架构

```
  浏览器 (React UI)          relay-node (Tokio TCP/UDP)
       │                          ▲
       ▼                          │
   relay-panel  ◄─── WebSocket 配置推送 + HTTP 状态上报
   (Axum API)                     │
       │                          ▼
   SQLite / PG              转发流量到真实目标
```

---

## 🔄 更新

**面板**（更新前请备份 `.env` 和数据库）：

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

**节点**：面板 → 节点状态 → 点「升级」，无需 SSH。仅 systemd 节点可用（Docker 节点改为更新镜像）；升级会断开该节点上正在进行的转发连接。详见 [转发节点文档](docs/NODE.zh-CN.md#更新)。

---

## 🛠️ 本地开发

```bash
cargo build && cargo run -p relay-panel &   # API 在 :18888
cd frontend && npm install && npm run dev   # UI 在 :5173
python3 tests/e2e_test.py                   # 端到端测试
```

---

## 📦 技术栈

Rust · Axum · Tokio · sqlx · SQLite/PostgreSQL · JWT · React 19 · TypeScript · Ant Design · Docker Compose

---

## 📄 许可证与免责声明

AGPL-3.0 —— 详见 [LICENSE](LICENSE)。

开源流量转发工具，**仅供个人学习与研究使用**。请在合法合规前提下使用，风险自负。

完整 **[免责声明](docs/DISCLAIMER.md)**
