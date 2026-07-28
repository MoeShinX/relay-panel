# RelayPanel 隧道转发实现规格 (入口 ↔ 出口)
状态:设计定稿,代码未开始。本文是给实现方的完整规格。
仓库:E:\ZCode\relay-panel(Rust 后端 + React/TS 前端)

## 1. 目标
国内入口机把 TCP 流量经加密隧道转发到国外落地,降低被 GFW 识别的概率。

非目标:不自研隧道协议;不做 UDP;不改动现有转发行为。

## 2. ⚠️ 动手前必读:现有代码有两处硬锁
### 2.1 forward_mode 被锁死为 direct

crates/panel/src/service/rules.rs 会拒绝任何 forward_mode != "direct" 和非空 device_group_out(提交 10324f2)。不解开这个,隧道规则根本创建不出来。

### 2.2 这套东西砍过两次,原因还在代码里

crates/shared/src/protocol.rs 注释:

```
chain is rejected (node engine not implemented)
```

面板侧的数据模型、API、UI 当年全做完了,唯独节点侧的隧道引擎从没实现。数据模型残骸可复用:device_groups.group_type(in/out)、forward_rules.device_group_out、tunnel_profiles 表。

本规格选择委托 sing-box,正是为了不重蹈"最难那一半没人写"的覆辙。

## 3. 架构
```
客户端
  ▼
入口 relay-node        ← 监听 rule.listen_port,★计费在这里★
  ▼ SOCKS5 (127.0.0.1)
入口 sing-box          ← 面板下发配置
  ▼ VLESS + REALITY,跨墙
出口 sing-box          ← 面板下发配置
  ▼
真实目标 (rule.target)
```

关键性质:出口 relay-node 不在数据路径上。

目标地址由 VLESS 协议本身携带,出口 sing-box 直接连目标。出口的 relay-node 只做控制面:接收配置、写文件、重启 sing-box、上报健康。

由此:重复计费在架构上不可能发生,不需要任何"出口不要上报"的约定兜底。

不影响现有转发:规则不选出口分组 → 走现有路径,代码零改动。可灰度。

## 4. 计费:完全不变
扣减额度 = (上行 + 下行) × 入口分组倍率
入口计数 + 入口分组倍率,就是现在的行为。理由:优质入口本就该卖更贵。

必须写测试钉住的不变量:

- 计数发生在内层数据流,不含 relay-node → 本机 sing-box 的 SOCKS5 握手字节
- 隧道不通导致连接失败 → 零字节 → 不扣额度

## 5. 数据模型
新表 `tunnels`。不复用 tunnel_profiles(它的 tls_mode/ws_path/sni/cert_id 本场景基本用不上)。

| 字段 | 说明 |
|------|------|
| id | |
| name | 显示名 |
| group_in | 入口分组 id,UNIQUE |
| group_out | 出口分组 id,不加唯一约束(见下) |
| protocol | vless_reality(第一版只此一种) |
| listen_port | 出口 sing-box 监听端口 |
| config_json | SNI、short_id、公钥等,不含私钥 |
| secret_json | 私钥等,只写不读 |
| enabled | |

group_in 必须 UNIQUE —— 入口节点必须能唯一确定用哪条隧道,否则配置有歧义。

group_out 不加唯一约束 —— 出口服务多个入口在架构上没有歧义(一个 inbound 可配多个客户端凭据),而「多入口 → 单落地」是自然成长路径。现在不锁,零成本;你不建第二条就是 1:1。

密钥只写不读:照搬 NotifyConfigPublic 对 bot token / SMTP 密码的处理 —— PUT 进去,GET 只回"是否已配置",永不回传明文。

规则侧复用已有的 forward_rules.device_group_out。

## 6. 协议与交接方式(两个耦合决策)
**隧道协议:VLESS + REALITY(TCP/443)**。抗主动探测最强 —— 探测者被转发到真实站点拿到真实响应,无法分辨。Hysteria2 的优势在速度不在隐蔽,且 UDP 在国内易被 QoS。

