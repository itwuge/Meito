# 凭据管理：frps `auth.token`

## 1. 目标与边界

- token 是 frpc/frps 之间**唯一**的登录凭据；拿到 token 的人可以注册代理。
- token 必须满足：
  - 至少 **256 位随机熵**；
  - 只存放在服务器 `/etc/frp/frps.token`（服务端权威副本）和受控客户端；
  - **不得进入本仓库、提交历史、命令行参数、systemd `Environment=`、日志输出**。
- frps 配置使用 `auth.tokenSource` 从文件读取（frp v0.64.0+，本节点锁定 v0.71.1）。
  `tokenSource` 在配置加载时读取一次，**不支持运行期热更新**，轮换必须重启 frps。
- frp 不会把 token 原文放在登录报文里：它用 token + 时间戳派生 `PrivilegeKey`
  （见 `pkg/auth/token.go`）。这不代表可以省掉 TLS；`transport.tls.force=true` 仍然必须保留。

## 2. 生成

在服务器上以 root 执行：

```bash
# 1) 创建配置目录（root 所有，frps 组可读执行）
sudo install -d -m 0750 -o root -g frps /etc/frp

# 2) 生成 32 字节 = 256 位随机 token，十六进制表示 64 个字符
#    umask 077 保证文件创建时先为 0600；命令本身不会把 token 打到终端
sudo sh -c 'umask 077; openssl rand -hex 32 > /etc/frp/frps.token'

# 3) 让运行用户 frps 可读，其他用户不可读
sudo chown root:frps /etc/frp/frps.token
sudo chmod 0640 /etc/frp/frps.token

# 4) 校验权限与可读性
sudo stat -c '%U %G %a %n' /etc/frp/frps.token
# 预期：root frps 640 /etc/frp/frps.token
sudo -u frps test -r /etc/frp/frps.token && echo 'OK: frps can read'
sudo -u nobody test -r /etc/frp/frps.token && echo 'BAD: others can read' || echo 'OK: others cannot read'
```

长度说明：

- 32 字节 = 256 位熵，暴力猜测不可行；64 个十六进制字符便于人工核对。
- 不要使用 `openssl rand -base64 32` 后手动去掉字符；也不要使用密码短语、UUID、时间戳。
- 不要把 token 打印到终端或复制到聊天窗口。需要核对时只比对指纹。

只输出指纹、不输出 token 的命令：

```bash
sudo awk 'BEGIN{ORS=""} {print}' /etc/frp/frps.token | sha256sum | awk '{print $1}'
```

服务端配置中的读取方式（已写入 `frps.toml`）：

```toml
[auth]
method = "token"
additionalScopes = ["HeartBeats", "NewWorkConns"]
tokenSource.type = "file"
tokenSource.file.path = "/etc/frp/frps.token"
```

## 3. 客户端如何使用同一个 token

token 必须同时存在于 frpc 侧，否则无法登录。客户端侧要求：

- **GitHub Actions**：把 token 存为仓库/环境的加密 Secret `MEITO_FRPS_TOKEN`；
  在 job 内写入 `$RUNNER_TEMP/frps.token`（`umask 077`，权限 0600），frpc 配置用
  `auth.tokenSource.type="file"` 指向它；在 `if: always()` 清理步骤中删除。
- 不要在 job 里 `echo` token，不要把 token 拼进 frpc 命令行，不要把 token 写进 artifact。
- 交互式 shell 能读到 job 环境变量；因此**调试 job 不注入生产 token**，
  生产 job 与调试 job 隔离（与设计文档的安全约束一致）。
- 本地 CLI：存于操作系统密钥环或 0600 权限的本地文件，不进入仓库。
- `meito-runner` 是否已支持 `tokenSource` 尚未接入/验证；本交付物只定义服务端与
  配置要求，客户端实现列入后续集成项。

## 4. 仓库防泄漏

本目录已包含 `.gitignore`，忽略 `secrets/`、`*.token`、`*.token.new`、`*.token.bak*`。
提交前额外检查：

