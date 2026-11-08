# Meito 设计计划：GitHub Actions 运行器上的点对点交互会话

> 文档版本：0.2.0（设计评审稿）
> 日期：2026-11-08
> 状态：设计已成型，未进入实现；**runner 内行为全部未实测**，无 GitHub 凭证，不得把本文件中的外部行为写成“已通过”。
> 已汇入：调研线结论、无中继硬约束、双入口设计、一套内核/两个外壳、研发规范与架构约定、消费者视角对比结论。
> 设计入口：本文件。实施入口、任务派发与验收记录统一从 [README](../README.md) 进入。
> 规则来源：`C:\Users\Administrator\Desktop\工程经验库\` 的 `README.md`、`铁律.md`、`开工清单.md`。本文只按编号引用，不复制经验库正文。

---

## 0. 一页结论

Meito 要在 GitHub 托管 runner 上获得一个可交互会话，用于 `make menuconfig` 这类必须人工操作的配置工作，并把生成的 `.config` / `defconfig` 带回本地仓库。

硬约束决定了方案不能再走中继：

1. tmate 公共服务器已永久停服；`action-tmate` 仍可维护，但默认配置不可用，只能指向自建 `tmate-ssh-server`。
2. upterm 是目前唯一开箱即用的组合；`action-upterm` v2 不再套 tmux，终端尺寸跟随接入方，v1 固定 132x43 会挤坏全屏界面。但 uptermd 仍是中继。
3. 用户现有公网小服务器**只允许打洞/协调，不允许承载会话字节**。因此 uptermd、tmate-ssh-server、cloudflared、ngrok、Cloudflare Named Tunnel 全部排除。
4. 托管 runner 单 job 最长 6 小时，到点直接 fail；runner 无公网入站，只能出站建隧道。
5. runner 是否允许出站 UDP、NAT 打洞成功率、ttyd/noVNC 在 runner 上的安装与渲染，均**未验证**。

推荐方案：

- **P2P 主路径：frp `xtcp`**。frps 只交换 peer 信息，数据走 UDP 直连；禁止配置 `fallbackTo` / `stcp`，避免任何字节回流服务器。
- **P2P 备路径：Headscale/Tailscale 直连模式**。控制面在服务器，数据走 WireGuard；必须禁用 DERP、禁用 peer relay，打洞失败即失败，不允许外部中继兜底。
- **接入层：SSH over P2P + 本地端口转发**。runner 上只跑回环服务；终端入口是 `ttyd -W`，图形入口是 `Xvfb + 轻量 WM + x11vnc + websockify`。两者都只绑 `127.0.0.1`，由 SSH 本地转发进入，再由本地 token 网关鉴权。
- **产物回传：artifact 优先**。会话结束上传 `.config`、`defconfig`、diff、日志；本地 `meito pull` 校验哈希并三方应用。提交到远端分支是可选路径，不是默认路径。
- **并行降依赖：menuconfig 改 defconfig 化**。`scripts/config` + `savedefconfig` 的非交互路线与交互会话并行建设，减少必须开 shell 的次数。
- **交付形态：一套内核、两个外壳**。内核用 Rust，本地版是 CLI，网页版是浏览器打开的单页工作台；会话编排、隧道策略、鉴权、artifact 回传、日志只有一份实现。
- **网页版不新增中继**：浏览器只能使用 WebRTC DataChannel + STUN 直连；**禁用 TURN**，ICE 直连失败即失败。对称 NAT 或 UDP 被封时连不上是常态，必须给出可解释状态并引导转本地版；产品文案不得宣称任意网络可连。
- **降级顺序：先 P2P 备路径，再非交互路线，最后本地容器/自托管 runner；任何情况下都不静默回落到中继。**

本设计的“自研”范围：Rust 内核、本地 CLI、网页单页外壳、状态机、workflow 编排、远端引导、双入口适配、artifact 回传、生命周期、安全策略、诊断与降级。frp / Tailscale / WebRTC 是网络组件，不重写它们的协议，也不把它们的库链接进数据面。

### 0.1 交付形态：一套内核、两个外壳

交付两种形态，但**会话编排、隧道协商、鉴权策略、入口生命周期、artifact 回传、日志与状态机只有一份实现**：

| 外壳 | 用户入口 | 传输适配 | 界面 |
|---|---|---|---|
| **本地版** | `meito` CLI | frp xtcp / Tailscale 直连 + SSH | 终端命令、浏览器（ttyd/noVNC） |
| **网页版** | 浏览器打开托管单页 | 轻量 WebSocket 语义适配（底层 WebRTC DataChannel 直连，禁用 TURN） | 单页工作台（xterm.js/noVNC，按需启动） |

- 技术栈：内核用 **Rust**；打洞层不重写协议，以外部进程调用 `frpc` 二进制；网页版不使用 SPA 框架，单页 HTML/CSS/JS + 轻量 WebSocket；本地版以 CLI 为主。
- 内核 `meito-core` 负责：会话状态机、actor/令牌鉴权、P2P 协商与无中继校验、入口启停、artifact 清单与冲突规则、日志/错误码。
- 两个外壳只做：入口差异（CLI 参数/浏览器 UI）、传输适配（SSH 本地转发 / WebRTC DataChannel）、身份获取方式（gh 令牌 / 网页 OAuth）。
- 禁止在 `crates/meito-web` 或 `crates/meito-cli` 复制会话编排、GitHub dispatch、artifact 清单、P2P 策略；CI 增加架构检查（见 5.9）。
- 网页版不能直接用 frp/SSH：浏览器只能使用 WebRTC。因此传输适配器允许不同，但**策略与业务内核仍是同一份**；WebRTC 侧禁止 TURN，ICE 直连失败即失败。
- 网页版的“轻量 WebSocket”只承载控制/信令；终端与图形字节走 WebRTC DataChannel 直连，服务端 WebSocket 不承载会话流量。
- 网页版的静态单页可以托管在 GitHub Pages、对象存储或任意静态站点；**不新增中继**。控制/信令由用户自己的公网小服务器承担，只传 peer 信息与密文，不传终端/图形字节。
- 网页版产物回传：浏览器直接从 GitHub artifact 下载 `.config` / patch，或复制链接；不经过用户服务器。本地版的 `meito pull --apply` 才负责落到仓库并处理冲突。

---

## 1. 目标与非目标

### 1.1 目标

| ID | 目标 | 可判定说明 |
|---|---|---|
| G1 | 本地一条命令进入 runner 会话 | `meito up` 成功后在本地打印可访问的终端/图形入口，并且入口健康检查通过 |
| G2 | 能完成 `make menuconfig` 全屏交互 | 方向键、保存、退出均可用，生成的 `.config` 可被校验 |
| G3 | 改动回到本地仓库 | `meito pull` 下载 artifact，哈希一致，diff 可应用 |
| G4 | 会话不把字节送回用户服务器 | 服务器侧计数与独立大文件传输对照满足判据（见 9.4） |
| G5 | 会话可被可靠收尾 | 到时、断线、取消、孤儿四种情况都有确定状态和清理结果 |
| G6 | 失败可诊断 | 每次会话有统一 `session_id` 的本地与 runner 日志，错误码明确 |
| G7 | 打洞失败有非中继降级 | 明确给出 defconfig、本地容器、自托管 runner 三条路径，不静默走中继 |
| G8 | 仓库可持续迭代 | 门禁、开工清单、分支登记、变更记录、CI、版本与出包规则可执行 |

### 1.2 非目标

- 不做通用 SSH 跳板、VPN、远程桌面或终端分享服务。
- 不做长期在线的自托管 runner；自托管 runner 只作为降级路径，不在本期默认交付。
- 不支持超过 6 小时的托管 runner 会话；超过 6 小时的需求走自托管 runner 或非交互构建。
- 不默认支持公有仓库交互会话；公有仓库必须显式开启并接受额外风险。
- 不把生产凭据放进调试 job；不承诺 GitHub 日志脱敏能保护秘密。
- 不替代编译缓存、构建编排、镜像发布。
- v1 只支持 `ubuntu-latest`/Linux x86_64；Windows/macOS runner 留到后续版本。
- 不承诺打洞成功率达到某个百分比；NAT 类型不可控，只承诺失败可判定、可诊断、可降级。

---

## 2. 成功判据

以下判据在实现阶段逐条执行；**当前全部为设计判据，除注明“本次已验证”的条目外均未实测**。本节出现的延迟、采样次数、传输量、时长、端口与状态码以 `contracts/acceptance.json`、`contracts/entry.json`、`contracts/state-machine.json` 为唯一来源，本文只做索引，不重复定义。

| ID | 判据 | 判定方式 | 当前状态 |
|---|---|---|---|
| SC-01 | 冷启动到入口可用 ≤ 180 s（p95），10 次采样 | 记录 dispatch 时间、入口 HTTP 健康检查时间；报告 p50/p95 | 未验证 |
| SC-02 | `make menuconfig` 可交互并保存 `.config` | 在 ttyd 中执行，方向键移动、保存、退出；对比 runner 与本地 `.config` 的 SHA256 | 未验证 |
| SC-03 | `meito pull` 带回 `.config` 与 `defconfig` | manifest 中 SHA256 与本地文件一致；`git diff` 只包含预期文件 | 未验证 |
| SC-04 | 未授权密钥无法接入 | 用另一把 SSH 私钥连接，10 s 内返回 `Permission denied (publickey)`；无 shell | 未验证 |
| SC-05 | 服务器不承载会话字节 | 100 MB 传输 3 次；服务器协调端口字节增量 < 5% 负载，且服务器抓包无数据流（见 9.4） | 未验证 |
| SC-06 | 入口只绑回环 | runner 上 `ss -ltnp` 中 7681/5900/6080 只出现 `127.0.0.1`；无 `0.0.0.0` | 未验证 |
| SC-07 | 图形入口 token 有效、VNC 密码不作为主鉴权 | 无 token 请求返回 401/403；仅持 VNC 8 位密码无法进入；token 正确后可操作 | 未验证 |
| SC-08 | 会话按时收尾 | `max_minutes` 到达后 ±60 s 内 job 结束；空闲 `idle_minutes` 后 ≤5 min 内结束；`meito down` 后 ≤60 s 取消/结束 | 未验证 |
| SC-09 | 无孤儿会话 | 收尾后 `gh run list` 中无该 workflow 的 active/queued 运行；`meito clean --orphans` 可清理人为制造的孤儿 | 未验证 |
| SC-10 | 日志链路完整 | 每次会话本地与 runner 都有同一 `session_id` 的日志；失败含错误码、阶段、证据路径 | 未验证 |
| SC-11 | 日志不泄露连接串/令牌/私钥 | 对本地与 runner 日志做正则扫描，命中数为 0；artifact 中不含连接串 | 未验证 |
| SC-12 | job 硬上限前主动收尾 | `max_minutes` 默认 60 min、硬上限 300 min；watchdog 在 300 min 前结束并上传 checkpoint；不依赖 360 min 的强制 fail | 未验证 |
| SC-13 | 非交互 defconfig 路线可复现 | 同一输入文件连续 2 次生成 `defconfig`，SHA256 相同；与 menuconfig 样本对比差异可解释 | 未验证 |
| SC-14 | 门禁与仓库初始化可用 | `scripts/gate.ps1 -SelfTest` 退出码 0；全量门禁退出码 0；`docs/分支归属.md` 六行已填 | 本次已验证：SelfTest PASS、全量 0 violations、首个提交 `7bb9335`（见变更记录） |
| SC-15 | 新用户按 README 快速开始可完成 | 另一台干净机器按 README 操作，15 min 内到达入口；记录实际失败点 | 未验证 |
| SC-16 | 网页版只走 WebRTC DataChannel + STUN，无 TURN | 浏览器 `RTCPeerConnection.getStats()` 中 selected candidate pair 为 `host`/`srflx`，无 `relay`；配置中 TURN URL 为空；强制禁用 TURN 后仍能连通，或明确返回 `E_WEB_NO_DIRECT_PATH` | 未验证 |
| SC-17 | 一套内核、两个外壳 | `scripts/arch-check.ps1` 退出 0；`crates/meito-web` 与 `crates/meito-cli` 不含会话状态机/artifact 清单/P2P 策略的重复定义；两个外壳都依赖同一 `meito-core` 公共接口 | 未验证 |
| SC-18 | 接口冻结可判定 | `docs/接口冻结.md` 记录公共 trait/命令/事件/日志字段/错误码/版本；冻结后两个外壳以 stub 编译通过；冻结后修改有变更记录与版本递增 | 本次已验证：`contracts/` 0.1.0 frozen；`cargo check/clippy/test` 通过；`contract-check` 与 `arch-check` 退出 0（见变更记录 M1） |
| SC-19 | 可写/只读双链接 | 只读链接无法执行输入、无法保存文件；写链接需一次性 token；无人加入超过 `no_join_timeout` 自动取消并上传空会话日志 | 未验证 |
| SC-20 | 网页版“连不上”可解释 | 对称 NAT/UDP 被封时返回 `E_WEB_NO_DIRECT_PATH`，页面显示原因、已尝试的 ICE 类型、转本地版的命令；文案不出现“任意网络可连” | 未验证 |
| SC-21 | 连接串进 Job Summary 但不泄密 | Job Summary 只含 session_id、状态、入口类型、过期时间、只读/写链接标记；`artifact_guard` 扫描连接串/令牌命中数为 0 | 未验证 |

判据纪律（R-D-3、R-D-4、R-D-5）：判据先写进 `docs/验收记录/`，再执行；不达标不改阈值、不放宽判据、不把“没报错”写成“通过”。未跑通项统一写“未验证”。

---

## 3. 用户使用流程

### 3.1 前置条件

两种外壳的公共前置与差异如下：

| 前置项 | 本地版（CLI） | 网页版（浏览器单页） |
|---|---|---|
| 目标仓库 | 默认分支已安装 Meito workflow 模板 | 同左；另需网页版控制服务能读取该仓库的 workflow 状态 |
| 本地依赖 | `git`、`gh`、OpenSSH 客户端、`frpc`（或 `tailscale`）；Rust 工具链仅在源码构建时需要 | 现代浏览器（WebRTC DataChannel + WebSocket）；无需安装 SSH/frpc |
| 身份 | `gh auth login`，fine-grained PAT：Actions read/write、Contents read、Metadata read | GitHub OAuth 登录；权限同上；令牌不落浏览器持久存储 |
| 网络 | 能访问 GitHub、协调服务器、runner 的 P2P UDP | 能访问静态页、控制/信令服务、GitHub API；UDP 打洞失败即失败，不走 TURN |
| 协调服务器 | frps + `meito-link`，只交换 peer 信息与密文 | 同左；另托管静态单页与 WebSocket 信令，**不承载终端/图形字节** |
| 秘密 | `MEITO_FRPS_TOKEN`、可选 `MEITO_TAILSCALE_AUTHKEY`；不配置生产凭据 | 同左；OAuth 令牌由控制服务短期持有，不进日志、不进 artifact |

补充约束：

1. 目标仓库默认分支上必须有 Meito workflow 模板；`workflow_dispatch` 只认默认分支上的 workflow 文件。
2. 协调服务器只做打洞/协调与密文交换，不允许承载会话流量；网页版静态托管可以是 GitHub Pages/对象存储，不新增中继。
3. 仓库变量/秘密：`MEITO_FRPS_TOKEN`（frps 控制鉴权）、可选的 `MEITO_TAILSCALE_AUTHKEY`；交互 job 与生产 job 分离，不注入生产凭据。
4. 本地版与网页版共用同一套内核配置；网页版不得要求用户安装 `frpc`/`tailscale`，这些进程由 runner 侧和协调侧承担。

### 3.2 从一条命令到 shell（本地版）

| 步骤 | 本地/远端 | 动作 | 失败时 |
|---|---|---|---|
| 1 | 本地 | `meito doctor --repo owner/repo --deep`：检查 gh 权限、默认分支 workflow、服务器连通性、本地 UDP、frpc/ssh、磁盘 | `E_PREFLIGHT`，打印缺失项与修复命令 |
| 2 | 本地 | `meito up --ref main --max 60m --idle 10m --entry terminal`：生成 `session_id`、一次性 SSH 密钥、通道密钥与 token，向 `meito-link` 注册写通道 | `E_LINK` / `E_AUTH` |
| 3 | 本地 | 调 GitHub API `workflow_dispatch`，输入不含任何秘密 | `E_DISPATCH`，保存 run 列表证据 |
| 4 | runner | 校验 actor 白名单、仓库、默认分支、workflow 版本；预检 UDP/DNS/磁盘 | `E_ACTOR_NOT_ALLOWED` / `E_UDP_BLOCKED`，job 立即失败，无会话 |
| 5 | runner | 安装/校验锁定版本的 frpc、ttyd、图形依赖；启动本机 sshd（只绑 127.0.0.1）；启动 frpc xtcp（无 `fallbackTo`） | `E_BOOTSTRAP` / `E_P2P` |
| 6 | runner | 生成一次性主机密钥，组装连接包；用本地公钥加密后 POST 到 `meito-link`；不打印连接串 | `E_LINK`，watchdog 结束 |
| 7 | 本地 | 轮询 `meito-link`，解密连接包；启动 frpc visitor；建立 SSH master；校验主机密钥 pin | `E_P2P` / `E_HOSTKEY` |
| 8 | 本地 | 建立本地转发：`127.0.0.1:<local_terminal> -> runner:127.0.0.1:7681`；启动本地 token 网关；打开浏览器 | `E_ENTRY` |
| 9 | 用户 | 在 ttyd 里执行 `make menuconfig`，保存 `.config`；可切到图形入口 | `E_ENTRY` / 用户操作 |
| 10 | 本地 | `meito pull --apply`：下载 artifact，校验 manifest 与 SHA256，三方应用 diff；冲突进 `.meito/conflicts/<sid>/` | `E_ARTIFACT` / `E_APPLY_CONFLICT` |
| 11 | 本地 | `meito down`：通知 runner 收尾、等待 artifact 上传、撤销通道、删除本地临时密钥、打印日志路径 | `E_CANCEL`，可 `--force` 后 `gh run cancel` |

端口与回环绑定以 `contracts/entry.json` 为准；时长与采样以 `contracts/acceptance.json`、`contracts/state-machine.json` 为准，本表只做索引。

### 3.2.1 从打开网页到 shell（网页版）

| 步骤 | 位置 | 动作 | 失败时 |
|---|---|---|---|
| 1 | 浏览器 | 打开托管单页，GitHub OAuth 登录，选择仓库/分支/时长/入口 | `E_AUTH` / `E_PREFLIGHT` |
| 2 | 浏览器 → 控制服务 | 创建会话：控制服务调用同一 `meito-core` 内核完成 dispatch 与通道注册 | `E_DISPATCH` / `E_LINK` |
| 3 | runner | 同 3.2 的 4–6 步；runner 额外启动 WebRTC peer（DataChannel），只连 STUN，不配 TURN | `E_UDP_BLOCKED` / `E_P2P` |
| 4 | 浏览器 ↔ runner | 通过控制服务交换 SDP/ICE（仅信令），建立 DataChannel 直连 | `E_WEB_NO_DIRECT_PATH`（对称 NAT/UDP 被封） |
| 5 | 浏览器 | DataChannel 上复用终端/图形字节流；写链接可输入，只读链接只能看 | `E_ENTRY` |
| 6 | 浏览器 | 会话结束展示 artifact 链接，直接下载 `.config`/patch；不经过用户服务器 | `E_ARTIFACT` |
| 7 | 浏览器 | `meito down` 等价操作：控制服务通知 runner 收尾、撤销通道 | `E_CANCEL` |

网页版连不上时，页面必须显示：已尝试的 candidate 类型、失败阶段、`E_WEB_NO_DIRECT_PATH`、以及“改在本机运行 `meito up ...`”的命令；不得只显示“连接失败”。

### 3.3 图形入口切换

```text
meito up --entry graphical --max 30m
meito entry switch vnc
meito entry switch terminal
meito entry open
```

默认入口是 `terminal`。`graphical` 只在用户显式请求时安装并启动，避免每次会话都付出 Xvfb/noVNC 的安装与 CPU 代价。`both` 仅用于排障，不作为默认。

### 3.4 非交互替代路径

```text
meito kconfig --from defconfig --set CONFIG_FOO=y --set CONFIG_BAR=n --save defconfig
meito kconfig --from .config --file meito.kconfig --apply
```

这条路径不打开交互 shell，直接在 workflow 里执行 `scripts/config`、`olddefconfig`、`savedefconfig`，把 `.config` 和 `defconfig` 作为 artifact 回传。它不能替代探索式 menuconfig，但能覆盖大多数“已知要改哪些项”的场景，降低对交互会话的依赖频次。

---

## 4. 技术选型

### 4.1 硬约束

- **服务器只做协调**：任何方案，只要会话字节经过用户服务器，就排除。
- **runner 无公网入站**：只能在 runner 内主动出站建立 P2P/控制连接。
- **6 小时 job 硬上限**：不设计超过 6 小时的会话；到时直接 fail，不能依赖优雅收尾。
- **不使用任何中继兜底**：`stcp`、`fallbackTo`、DERP、peer relay、Cloudflare/ngrok、uptermd、tmate 服务器全部禁止作为默认或静默降级。

### 4.2 方案对比

| 方案 | 数据路径 | 成本 | 安全 | 可靠性 | 结论 |
|---|---|---|---|---|---|
| tmate 公共服务 | 经 tmate 服务器 | 0，但已停服 | 会话串+token；公共服不可控 | 不可用 | 排除 |
| tmate 自建服务端 | 经自建服务器 | 需服务器常驻 | 服务器可见字节；需加固 | 依赖运维；tmux 尺寸需验证 | 违反硬约束，排除 |
| upterm / action-upterm | 经 uptermd | 公共 0；自建需服务器 | GitHub 身份白名单好；仍为中继 | 公共服有停服先例；v2 尺寸行为好 | 违反硬约束，排除 |
| upterm + 自建 uptermd | 经自建服务器 | 需服务器 | host key 必须 pin；v2 缺 `known-hosts` 会启动失败 | 可控但运维重 | 违反硬约束，排除 |
| cloudflared Quick/Named Tunnel + sshd/ttyd | 经 Cloudflare | 域名/账号 | Access 可加固；仍为中继 | Quick Tunnel 仅 HTTP 且限流；Named 稳定 | 违反硬约束，排除 |
| ngrok + sshd | 经 ngrok | 免费需账号/支付方式；付费更稳 | 第三方可见元数据；仍为中继 | 较稳但有额度 | 违反硬约束，排除 |
| ttyd + Cloudflare Named Tunnel | 经 Cloudflare | 域名/账号 | `-W`、Access、token；仍为中继 | 稳定但依赖 Cloudflare | 违反硬约束，排除；ttyd 仅作为入口保留 |
| 自建通用 relay | 经自建服务器 | 服务器+开发 | 需自研鉴权/限流 | 可控 | 违反硬约束，排除 |
| **frp `xtcp`** | **frps 只交换信息；数据 UDP 直连** | **复用现有服务器；软件开源** | **SSH 二次加密；frp token + 每会话 secret；无 stcp 回落** | **取决于 NAT；失败可判定** | **推荐主路径** |
| **Headscale/Tailscale 直连** | **控制面在服务器；数据 WireGuard 直连；禁用 DERP** | **复用服务器；软件开源** | **WireGuard 加密；ACL/tag 可收紧** | **NAT 穿透更强；但 TUN/userspace 与 DERP 禁用需验证** | **推荐备路径** |
| 自托管 runner | 不经隧道；SSH/VPN 直达 | 服务器/机器常驻 | 持久化面大，需隔离 | 高，但不临时 | 降级路径 |
| 本地容器 + act | 不经 runner | 本地资源 | 无云端暴露 | 与 GitHub runner 不等价 | 离线降级 |

### 4.3 推荐与理由

**主路径：frp `xtcp` + SSH 传输 + ttyd/noVNC 入口。**

理由：

1. 只要求 frps 交换 peer 信息；数据路径是 visitor 与 runner 之间的 UDP 直连，满足“字节不过服务器”。
2. frpc/frps 是用户态组件，不要求 runner 有 TUN 设备；托管 runner 的 `/dev/net/tun` 是否可用未验证，frp 避开了这个不确定项。
3. 没有像 Tailscale 那样的 DERP 默认兜底；只要不配 `fallbackTo`，打洞失败就是失败，不会静默走中继。
4. 服务器侧可用端口计数与抓包做独立证明，判定“没有回流”比 Tailscale 的 DERP 状态判断更直接。
5. SSH 作为传输层，给 ttyd/noVNC 提供端到端加密、密钥鉴权和本地端口转发；入口服务无需暴露到任何非回环地址。

代价：XTCP 的打洞成功率取决于双方 NAT 类型，稳定性不保证；失败率高时用 Tailscale 备路径或非交互路线。

**备路径：Headscale/Tailscale 直连模式。**

理由：WireGuard 加密、NAT 穿透更成熟、可做 ACL/tag。使用条件：

- runner 必须有可用 TUN，或以 userspace 模式配合可用的本地代理；两种都未验证。
- 必须关闭 DERP：`OmitDefaultRegions=true`，不配置自定义 DERP，不允许 peer relay；打洞失败即失败。
- 若只能通过 DERP 连通，视为违反硬约束，直接失败，不得继续。

**明确不选**：所有中继方案（tmate、upterm、cloudflared、ngrok、自建 relay）。即使它们更稳定，也不满足本次硬约束。

### 4.4 多方案并存与切换

P2P 层抽象为 `P2pProvider`：

| 能力 | frp_xtcp | tailscale_direct |
|---|---|---|
| 准备 | 锁定 frpc/frps 版本，写配置 | 加入 tailnet，关闭 DERP |
| 启动 runner 侧 | frpc 注册 xtcp proxy | tailscaled 连接，得到直连地址 |
| 启动本地侧 | frpc visitor 绑定本地端口 | 本地 tailscaled 连接 |
| 本地端点 | `127.0.0.1:<visitor_port>` | tailnet IP 或本地转发 |
| 证明无中继 | 服务器端口计数 + 抓包 + 大文件对照 | `tailscale status/ping` + DERP map 空 + 服务器计数 |
| 失败码 | `E_P2P_XTCP` | `E_P2P_TS` |
| 允许中继兜底 | 否 | 否（DERP/peer relay 均禁止） |

配置项 `p2p.provider` 可选 `frp_xtcp`（默认）或 `tailscale_direct`；`auto` 只按顺序尝试这两者，**不会尝试任何中继**。任何 provider 失败都必须落盘证据，再由 CLI 提示降级路径。

### 4.5 网页版传输选型：WebRTC DataChannel + STUN，禁用 TURN

浏览器不能直接使用 frp/SSH，但可以建立 WebRTC DataChannel。选型结论：

| 项 | 决定 | 理由 |
|---|---|---|
| 传输 | `RTCPeerConnection` + DataChannel | 浏览器原生、二进制透传、低延迟 |
| NAT 穿透 | 只用 STUN（host/srflx candidate） | STUN 只帮助发现地址，不转发字节 |
| 中继 | **禁用 TURN** | TURN 是字节中继，违反“服务器不承载会话流量” |
| 信令 | 控制服务上的轻量 WebSocket | 只交换 SDP/ICE/控制事件，不承载终端/图形字节 |
| 失败 | `E_WEB_NO_DIRECT_PATH` | 对称 NAT/UDP 被封时无法连通是常态，必须可解释并转本地版 |
| 文案 | 不得宣称“任意网络可连” | 产品承诺必须与硬约束一致 |

网页版与本地版共用 `P2pProvider` 的策略判定和状态机；WebRTC 只是 `meito-web` 的传输适配器。ICE 失败时由内核统一产生 `E_WEB_NO_DIRECT_PATH`，而不是让前端自行编造错误。

### 4.6 待拍板：用户自带中继（BYO TURN）

消费者线报告提出一个待用户拍板的选项：是否允许“用户自带中继（BYO TURN）”作为**默认关闭**的可选兜底。设计先预留接口位置，两种分支都成立：

| 分支 | 行为 | 默认值 | 需要的改动 |
|---|---|---|---|
| A：不允许 BYO TURN | `[turn].enabled=false` 且不可由会话输入开启；TURN URL 永远为空 | 当前默认 | 无 |
| B：允许 BYO TURN | 用户显式配置自己的 TURN；控制服务不托管 TURN；日志标注“使用了中继”；不计入“无回流”会话 | 待拍板 | 增加 `TurnPolicy` 适配器、UI 明示、验收判据分裂 |

在用户拍板前，代码只实现 A；B 的接口（`TurnPolicy`、`[turn]` 配置、`E_WEB_RELAY_USED` 错误码占位）保留但默认关闭。任何情况下都不能把 BYO TURN 设为默认，也不能让 Meito 自己的服务器变成 TURN。

---

## 5. 组件设计

### 5.1 总体架构

```text
本地版外壳                                    网页版外壳
┌────────────────────┐                    ┌────────────────────┐
│ crates/meito-cli   │                    │ web/ 单页 +        │
│ - 命令解析/展示    │                    │ crates/meito-web   │
│ - SSH 本地转发     │                    │ - 静态托管          │
│ - frpc visitor     │                    │ - 轻量 WebSocket    │
└─────────┬──────────┘                    │ - WebRTC 适配       │
          │                               └─────────┬──────────┘
          │        ┌──────────────────────────────┐ │
          └───────>│ crates/meito-core（唯一内核） │<┘
                   │ Model：会话状态机/鉴权/策略   │
                   │ Controller：命令与事件路由    │
                   │ 日志 JSONL/错误码/artifact 清单│
                   └───────┬──────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
