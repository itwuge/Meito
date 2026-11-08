# 验证清单：证明「只做打洞、会话流量没有回落」

> 本文件是**判据与操作步骤**，不是验收结果。
> 除特别标注「源码/文档已核对」外，所有运行期结论均为**未验证**；
> 只有在真机执行并留下时间、命令、输出、判定人后，才能写「通过」。

## 0. 证据纪律

每次验证建立一份记录（建议放在服务器之外的受控位置，不要提交真实 IP/会话数据到公开仓库）：

| 字段 | 要求 |
|---|---|
| 时间 | UTC 时间戳，精确到分钟 |
| 版本 | `frps -v`、`/etc/os-release`、`uname -r` |
| 命令 | 原样记录，可直接复现 |
| 输出 | 关键行/数值，不要只写「正常」 |
| 判定 | 通过 / 失败 / 未验证；失败必须写异常表现 |
| 证据 | 日志片段、计数前后值、pcap 大小、截图路径 |

判定原则：

- 不能只用 `systemctl active` 作为「节点正常」的证据；
- 不能只用应用日志作为「没有回流」的证据；
- 任何一项失败都先停止会话、保留证据，再改动配置；
- 未跑通的项写「未验证」，不得写成「通过」。

## A. 静态配置审计

```bash
# 1) 二进制版本
/usr/local/bin/frps -v
# 预期：0.71.1

# 2) 配置语法与字段校验
sudo /usr/local/bin/frps verify -c /etc/frp/frps.toml
# 预期：无错误退出（退出码 0）

# 3) 禁止能力字段审计（排除注释行，避免把说明文字误判为配置）
grep -vE '^[[:space:]]*#' /etc/frp/frps.toml \
  | grep -nE 'vhostHTTPPort|vhostHTTPSPort|tcpmuxHTTPConnectPort|kcpBindPort|quicBindPort|sshTunnelGateway|webServer|allowPorts|bind_udp_port' \
  || echo 'OK: no forbidden capability fields'

# 4) 确认没有内联 token
grep -vE '^[[:space:]]*#' /etc/frp/frps.toml \
  | grep -nE 'auth\.token[[:space:]]*=' || echo 'OK: token not inline'

# 5) 确认 tokenSource 指向服务端文件
grep -vE '^[[:space:]]*#' /etc/frp/frps.toml | grep -nE 'tokenSource|tls\.force'
```

预期：

- `frps verify` 通过；
- 第 3 项输出 `OK`（没有 vhost/HTTP/HTTPS/tcpmux/KCP/QUIC/SSH 网关/仪表盘/allowPorts/旧版 UDP 字段）；
- 第 4 项输出 `OK`；
- 第 5 项能看到 `tokenSource.type = "file"`、`tokenSource.file.path = "/etc/frp/frps.token"`、`tls.force = true`。

异常：出现任何被禁止字段，说明配置被改过，先回滚再继续。

**源码/文档已核对（非真机验证）**：frp v0.71.1 的 `ServerConfig` 没有 UDP 协调端口字段；
`server/service.go` 只在 `KCPBindPort > 0` 或 `QUICBindPort > 0` 时监听 UDP。
本配置两者都未设置，因此预期没有 frps UDP 监听。仍需 B 项真机确认。

## B. 运行期监听面

```bash
sudo systemctl status frps --no-pager
sudo ss -lntp | grep -E 'frps|:7000'
sudo ss -lunp | grep frps || echo 'OK: no frps UDP listener'
sudo ss -lxnp | grep frps || echo 'OK: no frps unix socket'
```

预期：

- 服务 `active (running)`；
- TCP 只有 `0.0.0.0:7000`（或 `[::]:7000`）由 frps 监听；
- **没有任何 frps 的 UDP socket**；
- 没有 frps 的 UNIX socket。

异常表现与判定：

| 异常 | 含义 | 判定 |
|---|---|---|
| 出现 7500 监听 | 仪表盘被启用 | 失败，立即停止并检查配置 |
| 出现 80/443 监听 | vhost 被启用 | 失败，违反「不承载流量」 |
| 出现额外 6000-6999 TCP/UDP 监听 | 有 tcp/udp 代理注册 | 失败，会话流量可能经服务器转发 |
| 出现 frps UDP 监听 | KCP/QUIC 被启用或版本行为不同 | 失败/未验证，先排查再放行 |
| 7000 未监听 | frps 未启动/端口冲突 | 失败，按 failure-recovery.md 处理 |