```bash
# 确认没有明文 token 字段（注释行除外）
grep -vE '^[[:space:]]*#' deploy/frp/frps.toml | grep -nE 'auth\.token[[:space:]]*=' || echo 'OK: no inline auth.token'

# 确认工作区没有 token 文件被跟踪
git ls-files | grep -E '\.token($|\.)|secrets/' || echo 'OK: no token files tracked'

# 确认历史中没有意外的 token 字样（只查字段赋值，不打印内容）
git log -S'auth.token =' --oneline -- deploy/frp || true
```

如果 GitHub 开启 push protection/secret scanning，保持开启；不要用 `git add -f` 绕过忽略规则。

## 5. 轮换步骤（有短暂中断）

frp 只支持一个 token，没有双 token 并行期。轮换会**短暂中断所有 frpc 登录**，必须安排维护窗口。

前置：

- 记录当前 token 指纹（不要记录内容）；
- 确认所有客户端都能在同一窗口更新 token；
- 停止发起新会话；保留当前 frps 运行状态日志作为回滚证据。

步骤：

1. 生成新 token 到临时文件（不要直接覆盖旧文件）：

```bash
sudo sh -c 'umask 077; openssl rand -hex 32 > /etc/frp/frps.token.new'
sudo chown root:frps /etc/frp/frps.token.new
sudo chmod 0640 /etc/frp/frps.token.new
```

2. 备份旧 token（仅 root 可读，24 小时后删除）：

```bash
sudo install -m 0600 -o root -g root /etc/frp/frps.token /root/frps.token.bak-$(date +%F-%H%M)
```

3. 原子替换并重启：

```bash
sudo mv /etc/frp/frps.token.new /etc/frp/frps.token
sudo systemctl restart frps
sudo systemctl is-active frps
journalctl -u frps -n 30 --no-pager
```

4. 更新所有客户端（GitHub Secret `MEITO_FRPS_TOKEN`、本地配置/密钥环），
   然后做一次真实会话验证（能登录、能打洞、能传输）。

5. 观察至少 24 小时，确认没有旧 token 的登录尝试和异常代理注册后：

```bash
sudo shred -u /root/frps.token.bak-*
# 在 GitHub 侧覆盖旧 Secret，确认旧值已不可用
```

回滚：

```bash
# 仅在切换到新 token 后出现不可用、且客户端尚未全部更新时使用
sudo install -m 0640 -o root -g frps /root/frps.token.bak-<时间> /etc/frp/frps.token
sudo systemctl restart frps
```

回滚后必须把客户端 Secret 一起回滚，否则仍然无法登录。

## 6. token 泄漏/疑似泄漏的处置

1. **立即轮换**（按第 5 节），不要等观察期。
2. 检查 frps 日志中的登录失败与代理注册记录：

```bash
journalctl -u frps --since '24 hours ago' --no-pager | grep -E 'token in login|register control|proxy' | tail -n 100
```

3. 对照服务端字节计数与监听面（见 `verification.md`）：如果出现额外的 tcp/udp 监听端口
   或异常流量，按「节点可能被当作转发节点」处理，立即停止 frps 并保留证据。
4. 轮换 GitHub Secret，审计 Actions 运行记录与 artifact；调试 job 的权限立即收回。
5. 如确认来自固定来源 IP 的滥用，可在安全组临时封禁该 IP；不要长期依赖 IP 封禁替代 token 轮换。

## 7. 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| frps 启动报读取 token 文件失败 | 文件不存在/路径错/权限不足 | `stat` + `sudo -u frps test -r`；修正 root:frps 0640 |
| 客户端报 token 不匹配 | 两侧 token 不一致或客户端仍在用旧值 | 比对指纹；统一更新后重启 frps 与 frpc |
| 改了 token 文件但没生效 | tokenSource 只在配置加载时读取 | `systemctl restart frps` |
| 日志里出现 token 原文 | 客户端把 token 写进命令行/日志 | 立即轮换，清理日志访问权限，修正客户端写法 |
