//! File: 冻结会话状态机（枚举、转移表、超时策略）。
//!
//! 唯一依据是 `contracts/state-machine.json`；这里只做声明与查表，实际推进逻辑由
//! `SessionMachine` 使用同一张表实现，禁止外壳复制第二份。

// ===== 状态枚举 =====

/// Model 的唯一会话状态枚举，两个外壳不得各自定义副本。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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

    /// 从契约状态名解析状态，供状态文件与日志回读使用。
    ///
    /// # 参数
    /// - `value`: 契约中的状态名。
    ///
    /// # 错误
    /// 未知名称返回 `None`；调用方必须显式处理。
    pub fn parse(value: &str) -> Option<Self> {
        let state = match value {
            "idle" => Self::Idle,
            "preflight" => Self::Preflight,
            "dispatching" => Self::Dispatching,
            "queued" => Self::Queued,
            "runner_starting" => Self::RunnerStarting,
            "p2p_connecting" => Self::P2pConnecting,
            "link_ready" => Self::LinkReady,
            "waiting_for_join" => Self::WaitingForJoin,
            "entry_starting" => Self::EntryStarting,
            "connected" => Self::Connected,
            "reconnecting" => Self::Reconnecting,
            "ending" => Self::Ending,
            "collecting" => Self::Collecting,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            "failed_preflight" => Self::FailedPreflight,
            "failed_dispatch" => Self::FailedDispatch,
            "failed_queue_timeout" => Self::FailedQueueTimeout,
            "failed_bootstrap" => Self::FailedBootstrap,
            "failed_p2p" => Self::FailedP2p,
            "failed_link" => Self::FailedLink,
            "failed_entry" => Self::FailedEntry,
            "failed_collect" => Self::FailedCollect,
            "no_direct_path" => Self::NoDirectPath,
            "orphaned" => Self::Orphaned,
            _ => return None,
        };
        Some(state)
    }

    /// 判断是否为契约声明的终态。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done
                | Self::Cancelled
                | Self::FailedPreflight
                | Self::FailedDispatch
                | Self::FailedQueueTimeout
                | Self::FailedBootstrap
                | Self::FailedP2p
                | Self::FailedLink
                | Self::FailedEntry
                | Self::FailedCollect
                | Self::NoDirectPath
                | Self::Orphaned
        )
    }
}

/// 日志阶段与状态机共用同一枚举，避免两侧漂移。
pub type Stage = SessionState;

// ===== 事件枚举 =====

/// 状态机事件，转移表见 `contracts/state-machine.json`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl SessionEvent {
    /// 返回与契约 JSON 一致的稳定事件名。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::PreflightOk => "preflight_ok",
            Self::PreflightFailed => "preflight_failed",
            Self::DispatchOk => "dispatch_ok",
            Self::DispatchFailed => "dispatch_failed",
            Self::QueueTimeout => "queue_timeout",
            Self::RunnerReady => "runner_ready",
            Self::RunnerFailed => "runner_failed",
            Self::P2pOk => "p2p_ok",
            Self::P2pFailed => "p2p_failed",
            Self::NoDirectPath => "no_direct_path",
            Self::LinkReady => "link_ready",
            Self::LinkFailed => "link_failed",
            Self::Join => "join",
            Self::JoinTimeout => "join_timeout",
            Self::EntryReady => "entry_ready",
            Self::EntryFailed => "entry_failed",
            Self::ConnectOk => "connect_ok",
            Self::Disconnect => "disconnect",
            Self::ReconnectOk => "reconnect_ok",
            Self::ReconnectFailed => "reconnect_failed",
            Self::Stop => "stop",
            Self::ForceStop => "force_stop",
            Self::CollectOk => "collect_ok",
            Self::CollectFailed => "collect_failed",
            Self::Checkpoint => "checkpoint",
            Self::OrphanDetect => "orphan_detect",
        }
    }
}

// ===== 转移表 =====

/// 一条合法状态转移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: SessionState,
    pub event: SessionEvent,
    pub to: SessionState,
}