┌───────▼────────┐ ┌───────▼─────────┐ ┌──────▼──────────┐
│ meito-link     │ │ GitHub runner   │ │ 静态站点        │
│ 协调/密文      │ │ meito-runner    │ │ 单页托管        │
│ 不转会话字节   │ │ frpc/sshd/ttyd  │ │ 不含会话字节    │
│                │ │ noVNC/watchdog  │ │                 │
└───────┬────────┘ └───────┬─────────┘ └─────────────────┘
        │ 只交换 peer 信息/密文 │
        └───────────────────┘
              P2P UDP 直连（frp xtcp 或 WireGuard）
```

数据流：

1. 本地 CLI 或网页控制服务调用同一 `meito-core`，生成 `session_id`、一次性密钥与通道令牌。
2. runner 的 `meito-runner` 启动 frpc xtcp proxy；本地/网页侧通过协调服务拿到 peer 信息后建立 P2P。
3. 本地版用 SSH 本地转发访问 runner 回环入口；网页版用 WebRTC DataChannel 直连 runner，信令经协调服务，字节不经协调服务。
4. 入口服务只监听回环；token 网关校验一次性 token 后再转发给浏览器。
5. 会话产物走 GitHub artifact，不走 P2P 文件传输；网页版直接下载 artifact，本地版 `pull --apply` 落仓库。

进程与 crate 的映射：

| 组件 | 形态 | 职责 | 禁止 |
|---|---|---|---|
| `meito-core` | Rust 库 | 状态机、鉴权、P2P 策略、入口生命周期、artifact 清单、日志/错误码 | 依赖 UI、依赖具体隧道协议 |
| `meito-cli` | Rust 二进制 | 本地外壳：参数、SSH 转发、frpc 进程管理、浏览器打开 | 复制状态机/artifact 逻辑 |
| `meito-web` | Rust 二进制 + 静态单页 | 网页外壳：静态托管、OAuth、WebSocket 信令、WebRTC 适配 | 承载终端/图形字节；TURN |
| `meito-runner` | Rust 二进制 | runner 侧监督：预检、frpc、sshd、入口、watchdog、collect | 反向控制本地 |
| `meito-link` | Rust 二进制 | 协调服务：通道注册、密文交换、限流 | 解析/转发会话字节 |
| `web/` | HTML/CSS/JS | 单页工作台（xterm.js/noVNC 按需加载） | SPA 框架、状态副本 |

### 5.2 本地 CLI

| 子命令 | 作用 | 关键参数 |
|---|---|---|
| `meito init` | 写入 `.meito/meito.toml`、生成 workflow 安装说明、检查默认分支 | `--repo`、`--workflow` |
| `meito doctor` | 本地全量预检；`--deep` 加服务器/UDP/默认分支检查 | `--repo`、`--deep`、`--json` |
| `meito up` | 启动会话并等待入口可用 | `--ref`、`--max`、`--idle`、`--entry`、`--p2p`、`--log-level` |
| `meito web` | 启动本地控制端并打开托管单页（网页版外壳） | `--static`、`--control`、`--open` |
| `meito share` | 生成只读链接或写链接（含过期时间） | `--read-only`、`--ttl`、`--no-join-timeout` |
| `meito status` | 打印状态机、run_id、入口 URL、最后心跳 | `--json`、`--session` |
| `meito connect` | 重连已有会话（SSH master/转发/入口） | `--session` |
| `meito entry` | 切换/打开/停止入口 | `switch terminal\|vnc\|both`、`open` |
| `meito pull` | 下载 artifact、校验、应用 diff | `--apply`、`--dest`、`--patch-only`、`--stash` |
| `meito kconfig` | 非交互改配置并生成 defconfig | `--from`、`--set`、`--file`、`--save` |
| `meito down` | 收尾、上报、撤销通道 | `--force`、`--rotate` |
| `meito logs` | 查看/跟随本地与 runner 日志 | `--session`、`--stage`、`--follow` |
| `meito env` | 输出环境台账（失败自动输出） | `--session`、`--json` |
| `meito clean` | 清理本地状态/旧日志/孤儿运行 | `--older-than`、`--orphans`、`--dry-run` |
| `meito p2p` | P2P 诊断与无中继证明 | `verify`、`bench`、`provider` |
| `meito config` | 读取/校验/回显配置 | `get`、`set`、`validate` |

命令示例：

```text
meito init --repo owner/repo --workflow .github/workflows/meito-session.yml
meito doctor --repo owner/repo --deep
meito up --ref main --max 60m --idle 10m --entry terminal
meito up --entry graphical --max 30m --log-level debug
meito web --static https://meito.example.com --open
meito share --read-only --ttl 30m
meito status --json
meito entry switch vnc
meito pull --apply --dest .
meito kconfig --from defconfig --set CONFIG_FOO=y --save defconfig
meito env --session latest --json
meito down --rotate
meito logs --session latest --stage p2p --follow
meito clean --older-than 7d --orphans --dry-run
```

### 5.3 配置文件

仓库级 `.meito/meito.toml`（提交到仓库，不含秘密）：

```toml
version = 1
repo = "owner/repo"
workflow = ".github/workflows/meito-session.yml"
ref = "main"

