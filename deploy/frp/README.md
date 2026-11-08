# Meito 打洞协调节点部署包（frps）

> 位置：`C:\Users\Administrator\Desktop\Meito-deploy`（git worktree）
> 分支：`feature/deploy-frp`，基于 `main` 的 `f0d1b49`
> 范围：只写部署交付物；未改动 `contracts/`、`crates/`、`templates/`；未合并主干。

## 1. 这个节点做什么

一台阿里云 ECS 上运行 `frps`，只负责：

1. 接受受控 `frpc` 的登录（TCP 7000，TLS + token）；
2. 交换 XTCP 打洞所需的 peer 信息；
3. 让两端 `frpc` 自行 UDP 打洞，会话数据走点对点直连。

它**不**负责：HTTP/HTTPS 代理、TCP/UDP 端口转发、stcp 转发、仪表盘、
SSH 网关、KCP/QUIC 控制通道、任何形式的中继。

## 2. 服务器事实与未知项

| 项 | 值 | 状态 |
|---|---|---|
| 云厂商/规格 | 阿里云 ECS，成都可用区 B，2 vCPU / 2 GiB | 用户提供 |
| 带宽 | 固定 3 Mbps | 用户提供 |
| 系统盘 | ESSD Entry 40 GiB | 用户提供 |
| 公网 IP | `47.109.32.249` | 用户提供 |
| 内网 IP | `172.31.251.216` | 用户提供 |
| 安全组 | `sg-2vchr9d5anwtxrk1y1vi` | 用户提供 |
| 系统 | Ubuntu 20.04 64 位（镜像 `alibase_20231113`） | 用户提供 |
| 域名 | `wuge.fun`（已备案，个人非商业） | 用户提供；**本部署不使用域名** |
| 计费 | 包年包月，长期在线 | 用户提供 |
| 补丁级别 / 内核版本 | 未知 | **未验证** |
| Ubuntu Pro/ESM 状态 | 未知 | **未验证** |
| 是否启用 IPv6 | 未知 | **未验证** |
| 宿主防火墙（ufw/nft/iptables）状态 | 未知 | **未验证** |
| SSH 端口/来源限制 | 未知 | **未验证** |
| 当前是否已装/运行 frps | 未知 | **未验证** |

## 3. 版本锁定与关键结论

### 3.1 锁定版本

- **frps / frpc：`v0.71.1`（linux_amd64）**
- 发布包：`frp_0.71.1_linux_amd64.tar.gz`
- SHA-256：`1d9875210972a5d1a62b2dc4c48df45ab0af05d3ca1b5e494a014428259df803`
  - 来源：GitHub Releases API 的 asset digest（写入时读取）；
  - **未在本机下载复算**，安装时必须用 `sha256sum -c` 自己校验。

为什么是 v0.71.1：

- `auth.tokenSource` 从 **v0.64.0** 才支持；用文件读 token 是本交付物的硬要求；
- `transport.tls` 自 v0.50.0 起在 frpc 侧默认开启，服务端可安全地 `tls.force=true`；
- 这是写入时 GitHub 上的最新稳定版（2026-10-14 发布）。

`tools.lock.json` 在 `main` 上目前仍把 `frpc` 标为 `unpinned`。本交付物**没有修改它**
（超出本次部署物范围）。接入 meito-runner 前必须把 frpc 也钉到 v0.71.1 并写入
`url`/`sha256`，否则「服务端锁定版本」没有意义。

> 如果以后换版本，必须重新核对：
> - 是否存在服务端 UDP 协调端口字段（旧版 `bind_udp_port`）；
> - `tokenSource` 字段是否仍存在；
> - `tls.force` 与客户端默认值是否兼容。

### 3.2 关键结论：v0.71.1 没有服务端 UDP 打洞端口

任务背景里预期「打洞端口 UDP」，但按 v0.71.1 的源码与配置参考：

- `ServerConfig`（`pkg/config/v1/server.go`）没有 UDP 协调端口字段；
- 服务端只在 `bindPort` 上监听 TCP；只有显式配置 `kcpBindPort` / `quicBindPort`
  时才额外监听 UDP（`server/service.go`）；