**本机交接:SOCKS5(第一版 TCP loopback,后续可切 unix socket)**。

⚠️ 这个选择由「重启生效」倒推而来,不能随意改:

sing-box 配置必须与规则无关。规则的增删改不得触发 sing-box 配置变更或重启。

因为 sing-box 用重启生效,而重启会断掉该节点上所有隧道连接。SOCKS5 只需一个固定 inbound,规则怎么改它都不变。

(备选方案"每条规则生成一个固定目标的 sing-box inbound"看似更优雅,但会让每次规则改动都触发重启,必须排除。)

为什么 SOCKS5 而不是自定义 IPC:
- sing-box 只接受标准协议输入。走自定义 IPC 意味着 fork sing-box,违背了"委托给 sing-box、不碰协议层"的前提。
- SOCKS5 对 loopback 的开销极小,且 splice() 同样有效。
- 后续如有 TIME_WAIT / 临时端口压力,只需改为 unix socket(协议不变,承载换掉)。

## 7. UDP:隧道规则强制 TCP
现有约束(rules.rs:60):含 UDP 的协议 ⇒ transport 必须是 raw
新增约束:            绑定隧道的规则 ⇒ protocol 必须是 tcp
复用 validate_protocol_transport 的同一模式和 UI 处理(选了 UDP 就禁用隧道选项并说明原因)。

为什么不允许 tcp_udp 混合:直连的 UDP 会把目标 IP 明文暴露在 IP 头里,观察者能推断同一入口的 TLS 流也去同一目的地 —— 明文那一半会削弱加密那一半的价值,而且运营者不会察觉。强制拆成两条规则,让暴露显式可见。

要 UDP 就单独建一条 raw 规则。

## 8. ⚠️ 两条安全红线
两者形状相同:把本该走隧道的流量悄悄明文发出去。

### 8.1 失败关闭,不降级

入口 sing-box 不可用 → 拒绝该规则的连接,绝不回退直连。
要求:连接被拒 + 规则显式报错(节点状态页可见)+ 日志写明原因。

**更强的做法:结构性保证。** 绑定隧道的规则,代码里根本不存在可达的直连分支。不要让 dial 路径有"先试代理,失败了怎么办"的逻辑 —— 编译期只有一条路径。这比靠人记住"别加 fallback"可靠得多,也不会在半年后被某个"改善可用性"的 PR 好心加回来。

### 8.2 版本门槛:拒绝下发,不静默忽略

新增规则字段要升 CONFIG_PROTOCOL_VERSION(现值 4,见 crates/shared/src/protocol.rs:33)。

老节点忽略未知字段 → 直连转发 → 明文出境。

所以面板必须在下发前拒绝把隧道规则关联到不支持的节点。参照 node_supports_restart_rule(protocol.rs:696)的写法,连同它的测试模式(None/空串/无法解析一律 gate out)。

性质比 restart_rule 更严重:那次是"功能静默失效",这次是"静默降级到不安全状态"。

## 9. 配置下发

### 9.1 通信通道
复用现有 WS 控制通道,新增消息类型 `tunnel_config`,用 NodeConnections::send_node(crates/panel/src/api/ws.rs:118)定向下发。

### 9.2 消息结构
```json
{
  "type": "tunnel_config",
  "version": 1,
  "tunnel_id": 42,
  "group_in": 7,
  "config_hash": "sha256hex",
  "config": "<base64 编码的完整 sing-box 配置 JSON>",
  "secret": "<base64 编码的密钥 JSON>"
}
```

- `version`: 消息格式版本,后续扩展用。
- `tunnel_id`: 对应的 tunnels.id。
- `group_in`: 入口分组 id,用于节点确认身份。
- `config_hash`: config 的 sha256 hex,节点用于比对,变化才写盘+重启。
- `config`: 面板生成的完整 sing-box 配置 JSON(编码后)。
- `secret`: 面板生成的密钥 JSON(编码后)。