[p2p]
provider = "frp_xtcp"          # frp_xtcp | tailscale_direct
frps = "coord.example.com:7000"
prefer_ipv6 = true
allow_relay = false            # 硬约束：必须 false
allow_derp = false             # 硬约束：必须 false

[entry]
default = "terminal"           # terminal | graphical
terminal_port = 7681
graphical_port = 6080

[vnc]
width = 1280
height = 800
depth = 24

[artifact]
mode = "artifact"              # artifact | local-commit
paths = [".config", "defconfig", "include/config.h"]

[limits]
max_minutes = 60
idle_minutes = 10
graphical_max_minutes = 120
no_join_timeout_minutes = 5      # 无人加入自动取消

[security]
allowed_actors = ["your-github-login"]
require_public_key = true
host_key_pin = ""              # 每会话由加密通道下发，不写死
redact = ["token", "authorization", "private_key", "secret", "password", "session_connect_string"]

[share]
read_only_default = true
read_link_ttl_minutes = 30
write_link_ttl_minutes = 60
join_timeout_minutes = 5

[web]
static_base_url = "https://meito.example.com"
control_base_url = "https://control.example.com"
signaling = "websocket"        # 只承载信令，不承载会话字节
stun_urls = ["stun:stun.example.com:3478"]
ice_transport_policy = "all"    # 浏览器只会得到 host/srflx
oauth_scopes = ["repo:read", "workflow:write"]
allow_byo_turn = false         # 待拍板；默认关闭

[turn]
enabled = false                # 禁止默认中继
urls = []                      # 用户自带时才填写；Meito 不托管 TURN

[log]
level = "info"                 # info | debug | trace（三级）
dir = "logs"
format = "jsonl"
redact_required = true
```

机器级设置（安装目录）按 R-C-16 与用户交付约定：`C:\Program Files\Meito\config\`，安装器只授予该子目录写权限；仓库级 `meito.toml` 只承载项目配置，不承载用户凭据。令牌一律走环境变量/`gh auth`/GitHub Actions secrets，不落仓库、不落日志。

### 5.4 状态机

```text
idle
  └─ preflight ──> failed_preflight
  └─ dispatching ──> failed_dispatch
  └─ queued ──> failed_queue_timeout
  └─ runner_starting ──> failed_bootstrap
  └─ p2p_connecting ──> failed_p2p / no_direct_path
  └─ link_ready ──> failed_link
  └─ waiting_for_join ──> cancelled（无人加入超时）
  └─ entry_starting ──> failed_entry
  └─ connected <──> reconnecting
  └─ ending ──> collecting ──> done
                    └─ failed_collect
  └─ cancelled
  └─ orphaned（由 doctor/clean 判定）