## C. 外部端口面

在**另一台非本机**（家宽/手机热点各一次，避免本地网络策略干扰）执行：

```bash
nc -vz 47.109.32.249 7000
nc -vz -w 3 47.109.32.249 7500
nc -vz -w 3 47.109.32.249 80
nc -vz -w 3 47.109.32.249 443
nc -vz -w 3 47.109.32.249 7001
# 如安装了 nmap
nmap -Pn -p 22,80,443,7000,7001,7500 47.109.32.249
```

预期：

- `7000` 为 open/succeeded；
- `7500`、`80`、`443`、`7001` 为 closed 或 filtered（超时/拒绝都算正常）；
- 22 只有在管理员来源 IP 下才 open，否则 closed/filtered。

异常：7500/80/443 可从公网访问，立即按安全事故处理：
停止 frps、检查配置、关闭安全组规则、轮换 token。

## D. 无回流证明（核心）

### D1. 服务器整机字节计数（主判据）

在服务器上：

```bash
# 取默认出口网卡名（ECS 通常是 eth0）
IFACE=$(ip -o route get 1.1.1.1 | awk '{for(i=1;i<=NF;i++) if($i=="dev"){print $(i+1); exit}}')
echo "IFACE=$IFACE"

# 读取当前 rx/tx 字节（读两次，间隔 60 秒，得到空闲基线）
awk -v i="$IFACE:" '$1==i {print strftime("%FT%TZ"), "rx="$2, "tx="$10}' /proc/net/dev
sleep 60
awk -v i="$IFACE:" '$1==i {print strftime("%FT%TZ"), "rx="$2, "tx="$10}' /proc/net/dev
```

然后执行 **3 次 100 MB 传输**（从本地 visitor 经 SSH 走 XTCP 到 runner，
在 runner 端把数据丢弃，避免写盘）：

```bash
# 在本地机器执行；<LOCAL_BIND_PORT> 是 xtcp visitor 的 bindPort
for i in 1 2 3; do
  date -u +%FT%TZ
  dd if=/dev/zero bs=1M count=100 2>/dev/null \
    | ssh -o Compression=no -p <LOCAL_BIND_PORT> <RUNNER_USER>@127.0.0.1 'cat > /dev/null'
done
date -u +%FT%TZ
```

传输结束后立刻再读一次服务器计数：

```bash
awk -v i="$IFACE:" '$1==i {print strftime("%FT%TZ"), "rx="$2, "tx="$10}' /proc/net/dev
```

判据：

- 3 × 100 MB = 300 MB；
- 服务器网卡 `rx` 与 `tx` 的增量都应 **< 15 MB（5%）**；
- 空闲 60 秒基线应 < 1 MB（心跳/日志量级）。

异常：

- 增量接近 300 MB 或与传输量成比例 → 会话数据经服务器转发，判定失败；
- 增量在 15 MB 附近 → 不能判定通过，需用 D2/D3 进一步区分重传/控制流量。

注意：整机计数包含 SSH 运维、DNS、系统更新等杂项流量，所以要先测基线，
并在传输窗口内不要同时做 apt 更新、日志下载等操作。

### D2. 控制端口计数（可选，需临时规则）

如果本机使用 iptables（含 iptables-nft 兼容层），可对 TCP 7000 单独计数。
**该规则会插到 INPUT 链首位，只在维护窗口使用，测完立即删除；它会绕过更早的限流规则。**

```bash
# 插入计数/放行规则（7000 本来就必须放行，所以不会新增可达性）
sudo iptables -I INPUT 1 -p tcp --dport 7000 -m comment --comment meito-frps-count -j ACCEPT

# 读计数（pkts/bytes 两列）
sudo iptables -L INPUT 1 -v -x -n

# 执行 D1 的 3 × 100 MB 传输后再次读取
sudo iptables -L INPUT 1 -v -x -n

# 删除临时规则（优先按完整匹配，避免规则顺序变化后误删）
sudo iptables -D INPUT -p tcp --dport 7000 -m comment --comment meito-frps-count -j ACCEPT
```