/// 契约中的全部合法转移。
static TRANSITIONS: [Transition; 33] = [
    Transition {
        from: SessionState::Idle,
        event: SessionEvent::Start,
        to: SessionState::Preflight,
    },
    Transition {
        from: SessionState::Preflight,
        event: SessionEvent::PreflightOk,
        to: SessionState::Dispatching,
    },
    Transition {
        from: SessionState::Preflight,
        event: SessionEvent::PreflightFailed,
        to: SessionState::FailedPreflight,
    },
    Transition {
        from: SessionState::Dispatching,
        event: SessionEvent::DispatchOk,
        to: SessionState::Queued,
    },
    Transition {
        from: SessionState::Dispatching,
        event: SessionEvent::DispatchFailed,
        to: SessionState::FailedDispatch,
    },
    Transition {
        from: SessionState::Queued,
        event: SessionEvent::RunnerReady,
        to: SessionState::RunnerStarting,
    },
    Transition {
        from: SessionState::Queued,
        event: SessionEvent::QueueTimeout,
        to: SessionState::FailedQueueTimeout,
    },
    Transition {
        from: SessionState::RunnerStarting,
        event: SessionEvent::RunnerReady,
        to: SessionState::P2pConnecting,
    },
    Transition {
        from: SessionState::RunnerStarting,
        event: SessionEvent::RunnerFailed,
        to: SessionState::FailedBootstrap,
    },
    Transition {
        from: SessionState::P2pConnecting,
        event: SessionEvent::P2pOk,
        to: SessionState::LinkReady,
    },
    Transition {
        from: SessionState::P2pConnecting,
        event: SessionEvent::P2pFailed,
        to: SessionState::FailedP2p,
    },
    Transition {
        from: SessionState::P2pConnecting,
        event: SessionEvent::NoDirectPath,
        to: SessionState::NoDirectPath,
    },
    Transition {
        from: SessionState::LinkReady,
        event: SessionEvent::LinkReady,
        to: SessionState::WaitingForJoin,
    },
    Transition {
        from: SessionState::LinkReady,
        event: SessionEvent::LinkFailed,
        to: SessionState::FailedLink,
    },
    Transition {
        from: SessionState::WaitingForJoin,
        event: SessionEvent::Join,
        to: SessionState::EntryStarting,
    },
    Transition {
        from: SessionState::WaitingForJoin,
        event: SessionEvent::JoinTimeout,
        to: SessionState::Cancelled,
    },
    Transition {
        from: SessionState::EntryStarting,
        event: SessionEvent::EntryReady,
        to: SessionState::Connected,
    },
    Transition {
        from: SessionState::EntryStarting,
        event: SessionEvent::EntryFailed,
        to: SessionState::FailedEntry,
    },
    Transition {
        from: SessionState::Connected,
        event: SessionEvent::Disconnect,
        to: SessionState::Reconnecting,
    },
    Transition {
        from: SessionState::Reconnecting,
        event: SessionEvent::ReconnectOk,
        to: SessionState::Connected,
    },
    Transition {
        from: SessionState::Reconnecting,
        event: SessionEvent::ReconnectFailed,
        to: SessionState::FailedP2p,
    },
    Transition {
        from: SessionState::Connected,
        event: SessionEvent::Stop,
        to: SessionState::Ending,
    },
    Transition {
        from: SessionState::WaitingForJoin,
        event: SessionEvent::Stop,
        to: SessionState::Ending,
    },
    Transition {
        from: SessionState::EntryStarting,
        event: SessionEvent::Stop,
        to: SessionState::Ending,
    },
    Transition {
        from: SessionState::Reconnecting,
        event: SessionEvent::Stop,
        to: SessionState::Ending,
    },
    Transition {
        from: SessionState::Ending,
        event: SessionEvent::CollectOk,
        to: SessionState::Collecting,
    },
    Transition {
        from: SessionState::Ending,
        event: SessionEvent::CollectFailed,
        to: SessionState::FailedCollect,
    },
    Transition {
        from: SessionState::Collecting,
        event: SessionEvent::CollectOk,
        to: SessionState::Done,
    },
    Transition {
        from: SessionState::Collecting,
        event: SessionEvent::CollectFailed,
        to: SessionState::FailedCollect,
    },
    Transition {
        from: SessionState::Connected,
        event: SessionEvent::ForceStop,
        to: SessionState::Cancelled,
    },
    Transition {
        from: SessionState::WaitingForJoin,
        event: SessionEvent::ForceStop,
        to: SessionState::Cancelled,
    },
    Transition {
        from: SessionState::P2pConnecting,
        event: SessionEvent::Stop,
        to: SessionState::Ending,
    },
    Transition {
        from: SessionState::Orphaned,
        event: SessionEvent::Stop,
        to: SessionState::Cancelled,
    },
];

/// 返回契约中的合法转移表；调用方只能查询，不得修改。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误。
pub fn transitions() -> &'static [Transition] {
    &TRANSITIONS
}

// ===== 超时与自动取消 =====

/// job 硬上限；托管 runner 单 job 最长 360 分钟。
pub const JOB_HARD_LIMIT_MINUTES: u32 = 360;

/// 会话默认时长。
pub const SESSION_DEFAULT_MINUTES: u32 = 60;

/// 会话最大时长；给 6 小时硬上限留收尾余量。
pub const SESSION_MAX_MINUTES: u32 = 300;

/// 空闲自动停止时长。
pub const IDLE_MINUTES: u32 = 10;

/// 无人加入自动取消时长。
pub const NO_JOIN_MINUTES: u32 = 5;

/// 距离最大时长多久先做 checkpoint。
pub const CHECKPOINT_BEFORE_MAX_MINUTES: u32 = 10;

/// SSH 重连上限。
pub const RECONNECT_MAX_SSH: u32 = 3;

/// ICE 重连上限。
pub const RECONNECT_MAX_ICE: u32 = 2;

/// 契约中的自动取消规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCancelRule {
    pub when: SessionState,
    pub timeout: &'static str,
    pub event: SessionEvent,
    pub error_code: crate::ErrorCode,
}

/// 契约中的四条自动取消规则。
static AUTO_CANCEL: [AutoCancelRule; 4] = [
    AutoCancelRule {
        when: SessionState::WaitingForJoin,
        timeout: "noJoinMinutes",
        event: SessionEvent::JoinTimeout,
        error_code: crate::ErrorCode::NoJoinTimeout,
    },
    AutoCancelRule {
        when: SessionState::Connected,
        timeout: "idleMinutes",
        event: SessionEvent::Stop,
        error_code: crate::ErrorCode::JobTimeout,
    },
    AutoCancelRule {
        when: SessionState::Connected,
        timeout: "sessionMaxMinutes",
        event: SessionEvent::Stop,
        error_code: crate::ErrorCode::JobTimeout,
    },
    AutoCancelRule {
        when: SessionState::Reconnecting,
        timeout: "reconnectMaxSsh",
        event: SessionEvent::ReconnectFailed,
        error_code: crate::ErrorCode::P2pXtcp,
    },
];

/// 返回契约中的自动取消规则。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误。
pub fn auto_cancel_rules() -> &'static [AutoCancelRule] {
    &AUTO_CANCEL
}