```

- 每次状态切换写 `logs/session-<id>.jsonl`，原子写 `.meito/state/<id>.json`（临时文件 + rename）。
- `--force` 只跳过确认，不跳过安全校验。
- `reconnecting` 允许有限次重连；超过上限进入 `failed_p2p`，并保留本地转发与日志供诊断。
- `waiting_for_join` 只用于分享链接/网页版；本地版默认不进入，直接连接。超过 `join_timeout_minutes` 自动取消并上传空会话日志（吸收 upterm 的“无人加入自动取消”）。
- `no_direct_path` 是网页版的可解释失败态：记录 candidate 类型、ICE 阶段、失败原因，并输出转本地版的命令；不是所有失败都归为 `failed_p2p`。
- `orphaned` 不是正常状态，由 `meito clean --orphans` 或 watchdog 收敛。

### 5.5 workflow 模板

模板文件：`templates/meito-session.yml`（评审用，不放在 `.github/workflows/` 防止误触发；`meito init` 负责安装到目标仓库）。

关键设计：

- 触发：`workflow_dispatch`，必须存在于默认分支。
- 输入上限：GitHub 最多 10 个输入、每个 1000 字符。本设计固定 10 个：

| # | 输入 | 内容 |
|---|---|---|
| 1 | `session_id` | UUID v4 |
| 2 | `ref` | 目标分支 |
| 3 | `max_minutes` | 60/120/300，CLI 先校验 |
| 4 | `idle_minutes` | 默认 10 |
| 5 | `p2p_provider` | `frp_xtcp` / `tailscale_direct` |
| 6 | `entry` | `terminal` / `graphical` / `both` |
| 7 | `channel` | JSON：`channel_id`、`write_token`、`public_key` |
| 8 | `artifact_spec` | JSON：路径白名单、模式 |
| 9 | `flags` | JSON：`share_mode`、`read_only`、`no_join_timeout_minutes`、`allow_graphical`、`checkpoint_minutes` |
| 10 | `meito_version` | 引导程序版本，必须与 CLI/内核一致 |

- **输入不含任何秘密**。连接串、frp `secretKey`、SSH 私钥、入口 token 都不作为输入；runner 侧生成后经 `meito-link` 加密下发。
- `permissions: contents: read, actions: read`；不申请 `id-token: write`；不注入生产 secrets；交互 job 使用独立的 `environment: meito-debug`。
- `timeout-minutes: 360`；watchdog 的 `max_minutes` 被 CLI 限制为 ≤300，给收尾留 60 分钟余量。
- `concurrency: group: meito-session-${{ github.repository }}`，`cancel-in-progress: false`；CLI 先检查 active run，`--replace` 才显式取消旧 run。
- 步骤顺序：actor 白名单 → 预检（UDP/DNS/磁盘）→ 安装校验 → sshd（回环）→ frpc xtcp → 生成主机密钥 → 加密下发 → 入口启动 → watchdog → `if: always()` 上传日志/产物 → 清理。
- 上传 artifact 前执行 `artifact_guard`：路径白名单、正则扫描 token/私钥/连接串；命中即失败，不上传。
- 用 `$GITHUB_STEP_SUMMARY` 写入会话摘要：`session_id`、状态、入口类型、只读/写链接、过期时间、artifact 名；**只写非秘密摘要**，不写连接串（吸收 upterm 的“连接串写入 Job Summary”，但收紧为脱敏摘要）。
- 分享链接：`flags.share_mode=read_only` 时 runner 只接受只读通道；写通道必须由发起人显式 `--write`，并用一次性 token。

### 5.6 远端引导

远端脚本保持“薄壳”，业务逻辑在 Rust 二进制里：

- `scripts/remote/bootstrap.sh`：`set -euo pipefail`，下载并校验锁定版本的 `meito-runner`、`frpc`、`ttyd`；所有下载带 SHA256；失败保留 `logs/session-<id>.jsonl`。
- `scripts/remote/start-terminal.sh`：调用 `meito-runner entry terminal`，启动 ttyd，绑定 `127.0.0.1:7681`（端口与回环地址取自 `contracts/entry.json`，不得硬编码）。
- `scripts/remote/start-graphical.sh`：调用 `meito-runner entry graphical`，启动 Xvfb/WM/x11vnc/websockify，全部绑定回环（端口与帧缓冲取自 `contracts/entry.json`）。
- `scripts/remote/watchdog.sh`：调用 `meito-runner watchdog`，处理空闲/最长时限/停止文件/checkpoint；不再使用 Python。
- `scripts/remote/collect.sh`：调用 `meito-runner collect`，生成 manifest、diff、untracked 包、日志，并调用 artifact_guard。
- `scripts/remote/check-actor.sh`：调用 `meito-runner check-actor`；actor 白名单是硬门禁。
- sshd 配置：`ListenAddress 127.0.0.1`、`PasswordAuthentication no`、`PermitRootLogin no`、`AllowUsers runner`、`AllowTcpForwarding yes`、`X11Forwarding no`、`PermitTunnel no`、`GatewayPorts no`、`ClientAliveInterval 10`、`ClientAliveCountMax 3`。
- 每会话生成一次性 sshd 主机密钥；公钥通过加密通道下发，本地写入临时 `known_hosts` 并 pin；不匹配立即失败。

### 5.7 协调服务 `meito-link`

运行在公网小服务器，只做协调；服务端语义以 `contracts/link.json`（0.3.2）为准：

- `POST /v1/channels`：CLI 注册通道，必须同时提交 `channel_id`、`read_token_hash`、`write_token_hash`、`public_key` 四个字段；缺 `write_token_hash` 时服务端无从校验 `PUT`。通道 TTL `7200` 秒。
- 成功响应统一 `200`：注册、`PUT`、`GET`、`DELETE` 均返回 `200`；同一通道仍存活时重复注册返回 `400`。
- 鉴权头统一 `Authorization: Bearer <token>`；读 token 只用于 `GET`，写 token 用于 `PUT` 与 `DELETE`。
- 令牌哈希：`sha256`、`hex` 编码；哈希只用于校验调用方令牌，不写日志。
- 读写 token 均为**每通道只签发一次**，不是只能读一次；两个哈希在注册时一次提交。
- `PUT /v1/channels/{id}/blob`：runner 用写 token 提交密文；单包上限不在本节复写，引用 `contracts/connection-package.json` 的 `serverHandling.maxCiphertextBytes`（当前 16384 字节）；服务端只存密文，不解析。
- `GET /v1/channels/{id}/blob`：CLI 用读 token 拉取；可重复读取直到 `DELETE` 或 TTL，每通道最多 10 次（`maxReads: 10`），读取后不立即删除。
- `DELETE /v1/channels/{id}`：会话收尾撤销。
- 限流：每通道每分钟 60 次、每 IP 每分钟 120 次，突发 10。
- HTTP 状态映射：令牌无效 `403`、通道不存在 `404`、包过大 `413`、限流 `429`、方法不允许 `405`、报文非法 `400`、内部错误 `500`。
- 日志只允许记录 `channel_id`、大小、状态；**禁止记录 blob 片段与任何摘要**（哈希仅用于完整性校验，不落日志）。
- 该服务不代理任何会话字节；它只是密文的短暂交换点。

### 5.8 日志与诊断

**唯一格式是 JSONL，唯一路径是 `logs/session-<id>.jsonl`。禁止用 `print`/`println!` 当日志。**

固定字段（六项，顺序固定；额外字段可加但不得替代）：

| 字段 | 含义 | 示例 |
|---|---|---|
| `ts` | UTC 时间戳（RFC 3339） | `2026-11-08T12:00:00Z` |
| `level` | 级别 | `info` / `warn` / `error` / `debug` / `trace` |
| `session_id` | 会话 id，贯穿本地/runner/协调服务 | UUID v4 |
| `stage` | 状态机阶段 | `p2p_connecting` |
| `event` | 事件名 | `p2p.ready` |
| `error_code` | 错误码；无错误时为空字符串 | `E_P2P_XTCP` |

可选字段：`run_id`、`duration_ms`、`evidence`、`entry`、`p2p_provider`、`tool_version`。所有密钥、令牌、连接串、私钥、Authorization 头强制脱敏；脱敏在日志层完成，不依赖调用点自觉。

日志分级（三级开关）：

- `info`（默认）：阶段切换、入口就绪、收尾、错误。
- `debug`：P2P 协商、ICE candidate 类型、入口健康检查、artifact 校验。
- `trace`：frpc/sshd 子进程输出摘要、帧级诊断（只用于排障，禁止长期开启）。

开关：`--log-level info|debug|trace`，或 `MEITO_LOG`；三级之外不接受任意值。`trace` 必须显式确认，避免把连接细节写进日志。

本地与 runner 都写同一路径（各自工作目录下）：

```text
logs/session-<id>.jsonl          # 主日志，JSONL
logs/env-session-<id>.json       # 失败时自动输出的环境台账
```

环境台账字段：镜像/runner 标签、内核与发行版、`meito`/`frpc`/`ttyd`/`x11vnc`/`websockify` 版本、P2P provider 与状态、UDP/ICE 探测结果、入口端口与绑定地址、退出码、`run_id`、`session_id`。**失败时自动生成，不要求用户复现**（吸收“失败自动输出环境台账”）。

诊断命令：

```text
meito doctor --deep --json
meito logs --session latest --stage p2p
meito env --session latest --json
meito p2p verify --bench 100MB --repeat 3
meito clean --orphans --dry-run
```

`doctor` 是只读诊断：检查 gh 权限、默认分支、控制器连通性、本地 UDP/STUN、frpc/tailscale、浏览器能力、磁盘、日志目录可写；输出人可读摘要与 `--json` 机器摘要。`doctor` 不得修改仓库或触发会话。

错误码：

| 错误码 | 含义 | 首要证据 |
|---|---|---|
| `E_AUTH` | gh/令牌/权限不足 | `gh auth status`、API 响应 |
| `E_DISPATCH` | 无法触发 workflow | run 列表、workflow 文件默认分支检查 |
| `E_ACTOR_NOT_ALLOWED` | actor 不在白名单 | `meito.toml` 与 `github.actor` |
| `E_UDP_BLOCKED` | runner 出站 UDP 不可用 | 预检 UDP 探测日志 |
| `E_P2P_XTCP` | xtcp 打洞失败 | frpc 日志、重试次数、NAT 诊断 |
| `E_P2P_TS` | Tailscale 直连失败或检测到 DERP | `tailscale status/ping`、DERP map |
| `E_WEB_NO_DIRECT_PATH` | 网页版 ICE 无直连路径 | candidate 类型、ICE 阶段、`getStats()` |
| `E_WEB_RELAY_USED` | 检测到 TURN/中继（占位，默认不出现） | TURN URL、candidate `relay` |
| `E_HOSTKEY` | 主机密钥不匹配 | 临时 known_hosts、pin |
| `E_ENTRY` | ttyd/noVNC 启动或健康检查失败 | 入口日志、`ss -ltnp` |
| `E_ARTIFACT` | artifact 缺失/哈希不符 | run artifacts、manifest |
| `E_APPLY_CONFLICT` | 本地 diff 冲突 | `.meito/conflicts/<sid>/`、`.rej` |
| `E_JOB_TIMEOUT` | 6 小时硬上限或 watchdog 超时 | run 状态、watchdog 日志 |
| `E_NO_JOIN_TIMEOUT` | 分享/网页会话无人加入 | 通道 TTL、join 计时 |
| `E_ORPHAN` | 孤儿会话 | active run 列表、清理记录 |

### 5.9 架构检查（一套内核的可执行约束）

`scripts/arch-check.ps1` 在本地与 CI 都执行，检查“两个外壳没有复制内核”：

| 检查 | 判定 | 失败码 |
|---|---|---|
| 单一 Model | 会话状态机枚举只在 `crates/meito-core/src/**` 定义；其他 crate 不得定义同名状态枚举 | `ARCH_MODEL_DUPLICATE` |
| 单一 artifact 逻辑 | manifest/哈希/冲突规则只在 `meito-core`；`meito-cli`/`meito-web` 只能调用 | `ARCH_ARTIFACT_DUPLICATE` |
| 单一 P2P 策略 | `allow_relay`/`allow_derp`/TURN 判定只在 `meito-core`；外壳不得自行放行 | `ARCH_P2P_POLICY` |
| 无 TURN | `meito-web`、`web/` 不出现 `turn:` URL、`iceServers` 不含 TURN；配置默认空 | `ARCH_TURN_FORBIDDEN` |
| 无 SPA 框架 | `web/` 不含 React/Vue/Angular/Svelte 构建配置 | `ARCH_SPA_FORBIDDEN` |
| 数据面直通 | 终端/图形数据路径不出现逐帧 JSON、行缓冲包装、二次代理 | `ARCH_DATA_PATH` |
| 日志规范 | 只用 `tracing` JSONL；非 CLI 展示层不出现 `println!`/`print` | `ARCH_LOG_BYPASS` |
| 注释规范 | Rust 文件首 25 行有 `//!` 头；`pub fn` 有 `///`；关键文件有 `// ===== 组件名 =====` | `ARCH_NOTE` |
| 数值单一来源 | 文档正文出现的阈值、时长、上限、端口、状态码必须在 `contracts/` 有对应字段；散文不得成为数值的唯一定义处 | `CONTRACT_DOC_NUMERIC` |

**机器覆盖段落（`CONTRACT_DOC_NUMERIC` 实际校验）**：design §2、§3.2、§5.6、§5.7、§5.10、§8.3、§9.4、§10.2（启停顺序部分）、§10.3、§10.4、§10.7，以及 `docs/接口冻结.md` §5.2–§5.4。抽取规则为：带单位数值（秒/分钟/小时/次/字节/端口/s/min/m/ms/MB/KB/%/位，分钟与小时按秒换算后仍允许原值）、4–5 位端口、`宽x高x深` 分辨率与 `4xx/5xx` 状态码；命中后必须能在 `contracts/*.json` 的数值集合中找到。

**人工审计项（未纳入机器覆盖，文档已显式标注“人工审计”）**：§10.2 的依赖安装耗时估算表、§10.5 带宽与 CPU 估算、§10.6 job 硬上限下的取舍，以及 §4/§7/§11/§12/§14 中的工程估算与对比数据。这些数值不是验收阈值或契约字段，改由评审人对照契约为准；若后续成为验收依据，必须先落 `contracts/` 再移入机器覆盖。

架构检查失败即阻断合并；豁免只能写进 `docs/门禁豁免清单.md` 并给出替代验证，不能静默跳过。

### 5.10 网页版外壳的边界

- `web/` 是单页 HTML/CSS/JS，**不引入 SPA 框架、不打包状态副本**；xterm.js/noVNC 按需动态加载。
- `crates/meito-web` 只做：静态文件服务、OAuth 回调、WebSocket 信令、WebRTC DataChannel 适配、把控制事件转成 `meito-core` 调用。
- 终端/图形字节从 DataChannel 直接进 xterm.js/noVNC，不经过服务端 WebSocket；服务端只转发 SDP/ICE/控制消息。
- 网页版的“本地版回退”只显示命令与链接，不在浏览器里模拟 `meito pull --apply`；落到本地仓库必须由本地版完成。
- 网页版不得直接暴露 6080/7681（端口取自 `contracts/entry.json`）；所有入口只在 runner 回环，DataChannel 是唯一入口。

---

## 6. 产物回传

### 6.1 默认：artifact

会话结束（正常退出、`meito down`、watchdog 到时）后，runner 执行 `collect.sh`：

1. `git status --porcelain` 记录改动；`git diff --binary HEAD > changes.patch`。
2. 未跟踪文件按 `artifact_spec.paths` 白名单打包；拒绝 `.env`、`id_*`、`*.pem`、token 文件。
3. 生成 `manifest.json`：`session_id`、`base_commit`、文件列表、每个文件 SHA256、tool_version、created_at。
4. 上传 `meito-result-<sid>`（代码/配置）与 `meito-logs-<sid>`（日志），retention 1 天。
5. artifact_guard 扫描通过才上传；命中敏感信息则 job 失败并只保留脱敏日志摘要。

本地 `meito pull`：

1. 用 `gh run download` 下载指定 artifact。
2. 校验 `manifest.json` 与每个文件 SHA256；不匹配 `E_ARTIFACT`，不落盘到工作区。
3. 校验本地 `HEAD` 是否等于 `base_commit`；不等则先做三方检查。
4. `git apply --check` 通过后 `git apply --3way --binary`；未跟踪文件按白名单复制，已存在则拒绝覆盖。
5. 冲突：保留 patch、写 `.meito/conflicts/<sid>/`、列出冲突文件，退出码 3。绝不 `git checkout --` 或强制覆盖。
6. 本地工作区 dirty：默认拒绝；`--stash` 才 stash（并明确提示如何恢复）。

### 6.2 可选：本地提交

默认不在 workflow 里 `contents: write`。`meito pull --apply` 成功后由用户在本地审查并提交；这是最小权限路径。

如果确实需要远端分支：

- 使用**独立 job**（不与交互 job 共用权限）消费 artifact，`permissions: contents: write`，放在受保护 environment 后。
- 分支名 `meito/session-<sid>`；推送失败不 force，不 rebase 自动解决；冲突交回本地。
- 该路径默认关闭；开启需要用户决策并记录在 `docs/变更记录.md`。

### 6.3 冲突规则

| 情况 | 处理 |
|---|---|
| 本地 HEAD 与 `base_commit` 不同 | 先 `git apply --check`；失败则只保存 patch，不应用 |
| 本地工作区 dirty | 默认拒绝；`--stash` 显式允许 |
| 文件已被本地修改 | 三方应用；冲突写 `.rej`，进 `.meito/conflicts/<sid>/` |
| 文件已删除/重命名 | 不自动删除；列出差异，要求人工确认 |
| 二进制文件 | artifact 原样复制 + SHA256；不做文本 diff |
| `.config` 行尾/排序差异 | 不做自动规范化；按字节比较，差异如实报告 |

### 6.4 网页版回传

网页版不执行本地 `git apply`，只提供：

1. artifact 直链/下载按钮（浏览器直接访问 GitHub artifact，不经过协调服务器）。
2. `manifest.json` 的 SHA256 与 `base_commit` 展示，供用户在本机核对。
3. 一键复制本地版命令：`meito pull --apply --session <id>`。

网页版不得声称“已应用到你的仓库”；也不得让控制服务代理 artifact 字节。若 GitHub API 的跨域/鉴权导致浏览器下载失败（**未验证**），页面给出 `gh run download <run_id>` 命令作为回退，而不是绕道服务器。

---

## 7. 会话生命周期与失败处理

### 7.1 超时与收尾

| 机制 | 设计 | 判据 |
|---|---|---|
| 最长会话 | CLI 默认 60 min；硬上限 300 min；workflow `timeout-minutes: 360` | 300 min 前 watchdog 结束并上传 checkpoint |
| 空闲 | 默认 10 min 无心跳 | 心跳过期后 ≤5 min 内结束 |
| 加入超时 | 入口启动后按 `join_timeout_minutes`（默认 5 min）无连接 | 结束会话，`E_NO_JOIN_TIMEOUT`；上传空会话日志 |
| 手动取消 | `meito down` 写 stop 文件；无响应再 `gh run cancel` | ≤60 s 进入 ending/cancelled |
| 断线重连 | SSH `ServerAliveInterval=10`，本地重连 ≤3 次；网页版 ICE restart ≤2 次 | 超过进入 `failed_p2p`/`no_direct_path` 并保留日志 |
| 只读/写链接 | 只读链接不能输入/保存；写链接一次性 token，过期即失效 | 只读尝试写操作返回 403，日志记 `E_AUTH` |
| 6 小时硬限 | 平台到点直接 fail | CLI 标 `E_JOB_TIMEOUT`；不承诺 artifact 完整 |
| checkpoint | `max_minutes - 10` 时先上传一次结果与日志 | 即使后续被杀也有可用产物 |

心跳：CLI 通过独立 SSH 连接每 15 s 更新 `.meito/heartbeat`；watchdog 检查心跳文件 mtime。CLI 崩溃后心跳停止，watchdog 在空闲阈值后收尾，避免孤儿。

### 7.2 孤儿清理

- `meito doctor --orphans`：按 workflow、`session_id`、创建时间列出 active/queued run。
- `meito clean --orphans`：默认只列出；`--apply` 才取消。取消后再次核对 active 列表为空。
- 自托管 runner 降级路径：必须有开机/定时清理脚本，不能依赖 runner 临时性。
- 每次收尾写 `docs/变更记录.md` 或对应验收记录；不把“已取消”当作“已清理”，必须复核。

### 7.3 失败日志落盘

- 本地：`logs/session-<sid>.jsonl`，包含阶段、错误码、HTTP 状态、命令退出码、证据路径；失败时同时写 `logs/env-session-<sid>.json`。
- runner：同一 `logs/session-<sid>.jsonl` 随 `meito-logs-<sid>` 上传；`if: always()`。
- 取消/6 小时硬杀时 runner 可能来不及上传：CLI 回退到 `gh run view --log` 保存到本地 `logs/session-<sid>-gh.log`；仍拿不到则 `E_LOGS_UNAVAILABLE`，如实记录，不伪造。
- 任何失败都必须有：错误码、发生阶段、复现命令、日志路径、下一步建议。

---

## 8. 安全设计

### 8.1 威胁模型

| 威胁 | 防护 |
|---|---|
| 非授权用户接入会话 | actor 白名单 + 一次性 SSH 公钥 + 一次性入口 token + 只读/写链接分离 |
| 连接串泄露 | 连接串不进 artifact、不进日志、不进 Job Summary；经 `meito-link` 密文下发 |
| 中间人 | 每会话 sshd 主机密钥 pin；frp 控制 token；Tailscale/WireGuard 加密；SSH 二次加密 |
| 服务器看到会话内容 | 服务器只交换 peer 信息与密文；frp 无 stcp 回落；Tailscale 禁用 DERP；网页版禁用 TURN |
| 公有仓库暴露 | 默认禁止公有仓库交互会话；显式 `--allow-public` 需确认；仍建议私有仓库 |
| GitHub 日志脱敏失效 | 不把秘密放进 runner 环境；假设日志可能泄露；会话结束按泄露处理并轮换 |
| 交互 shell 读取环境变量 | 调试 job 不注入生产凭据；`env` 能看到的内容都视为可泄露 |
| 网页 OAuth 令牌泄露 | 控制服务短期持有；不落 localStorage；日志脱敏；会话结束撤销 |
| 只读链接被升级 | 只读通道在 runner 侧拒绝 stdin/写入；写操作需另一枚一次性 token；不可由前端自行切换 |
| artifact 夹带秘密 | 路径白名单 + 正则扫描 + 上传前阻断 |
| noVNC 弱鉴权 | VNC 8 位 DES 密码只作纵深，主鉴权由隧道侧 token 承担 |
| 入口被公网直连 | ttyd/x11vnc/websockify 全部绑 `127.0.0.1`；`ss` 检查；禁止 0.0.0.0；网页版只能经 DataChannel |
| 供应链 | frp/ttyd/noVNC/actions 全部锁版本 + SHA256/commit pin；不 `curl \| bash` |
| 会话残留 | `meito down` 停止进程、撤销通道、删除临时密钥；自托管 runner 额外清理 |

### 8.2 身份与凭据

- **actor 白名单**：`meito.toml [security] allowed_actors` 与 `github.actor` 双重校验；不在名单直接失败。
- **一次性 SSH 密钥**：CLI 每会话生成 ed25519 密钥；公钥作为 workflow 输入（公开无害），私钥只在本机内存/`0600` 临时文件；会话结束删除。
- **入口 token**：每会话 256-bit 随机；写 token 与只读 token 分离；只有本地 token 网关/runner 侧网关校验；不写入 runner 日志；`meito-link` 密文下发。
- **网页 OAuth**：控制服务持有短期令牌，浏览器不落 localStorage/IndexedDB；回调校验 `state` 与 PKCE；日志只记 open_id 哈希。
- **frp secretKey**：runner 每会话生成，配置 frpc 后随连接包加密下发；不放进 workflow 输入。
- **frps token**：GitHub Actions secret，只用于控制通道；定期轮换；不进入交互 shell 的环境变量。
- **GitHub 权限**：交互 job 只有 `contents: read` + `actions: read`；不申请 `id-token: write`；不使用生产 secrets；`gh` 本地令牌只授予 dispatch/artifact 所需权限。

### 8.3 入口安全

- ttyd：`-W`（可写）、`-i 127.0.0.1`、`-p 7681`、`-m 1`、`-O`；只读分享链接启动不带 `-W` 的实例，或在网关层丢弃 stdin；可选 `-c user:pass` 作为纵深，不作为主鉴权。
- noVNC：
  - `Xvfb :99 -screen 0 1280x800x24 -nolisten tcp`
  - `x11vnc -display :99 -localhost -rfbport 5900 -rfbauth ~/.vnc/passwd -safer`
  - `websockify --web=/usr/share/novnc 127.0.0.1:6080 127.0.0.1:5900`
  - VNC 密码是 8 位 DES，**不可依赖**；主鉴权是隧道侧一次性 token + SSH 密钥/WebRTC 信令鉴权 + actor 白名单。
  - 禁止 `-listen 0.0.0.0`、禁止把 6080 直接暴露公网、禁止任何隧道中继。
- 本地 token 网关：只绑 `127.0.0.1`；校验 token 后转发 HTTP/WebSocket；不记录 token；错误返回 401/403；会话结束关闭并清空。
- 验收：runner 上 `ss -ltnp` 只允许 7681/5900/6080 出现在 `127.0.0.1`；任何 `0.0.0.0` 直接失败。
- 本节端口 7681/5900/6080、绑定地址 `127.0.0.1`、帧缓冲 1280x800x24、VNC 密码 8 位，以及 ttyd 的 `-W`/`-m 1`/`-O` 全部取自 `contracts/entry.json`；代码与脚本不得另写数值。

### 8.4 GitHub 脱敏与交互 shell 的特别纪律

- GitHub 的日志脱敏不保证覆盖所有格式；交互 shell 里 `env`、`ps`、`cat /proc/*/environ` 都可能看到注入变量。因此：
  1. 调试 job 与生产凭据完全隔离；生产 secrets 不注入。
  2. 交互 job 不申请写权限，不持有可推送的令牌。
  3. 公有仓库默认禁用；必须用时走白名单与一次性凭证，并假定会话内容可能公开。
  4. 连接串、token、私钥不写 artifact、不写 Job Summary（只写脱敏摘要）、不写日志。
  5. 会话结束按“可能已泄露”处理：撤销通道、轮换会话级 secret；若环境曾出现长期凭据，轮换该凭据并写事故记录（R-D-8）。
  6. 只读链接不是安全边界：它只降低误操作，不替代 actor 白名单与一次性 token；写链接必须单独授权。
  7. 不把“日志里没有”当作安全证明；以权限隔离和最小化为主。

---

## 9. 点对点实现与“无回流”证明

### 9.1 frp xtcp 拓扑

- 服务器：frps 只监听控制端口与 XTCP 协调所需端口；dashboard 只绑 `127.0.0.1` 或关闭；`auth.token` 来自 `MEITO_FRPS_TOKEN`。
- runner：`frpc` 配置 `type = "xtcp"`、`name = "ssh-<sid>"`、`localIP = "127.0.0.1"`、`localPort = 22`、`secretKey = <每会话>`；**不写 `fallbackTo`**。
- 本地：`frpc` visitor `type = "xtcp"`、`serverName = "ssh-<sid>"`、`secretKey` 同值、`bindAddr = "127.0.0.1"`、`bindPort = <随机空闲端口>`；**不写 `fallbackTo`**。
- 失败即失败：XTCP 打洞失败不会自动走 frps 数据转发；`stcp` 配置视为违规，启动前做配置审计。

具体 frps 的 UDP 协调端口字段与版本相关，M2 阶段按锁定版本核对并写入验收记录；本设计不假定未验证的字段名。

### 9.2 Tailscale/Headscale 直连

- Headscale 控制面在服务器；`derp.server.enabled = false`，不配置 `derp.urls`；确保客户端 DERPMap 没有默认区域（`OmitDefaultRegions=true`），且没有 peer relay。
- runner 与本地各自加入 tailnet；runner 的 sshd 只允许 tailnet/回环访问。
- 若 `tailscale status` 出现 `relay` 或 `tailscale ping` 显示 DERP，立即判定 `E_P2P_TS`，不得继续。
- TUN 与 userspace 两种模式都未验证；M2 先做最小实验，失败则只保留 frp 主路径。

### 9.3 失败顺序

```text
frp xtcp 打洞
  ├─ 成功 → SSH → 入口
  └─ 失败（重试 N 次后）
       ├─ 若用户启用 tailscale_direct → 尝试直连
       │    ├─ 成功 → SSH → 入口
       │    └─ 失败 / 检测到 DERP → E_P2P_TS
       └─ 不启用或也失败
            ├─ 提示 meito kconfig（非交互）
            ├─ 提示本地容器 + act
            └─ 提示自托管 runner（如用户有）