判据：TCP 7000 的累计 bytes 增量 < 15 MB（同样按 300 MB 的 5% 计）。
如果接近 300 MB，说明数据走了控制端口 → 回流/中继，判定失败。

如果本机只有 nftables 且没有可安全插入的链，跳过 D2，以 D1 + D3 + D4 为准。

### D3. 服务器抓包（辅助判据）

在服务器上，传输开始前启动：

```bash
sudo timeout 120 tcpdump -i any -nn -s 0 'tcp port 7000' -w /tmp/meito-frps-7000.pcap
ls -lh /tmp/meito-frps-7000.pcap
```

预期：

- pcap 大小只有 MB 级以下（控制/心跳/协调包）；
- 若出现与 100 MB 传输成比例的持续大包流（pcap 达数十 MB），判定失败。

同时观察 UDP（把 `<PEER_IP>` 换成实际 runner/本地公网 IP）：

```bash
sudo timeout 120 tcpdump -i any -nn 'udp and host <PEER_IP>'
```

预期：没有持续的 peer 数据包；只允许偶发 DNS/NTP 等系统流量。
若 peer 的 UDP 数据持续发往本机，说明打洞路径异常（例如误用了服务器侧 UDP），
必须停下来查明。

### D4. 两端 socket 证据（最直接）

在本地 visitor 机器与 runner 上各执行：

```bash
# 查看 frpc 的 TCP socket、对端地址与字节数
sudo ss -tinp | grep -A2 frpc
```

预期：

- 到 `47.109.32.249:7000` 的控制连接存在，但 `bytes_sent/bytes_received` 很小（心跳/协调量级）；
- 大数据量（约 100 MB）的 socket 对端是 **peer 的公网 IP:端口**，不是 `47.109.32.249:7000`；
- 如果大数据量出现在到 `47.109.32.249:7000` 的 socket 上 → 数据经服务器，判定失败。

frpc 日志中应有打洞成功的记录（具体措辞随版本变化，**未验证**）：

```bash
journalctl -u frpc --since '10 minutes ago' --no-pager | grep -iE 'xtcp|punch|hole|p2p' | tail -n 50
# 如果 frpc 不是 systemd 服务（由 meito-runner 启动），改看 meito-runner 的日志输出/日志文件
```

预期：能看到 XTCP/P2P 成功；如果一直是 punch failed 且流量回落到 7000，判定失败。

### D5. 带宽上限交叉验证（辅助）

服务器固定带宽 3 Mbps ≈ 0.375 MB/s。单次 100 MB 经服务器至少需要约 267 秒（理论值）。
如果一次 100 MB 传输明显快于该时间（例如几十秒），它不可能全程经过服务器；
这可以作为「没有回流」的旁证，但不能单独作为通过依据（可能存在突发带宽/统计误差）。

### D6. 代理类型审计（服务端日志）

```bash
journalctl -u frps --since '1 hour ago' --no-pager | grep -iE 'proxy|xtcp|stcp|tcp|udp' | tail -n 100
```

预期：

- 只看到 `xtcp` 类型 / `ssh-<sid>` 之类的代理注册；
- 不出现 `stcp`、`tcp`、`udp` 代理；
- 若出现非 xtcp 代理，立即停止 frps、保留日志、按安全事故处理。

> 注意：frps 本身没有「按代理类型白名单」配置项；如果 token 被持有者滥用，
> 它可以在协议层注册 stcp/tcp。服务端做不到类型级强制（除非自研 NewProxy 插件）。
> 因此本项审计 + 安全组关闭数据端口 + token 保密是组合防线，不能只依赖配置。

### D7. 判定汇总