- XTCP 的 UDP socket 在两端 frpc 上，STUN 由客户端访问外部 STUN 服务；
- NAT 分析控制器只在内存中维护策略数据，不监听端口。

因此：

- **安全组不要开放任何 UDP 入方向端口**；
- 真机仍必须用 `sudo ss -lunp | grep frps` 确认没有 UDP 监听；
- 这项运行期结论**未验证**，只有真机输出才能写「通过」。

如果你的实际 frp 版本低于 v0.52（INI 配置、`bind_udp_port`），规则需要按该版本调整；
本包不覆盖该分支。

## 4. 文件清单

| 文件 | 用途 |
|---|---|
| `frps.toml` | frps 配置，逐字段注释；不含密钥 |
| `frps.service` | systemd 单元：自动重启、启动依赖、权限收敛、journald 日志 |
| `journald-frps-cap.conf` | journald 落盘与容量上限（256 MiB / 14 天） |
| `security-group.md` | 安全组端口清单、来源限制、禁止端口与原因 |
| `credentials.md` | token 生成、存放、防泄漏、轮换、泄漏处置 |
| `os-ubuntu-20.04.md` | ESM 与升级 22.04/24.04 的取舍、前置检查、回滚点 |
| `verification.md` | 可判定的「只打洞、无回流」验证清单与记录模板 |
| `failure-recovery.md` | frps 挂掉、端口被封、token 不匹配等故障判定与恢复 |
| `.gitignore` | 禁止 token/密钥文件入库 |

## 5. 部署步骤（复制到服务器执行前先读一遍）

以下命令假设你已把本目录上传/解压到服务器的 `/tmp/meito-frp/`。
不要在没有快照和维护窗口的情况下直接操作生产机。

### 5.1 下载并校验 frps

```bash
cd /tmp
curl -fL -o frp_0.71.1_linux_amd64.tar.gz \
  https://github.com/fatedier/frp/releases/download/v0.71.1/frp_0.71.1_linux_amd64.tar.gz

printf '%s  %s\n' \
  '1d9875210972a5d1a62b2dc4c48df45ab0af05d3ca1b5e494a014428259df803' \
  'frp_0.71.1_linux_amd64.tar.gz' | sha256sum -c -

tar -xzf frp_0.71.1_linux_amd64.tar.gz
sudo install -m 0755 frp_0.71.1_linux_amd64/frps /usr/local/bin/frps
/usr/local/bin/frps -v    # 预期 0.71.1
```

如果服务器无法直连 GitHub，先在本地下好并核对哈希，再通过受控通道上传；
不要跳过校验。若系统没有 `curl`，可用 `wget -O frp_0.71.1_linux_amd64.tar.gz <同一 URL>`。

### 5.2 创建运行用户与目录

```bash
getent group frps >/dev/null || sudo groupadd --system frps
id -u frps >/dev/null 2>&1 || sudo useradd --system --gid frps \
  --no-create-home --shell /usr/sbin/nologin frps

sudo install -d -m 0750 -o root -g frps /etc/frp
```

### 5.3 安装配置、unit、journald 上限

```bash
sudo install -m 0644 /tmp/meito-frp/frps.toml /etc/frp/frps.toml
sudo install -m 0644 /tmp/meito-frp/frps.service /etc/systemd/system/frps.service
sudo install -d -m 0755 /etc/systemd/journald.conf.d
sudo install -m 0644 /tmp/meito-frp/journald-frps-cap.conf \
  /etc/systemd/journald.conf.d/99-frps-cap.conf
```

### 5.4 生成 token（不要打印 token）

严格按 [`credentials.md`](credentials.md) 第 2 节执行。核心命令：

```bash
sudo sh -c 'umask 077; openssl rand -hex 32 > /etc/frp/frps.token'
sudo chown root:frps /etc/frp/frps.token
sudo chmod 0640 /etc/frp/frps.token
sudo -u frps test -r /etc/frp/frps.token && echo 'OK: frps can read'
```

### 5.5 启动与自检