### 9.3 下发流程
1. 面板生成两端配置(含 REALITY keypair + short_id)→ 分别下发。
2. 节点收到 `tunnel_config` 消息后,比较 `config_hash` 与本地 cache。
3. 若 hash 不同 → 写 config 到约定路径 → 写 secret 到约定路径 → 重启 sing-box。
4. 若 hash 相同 → 不写盘,不重启。
5. 必须实现:下发前比对配置哈希,无变化不重启。否则每次心跳都可能断连。

### 9.4 离线同步
节点离线后重连,通过现有的 WS 推送 + 10 秒 HTTP 轮询兜底机制,隧道配置搭同一班车同步,不需要新造通道。

## 9.5 配置生成归属
**面板生成完整配置,节点只做"写入 + 比较 + 重启"**。

节点永远不需要理解 sing-box 配置格式。面板是唯一的配置生成方,节点只负责:
1. 接收 config + secret(编码后)
2. 比较 config_hash
3. 写文件到约定路径
4. 重启 sing-box

## 10. UI
- forwardRules(转发规则)/ myRules(我的规则)→ 改名 单端转发(frontend/src/i18n/zh-CN.ts:23,58 + en-US 对应行)
- 新增 隧道转发 页(独立页面,不与单端混表)
- 新增 隧道管理 页(管理员):建隧道、选入口/出口分组、看两端状态

## 11. 必须有的测试
- SOCKS5 握手成功/失败
- 隧道不通 → 连接被拒,且未发生直连(最关键)
- 计费不变量:计数 == 内层字节,不含握手
- 老节点关联隧道规则 → 面板拒绝(参照 node_supports_restart_rule_version_gate 的测试写法)
- 隧道规则 protocol 非 tcp → 拒绝
- 密钥材料不出现在任何 GET 响应
- 配置无变化 → 不触发重启
- SQLite / PG 双实现对等(scripts/check-repo-test-parity.sh 会强制)

## 12. 分阶段
| 阶段 | 内容 |
|------|------|
| 0 | 手工验证链路。不动代码,国内节点手工装 sing-box + REALITY,实测延迟/速度/稳定性。沉没成本≈0,链路不成立则后面都不用做 |
| 1 | tunnels 表 + 解锁 forward_mode + 生成配置供复制粘贴 + 计费口径验证 |
| 2 | WS 下发 + 节点管理 sing-box 进程。工作量大头 |

## 13. sing-box 安装与生命周期
### 13.1 安装方式
扩展 scripts/relay-node-install.sh,从 sing-box 官方 Release 下载。复用已有的 uname -m 架构检测、-p 代理支持和 RELAY_NODE_BASE_URL 镜像机制。

**不随包分发**:随包分发要维护 amd64/arm64 两套大二进制,拖累每次 relay-node 发版,且得跟着 sing-box 的发布节奏走。

### 13.2 版本锁定
必须 pin 版本,不能取 latest。建议:
1. 脚本里 pin 一个已验证版本
2. 把这个版本号放进面板配置,集中升级、灰度,而非重装所有节点

### 13.3 进程管理
sing-box 作为独立的 systemd unit,`Restart=always` —— 和 relay-node 自己的托管方式完全一致。

relay-node **不要**当进程管理器(不自行管理 sing-box 生命周期)。它只负责:
- 健康探测本机 SOCKS5 端口
- 探测失败 → 上报"隧道不可用"

这样即使 sing-box 在反复崩溃重启,面板也能看到状态异常。

## 14. 实现注意事项
- 入口节点上的 sing-box 配置由隧道规则决定,而非转发规则。规则增删改不触发 sing-box 配置变更。
- 一个入口分组最多一条隧道(group_in UNIQUE),所以一个入口节点最多一个 sing-box 实例。
- 失败关闭是结构性保证:编译期只有一条 dial 路径,不存在 fallback 分支。
- 计费计数发生在 sing-box 内层数据流,不计 SOCKS5 握手字节。
