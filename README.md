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

- 🔀 **转发规则** — TCP/UDP 端口转发，多目标、故障转移与轮询负载均衡；Linux 下不限速规则走 `splice` 零拷贝转发，长链路低延迟、低抖动
- 🛡️ **目标熔断** — 单目标连续失败自动跳过一段时间，全部熔断时自动试探恢复
- 🌐 **域名目标 & DDNS 跟随** — 目标可填域名，DNS 变更自动跟随新 IP（TCP 30 秒缓存、UDP 会话级重解析），DDNS 换 IP 无需手动重启规则或节点
- ♻️ **高并发连接稳定** — TCP keepalive 自动回收死连接（NAT 超时、断线）避免文件描述符耗尽；节点启动自动抬高 fd 上限，撑住长期高并发
- 🛒 **套餐商城与计费** — 用户自助购买（余额扣费）、查看订单；管理员配置套餐（增删改），套餐绑定线路并在购买时自动授权
- 💳 **上下行计费 + 分组倍率** — 按「(上行 + 下行) × 线路倍率（0.1–100）」从套餐额度扣除
- 🔁 **单套餐模型** — 一人一个当前套餐：买相同套餐＝续费（流量叠加 / 限时延期），买不同套餐＝切换（整体替换，切换前弹确认）；无权规则系统自动暂停、重授权后自动恢复
- 📈 **流量与配额** — 按规则 / 按用户计量流量，可设规则数、带宽、流量上限
- 📋 **多套餐注册** — 管理员配置允许注册的套餐，用户注册时自行选择
- 👤 **用户管理** — 管理员直接管理任意用户的规则、套餐（开通 / 续费 / 切换 / 改期 / 删除）、重置流量、重置密码、封禁 / 解封
- 🖥️ **设备分组管理** — 分组可展开查看节点列表，支持「隐藏」（仅对普通用户节点状态页隐藏，规则照常用），节点卸载不影响分组和规则
- ⬆️ **节点一键升级** — 面板下发升级，节点从官方 Release 自更新（校验 sha256、只升不降、按 systemd / docker / 手动区分安装方式）；节点原生支持 amd64 / arm64
- 🖱️ **规则极简导入/导出** — 单行 JSON 简洁格式，支持批量导入、批量启停并自动下发
- 🖥️ **实时节点状态** — CPU、内存、连接数、节点版本（可升级时高亮提示）
- 🌍 **节点地区识别** — 自动识别节点所在国家/地区，显示国旗标识
- 🗄️ **双数据库** — SQLite（默认，零配置）或 PostgreSQL
- 🔒 **安全** — 首次登录强制改密码，节点 Bearer Token 鉴权

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

## 🚀 快速开始

**一条命令部署：**

```bash
curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
```

> 🔑 **默认账号 `admin` / `admin123`，首次登录强制修改密码。**

> 🖥️ **平台支持**：面板镜像与节点均支持 **amd64 / arm64**，ARM 服务器可直接部署；面板镜像为多架构 manifest，`docker pull` 自动选对架构，节点安装脚本 `uname -m` 自动适配，均无需手动指定。

📖 完整指南：**[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**

---

## 🔄 更新

```bash
cd /opt/relay-panel && git pull --quiet && ./deploy.sh
```

> ⚠️ 更新前请备份 `.env` 和数据库。

节点更新：面板 **设备分组 → 复制对接命令** → 粘贴到节点执行。

---

## 🛠️ 本地开发

```bash
cargo build && cargo run -p relay-panel &   # API 在 :18888
cd frontend && npm install && npm run dev   # UI 在 :5173
python3 tests/e2e_test.py                   # 端到端测试
```

---

## 📦 技术栈

| 层级 | 选型 |
|------|------|
| 后端 | Rust · Axum 0.8 · Tokio · sqlx |
| 数据库 | SQLite / PostgreSQL |
| 鉴权 | JWT · bcrypt |
| 转发 | Tokio 异步 TCP + UDP |
| 前端 | React 19 · TypeScript · Ant Design |
| 部署 | Docker 多阶段构建 · Compose |

---

## 📄 许可证与免责声明

AGPL-3.0 —— 详见 [LICENSE](LICENSE)。

开源流量转发工具，**仅供个人学习与研究使用**。请在合法合规前提下使用，风险自负。

完整 **[免责声明](docs/DISCLAIMER.md)**
