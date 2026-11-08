//! File: meito-core 公共契约（M1 冻结）。
//!
//! 本 crate 只定义两个外壳共用的 Model、Controller、P2P、入口、artifact 与日志接口。
//! 冻结来源见仓库根目录 `contracts/`；禁止在 CLI 或网页外壳复制状态机与策略。

// ===== 契约常量 =====

/// 当前冻结的契约版本，必须与 `contracts/contract-version.json` 一致。
pub const CONTRACT_VERSION: &str = "0.1.0";

/// 会话日志路径前缀，完整路径为 `logs/session-<id>.jsonl`。
pub const SESSION_LOG_PATH_PREFIX: &str = "logs/session-";

// ===== 会话状态机 =====

/// Model 的唯一会话状态枚举，两个外壳不得各自定义副本。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionState {
    #[default]
    Idle,
    Preflight,
    Dispatching,
    Queued,
    RunnerStarting,
    P2pConnecting,
    LinkReady,
    WaitingForJoin,
    EntryStarting,
    Connected,
    Reconnecting,
    Ending,
    Collecting,
    Done,
    Cancelled,
    FailedPreflight,
    FailedDispatch,
    FailedQueueTimeout,
    FailedBootstrap,
    FailedP2p,
    FailedLink,
    FailedEntry,
    FailedCollect,
    NoDirectPath,
    Orphaned,
}

impl SessionState {
    /// 返回与契约 JSON 一致的稳定状态名。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preflight => "preflight",
            Self::Dispatching => "dispatching",
            Self::Queued => "queued",
            Self::RunnerStarting => "runner_starting",
            Self::P2pConnecting => "p2p_connecting",
            Self::LinkReady => "link_ready",
            Self::WaitingForJoin => "waiting_for_join",
            Self::EntryStarting => "entry_starting",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Ending => "ending",
            Self::Collecting => "collecting",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::FailedPreflight => "failed_preflight",
            Self::FailedDispatch => "failed_dispatch",
            Self::FailedQueueTimeout => "failed_queue_timeout",
            Self::FailedBootstrap => "failed_bootstrap",
            Self::FailedP2p => "failed_p2p",
            Self::FailedLink => "failed_link",
            Self::FailedEntry => "failed_entry",
            Self::FailedCollect => "failed_collect",
            Self::NoDirectPath => "no_direct_path",
            Self::Orphaned => "orphaned",
        }
    }
}

/// 日志阶段与状态机共用同一枚举，避免两侧漂移。
pub type Stage = SessionState;

/// 状态机事件，转移表见 `contracts/state-machine.json`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    Start,
    PreflightOk,
    PreflightFailed,
    DispatchOk,
    DispatchFailed,
    QueueTimeout,
    RunnerReady,
    RunnerFailed,
    P2pOk,
    P2pFailed,
    NoDirectPath,
    LinkReady,
    LinkFailed,
    Join,
    JoinTimeout,
    EntryReady,
    EntryFailed,
    ConnectOk,
    Disconnect,
    ReconnectOk,
    ReconnectFailed,
    Stop,
    ForceStop,
    CollectOk,
    CollectFailed,
    Checkpoint,
    OrphanDetect,
}

// ===== 日志与错误 =====

/// 日志级别；对外只暴露 info/debug/trace 三级开关的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
    Trace,
}

/// 冻结错误码，字符串与 `contracts/error-codes.json` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Internal,
    Usage,
    Auth,
    ActorNotAllowed,
    Preflight,
    Bootstrap,
    Dispatch,
    QueueTimeout,
    Link,
    UdpBlocked,
    P2pXtcp,
    P2pTs,
    HostKey,
    Entry,
    Artifact,
    ApplyConflict,
    JobTimeout,
    NoJoinTimeout,
    Cancel,
    WebNoDirectPath,
    WebRelayUsed,
    LogsUnavailable,
    Orphan,
}

