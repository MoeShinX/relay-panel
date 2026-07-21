# 转发节点（relay-node）

`relay-node` 是运行在每台中转服务器上的转发守护进程。它监听你在面板里
配置的端口，把 TCP/UDP 流量转发到目标地址，同时通过 HTTP 把 CPU / 内存 /
硬盘 / 网络 / 活跃连接数回报给面板，让面板能显示实时节点状态。

本文档覆盖：**二进制说明、安装、配置、更新、卸载、故障排查、注意事项。**

> 英文版见 [NODE.md](./NODE.md)。

---

## 二进制

每个 GitHub Release 会发布两个预构建的静态二进制（musl + rustls，不依赖
glibc，不需要装额外系统库）：

| 文件 | 架构 | 典型服务器 |
|------|------|-----------|
| `relay-node-linux-amd64` | x86_64 | 大多数 VPS / 云服务器 / Intel / AMD |
| `relay-node-linux-arm64` | aarch64 | ARM VPS、树莓派 4、Apple Silicon（Linux 虚拟机） |

先确认你服务器的架构，选对应的二进制：

```bash
uname -m
# x86_64   → 用 relay-node-linux-amd64
# aarch64  → 用 relay-node-linux-arm64
```

一键安装脚本会自动检测架构并下载对应的文件；只有手动安装时才需要自己选。

> **不提供 Windows / macOS 二进制。** 节点只能在 Linux 上运行。面板可以跑在
> 任何地方（Docker），但转发节点必须是 Linux。

---

## 安装

### 方式 A：一行脚本（推荐）

这是推荐方式。脚本会自动检测架构、从 GitHub Releases 下载对应二进制、
写好 systemd 服务并启动：

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
  -t <NODE_TOKEN> \
  -u https://your-panel.example.com
```

`<NODE_TOKEN>` 从面板获取：在面板里创建一个**入口（inbound）设备分组**，
复制它的 token。

脚本参数：

| 参数 | 含义 | 默认值 |
|------|------|--------|
| `-t, --token` | 节点 token（必填，从面板 UI 获取） | - |
| `-u, --url` | 面板地址，如 `https://panel.example.com`（必填） | - |
| `-s, --service-name` | systemd 服务名 | `relay-node` |
| `-p, --proxy` | 下载用的代理，如 `socks5://127.0.0.1:10808` | 无 |

用 **root** 运行（或加 `sudo`）。脚本会：
1. 检测架构（`uname -m`），选 `amd64` 或 `arm64`
2. 下载 `relay-node-linux-<架构>` 到 `/opt/relay-node/relay-node`
3. 用你传的 `PANEL_URL` + `NODE_TOKEN` 生成 `/opt/relay-node/start.sh`
4. 写 `/etc/systemd/system/relay-node.service` 并启用
5. 启动服务

### 方式 B：手动安装

适用于无法跑脚本的情况（没有 systemd、自定义路径、离线服务器手动拷贝二进制）。

```bash
# 1. 下载对应架构的二进制（替换为你要的版本）
ARCH=amd64   # 或 arm64
VERSION=1.0.3
curl -fL -o relay-node \
  "https://github.com/MoeShinX/relay-panel/releases/download/v${VERSION}/relay-node-linux-${ARCH}"

# 2. 加可执行权限，放到固定位置
chmod +x relay-node
sudo mkdir -p /opt/relay-node
sudo mv relay-node /opt/relay-node/relay-node
```

### 手动 systemd 配置

生产环境应该用 systemd 托管，这样能开机自启、崩溃自动重启。创建下面两个文件
（和一键脚本生成的一致）：

**`/opt/relay-node/start.sh`** —— 设置环境变量并启动二进制：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "/opt/relay-node"
export PANEL_URL="https://your-panel.example.com"   # <-- 改成你的面板地址
export NODE_TOKEN="your-node-token"                  # <-- 面板里的 token
export POLL_INTERVAL="${POLL_INTERVAL:-10}"
export RUST_LOG="${RUST_LOG:-info}"
exec ./relay-node
```

```bash
sudo chmod 700 /opt/relay-node/start.sh
```

**`/etc/systemd/system/relay-node.service`**：

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

然后启用并启动：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now relay-node
systemctl status relay-node   # 应显示 active (running)
```

---

## 配置

节点完全通过**环境变量**配置（没有配置文件）。一键脚本会把它们写进
`/opt/relay-node/start.sh`；手动运行时在启动前 export 即可。