```

**任何分支都不允许**：stcp、frps 数据转发、DERP、peer relay、Cloudflare、ngrok、uptermd、tmate。`allow_relay=false` / `allow_derp=false` 是硬校验，不是提示。

### 9.4 如何证明“流量确实没回落到服务器”

不能只靠应用日志。采用三层证据：

1. **配置审计**：解析 frpc/frps 配置，确认没有 `stcp`、没有 `fallbackTo`；Tailscale 确认 DERP map 为空、无 peer relay。审计结果写入会话日志。
2. **服务器侧字节计数**：
   - 在服务器上对 frps 控制端口与协调端口加独立计数（`iptables`/`nft` 计数器或 `/sys/class/net/*/statistics` 前后差值）。
   - 先测空闲 60 s 的基线；再让本地与 runner 之间传输 100 MB（`dd`/`scp` 经 SSH），重复 3 次。
   - 判据：服务器对应端口/接口的字节增量 < 5% × 100 MB，且抓包中不出现持续的大包数据流；控制流量只应是心跳/协调量级。
   - 同时记录本地发送量、runner 接收量，三者对照。
3. **服务器抓包**：在服务器协调接口上 `tcpdump` 过滤 peer 地址与端口；传输期间只应看到控制/协调包；若出现与 payload 成比例的数据包，判定失败。

本节传输量、重复次数、负载阈值与空闲基线取自 `contracts/acceptance.json`；验收脚本必须读取同一字段，不得另写一套数值。

Tailscale 额外证据：

- `tailscale status --json` 中 peer 的 `Relay` 为空，`CurAddr` 为直连地址；
- `tailscale ping` 显示 direct，不显示 via DERP；
- DERP map 为空或 `OmitDefaultRegions=true`，且无 peer relay；
- 服务器字节计数与 100 MB 对照同上。

证明失败的处理：立即停止会话、保留证据、标记该 provider 不可用；不得用“日志看起来正常”作为通过依据。

### 9.5 runner 出站 UDP（未验证）

托管 runner 是否允许出站 UDP、目标端口是否被网络策略拦截，均未验证。workflow 预检必须：

- 向协调服务器发送带序号的小 UDP 探测包并等待回执；
- 失败则 `E_UDP_BLOCKED`，job 立即结束，不建立会话、不安装图形依赖；
- 记录探测目标、时间、结果、错误；不得把“TCP 443 可用”推断为“UDP 可用”。

若 UDP 被禁：frp xtcp 与 Tailscale 直连都不可用，按 9.3 走非交互/本地容器/自托管 runner，**不回退任何中继**。

**补充未验证项**：GitHub 托管 runner 是否“完全无入站 TCP”仍属推断，本设计只按“无公网入站”做保守假设；WebRTC 在 runner 侧的可达性（UDP、STUN、ICE）同样未实测。两者都不得写成已验证。

### 9.6 网页版“无 TURN”的可判定证据

网页版必须证明浏览器与 runner 之间没有第三方转发：

1. **配置审计**：`web/` 与 `meito-web` 的 `iceServers` 只允许 STUN；配置中 `turn:` URL 必须为空；`[turn].enabled=false`。
2. **浏览器侧统计**：会话建立后执行 `RTCPeerConnection.getStats()`，读取 selected candidate pair；只允许 `host`/`srflx`，出现 `relay` 即 `E_WEB_RELAY_USED` 并终止。
3. **ICE 日志**：记录 candidate 类型、优先级、`currentRoundTripTime`、`bytesSent/Received`；日志只写类型与地址族，不写完整 IP。
4. **服务器侧对照**：按 9.4 的计数/抓包方法，在信令服务上确认只有 SDP/ICE/控制消息，没有与 DataChannel payload 成比例的流量。
5. **失败可解释**：若只有 host/srflx 且 ICE 失败，返回 `E_WEB_NO_DIRECT_PATH`，展示已尝试的 candidate 类型与转本地版命令；不能尝试 TURN 兜底（除非用户已拍板 BYO TURN 分支 B）。

---

## 10. 双入口适配层

### 10.1 入口抽象

```text
EntryAdapter
  prepare()   安装/校验依赖，幂等
  start()     绑定回环端口，启动进程
  health()    HTTP/TCP/WebSocket 健康检查
  info()      本地端口、token、URL
  stop()      停止进程，清理临时文件
```

入口只负责“渲染/接入”，不负责鉴权与加密；鉴权在本地 token 网关，加密在 SSH（或 Tailscale WireGuard + SSH）。

### 10.2 启停顺序

```text
1. runner 预检（UDP/DNS/磁盘）
2. sshd 启动并只绑 127.0.0.1
3. frpc xtcp 注册；本地 visitor 打洞
4. 本地 SSH master + 主机密钥 pin
5. 选择入口：
   terminal  → 启动 ttyd（127.0.0.1:7681）
   graphical → 安装依赖 → Xvfb → WM → x11vnc → websockify（127.0.0.1:5900/6080）
6. 本地 SSH 端口转发 + token 网关
7. 健康检查通过 → 打开浏览器
8. 会话结束 → 停止入口 → 收集 artifact → 停止 sshd/frpc → 撤销通道
```

本节端口与回环地址取自 `contracts/entry.json`，不另写数值。

依赖安装耗时（**人工审计**：未实测，仅作工程估算，不作为验收依据，不纳入 `CONTRACT_DOC_NUMERIC` 机器覆盖）：

| 入口 | 依赖 | 估算 | 说明 |
|---|---|---|---|
| terminal | ttyd 二进制 + tmux | 数秒（若已缓存/预装） | 每次 runner 临时，仍可能需下载 |
| graphical | Xvfb、openbox/fluxbox、x11vnc、websockify、noVNC、字体、xterm | 1–3 min（apt 网络正常时） | 受镜像源、网络、包缓存影响；必须实测 |
| graphical（预构建镜像） | 上述全部预装 | 镜像拉取时间 | 可减少安装时间，但增加镜像维护与拉取带宽 |

默认入口是 terminal；graphical 只在 `--entry graphical` 或 `meito entry switch vnc` 时启动。`both` 仅排障。

### 10.3 终端入口（ttyd）

```text
ttyd -W -i 127.0.0.1 -p 7681 -m 1 -O -T xterm-256color \
  tmux new -A -s meito
```

- `-W` 必须：默认只读，无法操作 menuconfig。
- 只绑 `127.0.0.1`；`-m 1` 单客户端；`-O` 校验 Origin。
- tmux 提供 WebSocket 断线重连后的会话保持；ttyd 终端尺寸跟随浏览器，避免 v1 upterm 固定字符尺寸挤坏全屏的问题。
- menuconfig 的 TUI 依赖 PTY 与 xterm.js 兼容性；**未验证**，M4 用真实 menuconfig 做冒烟。
- 端口、绑定地址与 `-W`/`-m 1`/`-O` 取自 `contracts/entry.json`，本文不另写数值。

### 10.4 图形入口（noVNC）

```text
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp &
DISPLAY=:99 openbox &
DISPLAY=:99 x11vnc -localhost -rfbport 5900 -rfbauth ~/.vnc/passwd -safer &
websockify --web=/usr/share/novnc 127.0.0.1:6080 127.0.0.1:5900 &
```

- 所有监听都在 `127.0.0.1`；VNC 与 websockify 都不暴露公网。
- noVNC 的 VNC 密码是 8 位 DES，**不能作为主鉴权**；`x11vnc` 的 `-rfbauth` 只作纵深。
- 主鉴权：隧道侧一次性 token + SSH 密钥/WebRTC 信令鉴权 + actor 白名单。
- 禁止 `0.0.0.0:6080`、禁止公网直连、禁止任何隧道中继；`ss -ltnp` 是验收项。
- 图形入口用于必须鼠标操作的配置工具；普通 menuconfig 优先 terminal。
- 端口 5900/6080、帧缓冲 1280x800x24 与 VNC 密码 8 位取自 `contracts/entry.json`，本文不另写数值。

### 10.5 带宽与 CPU 代价（人工审计：估算，未实测）

| 入口 | 带宽 | CPU | 适用 |
|---|---|---|---|
| terminal (ttyd) | 文本流，通常 <100 kb/s，刷新时短时突发 | 很低 | menuconfig、shell、日志查看 |
| graphical (noVNC) | 1280x800x24 帧缓冲，静态界面低；连续刷新时可达数 Mbps 到十余 Mbps | Xvfb + x11vnc 编码 + websockify，约占 10–30% 单核（估算） | 必须鼠标的图形配置工具 |

以上是工程估算，不是实测数据；M6 用 `nload`/`pidstat` 记录实际值。若图形入口连续刷新导致带宽/CPU 过高，优先关闭动画、降低分辨率、改用 terminal。

### 10.6 job 硬上限下的取舍（人工审计）

- 默认只开 terminal；graphical 默认 30 min、上限 120 min，且必须在 `max_minutes` 内。
- 图形入口的安装时间从会话预算里扣除；若安装 + 启动超过 `max_minutes` 的 20%，CLI 应警告并建议 terminal。
- job 硬上限前，watchdog 先停 graphical（它最耗资源），再收尾收集；不把图形会话拖到 360 min。

### 10.7 可写/只读双链接

吸收 tmate 的“可写/只读双链接”，但收紧为会话级策略：

| 链接类型 | 能力 | token | 默认 |
|---|---|---|---|
| 写链接 | 输入、保存、执行 | 一次性写 token，绑定 session_id + actor | `meito up` 默认生成 |
| 只读链接 | 只看，不可输入/保存 | 每通道签发一次读 token，可用至 `DELETE`/TTL，最多 10 次读取 | `meito share --read-only` |

- 只读实例优先用不带 `-W` 的 ttyd；若必须复用同一实例，网关层丢弃 stdin/键盘事件。
- 只读链接不能升级为写；升级必须重新走 `meito share --write` 并校验 actor。
- 两类链接都不写入 Job Summary 的完整连接串；只写类型与过期时间。

---

## 11. menuconfig 改 defconfig 化（并行路线）

目标：把“必须开交互 shell”降为“只在探索未知配置项时开”，大多数已知改动走非交互 workflow。

流程：

1. 从 `defconfig` 或现有 `.config` 开始。
2. 用项目适配器应用变更：
   - Linux kernel：`scripts/config --file .config --enable FOO` / `--disable FOO` / `--set-str FOO bar`；
   - Buildroot：`make savedefconfig BR2_DEFCONFIG=...` + `utils/config`；
   - 其他：项目适配器实现 `apply()` 与 `save()`。
3. `make olddefconfig` 处理依赖。
4. `make savedefconfig` 生成最小 defconfig。
5. 上传 `.config`、`defconfig`、diff、manifest；本地 `meito pull` 应用。

判据（SC-13）：同一输入连续两次生成 `defconfig` 的 SHA256 相同；与 menuconfig 手工结果的差异必须逐项可解释；不允许“看起来差不多”作为通过。

这条路线与交互入口并列，不替代 menuconfig 的探索能力。

---

## 12. 与既有工程纪律的对接

### 12.1 规则引用

| 规则 | 在本项目中的落点 |
|---|---|
| R-D-1 单一指挥口 | README 指向本设计；任务派发从同一入口发出；分支合并由指挥统一安排 |
| R-D-2 派发写全 | 每个实施任务按“目标/输入/约束/判据/产出/停止点/报告触发”七项派发（模板见附录 B） |
| R-D-3 判据先行 | 每条判据先写进 `docs/验收记录/`，再执行；SC-01…SC-21 已先行定义 |
| R-D-4 验收不放宽 | 不达标不改阈值；未跑通写“未验证”；禁止伪造通过 |
| R-D-5 结论必带证据 | 日志、run_id、artifact、服务器计数、SHA256；未实测标“未验证” |
| R-D-6 仅三类请示 | ①推翻已定偏好 ②协议/架构 ③性能与观感取舍；其余按设计执行 |
| R-D-7 资源治理 | `.meito/build`、`dist/`、日志轮转与 `meito clean`；每轮报告释放空间 |
| R-D-8 事故入库 | 隧道被禁、凭证泄露、孤儿会话等当天写复盘并更新风险清单 |
| R-D-9 版本与出包冒烟 | `version.txt` 单一来源；产物名含版本/commit/平台；安装→启动→首屏→退出四步冒烟 |
| R-D-10 并行安全 | 一个 worktree 一条线；文件边界写进派发；不共享工作树 |
| R-D-11 CI 打包与自更新 | GitHub Actions 构建、校验、发布；CLI 自更新校验哈希/签名 |
| R-D-12 代码证据 | 文中数值/常量在实现后必须能在代码中定位；未验证项不得写成实测 |
| R-C-1 编码 | 源码 UTF-8；含中文的 C/C++ 需 BOM；脚本按各自要求 |
| R-C-2/3 错误处理 | 不吞错；错误必须带阶段、上下文、可操作建议 |
| R-C-5 有界队列 | 日志/事件/轮询队列必须有上限；禁止无界内存增长 |
| R-C-6 能力探测 | `doctor` 探测 gh/ssh/frpc/浏览器/磁盘，而不是假设存在 |
| R-C-7 设置回读 | `meito config validate` 回显生效值；不把“保存成功”当“生效” |
| R-C-11 跨平台 CI | CLI 在 Windows/Linux/macOS 构建与测试；入口仅 Linux runner |
| R-C-12 协议/安全边界 | P2P、SSH、入口、artifact 的信任边界在本文第 8/9/10 节固定 |
| R-C-14 版本一致 | 版本号、包元数据、构建产物、日志 `tool_version` 同源 |
| R-C-15 构建位置 | 构建产物放 `.meito/build`；仅 C 盘时记录豁免并保持空间阈值 |
| R-C-16 安装与配置 | Windows 装 `C:\Program Files\Meito\`；配置在 `config\`；仅该目录可写 |
| R-N-1…N-9 注释 | 代码文件头、函数文档、参数段、组件/控制分区、同批更新；见 `docs/工程规范索引.md` |
| R-L-1…L-11 日志 | 统一 JSONL 日志层；CLI stdout 仅用户可见输出；不吞错；日志落盘与轮转 |
| R-T-1 角色 | 产品/开发/测试/用户/指挥五角色，职责见 12.3；可兼任但边界不混 |
| R-T-2 冲突标记 | 门禁检查 `<<<<<<<`；禁止带冲突提交 |
| R-T-3 公共文件 | 共享文件改动先登记；一个提交只做一件事 |
| R-T-4 改动记录 | 每次源码/设计变更写 `docs/变更记录.md` |
| R-T-5 里程碑 | 每阶段结束向指挥报告，不静默跨阶段 |
| R-T-7 不自行合并 | 各线不自行合并主干；合并由指挥安排 |
| R-T-8 共享文件隔离 | 同一文件不由两条线同时修改；冲突先停下 |
| R-T-9 一事一提交 | 文档、门禁、功能分开提交；不混提交 |
| R-T-10 不伪造证据 | 本次未跑 runner；所有 runner 行为标“未验证” |
| R-T-11 工作树归属 | `docs/分支归属.md` 六行；提交前核对 |

### 12.2 门禁

- 模板来源：`C:\Users\Administrator\Desktop\工程经验库\门禁模板\`，已拷入 `tools/gate-template/`，活动脚本 `scripts/gate.ps1`，配置 `gate.config.json`。
- 命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -SelfTest
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Changed -BaseRef HEAD~1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\quality-gate.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\arch-check.ps1
```

- `quality-gate.ps1` 串接：经验库门禁 → `cargo fmt --check` → `cargo clippy --all-targets -- -D warnings` → `cargo test` → `arch-check.ps1`。任一失败即退出非 0。
- CI：`.github/workflows/gate.yml`，`windows-latest` 跑经验库门禁与架构检查，`ubuntu-latest` 跑 Rust 格式/Clippy/测试；不使用 `|| true`。
- 豁免：仅“构建目录位于系统盘”一项，原因是本机只有 C 盘（约 8.84 GB 可用），无法满足“构建产物不落系统盘”的物理前提；仍保留 `minFreeGB=10`，空间不足时不打包。豁免记录在 `docs/门禁豁免清单.md`，不是放宽判据。

### 12.3 角色

| 角色 | 职责 | 本项目当前 |
|---|---|---|
| 产品/站长 | 决定目标、优先级、三类请示 | 用户 |
| 开发 | 实现 CLI/workflow/远端脚本 | 后续实施线 |
| 测试 | 执行判据、留日志、判定未验证 | 后续测试线 |
| 用户代表 | 从真实菜单操作视角验收 | 站长兼 |
| 指挥 | 派发、验收、合并、资源治理 | 站长兼 |

角色可兼任，但“开发不得自己判定验收通过”和“指挥不得替开发伪造证据”的边界不混（R-T-1、R-T-10）。

### 12.4 阶段与提交

- 每阶段一个分支/worktree；阶段结束提交一次，标注阶段号；不把多个阶段攒成一个提交（R-D-10、R-T-9）。
- 本文档与仓库初始化是 M0；门禁与 CI 单独提交；后续阶段按 M1…M10 推进，M1 为独立的接口冻结阶段。
- 合并由指挥统一安排；各线不自行合并主干（R-T-7）。

### 12.5 研发规范

研发规范与架构约定见第 13 章；可执行门禁条目见 13.8。本章只负责与经验库规则编号的对接，不重复规范正文。

---

## 13. 研发规范与架构约定

本章是已定结论，不再做方案对比；后续实现按此执行，偏离需走 R-D-6 的三类请示。

### 13.1 技术栈

| 层 | 决定 |
|---|---|
| 核心 | **Rust**；`meito-core` 单一内核，负责状态机/鉴权/P2P 策略/入口/artifact/日志 |
| 打洞层 | **不重写协议**；以外部进程调用锁版本的 `frpc` 二进制；Tailscale 同理调用 `tailscaled`/`tailscale` |
| 网页版 | **不上 SPA 框架**；单页 HTML/CSS/JS + 轻量 WebSocket；xterm.js/noVNC 按需加载；noVNC 只在图形入口按需启动 |
| 本地版 | **以 CLI 为主**；Rust 二进制 `meito`，不要求浏览器 |
| runner 侧 | `meito-runner` Rust 二进制 + 薄 shell 脚本；不引入 Python/Node 作为运行依赖 |
| 数据面 | 传输用成熟进程/库（SSH、frpc、WebRTC），Meito 只做编排与适配，不写新隧道协议 |

### 13.2 低延迟原则

- **数据面直通**：终端/图形字节从 runner 入口到用户界面只经过必要的加密传输，不做应用层拆包/重排。
- **终端二进制透传**：PTY/DataChannel 上直接传二进制；禁止每帧 JSON 包装，禁止把按键/输出转成结构化消息再还原。
- **禁止行缓冲**：不引入逐行读取后再转发的包装层；保持流式。
- **禁止应用层二次代理**：不为了“统一入口”再套一层 HTTP/WebSocket 代理；ttyd/noVNC 的端口只给隧道/网关用。
- **禁止 TLS 套 TLS**：SSH/ WireGuard/ WebRTC 已提供加密；不再在隧道内套一层 TLS，避免双重握手与额外 RTT。
- 例外只能用于控制/信令（SDP/ICE、状态事件），且必须在日志中标注 `control`，不能进入数据面。

### 13.3 MVC 映射

| MVC | 在本项目 | 约束 |
|---|---|---|
| Model | 会话状态机：启动 → 打洞 → 接入 → 保活 → 回传 → 清理 | 只在 `meito-core`；两个外壳共用同一 Model，禁止各自持有状态副本 |
| View | 终端外壳（CLI/ttyd）与网页外壳（单页/noVNC） | 只做展示与输入；不判断 P2P/鉴权/artifact 策略 |
| Controller | 命令与事件路由：CLI 子命令、web 控制事件、runner 事件 | 只做路由与参数校验；业务决定回到 Model |

- 两个外壳通过同一组 `Command`/`Event`/`SessionView` 接口访问 Model；接口在 M1 冻结。
- 禁止 `meito-web` 在前端维护一份 `SessionState` 并与后端“同步”；页面只渲染内核推送的只读视图。
- 禁止 `meito-cli` 与 `meito-web` 各自实现 dispatch/artifact/P2P 判定；只能调用 `meito-core`。

### 13.4 日志设计

- **JSONL**，字段固定：`ts`、`level`、`session_id`、`stage`、`event`、`error_code`；`session_id` 贯穿本地、runner、协调服务。
- 路径固定：`logs/session-<id>.jsonl`；失败环境台账：`logs/env-session-<id>.json`。
- 密钥强制脱敏：token、Authorization、私钥、连接串、frp secretKey、OAuth code 一律在日志层替换为 `[REDACTED]`。
- **禁止 print 当日志**：Rust 用 `tracing` + JSON formatter；CLI 的 stdout 只用于用户可见结果，不用于诊断记录。
- 日志轮转与保留：本地默认保留 7 天，`meito clean --older-than` 清理；artifact 日志 retention 1 天。

### 13.5 debug 设计

- **三级日志开关**：`info`（默认）、`debug`、`trace`；通过 `--log-level` 或 `MEITO_LOG` 设置，非法值拒绝。
- **失败自动输出环境台账**：镜像、包版本、隧道状态、UDP/ICE 探测、入口端口、退出码、run_id；写入 `logs/env-session-<id>.json`。
- **`doctor` 子命令**：只读体检，覆盖 gh 权限、默认分支、控制器、UDP/STUN、frpc/tailscale、浏览器能力、磁盘、日志目录。
- `trace` 必须显式开启并提示可能包含连接细节；默认绝不开启。
- 失败信息必须包含：错误码、阶段、复现命令、日志路径、下一步建议（R-C-3）。

### 13.6 代码规范

- `rustfmt` + `clippy --all-targets -- -D warnings` 纳入门禁；CI 与本地都执行。
- 文件头用 `//!` 说明模块用途、边界、不变量；首 25 行内必须出现。
- 函数用 `///`，含参数说明与错误说明；返回 `Result` 的公共函数必须写清错误条件。
- 关键分支逐行注释，只写“为什么”，不复述代码；分区注释统一为 `// ===== 组件名 =====`。
- 命名、错误码、日志字段、配置键在 M1 冻结；后续新增只能追加，不能重命名已有键。
- 禁止在数据面使用会引入缓冲/序列化的“顺手抽象”；性能敏感路径先用最小实现，再按证据优化。

### 13.7 迭代策略

- **接口先冻结再并行**：M1 冻结公共 trait、命令、事件、日志字段、错误码、配置键、版本号规则；冻结后两外壳可并行。
- **阶段独立可交付**：每阶段有独立产出物与验收判据，不把多个阶段混成一个交付。
- **每阶段一个 commit 作回滚点**：阶段结束提交一次；不把文档、门禁、功能混提交（R-T-9）。
- **版本号随改动递增**：`Cargo.toml` 的 workspace version 是唯一来源；源码变更必须 bump；文档-only 变更可不 bump，但需在变更记录说明。
- 冻结后的接口修改：必须写 `docs/变更记录.md`、更新接口文档、评估两外壳影响，并递增次版本号。

### 13.8 可执行门禁条目（与经验库模板衔接）

| # | 规范 | 门禁/CI 条目 | 对应规则 |
|---|---|---|---|
| G-01 | Rust 核心 | `cargo fmt --check`；`cargo clippy --all-targets -- -D warnings`；`cargo test` | R-C-1、R-C-2/3 |
| G-02 | frpc 外部进程 | `arch-check`：不得链接 frp 库、不得实现隧道协议；只允许 `Command::new("frpc")` 适配 | R-C-12 |
| G-03 | 无 SPA | `arch-check`：`web/` 不得出现 React/Vue/Angular/Svelte 依赖或构建配置 | 本设计 13.1 |
| G-04 | 低延迟 | `arch-check`：数据面不得出现逐帧 JSON、行缓冲、二次代理；控制面例外需标 `control` | 本设计 13.2 |
| G-05 | MVC 单一 Model | `arch-check`：状态枚举只在 `meito-core`；两外壳不得定义 `SessionState` 副本 | 本设计 13.3 |
| G-06 | JSONL 日志 | `gate.ps1` 的 R-L 规则 + `arch-check`：非 CLI 展示层禁止 `println!`；字段 schema 单测 | R-L-1、R-L-4、R-L-9 |
| G-07 | 日志路径/脱敏 | 单测断言 `logs/session-<id>.jsonl`；脱敏用例覆盖 token/私钥/连接串/OAuth code | R-L-9、SC-11 |
| G-08 | 三级 debug + doctor | `meito doctor` 可执行且只读；失败注入测试断言 `env-session-<id>.json` 生成 | R-C-6 |
| G-09 | 注释规范 | `gate.ps1` 的 R-N-1/R-N-2/R-N-4；`//!` 头、`///` 参数/错误、`// =====` 分区 | R-N-1…N-9 |
| G-10 | 接口冻结 | `docs/接口冻结.md` 存在且版本化；两外壳 stub 编译通过；冻结后改动有变更记录 | R-D-9、本设计 13.7 |
| G-11 | 一事一提交 | 提交前人工核对；CI 不强制 commit 粒度，但阶段验收检查 commit 数与阶段号 | R-T-9 |
| G-12 | 版本递增 | `gate.ps1` 的 R-D-9：源码变更未 bump 版本即失败 | R-D-9、R-D-12 |
| G-13 | 数值单一来源 | `contract-check`：文档正文的阈值/时长/上限/端口/状态码必须在 `contracts/` 有对应字段；机器校验覆盖 design §2、§3.2、§5.6、§5.7、§5.10、§8.3、§9.4、§10.2（启停顺序）、§10.3、§10.4、§10.7 与接口冻结 §5.2–§5.4；§10.2 估算表、§10.5、§10.6 等显式标注“人工审计”，不纳入机器覆盖 | 本设计 5.9、R-D-13 |

门禁配置（`gate.config.json`）必须体现：`crates/*/src/**/*.rs` 扫描、`//!` 头注释、`// ===== 组件名 =====` 分区、`tracing` 依赖、CLI 展示层 print 白名单。模板原件的 `tools/gate-template/` 保留，活动脚本在 `scripts/`。

