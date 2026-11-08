//! File: 冻结会话状态机（枚举、转移表、超时策略）。
//!
//! 唯一依据是 `contracts/state-machine.json`；这里只做声明与查表，实际推进逻辑由
//! `SessionMachine` 使用同一张表实现，禁止外壳复制第二份。

use crate::error::{ErrorCode, MeitoError};
use crate::log::is_uuid_v4;

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

    /// 从契约事件名解析事件。
    ///
    /// # 参数
    /// - `value`: 契约中的事件名。
    ///
    /// # 错误
    /// 未知名称返回 `None`；调用方必须显式处理。
    pub fn parse(value: &str) -> Option<Self> {
        let event = match value {
            "start" => Self::Start,
            "preflight_ok" => Self::PreflightOk,
            "preflight_failed" => Self::PreflightFailed,
            "dispatch_ok" => Self::DispatchOk,
            "dispatch_failed" => Self::DispatchFailed,
            "queue_timeout" => Self::QueueTimeout,
            "runner_ready" => Self::RunnerReady,
            "runner_failed" => Self::RunnerFailed,
            "p2p_ok" => Self::P2pOk,
            "p2p_failed" => Self::P2pFailed,
            "no_direct_path" => Self::NoDirectPath,
            "link_ready" => Self::LinkReady,
            "link_failed" => Self::LinkFailed,
            "join" => Self::Join,
            "join_timeout" => Self::JoinTimeout,
            "entry_ready" => Self::EntryReady,
            "entry_failed" => Self::EntryFailed,
            "connect_ok" => Self::ConnectOk,
            "disconnect" => Self::Disconnect,
            "reconnect_ok" => Self::ReconnectOk,
            "reconnect_failed" => Self::ReconnectFailed,
            "stop" => Self::Stop,
            "force_stop" => Self::ForceStop,
            "collect_ok" => Self::CollectOk,
            "collect_failed" => Self::CollectFailed,
            "checkpoint" => Self::Checkpoint,
            "orphan_detect" => Self::OrphanDetect,
            _ => return None,
        };
        Some(event)
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

// ===== 状态机推进 =====

/// 查询某状态在收到某事件时应到达状态。
///
/// # 参数
/// - `from`: 当前状态。
/// - `event`: 待应用事件。
///
/// # 错误
/// 不返回错误；非法转移返回 `None`，由 `SessionMachine::apply` 转换为错误。
pub fn transition_for(from: SessionState, event: SessionEvent) -> Option<&'static Transition> {
    TRANSITIONS
        .iter()
        .find(|transition| transition.from == from && transition.event == event)
}

/// 单个会话的状态机实例；每次推进都必须命中冻结转移表。
#[derive(Debug, Clone)]
pub struct SessionMachine {
    session_id: String,
    state: SessionState,
    history: Vec<(SessionEvent, SessionState)>,
}

impl SessionMachine {
    /// 创建处于 `idle` 的状态机。
    ///
    /// # 参数
    /// - `session_id`: 会话 id，必须是 UUID v4。
    ///
    /// # 错误
    /// 会话 id 非法返回 `E_USAGE`。
    pub fn new(session_id: impl Into<String>) -> Result<Self, MeitoError> {
        let session_id = session_id.into();
        if !is_uuid_v4(&session_id) {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("会话 id 必须是 UUID v4，收到 {session_id}"),
            ));
        }
        Ok(Self {
            session_id,
            state: SessionState::Idle,
            history: Vec::new(),
        })
    }

    /// 返回会话 id。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 返回当前状态。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// 返回是否已进入终态。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// 判断当前状态是否接受某事件。
    ///
    /// # 参数
    /// - `event`: 待查询事件。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn can_apply(&self, event: SessionEvent) -> bool {
        transition_for(self.state, event).is_some()
    }

    /// 返回已应用事件与到达状态的顺序记录。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn history(&self) -> &[(SessionEvent, SessionState)] {
        &self.history
    }

    /// 应用一个事件；非法转移必须报错且状态保持不变。
    ///
    /// # 参数
    /// - `event`: 待应用事件。
    ///
    /// # 错误
    /// 当前状态没有该事件对应的合法转移时返回 `E_INTERNAL`，错误信息列出当前状态允许的事件。
    pub fn apply(&mut self, event: SessionEvent) -> Result<SessionState, MeitoError> {
        let transition = transition_for(self.state, event).ok_or_else(|| {
            let mut legal: Vec<&'static str> = TRANSITIONS
                .iter()
                .filter(|candidate| candidate.from == self.state)
                .map(|candidate| candidate.event.as_str())
                .collect();
            legal.sort_unstable();
            let legal_text = if legal.is_empty() {
                "无（已到终态）".to_string()
            } else {
                legal.join(", ")
            };
            MeitoError::new(
                ErrorCode::Internal,
                format!(
                    "非法状态转移：{} + {}；当前状态允许的事件：{}",
                    self.state.as_str(),
                    event.as_str(),
                    legal_text
                ),
            )
        })?;
        self.state = transition.to;
        self.history.push((event, self.state));
        Ok(self.state)
    }
}