| 检查 | 正常预期 | 异常表现 | 判定 |
|---|---|---|---|
| A 配置审计 | 无禁止字段；verify 通过 | 出现 vhost/KCP/QUIC/仪表盘字段 | 失败 |
| B 监听面 | 仅 TCP 7000；无 UDP | 额外监听/UDP | 失败 |
| C 外部端口 | 仅 7000 可达 | 7500/80/443 可达 | 失败/安全事故 |
| D1 整机字节 | 300 MB 传输后增量 < 15 MB | 接近 300 MB | 失败 |
| D2 端口字节 | 7000 增量 < 15 MB | 接近 300 MB | 失败 |
| D3 抓包 | pcap 很小；无持续 UDP peer 流量 | pcap 与 payload 成比例 | 失败 |
| D4 socket | 大数据 socket 对端是 peer | 大数据 socket 对端是服务器 7000 | 失败 |
| D5 带宽 | 传输速度不受 3 Mbps 上限约束 | 速度恰好压在 3 Mbps 附近 | 存疑，需 D1-D4 |
| D6 类型审计 | 仅 xtcp | 出现 stcp/tcp/udp | 失败/安全事故 |

## E. 鉴权与 TLS 负向验证

TLS 握手（只验证加密通道存在，不发送 token）：

```bash
timeout 10 openssl s_client -connect 47.109.32.249:7000 -brief < /dev/null
```

预期：TLS 握手成功；因使用自动生成的自签名证书，可能出现证书校验提示，这是预期现象，
但不能因此关闭 `tls.force`。

错误 token 测试（在受控客户端、维护窗口执行，不要用生产 token）：

```bash
# 写一个临时 frpc 配置，auth.token 填一个明显错误的值，指向本节点 7000
# 不要把这个错误值写进仓库或日志正文
timeout 10s frpc -c /tmp/frpc-wrong-token.toml
# 预期：登录失败，frps 日志出现 token 不匹配；不会影响已登录的正常客户端
```

服务端日志预期（措辞随版本变化）：

```bash
journalctl -u frps --since '5 minutes ago' --no-pager | grep -iE 'token|login|auth' | tail -n 30
```

预期能看到 token 不匹配/登录失败；如果错误 token 竟然登录成功，立即停止并排查。

## F. 日志与磁盘

```bash
df -h /
journalctl --disk-usage
# 预期：journald 用量 <= 256M；磁盘剩余空间充足

journalctl -u frps --since '1 hour ago' --no-pager | tail -n 50
# 预期：有启动、监听、登录/心跳等正常记录；无 panic/OOM/磁盘满

# token 是否出现在日志：只输出命中数量，不输出正文，避免二次泄漏
sudo sh -c 'journalctl -u frps --no-pager | grep -F -f /etc/frp/frps.token -c || true'
# 预期：0
```

异常：

- 命中数 > 0：立即轮换 token（按 credentials.md），并检查日志访问权限；
- `df` 剩余 < 5 GiB：清理/扩容，暂不要继续压测；
- journald 用量 > 256M：检查 drop-in 是否被加载、journald 是否重启过。

## G. 服务自愈与恢复演练（维护窗口）

```bash
# 1) 手动停止：外部应无法连接 7000
sudo systemctl stop frps
nc -vz -w 3 47.109.32.249 7000   # 预期失败

# 2) 启动：应恢复
sudo systemctl start frps
nc -vz -w 3 47.109.32.249 7000   # 预期成功

# 3) 强杀验证 Restart=always：应在约 5 秒后自动恢复
sudo systemctl kill -s SIGKILL frps
sleep 8
sudo systemctl is-active frps     # 预期 active
journalctl -u frps -n 20 --no-pager
```

注意：这些操作会中断正在进行的会话。只在维护窗口做，并提前通知。

## H. 记录模板

```text
日期/时间(UTC):
节点版本: frps v0.71.1 / Ubuntu 20.04 / kernel:
执行人:
A 配置审计: 命令: ... 输出摘要: ... 判定:
B 监听面: 命令: ... 输出摘要: ... 判定:
C 外部端口: 来源网络: ... 结果: ... 判定:
D1 整机计数: before rx/tx=..., after rx/tx=..., 增量=... 判定:
D2 7000 计数: pkts/bytes before..., after... 判定:
D3 pcap: 文件大小=... 判定:
D4 socket: 大数据对端=... 判定:
D5 传输耗时: ... 判定:
D6 类型审计: 结果=... 判定:
E TLS/错误 token: 结果=... 判定:
F 日志/磁盘: journald=..., df=..., token命中=... 判定:
G 自愈: stop/start/kill 结果=... 判定:
总体判定: 通过 / 失败 / 未验证
遗留风险:
```