---

## 14. 同类产品对比与取舍（消费者视角）

本章结论来自消费者线报告，**不重做对比、不自行补充产品事实**；未列出的产品能力不做推断。

### 14.1 总取舍

- 可借鉴的是**交互约定**，不是连接层。托管式中继（Codespaces 托管隧道、tmate 分享链接、VS Code Remote Tunnels、uptermd、Guacamole 网关）带来的便利，在“只打洞、不走流量”约束下整体放弃。
- 保留四类可迁移经验：链接形态、角色模型、生命周期、失败提示。
- 不因为竞品“开箱即用”就引入中继；Meito 的产品承诺必须与硬约束一致。

### 14.2 吸收清单

| 来源 | 值得吸收 | 在 Meito 的落点 | 明确不吸收 |
|---|---|---|---|
| tmate | 可写/只读双链接 | 10.7 的读/写 token 与只读实例 | 公共/自建 tmate 服务器中继 |
| upterm / action-upterm | 账号白名单、无人加入自动取消、连接串写入 Job Summary | actor 白名单（8.2）、`E_NO_JOIN_TIMEOUT`（7.1）、Job Summary 只写脱敏摘要（5.5） | uptermd 中继；v2 的托管依赖 |
| Codespaces | 生命周期与端口可见性展示 | 状态机可视化、入口端口/健康状态展示、`meito status` | 托管隧道与云端代管 |
| VS Code Remote Tunnels | 待消费者线清单补充 | 占位：可能用于“入口注册/设备信任”的交互约定 | 微软托管中继 |
| ttyd/gotty | 待消费者线清单补充 | 已采用 ttyd `-W` 回环模式；其余交互细节待清单 | 任何公网直曝 |
| Guacamole | 待消费者线清单补充 | 占位：图形入口的会话/权限模型可参考 | Guacamole 网关中继 |