| 变量 | 含义 | 默认值 |
|------|------|--------|
| `PANEL_URL` | 面板地址，如 `https://panel.example.com` | `http://127.0.0.1:18888` |
| `NODE_TOKEN` | 面板入口分组的 token | `default-token` |
| `POLL_INTERVAL` | HTTP 轮询 / 状态上报间隔（秒） | `10` |
| `PUBLIC_IP_CHECK_URL` | 检测节点公网出口 IP 用的地址 | `https://api.ipify.org` |
| `NETWORK_INTERFACE` | 统计整机流量的网卡，`auto` 读默认路由 | `auto` |
| `RUST_LOG` | 日志级别：`error` / `warn` / `info` / `debug` | `info` |

说明：
- `PANEL_URL` 和 `NODE_TOKEN` 是**必填**的，否则节点无法和你的面板通信。
  不填的话会回落到默认值，认证会失败。
- `PUBLIC_IP_CHECK_URL` 启动时检测一次，之后每 30 分钟刷新一次（不是每次
  轮询都请求）。检测失败不影响节点运行，只是面板上公网 IP 显示为「-」。
  如果不想依赖 ipify，可以指向你自己的回显 IP 服务。
- `POLL_INTERVAL` 同时控制配置轮询和状态上报。调小 = 状态更新更快但 HTTP
  流量更多。10 秒是个不错的默认值。
- `NETWORK_INTERFACE` 决定整机流量统计（面板「整机上行/下行」列）统计哪张
  网卡。默认 `auto` 读取默认路由对应的网卡（通常 eth0 / ens3 / venet0 / wg0），
  只统计这一张，避免 Docker bridge、veth 被重复累加。多网卡 / 策略路由 / 特殊
  VPS 可显式指定，例如 `NETWORK_INTERFACE=eth0`。面板「网卡」列会显示当前统计
  的网卡名。该统计是系统级（含 SSH、系统更新等），与 RelayPanel 转发量无关。

---

## 验证是否正常

安装后：

```bash
# 1. 版本号应该秒退（不会启动服务）
timeout 3 /opt/relay-node/relay-node --version
# 期望输出：relay-node 1.0.3

# 2. 服务状态
systemctl status relay-node

# 3. 实时日志
journalctl -u relay-node -f
```

日志里应该看到：
- `RelayPanel 1.0.3 starting, panel=...`
- `websocket connected`（如果你的反代支持 WS）
- `TCP listening on <端口> (rule <id>)` / `UDP listening on ...` 每条规则一行
- `report_traffic HTTP 200`（每次上报的状态码；详细的周期指标在 `debug` 级别）

面板侧，节点会在约 30 秒内出现在**节点状态**页，显示绿色「在线」标签。

---

## 更新

### 方式 A：面板一键升级（推荐，不用登服务器）

**面板 → 节点状态 → 找到那个节点 → 点「升级」。**

面板把升级指令通过 WebSocket 下发给节点，节点自己去官方 Release 拉新版本：
校验 sha256、只升不降、备份后原子替换二进制并重启自己。整个过程不需要你
SSH 登录节点。

节点版本落后时，节点状态页会高亮提示可升级。

> **升级会断开该节点上正在进行的转发连接**（替换二进制必须重启进程）。
> 业务繁忙时挑个低峰时段。

**按安装方式区分**——自升级的前提是「旧进程退出后，有东西把新进程拉起来」：

| 安装方式 | 节点状态页表现 |
| --- | --- |
| **systemd** | 蓝色升级按钮，落后且在线时可一键升级 |
| **Docker** | 黄色提示「请更新镜像」——拉最新 `relay-panel-node` 镜像重建容器 |
| **手动运行**（nohup / screen / 直接跑二进制） | 灰色禁用，提示「退出后无人拉起」 |

显示「手动运行：不支持一键升级」的节点，在该节点上重跑一次方式 B 的对接命令即可
——脚本会把它装成 systemd 服务，之后就能一键升级。

### 方式 B：重跑安装脚本

更新就是重新执行同一个一行脚本。**最简单的做法是把面板里复制出来的对接命令
重新执行一次** —— 那条命令已经带了正确的 `-t <NODE_TOKEN> -u <PANEL_URL>`，
不用自己记：

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
  -t <NODE_TOKEN> \
  -u https://your-panel.example.com