```bash
sudo /usr/local/bin/frps verify -c /etc/frp/frps.toml
sudo systemctl daemon-reload
sudo systemctl restart systemd-journald
sudo systemctl enable --now frps

sudo systemctl status frps --no-pager
sudo ss -lntp | grep ':7000'
sudo ss -lunp | grep frps || echo 'OK: no frps UDP listener'
```

预期：只有 7000 TCP；没有 frps 的 UDP 监听。
如果出现 UDP 或其他端口，先停下来，不要放行安全组。

### 5.6 配置安全组

按 [`security-group.md`](security-group.md) 只放行 TCP 7000；
不要开放 7500/80/443/7001 或任何 UDP 入方向端口。

### 5.7 执行验证

按 [`verification.md`](verification.md) 逐项执行 A-D（至少 A、B、C、D1、D4），
把命令、输出、判定写进记录。**部署完成不等于验证通过。**

## 6. 安全边界与残余风险

### 6.1 已做的配置层收敛

- 不配置 vhost HTTP/HTTPS、tcpmux、SSH 网关、仪表盘、KCP/QUIC；
- 不配置任何 tcp/udp/stcp 代理；
- 服务端只监听 TCP 7000；
- token 从服务端文件读取，TLS 强制开启；
- systemd 非 root、最小 capability、只读系统目录；
- journald 硬容量上限。

### 6.2 必须承认的残余风险

1. **frps 没有代理类型白名单**。持有 token 的客户端理论上可以注册 stcp/tcp/udp 代理；
   服务端配置无法按类型拒绝。防线是：token 保密 + 客户端配置审计（禁止
   `stcp`/`fallbackTo`）+ 安全组关闭数据端口 + `ss` 监听面与字节计数验证。
   若需要协议层强制，只能自研 `NewProxy` 插件；当前**未实现**。
2. **TCP 7000 默认对全网开放**。GitHub 托管 runner 出口 IP 不稳定，无法可靠白名单；
   防护依赖 token/TLS/日志与监控。固定出口 IP 的自托管 runner 可收窄来源。
3. **XTCP 成功率取决于 NAT**。对称 NAT 或 UDP 被封时打洞会失败；项目禁止中继，
   只能转本地版/自托管 runner等降级路径，不能宣称任意网络可连。
4. **安全组是云侧控制**。本包只能给规则建议；实际规则是否生效、优先级是否正确，
   必须在控制台核对并从外部扫描验证。
5. **Ubuntu 20.04 已过标准支持期**。ESM 可用性与升级风险见 `os-ubuntu-20.04.md`。

## 7. 明确「未验证」的项

以下全部**没有在真机执行**，不得写成「已通过」：

- frps v0.71.1 在本机的实际监听面（尤其「无 UDP 监听」）；
- 发布包 sha256 未在本机复算（哈希来自 GitHub API）；
- 阿里云安全组规则的实际生效与外部端口扫描；
- 公网 7000 的可达性、GitHub runner 出口 IP/UDP 行为；
- XTCP 在本节点 + 实际 NAT 组合下的打洞成功率；
- `systemd` 权限收敛指令在 Ubuntu 20.04（systemd 245）上是否全部被接受；
- journald `SystemMaxUse` 在日志洪峰下是否按预期清理；
- Ubuntu Pro/ESM 是否可用、`pro` 客户端是否预装；
- `meito-runner` 的 `tokenSource` 支持与 frpc 版本锁定；
- 故障手册中的日志措辞与恢复时间。

已核对（来自上游源码/文档，不是真机验证）：

- v0.71.1 `ServerConfig` 字段与 `server/service.go` 的监听逻辑；
- `auth.tokenSource` 自 v0.64.0 起支持、文件读取会 trim 空白；
- frpc TLS 自 v0.50.0 起默认开启；
- v0.71.1 发布包 digest（来自 GitHub Releases API）。

## 8. 变更与提交

- 分支：`feature/deploy-frp`；worktree：`C:\Users\Administrator\Desktop\Meito-deploy`；
- 本包只新增 `deploy/frp/` 下文件；
- 未修改 `contracts/`、`crates/`、`templates/`；
- 未合并 `main`，合并由用户统一安排；
- 后续若要改配置，必须同步更新 `docs/变更记录.md`（在主干线统一处理）并递增版本记录。