**待补充项**：VS Code Remote Tunnels、ttyd/gotty、Guacamole 的具体“值得抄的优点”尚未在报告中逐条列出；本表只留占位与判据，等清单回来再填，不编造结论。

### 14.3 网页版的硬约束与文案

- 浏览器只能用 WebRTC DataChannel + STUN；排除 TURN。
- 对称 NAT 或 UDP 被封时连不上属**常态**，不是异常。
- 必须把“连不上”做成可解释状态：`E_WEB_NO_DIRECT_PATH` + 已尝试 candidate 类型 + 转本地版命令。
- 产品文案不得宣称“任意网络可连”；不得用“通常可用”掩盖失败率。

### 14.4 取舍判据

新增任何“借鉴项”前，必须同时满足：

1. 不引入中继、不把字节送到用户服务器；
2. 不复制第二份会话状态/业务逻辑；
3. 有可判定验收（状态、日志、计数、UI 行为），不是“看起来更好”；
4. 不突破 6 小时 job 上限与权限最小化；
5. 与 M1 冻结的接口兼容，或按 13.7 走变更记录与版本递增。

### 14.5 待拍板：BYO TURN

见 4.6。默认关闭；两种分支的接口位置已预留，等用户结论后再填默认值。

---

## 15. 里程碑与验收

| 阶段 | 产出物 | 可判定验收 | 回滚点 |
|---|---|---|---|
| **M0 设计/仓库** | 本设计、README、规范索引、开工清单、分支表、变更记录、门禁配置/脚本、CI、workflow 模板 | `gate.ps1 -SelfTest` 退出 0；全量门禁退出 0；`docs/分支归属.md` 六行；Git 提交存在；文档含“未验证”清单 | 本次提交 |
| **M1 接口冻结** | Rust workspace（core/cli/runner/web/link 骨架）；`docs/接口冻结.md`；公共 trait/命令/事件/日志字段/错误码/配置键/版本规则；两外壳 stub | 接口文档版本化；`cargo check` 通过；两个外壳以 stub 编译（不复制 Model）；冻结后改动需变更记录；SC-18 通过 | M1 分支提交 |
| **M2 P2P 实验（无 GitHub）** | 两台本地机 + 公网服务器；frp xtcp SSH 通道；服务器计数脚本；Tailscale 直连最小实验 | 本地到本地经 xtcp SSH 成功；100 MB 传输服务器字节 <5%；配置审计无 stcp/fallbackTo；Tailscale DERP 为空或明确失败 | M2 分支提交 |
| **M3 runner 预检** | workflow 最小 job：actor 校验、UDP 探测、依赖安装、加密 link 下发 | dispatch 成功；UDP 探测结果明确；link 只收密文；失败码正确；无会话也不留孤儿 | M3 分支提交 |
| **M4 CLI 编排 + ttyd** | `init/doctor/up/status/down`、frpc visitor、SSH master、ttyd、token 网关 | `meito up` 到达 ttyd；menuconfig 可交互并保存；SC-01/02/06 有实测记录 | M4 分支提交 |
| **M5 产物回传 + defconfig** | `pull`、manifest、diff、冲突处理、`kconfig` workflow | SC-03/13 通过；冲突样本按预期进 `.meito/conflicts`；不覆盖本地改动 | M5 分支提交 |
| **M6 图形入口** | Xvfb/WM/x11vnc/websockify、noVNC、token 网关、启停脚本 | SC-07 通过；`ss` 只见回环；无 token 401/403；带宽/CPU 实测记录 | M6 分支提交 |
| **M7 Tailscale 备路径** | Headscale 配置、直连模式、DERP 禁用、诊断 | SC-05 的 Tailscale 证据通过；检测到 DERP 时明确失败；不中继 | M7 分支提交 |
| **M8 生命周期/可观测** | watchdog、心跳、checkpoint、无人加入取消、孤儿清理、错误码、失败注入、doctor/env | SC-08/09/10/11/12/19 有实测；取消/硬杀场景日志完整或明确缺失 | M8 分支提交 |
| **M9 网页外壳** | 单页工作台、OAuth、WebSocket 信令、WebRTC DataChannel、只读/写链接、artifact 下载入口、架构检查 | SC-16/17/20/21 通过；无 TURN；对称 NAT 失败显示 `E_WEB_NO_DIRECT_PATH`；两外壳共用 Model | M9 分支提交 |
| **M10 打包/自更新/交付** | CI 构建、版本、安装器、自更新校验、四步冒烟 | R-D-9/R-D-11/R-C-16 验收；安装→启动→首屏→退出无残留 | M10 分支提交 |