```

> **每次运行都必须传 `-t` 和 `-u`**（包括更新时）。脚本**不会**读取上一次
> `start.sh` 里的参数 —— 它要求你作为参数传入，并用传入值重新生成 `start.sh`。
> 缺任意一个脚本都会报错中止。

更新时发生的事：
1. 下载新二进制到临时文件并校验
2. **停止正在运行的服务**（这样才能干净地替换旧二进制，避免 Linux 报
   "Text file busy"）
3. 替换二进制，并用你传的 `-t`/`-u` 重新写 `start.sh`
4. 重启服务

重启后，转发规则会通过 WebSocket 推送或 10 秒 HTTP 轮询重新加载；如果面板
不可达，节点会从本地 `config-cache.json` 加载上次配置继续转发。

更新后检查版本：

```bash
/opt/relay-node/relay-node --version
```

下载的版本由脚本里的 `SCRIPT_VERSION` 决定。要固定旧版本见下文
[版本固定](#版本固定)。

---

## 卸载

```bash
systemctl disable --now relay-node
rm -f /etc/systemd/system/relay-node.service
systemctl daemon-reload
rm -rf /opt/relay-node
```

---

## 故障排查

### 节点在面板里显示「离线」
- 检查 `systemctl status relay-node` 是否 `active (running)`
- 检查节点能否访问面板：`curl -sf $PANEL_URL/`
- 检查 `NODE_TOKEN` 是否和面板入口分组的 token 一致
- 看 `journalctl -u relay-node` 里有没有 `report_status` 报错
- 在线判定阈值：超过 30 秒（默认轮询周期的 3 倍）没收到状态上报就判离线

### `websocket error: ... sec-websocket-key ...`
确认你在最新版本：
`/opt/relay-node/relay-node --version`。如果还出现，可能是你的反代没透传
WebSocket Upgrade 头 —— 但注意 WS 只是控制通道，转发和状态上报走的是纯
HTTP，不受影响。

### WebSocket 每隔约 2 分钟断开重连
节点每 25 秒发一次心跳 Ping，连接不会被当空闲。如果仍然周期性
断开，可能是反代 / CDN 的空闲超时短于 25 秒，或没转发 Pong 帧。偶尔重连
无害（配置会重新同步），但如果很频繁，检查反代的 WebSocket 超时设置。
注意：任何 WS 中断期间节点照常转发。

### 转发不通（连不上监听端口）
- 确认端口在监听：`ss -tlnp | grep <端口>`（TCP）/ `ss -ulnp | grep <端口>`（UDP）
- 检查服务器防火墙 / 云安全组是否放行了该端口入站
- 检查规则的目标地址从节点能否访问到

### 更新时报 "Text file busy"
说明旧二进制还在运行时就被替换了。脚本会先停服务来避免这个；如果还是遇到，
手动执行 `systemctl stop relay-node` 后再跑脚本。

### 连接数一直是 0
如果没有用户连接，显示 0 是正常的。TCP 连接在建立/断开时计数；UDP 会话按
（客户端, 规则）统计，60 秒无数据后过期。产生真实流量后连接数就会动。

### 下载 GitHub 很慢
GitHub Releases 在国内访问可能很慢。可以：
- 用代理下载：脚本支持 `-p socks5://127.0.0.1:10808`（或其他代理）
- 或用镜像 / CDN 加速地址，手动下载二进制后放到 `/opt/relay-node/`

---

## 注意事项

1. **仅支持 Linux。** 没有 Windows/macOS 节点二进制。面板可以跑在 Docker 里，
   但转发节点必须是 Linux。

2. **用 root 或 systemd 运行。** 监听 1024 以下端口需要 root 或
   `CAP_NET_BIND_SERVICE`。脚本的 systemd 服务会处理这个。

3. **WebSocket 是可选的。** 节点用纯 HTTP 做配置轮询兜底和状态上报。WebSocket
   只是实时推送通道，让配置变更更快同步。如果你的反代不支持 WS，转发和状态
   上报照常工作，只是配置变更会延迟到下一次轮询（每 `POLL_INTERVAL` 秒）。

4. **离线兜底。** 面板挂了时，节点会用最后一次收到的配置继续转发（缓存在
   `config-cache.json`）。面板不可达时节点**不会**停掉已有的监听。状态上报
   失败会静默跳过，面板恢复后自动续上。

5. **不要在运行时直接改二进制。** Linux 会报 "Text file busy"。一定要先停服务
   （脚本会自动做这一步）。

6. **公网 IP 检测依赖外部服务**（默认 ipify）。如果你的节点除了面板外没有
   其他出网，可以把 `PUBLIC_IP_CHECK_URL` 指向你自己的回显 IP 服务，否则
   面板上公网 IP 显示「-」。这不影响转发。

7. **日志级别。** 默认 `RUST_LOG=info` 会显示启动、连接、监听、以及每次上报
   的 `report_traffic HTTP 200` 状态行。详细的周期指标（`report_status: cpu=...
   mem=...`）和连接建立/断开事件在 `debug` 级别，所以 `info` 在健康节点上**不会**
   刷屏。想更安静就设 `RUST_LOG=warn`（只显示警告/错误）。只有排查问题时才设
   `RUST_LOG=debug`（会打印每次状态上报 + 每次连接建立/断开）。

