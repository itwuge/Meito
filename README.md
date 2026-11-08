# Meito

Meito 是一个在 GitHub Actions 托管 runner 上获得**点对点交互会话**的自研工具，用来替代已失效的 tmate，在 runner 上跑 `make menuconfig` 等必须人工操作的配置工作，并把 `.config` / `defconfig` 带回本地仓库。

当前状态：**设计阶段（M0）**，尚未进入实现。runner 内行为、UDP 可达性、WebRTC 可达性均未实测，任何“已通过”都必须有 run_id、日志、artifact 或服务器计数证据。

## 文档入口

| 文档 | 用途 |
|---|---|
| [docs/design.md](docs/design.md) | 唯一设计规格：目标、判据、选型、组件、生命周期、安全、里程碑、风险 |
| [docs/工程规范索引.md](docs/工程规范索引.md) | 按编号引用外部工程经验库，不复制正文 |
| [docs/开工清单.md](docs/开工清单.md) | 本项目分阶段开工/收工核对 |
| [docs/分支归属.md](docs/分支归属.md) | 工作树与分支归属（R-T-11） |
| [docs/变更记录.md](docs/变更记录.md) | 变更、理由、判据、证据与未验证项 |
| [docs/门禁豁免清单.md](docs/门禁豁免清单.md) | 仅记录环境豁免，不放宽判据 |

## 快速开始（设计阶段）

当前没有可运行二进制；以下命令在 M1 之后才可用，先写在这里作为接口承诺：

```powershell
# 只读体检
meito doctor --repo owner/repo --deep

# 本地版：启动会话并进入终端入口
meito up --ref main --max 60m --idle 10m --entry terminal

# 网页版：打开托管单页
meito web --static https://meito.example.com --open

# 会话结束后把产物带回仓库
meito pull --apply --dest .
```

## 门禁

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -SelfTest
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\quality-gate.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\arch-check.ps1
```

- `gate.ps1` 来自 `C:\Users\Administrator\Desktop\工程经验库\门禁模板\`，活动脚本在 `scripts/`，原件保留在 `tools/gate-template/`。
- `quality-gate.ps1` 串接经验库门禁、`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`arch-check.ps1`。M0 尚无 Rust workspace 时，Rust 项明确标记“未启用”，不伪造通过。
- CI 见 `.github/workflows/gate.yml`，不使用 `|| true`。

## 分支与提交

- 一个工作树只归一条线使用；提交前用 `git branch --show-current` 核对 [docs/分支归属.md](docs/分支归属.md)（R-T-11）。
- 一事一提交；每阶段一个 commit 作为回滚点（R-T-9、R-D-10）。
- 不自行合并主干；合并由指挥统一安排（R-T-7）。
- 源码变更必须递增版本；文档-only 变更可不递增，但要在变更记录说明（R-D-9）。

## 硬约束

1. 用户公网小服务器**只允许打洞/协调，不允许承载会话字节**；所有中继方案排除。
2. P2P 主路径 frp `xtcp`，备路径 Headscale/Tailscale 直连；禁止 `fallbackTo`/`stcp`/DERP/peer relay。
3. 网页版只能用 WebRTC DataChannel + STUN，**禁用 TURN**；连不上是常态，必须可解释并转本地版。
4. 托管 runner 单 job 最长 6 小时，到点直接 fail；交互 job 与生产凭据隔离。
5. GitHub 日志脱敏不保证；交互 shell 可读取环境变量；调试 job 不注入生产凭据。