impl ErrorCode {
    /// 返回稳定错误码字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Internal => "E_INTERNAL",
            Self::Usage => "E_USAGE",
            Self::Auth => "E_AUTH",
            Self::ActorNotAllowed => "E_ACTOR_NOT_ALLOWED",
            Self::Preflight => "E_PREFLIGHT",
            Self::Bootstrap => "E_BOOTSTRAP",
            Self::Dispatch => "E_DISPATCH",
            Self::QueueTimeout => "E_QUEUE_TIMEOUT",
            Self::Link => "E_LINK",
            Self::UdpBlocked => "E_UDP_BLOCKED",
            Self::P2pXtcp => "E_P2P_XTCP",
            Self::P2pTs => "E_P2P_TS",
            Self::HostKey => "E_HOSTKEY",
            Self::Entry => "E_ENTRY",
            Self::Artifact => "E_ARTIFACT",
            Self::ApplyConflict => "E_APPLY_CONFLICT",
            Self::JobTimeout => "E_JOB_TIMEOUT",
            Self::NoJoinTimeout => "E_NO_JOIN_TIMEOUT",
            Self::Cancel => "E_CANCEL",
            Self::WebNoDirectPath => "E_WEB_NO_DIRECT_PATH",
            Self::WebRelayUsed => "E_WEB_RELAY_USED",
            Self::LogsUnavailable => "E_LOGS_UNAVAILABLE",
            Self::Orphan => "E_ORPHAN",
        }
    }

    /// 返回契约中声明的进程退出码。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::Usage => 2,
            Self::Auth | Self::ActorNotAllowed => 3,
            Self::Preflight | Self::Bootstrap => 4,
            Self::Dispatch | Self::QueueTimeout => 5,
            Self::Link | Self::UdpBlocked | Self::P2pXtcp | Self::P2pTs | Self::HostKey => 6,
            Self::Entry => 7,
            Self::Artifact | Self::ApplyConflict => 8,
            Self::JobTimeout | Self::NoJoinTimeout => 9,
            Self::Cancel => 10,
            Self::WebNoDirectPath | Self::WebRelayUsed => 11,
            Self::LogsUnavailable | Self::Orphan => 12,
        }
    }
}

/// 一条 JSONL 日志事件；固定字段与 `contracts/log.json` 一致。
#[derive(Debug, Clone)]
pub struct LogEvent {
    pub ts: String,
    pub level: LogLevel,
    pub session_id: String,
    pub stage: Stage,
    pub event: String,
    pub error_code: Option<ErrorCode>,
}

// ===== 控制器与外壳接口 =====

/// 冻结命令；参数细节见 `contracts/cli.json`。
#[derive(Debug, Clone)]
pub enum Command {
    Init,
    Doctor,
    Up {
        entry: String,
        max_minutes: u32,
        idle_minutes: u32,
    },
    Web,
    Share {
        read_only: bool,
        ttl_minutes: u32,
    },
    Status,
    Connect,
    Entry {
        switch: String,
        target: String,
    },
    Pull {
        apply: bool,
        dest: String,
    },
    Kconfig {
        save: String,
    },
    Down {
        force: bool,
        rotate: bool,
    },
    Logs,
    Env,
    Clean {
        orphans: bool,
    },
    P2p {
        action: String,
    },
    Config {
        action: String,
    },
}

/// 两个外壳共享的只读会话视图；页面与 CLI 都只渲染它。
#[derive(Debug, Clone, Default)]
pub struct SessionView {
    pub session_id: String,
    pub state: SessionState,
    pub error_code: Option<ErrorCode>,
    pub run_id: Option<String>,
    pub entry: Option<String>,
    pub p2p_provider: Option<String>,
    pub read_link_present: bool,
    pub write_link_present: bool,
}

/// Controller 只做命令与事件路由；业务决定回到 Model。
pub trait Controller {
    /// 分发一条冻结命令。
    ///
    /// # 参数
    /// - `command`: 冻结命令。
    ///
    /// # 错误
    /// 返回 `ErrorCode`，调用方必须映射到退出码。
    fn dispatch(&self, command: &Command) -> Result<SessionView, ErrorCode>;
}

/// P2P 提供者；默认禁止中继，外壳不得自行放行。
pub trait P2pProvider {
    fn name(&self) -> &'static str;

    fn relay_allowed(&self) -> bool {
        false
    }
}

/// 入口适配器：终端与图形入口都只允许回环。
pub trait EntryAdapter {
    fn name(&self) -> &'static str;

    fn loopback_only(&self) -> bool {
        true
    }
}

/// artifact 清单与校验的唯一实现位置。
pub trait ArtifactStore {
    fn manifest_path(&self) -> &'static str;
}

/// 统一日志出口；禁止绕过它直接 print。
pub trait LogSink {
    fn emit(&self, event: &LogEvent);
}