8. **转发传输方式。** 业务转发当前支持 `raw`、`ws` 和 `tls_simple`。
   其中 `ws` 是明文 WebSocket 转发；`tls_simple` 由 relay-node 直接终止 TCP
   TLS，证书通过节点侧 `TLS_CERT_PATH` / `TLS_KEY_PATH` 配置。业务 `wss` 已取消，
   如需面板管理界面 HTTPS，请使用外部反代或 Compose Caddy。

---

## 版本固定

一行脚本（`scripts/relay-node-install.sh`）总是从 `main` 分支拉取，它下载的
二进制版本由脚本自身的 `SCRIPT_VERSION` 决定。所以 `main` 上的脚本总是装
**最新**版本。

如果你需要**固定某个旧版本**（比如暂时留在某个版本测试），**不要**用 `main`
的脚本 —— 直接下载那个版本的二进制手动安装（见上文[手动安装](#方式-b手动安装)）：

```bash
# 示例：固定到某个版本，amd64
VERSION=1.0.3
ARCH=amd64   # 或 arm64
curl -fL -o relay-node \
  "https://github.com/MoeShinX/relay-panel/releases/download/v${VERSION}/relay-node-linux-${ARCH}"
```

然后按[手动 systemd 配置](#手动-systemd-配置)运行。

所有已发布版本和资产见 [Releases 页面](https://github.com/MoeShinX/relay-panel/releases)。

---

## Token 与连接安全

`NODE_TOKEN` 是敏感凭据——拿到它就能以该分组节点的身份上报流量、拉取配置。请按下列要求保护它。

### 强烈建议：用 HTTPS / WSS 连接面板

生产环境**务必**让 `PANEL_URL` 指向 `https://`（配合反向代理，见
[REVERSE-PROXY.md](./REVERSE-PROXY.md)）。

- 如果 `PANEL_URL` 是 `http://`，节点上报流量、拉配置、WebSocket 控制通道**全程明文**，
  `NODE_TOKEN` 会以 `Authorization: Bearer ...` 的形式**明文经过网络**，任何中间人
  （被入侵的路由器、ISP、公共 Wi-Fi、抓包）都能截获。
- HTTPS / WSS 下 token 与数据都加密传输，这是最低要求。
- 节点之间的转发流量（listener）是否加密取决于规则自身的 transport（raw/ws/tls），
  与面板连接无关——这里说的是**节点 ↔ 面板**这条控制通道。

### Token 的处理

- 不要把 token 放进 URL（会出现在访问日志、命令历史、浏览器历史、截图里）。
  本项目只从 `Authorization` 请求头读取 token，但你自己执行安装命令时，`-t <TOKEN>`
  会进入 shell 历史——避免在不可信环境粘贴，或事后清理历史。
- 不要把含 token 的对接命令、`start.sh` 截图或粘贴到工单、聊天、issue 里。
- 反向代理 / 面板的访问日志若记录了完整请求头，也会记录 `Authorization`——
  生产环境应关闭或脱敏该字段的日志记录。

### Token 泄露后的处置

- **立即轮换**：在面板「设备分组」里重新生成该分组的 token（旧 token 失效）。
- **同组共享**：当前同一入口分组的所有节点共用一个 token，轮换后该组**全部节点**
  都要用新 token 重新配置（重新跑一行脚本，或手改 `start.sh` 后 `systemctl restart`）。
  这是为什么 token 一旦泄露影响面较大——建议每个物理节点一个独立分组，便于最小化轮换范围。
- 轮换只让旧 token 失效；已经上报过的流量统计不会被回滚。

### 信任模型（务必理解）

- 面板**信任受控节点上报的计量数据**（流量、连接数、状态）。面板无法独立验证某个连接
  真的发生了——它只能记录持有有效 token 的节点上报的数字。一个被入侵的节点可以少报、
  多报或伪造流量数字。
- 因此 token 的保护等价于「谁能影响你的计费与配额」。不要把它发给不受信任的人，
  也不要部署到不受信任的机器上。配额是**软限制**，依赖节点诚实上报。

---

## 相关文档
- [DEPLOYMENT.md](./DEPLOYMENT.md) —— 面板本身的部署（Docker Compose）
- [NODE.md](./NODE.md) —— 本文档的英文版
- [../README.md](../README.md) —— 项目总览（中文）
- [../CHANGELOG.md](../CHANGELOG.md) —— 版本历史
