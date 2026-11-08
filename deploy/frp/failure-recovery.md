# 失败与恢复手册

> 适用范围：Meito 打洞协调节点（frps v0.71.1，Ubuntu 20.04）。
> 所有日志措辞随 frp 版本可能变化；下文的匹配关键词用于定位，不代表逐字一致。
> 真机故障表现**未验证**；本文给出判定方法与恢复路径，执行后必须留下证据。

## 0. 通用排查顺序

```bash
# 1) 服务状态
sudo systemctl status frps --no-pager
sudo systemctl is-active frps
sudo systemctl is-enabled frps

# 2) 最近日志
sudo journalctl -u frps -n 100 --no-pager
sudo journalctl -u frps --since '30 minutes ago' --no-pager | tail -n 200

# 3) 监听面
sudo ss -lntp | grep -E 'frps|:7000'
sudo ss -lunp | grep frps || echo 'OK: no frps UDP listener'

# 4) 磁盘/内存
df -h /
free -m
sudo journalctl --disk-usage

# 5) 配置可读性
sudo /usr/local/bin/frps verify -c /etc/frp/frps.toml
sudo stat -c '%U %G %a %n' /etc/frp/frps.toml /etc/frp/frps.token
```

原则：

- 先取证，再重启；重启会丢失现场（但 journald 会保留日志）。
- 恢复后必须重跑 `verification.md` 的 A-D，不得只看 `active`。
- 任何涉及安全组、token、配置的改动都要记录时间、原因、执行人。

## 1. frps 进程挂掉 / 启动失败

### 判定

```bash
sudo systemctl status frps --no-pager
# active (running) = 正常；failed / activating (auto-restart) = 异常
sudo systemctl show frps -p NRestarts -p ExecMainStatus -p Result
```

日志中重点找：

```bash
sudo journalctl -u frps -n 100 --no-pager | grep -iE 'panic|fatal|error|failed|listen|token|permission|bind'
```

### 常见原因与处理

| 原因 | 表现 | 判定 | 恢复 |
|---|---|---|---|
| 配置语法错误 | 启动即退出，日志有 parse/config 错误 | `frps verify` 失败 | 恢复上一版 `/etc/frp/frps.toml`，verify 通过后重启 |
| token 文件缺失/权限错 | 日志有读取文件失败；`sudo -u frps test -r` 失败 | `stat` 权限不是 root:frps 0640 | 按 credentials.md 重建/修正权限，重启 |
| 7000 被占用 | 日志 bind/listen error；`ss -lntp '( sport = :7000 )'` 显示其他进程 | 端口冲突 | 停止冲突进程；不要改 frps 端口除非同步安全组 |
| 磁盘满 | 日志写入失败/无法读取配置；`df` 100% | 磁盘满 | 清理 journald/大文件，确认限额生效后重启 |
| OOM | `dmesg` 有 oom-kill；`journalctl -u frps` 突然中断 | 2 GiB 内存不足 | 释放内存/限制并发；必要时加 swap 或升配 |
| 二进制损坏/权限 | 启动失败，`file /usr/local/bin/frps` 异常 | 校验失败 | 重新下载并核对发布包 sha256 |
| 启动限流 | `start-limit-hit`，服务不再自动重启 | 60 秒内失败 5 次 | 先修根因，再 `sudo systemctl reset-failed frps`，然后启动 |

### 自动重启行为

- `Restart=always`、`RestartSec=5`：崩溃后约 5 秒自动重启。
- `StartLimitIntervalSec=60`、`StartLimitBurst=5`：连续失败会进入 failed，避免无限循环。
- 验证自愈的方法见 `verification.md` G 节（维护窗口）。

### 恢复判据

- `systemctl is-active frps` = active；
- `ss -lntp` 只有 7000；
- 外部 `nc -vz 47.109.32.249 7000` 成功；
- 日志无 panic/权限/磁盘错误；
- 重跑 A-D 验证。

## 2. 端口被封 / 外部连不上 7000

### 判定步骤