每个阶段开始前写验收记录，结束后附证据；未验证项不得写成通过。M1 接口冻结必须先于并行开发；M2 不需要 GitHub 凭证，应优先做，用来提前证伪 UDP/NAT 假设。

---

## 16. 风险清单与降级

| 风险 | 影响 | 触发信号 | 缓解/降级 |
|---|---|---|---|
| runner 出站 UDP 被禁 | frp/Tailscale/WebRTC 都不可用 | M3 预检 `E_UDP_BLOCKED` | 非交互 defconfig；本地容器 + act；自托管 runner |
| NAT 类型不兼容，xtcp 打洞失败 | 无会话 | `E_P2P_XTCP`，重试后仍失败 | Tailscale 直连；失败则非交互/本地/自托管；**不中继** |
| Tailscale 只能用 DERP | 违反硬约束 | `tailscale status` 出现 relay；`ping` via DERP | 直接失败；检查 DERP map/peer relay；不改用中继 |
| 服务器字节回流 | 违反硬约束 | 100 MB 对照服务器增量 >5% | 停会话、保留证据、修配置；该 provider 禁用 |
| 网页版对称 NAT/UDP 被封 | 浏览器连不上 | `E_WEB_NO_DIRECT_PATH` | 明示原因；转本地版；不启用 TURN（除非拍板 BYO TURN） |
| 网页版被要求“任意网络可连” | 承诺无法兑现 | 文案/需求出现绝对化表述 | 拒绝文案；用 14.3 的失败提示 |
| BYO TURN 被误设为默认 | 违反硬约束 | `[turn].enabled=true` 或 TURN URL 非空 | 门禁阻断；默认关闭；需用户拍板 |
| ttyd TUI 渲染异常 | menuconfig 不可用 | 全屏错位/按键失效 | 缩小窗口/调整 `-t`；改用 SSH 原生终端；必要时降级到本地容器 |
| noVNC 依赖安装过慢 | 会话启动超时 | 安装 >5 min | 预构建镜像；默认 terminal；只在必要时图形 |
| 图形带宽/CPU 过高 | 体验差、挤占 runner | 实测 >10 Mbps 或 CPU >30% | 降分辨率、关动画、改 terminal |
| 6 小时硬上限 | 到点 fail、artifact 丢失 | run 到 360 min | watchdog 300 min 前收尾；checkpoint；长任务走 CI |
| 取消时日志未上传 | 诊断缺失 | 取消后 artifact 缺失 | `gh run view --log` 本地落盘；明确 `E_LOGS_UNAVAILABLE` |
| 公有仓库暴露 | 连接信息/日志公开 | repo 为 public | 默认禁止；显式确认；白名单；不注入秘密 |
| GitHub 脱敏失效 | 环境变量泄露 | 日志/`env` 出现 secret | 调试与生产凭据隔离；不注入；会话结束轮换 |
| 供应链被篡改 | 执行恶意二进制 | 校验失败 | 版本+SHA256/commit pin；不 `curl \| bash` |
| 孤儿会话占用额度 | 浪费分钟/暴露 | active run 超时 | watchdog；`meito clean --orphans`；自托管 runner 定时清理 |
| 本地 Windows 差异 | 命令/路径失败 | `doctor` 失败 | 统一 UTF-8；PowerShell 兼容；CI 三平台 |
| 服务器带宽/额度被误用 | 违反约束或账单 | 月流量异常 | 服务器只跑协调；监控出口；告警阈值 |

### 16.1 隧道被封的降级路径

按顺序：

1. 换 P2P provider：frp xtcp ↔ Tailscale 直连（两者都不中继；如果 Tailscale 只能用 DERP，则不能用）。
2. 非交互路线：`meito kconfig`（defconfig/savedefconfig），不需要 shell。
3. 本地容器 + act：在本地复现 runner 镜像，执行 menuconfig/构建；结果是本地环境，不是 runner 环境，需标注。
4. 自托管 runner：用户已有/可提供的机器，通过 SSH/VPN 直达；必须做隔离、清理、权限最小化。
5. 如果用户未来允许中继，必须重新拍板修改硬约束，不能由实现线自行放开 `allow_relay`。

---

## 17. 未验证清单

**runner 内行为全部未实测**（无 GitHub 凭证）。以下每一项都必须保留“未验证”，直到有 run_id、日志、artifact 或服务器计数证据。

| ID | 未验证项 | 依据/待验证方法 | 影响 |
|---|---|---|---|
| U-01 | 托管 runner 是否允许出站 UDP | M3 预检向服务器发 UDP 探测并收回执 | 决定 frp/Tailscale/WebRTC 是否可用 |
| U-02 | frp xtcp 在“本地 NAT + runner NAT”下的打洞成功率 | M2 两机实验，记录 NAT 类型与重试次数 | 决定主路径可用性 |
| U-03 | frps 的 XTCP 协调端口/字段与锁定版本 | M2 按版本核对配置并做连通实验 | 配置正确性 |
| U-04 | runner 上 ttyd 安装、启动、TUI 渲染 | M4 真实 menuconfig 冒烟 | 终端入口可用性 |
| U-05 | ttyd 全屏尺寸/按键与 menuconfig 兼容 | M4 `stty size`、方向键、保存 | 核心目标 G2 |
| U-06 | Xvfb/x11vnc/websockify 安装耗时与稳定性 | M6 记录安装时间、健康检查 | 图形入口启动预算 |
| U-07 | noVNC 在 runner 的 CPU/带宽 | M6 `pidstat`/`nload` | 6 小时内的取舍 |
| U-08 | runner 的 `/dev/net/tun` 是否可用 | M2/M7 最小实验 | Tailscale 备路径 |
| U-09 | Tailscale/Headscale 能否彻底禁用 DERP/peer relay | M7 `status/ping` + DERP map 检查 | 硬约束 |
| U-10 | GitHub artifact 下载延迟与 CLI 轮询 | M3/M5 记录时间戳 | 冷启动判据 SC-01 |
| U-11 | 取消/硬杀时 artifact/日志是否可获取 | M8 失败注入 | 诊断与审计 |
| U-12 | `workflow_dispatch` 默认分支与输入可见性 | M3 在私有仓库验证；公有仓库另测 | 安全与流程 |
| U-13 | 6 小时到点 fail 的实际表现 | 不做 6 小时长跑；用文档依据 + 短超时模拟 | 收尾设计 |
| U-14 | 公有仓库日志/artifact 的实际可见范围 | 仅在测试仓库验证，不碰生产 | 安全策略 |
| U-15 | 自托管 runner 降级路径 | 另立阶段，不在本期默认 | 降级可用性 |
| U-16 | `meito-link` 自身实现与 TTL/限流 | M3 单元测试 + 集成 | 连接信息下发 |
| U-17 | 本地 Windows 的 frpc/SSH/浏览器行为 | M4 在三平台跑 doctor/up | 用户体验 |
| U-18 | 服务器“无回流”证明脚本的准确性 | M2 用已知 payload 校准计数/抓包 | 硬约束证据 |
| U-19 | 托管 runner 是否“完全无入站 TCP” | 仅按“无公网入站”做保守假设；M3 探测，不写成已验证 | 保守设计前提 |
| U-20 | WebRTC 在 runner 侧的可达性（UDP/STUN/ICE） | M9 最小 DataChannel 实验；记录 candidate 类型 | 网页版可用性 |
| U-21 | 浏览器直连 GitHub artifact 的 CORS/鉴权 | M9 实测；失败则给 `gh run download` 命令 | 网页版回传 |
| U-22 | 网页版 OAuth 会话与权限范围 | M9 在测试仓库验证；不碰生产 | 安全 |
| U-23 | `getStats()` 判定 relay candidate 的稳定性 | M9 用已知 TURN 环境做反例校准 | 无 TURN 证明 |
| U-24 | BYO TURN 分支的产品/安全影响 | 待用户拍板后再验证；默认不实现 | 待决策 D-4 |

本次任务已实际验证的只有：经验库与门禁模板可读、门禁脚本可执行、仓库文件可写、Git 可初始化/提交。**没有任何 runner 行为被验证。**

---

## 18. 待决策（仅三类）

按 R-D-6，只列需要用户拍板的三类；其余由实现线按本设计执行。

| ID | 类型 | 问题 | 推荐 | 不决定的后果 |
|---|---|---|---|---|
| D-1 | 架构/协议 | P2P 主路径选 frp xtcp，还是 Tailscale 直连？ | frp xtcp 主、Tailscale 备；先做 M2 实测再定默认 | 选错会导致返工或 UDP 受限时无备路径 |
| D-2 | 产品边界 | 默认入口是否只保留 terminal，graphical 仅按需？ | 默认 terminal；graphical 默认 30 min、上限 120 min | 每次都装图形依赖，冷启动与 CPU 成本高 |
| D-3 | 性能/观感取舍 | 图形依赖用 apt 现装，还是维护预构建镜像？ | M6 先 apt 实测；若安装 >3 min 或经常超时，转预构建镜像 | 图形入口体验不稳定，或增加镜像维护成本 |
| D-4 | 架构/安全 | 是否允许“用户自带中继（BYO TURN）”作为默认关闭的可选兜底？ | 默认不允许（分支 A）；接口预留，等用户拍板后再实现分支 B | 网页版在对称 NAT 下只能失败转本地版；若用户需要 BYO TURN，需补实现与验收 |

不在以上三类的事项（命名、日志字段、目录结构、错误码、测试顺序等）由实现线自行决定并记录。

---

## 19. 附录

### A. workflow 模板骨架

完整评审版见 `templates/meito-session.yml`。骨架：

```yaml
name: meito-session
on:
  workflow_dispatch:
    inputs:
      session_id: { required: true, type: string }
      ref: { required: true, default: main, type: string }
      max_minutes: { required: true, default: "60", type: string }
      idle_minutes: { required: true, default: "10", type: string }
      p2p_provider: { required: true, default: frp_xtcp, type: choice, options: [frp_xtcp, tailscale_direct] }
      entry: { required: true, default: terminal, type: choice, options: [terminal, graphical, both] }
      channel: { required: true, type: string }
      artifact_spec: { required: true, type: string }
      flags: { required: true, type: string }
      meito_version: { required: true, type: string }
permissions:
  contents: read
  actions: read
concurrency:
  group: meito-session-${{ github.repository }}
  cancel-in-progress: false
jobs:
  session:
    runs-on: ubuntu-latest
    timeout-minutes: 360
    environment: meito-debug
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - name: actor allowlist
        run: bash scripts/remote/check-actor.sh
      - name: preflight
        run: bash scripts/remote/preflight.sh
      - name: bootstrap
        run: bash scripts/remote/bootstrap.sh
      - name: session
        run: bash scripts/remote/run-session.sh
      - name: job summary
        if: always()
        run: bash scripts/remote/summary.sh
      - name: collect
        if: always()
        run: bash scripts/remote/collect.sh
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: meito-logs-${{ inputs.session_id }}
          path: logs/session-${{ inputs.session_id }}.jsonl
          retention-days: 1
```

补充：`logs/env-session-<id>.json` 与 `.config`/diff 走独立 artifact（`meito-artifacts-<id>`），上传前由 `meito-runner collect` 执行 `artifact_guard`；Job Summary 只写脱敏摘要，不写连接串。

### B. 实施派发单模板（R-D-2）

```text
目标：<一句话，可验收>
输入：<文件/分支/服务/凭证来源>
约束：<必须遵守的规则编号、禁止事项、文件边界>
判据：<可判定标准，含阈值与证据形式>
产出：<代码/文档/日志/artifact>
停止点：<做到哪里停，不越界>
报告触发：<何时必须向指挥报告>
```

### C. 错误码与退出码

- CLI 退出码：0 成功；2 用法错误；3 diff 冲突；4 预检失败；5 P2P 失败；6 入口失败；7 artifact 失败；8 超时/取消；9 安全阻断。
- 错误码见 5.8；日志中必须同时有 `error_code` 与 `stage`。
- 禁止只用“失败”两个字结束；必须带可操作建议。

### D. 外部事实来源（设计输入，不是运行证据）

- tmate 公共服务器停服与 `action-tmate` 默认不可用：<https://mxschmitt.github.io/action-tmate/>
- upterm / action-upterm v2 行为与 GitHub 身份白名单：<https://github.com/owenthereal/action-upterm>、<https://upterm.dev/docs/upterm_host.html>
- frp XTCP 的 P2P 特性与 `fallbackTo` 回落 STCP：<https://gofrp.org/en/docs/features/xtcp/>
- Tailscale DERP 回落与自定义 DERP/禁用默认区域：<https://tailscale.com/docs/reference/derp-servers>
- Cloudflare Quick Tunnel 仅 HTTP、Named Tunnel 才支持非 HTTP：<https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/>、<https://developers.cloudflare.com/tunnel/setup/>
- ngrok TCP 免费计划需支付方式：<https://ngrok.com/docs/pricing-limits>
- ttyd `-W` 可写、默认只读：<https://github.com/tsl0922/ttyd>
- GitHub Actions 6 小时 job 上限：<https://github.github.io/gh-aw/reference/sandbox/>
- `workflow_dispatch` 必须在默认分支：<https://githubnext.github.io/gh-aw/guides/github-actions-primer/>

以上只作为选型依据；其中任何 runner 内行为都仍需实测，标“未验证”。

### E. 术语

- **P2P 打洞**：双方通过公网协调服务器交换地址，随后 UDP 直连；服务器不转发会话字节。
- **协调服务器**：只交换 peer 信息与密文的服务器；本项目中的 frps + `meito-link`。
- **入口**：runner 上的渲染服务（ttyd/noVNC），不等于隧道。
- **无回流**：服务器字节计数与抓包证明会话 payload 未经过服务器。
- **checkpoint**：会话结束前先上传一次产物与日志，降低硬杀丢失风险。
- **未验证**：没有 run_id/日志/计数等直接证据，不能写成通过。