1. 服务器本机：
```bash
sudo ss -lntp | grep ':7000'
# 有监听 => frps 正常，问题在链路/安全组/宿主防火墙
# 无监听 => 先按第 1 节处理 frps
```
2. 从外部：
```bash
nc -vz -w 3 47.109.32.249 7000
# succeeded => 链路正常
# timeout / refused => 继续
```
3. 服务器抓包，看 SYN 是否到达：
```bash
sudo timeout 30 tcpdump -i any -nn 'tcp port 7000'
# 外部连接时：
#   看到 SYN 到达但没有 SYN-ACK => 宿主防火墙/安全组出方向或本机丢弃
#   完全没有 SYN => 安全组未放行，或请求未到达该实例
```
4. 检查宿主防火墙：
```bash
sudo ufw status verbose 2>/dev/null || true
sudo nft list ruleset 2>/dev/null | head -n 80
sudo iptables -S 2>/dev/null | head -n 80
```
5. 检查安全组：
- 实例是否绑定 `sg-2vchr9d5anwtxrk1y1vi`；
- 是否有 TCP 7000 允许规则；
- 是否有优先级更高的拒绝规则/全端口规则；
- 规则是入方向而不是出方向。

### 恢复

- 补一条 TCP 7000 入方向允许规则（来源按安全组文档）；
- 修正宿主防火墙（如果启用）：
```bash
sudo ufw allow 7000/tcp      # 仅在确实使用 ufw 时
```
- 不要为了排障临时开放 7500/80/443 或全端口；这会把「只打洞」变成「可承载流量」。

### 判据

- 从两个不同外部网络 `nc` 都成功；
- 服务器抓包能看到握手/控制流量；
- 外部扫描仍只有 7000 可达。

## 3. token 不匹配

### 表现

- frpc 侧登录失败、反复重试；
- frps 日志出现类似：

```text
token in login doesn't match token from configuration
token in heartbeat doesn't match token from configuration
token in NewWorkConn doesn't match token from configuration
```

（源码 v0.71.1 `pkg/auth/token.go` 中的错误文本；实际日志措辞**未验证**。）

### 判定

```bash
# 服务端日志
sudo journalctl -u frps --since '10 minutes ago' --no-pager | grep -iE 'token|login|auth' | tail -n 50
```

- 只有新客户端失败、旧客户端正常：新客户端用了旧/错 token，或没重新读取 Secret；
- 所有客户端失败：服务端 token 刚被改过/文件不可读/权限错；
- 服务端日志完全无登录记录：先确认端口可达（第 2 节）。

### 恢复

1. 只比对指纹，不打印 token：
```bash
sudo awk 'BEGIN{ORS=""} {print}' /etc/frp/frps.token | sha256sum | awk '{print $1}'
# 客户端用同样方法算指纹，两端必须一致
```
2. 修正不一致的一端（优先改客户端 Secret/本地文件，不要轻易改服务端）；
3. 如果服务端 token 文件被改过，按 credentials.md 的轮换/回滚步骤处理；
4. `sudo systemctl restart frps`（tokenSource 不支持热更新）；
5. 用一次真实会话验证登录与打洞。

### 注意

- 不要把 token 贴进聊天、工单、日志；
- 如果怀疑 token 泄漏，直接按泄漏处置（credentials.md 第 6 节），不要只修「不匹配」。

## 4. TLS 握手失败

### 表现

- frpc 日志有 TLS/handshake/certificate 相关错误；
- 服务端日志有 tls 相关错误；
- 客户端版本低于 v0.50，或显式设置了 `transport.tls.enable = false`。

### 判定

```bash
openssl s_client -connect 47.109.32.249:7000 -brief < /dev/null
```

- 能完成 TLS 握手 => 服务端 TLS 正常，问题在客户端配置/版本；
- 握手失败 => 先确认 7000 可达、frps 正常监听。

### 恢复

- 升级 frpc 到与服务端匹配的 v0.71.1（至少 >= v0.50）；
- 确认 frpc 没有关闭 TLS；
- **不要**为了让旧客户端连上而把 `transport.tls.force` 改回 false；这会退回到明文/可降级状态。

## 5. UDP 被封 / 打洞失败

### 表现

- frpc 登录成功（TCP 7000 正常），但 XTCP 打洞失败；
- 本地 visitor 的 `bindPort` 可监听但没有可用连接；
- frpc 日志反复 punch/detect 超时；
- runner/本地两侧 NAT 类型不兼容（如双方均对称 NAT）。

### 判定

1. 先确认不是服务器问题：
```bash
# 服务器仍只有 TCP 7000，无 UDP
sudo ss -lunp | grep frps || echo 'OK: no frps UDP listener'
```
2. 在客户端侧测试出站 UDP/STUN：
```bash
# 具体 STUN 服务器由 frpc 配置 natHoleStunServer 决定（默认 stun.easyvoip.com:3478）
# 用客户端系统工具验证 DNS/UDP 可达；不要把 TCP 443 可用推断为 UDP 可用
```
3. 看 frpc 日志中的 punch 结果（关键词 `xtcp`、`punch`、`hole`、`p2p`，措辞未验证）。
4. 若两端都在对称 NAT 下，XTCP 本身可能不可用——这不是服务器故障。

### 恢复（不允许中继）

- 换网络（如 runner 换出口、用户换网络）；
- 改用自托管 runner/本地容器等设计文档 9.3 的降级路径；
- 如果用户明确启用 Tailscale 直连，按 Tailscale 路径单独诊断；
- **禁止**通过 stcp/fallbackTo/frps 数据转发/DERP/任何中继来「修好」；
  这类改动违反项目硬约束。

### 判据

- XTCP 成功，且 D1-D4 证明数据不走服务器；
- 或者明确判定当前网络不可打洞，记录 `E_P2P_XTCP`/`E_UDP_BLOCKED`（具体错误码以契约文件为准），
  转降级路径；不能写成「已通过」。

## 6. 磁盘/日志异常

### 判定

```bash
df -h /
du -sh /var/log/journal 2>/dev/null
sudo journalctl --disk-usage
sudo journalctl -u frps --since '1 hour ago' --no-pager | wc -l
```

- journald 用量 > 256M：drop-in 未生效或 journald 未重启；
- `/` 使用率 > 90%：立即清理，不要继续压测；
- 日志中出现大量重复错误：先限流/修根因，不要只删日志。

### 恢复

```bash
# 确认 drop-in 已安装
sudo systemctl cat systemd-journald | grep -A20 '99-frps-cap'
# 重新加载并重启 journald
sudo systemctl restart systemd-journald
# 手动清理到上限（会删除最旧日志）
sudo journalctl --vacuum-size=256M
```

如果 frp 被改成文件日志模式，必须同时有文件级大小上限；不要只依赖 `maxDays`。

## 7. 配置错误 / 启动限流

```bash
sudo /usr/local/bin/frps verify -c /etc/frp/frps.toml
sudo systemctl reset-failed frps   # 仅在根因已修复后
sudo systemctl restart frps
```

保留一份「上一版可用配置」：

```bash
sudo install -m 0644 /etc/frp/frps.toml /root/frps.toml.bak-$(date +%F-%H%M)
```

回滚时先 `verify`，再重启；不要跳过验证直接上线。

## 8. OOM / 内存压力

节点只有 2 GiB。排查：

```bash
free -m
sudo dmesg | grep -iE 'oom|killed process' | tail -n 20
sudo systemctl status frps --no-pager | grep -i memory
```

- frps 本身内存占用通常很小；如果持续增长，检查是否有异常代理连接/日志洪峰；
- 不要用 `MemoryMax` 随意限制到很低，可能导致 frps 被 OOM 杀掉；
- 必要时增加 swap 或升配，并把结论记录为「容量不足」，而不是「服务正常」。

## 9. 安全事故处置（疑似被当转发节点）

触发条件：

- 服务端出现额外 TCP/UDP 监听；
- D1/D2 字节计数与 payload 成比例；
- 日志出现非 xtcp 代理；
- token 泄漏证据。

处置顺序：

1. 停止 frps：`sudo systemctl stop frps`（保留日志与抓包）；
2. 关闭安全组所有非 7000 规则，确认外部不可达；
3. 轮换 token（credentials.md）；
4. 导出证据：`journalctl -u frps`、`ss` 输出、计数、pcap；
5. 检查是否有其他进程/定时任务；
6. 复盘并更新本文档/验证清单；
7. 未查清前不要重新开放 7000。

## 10. 恢复后的强制验收

任何故障恢复后，至少重跑：

- A 配置审计；
- B 监听面；
- C 外部端口；
- D1/D4 无回流核心判据；
- F 日志/磁盘。

只看到 `systemctl active` 不等于恢复；只看到「日志正常」不等于没有回流。
