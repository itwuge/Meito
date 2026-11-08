//! File: 状态机与状态快照运行用例——合法推进、非法转移必须报错、快照落盘回读。
//!
//! 这些用例把 `contracts/state-machine.json` 的静态转移表变成可跑语义：
//! 合法路径必须推进到终态，非法转移必须返回 `E_INTERNAL` 且当前状态保持不变。

use std::path::{Path, PathBuf};

use meito_core::{
    new_session_id, read_status, transition_for, ErrorCode, SessionEvent, SessionMachine,
    SessionSnapshot, SessionState, StateStore,
};

// ===== 临时目录 =====

/// 测试用临时目录，析构时清理。
struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    /// 创建唯一临时目录。
    ///
    /// # 参数
    /// - `label`: 目录名前缀，便于定位失败用例。
    ///
    /// # 错误
    /// 目录创建失败时 panic。
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("meito-{}-{}", label, new_session_id()));
        std::fs::create_dir_all(&path).expect("创建测试目录");
        Self { path }
    }

    /// 返回目录路径。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("清理临时目录 {} 失败：{error}", self.path.display());
        }
    }
}

// ===== 合法转移 =====

/// 主干路径：idle → … → done，每一步都必须命中冻结转移表。
fn main_path_events() -> [(SessionEvent, SessionState); 12] {
    [
        (SessionEvent::Start, SessionState::Preflight),
        (SessionEvent::PreflightOk, SessionState::Dispatching),
        (SessionEvent::DispatchOk, SessionState::Queued),
        (SessionEvent::RunnerReady, SessionState::RunnerStarting),
        (SessionEvent::RunnerReady, SessionState::P2pConnecting),
        (SessionEvent::P2pOk, SessionState::LinkReady),
        (SessionEvent::LinkReady, SessionState::WaitingForJoin),
        (SessionEvent::Join, SessionState::EntryStarting),
        (SessionEvent::EntryReady, SessionState::Connected),
        (SessionEvent::Stop, SessionState::Ending),
        (SessionEvent::CollectOk, SessionState::Collecting),
        (SessionEvent::CollectOk, SessionState::Done),
    ]
}

#[test]
fn legal_main_path_reaches_done() {
    let mut machine = SessionMachine::new(new_session_id()).expect("创建状态机");
    assert_eq!(machine.state(), SessionState::Idle);
    assert!(!machine.is_terminal());

    for (event, expected) in main_path_events() {
        assert!(
            machine.can_apply(event),
            "{} 应接受事件 {}",
            machine.state().as_str(),
            event.as_str()
        );
        let reached = machine.apply(event).expect("合法转移不应报错");
        assert_eq!(reached, expected, "事件 {} 到达状态错误", event.as_str());
    }

    assert_eq!(machine.state(), SessionState::Done);
    assert!(machine.is_terminal());
    assert_eq!(machine.history().len(), 12, "每一步都应记录历史");
}

#[test]
fn failure_branch_reaches_terminal_state() {
    let mut machine = SessionMachine::new(new_session_id()).expect("创建状态机");
    machine
        .apply(SessionEvent::Start)
        .expect("idle → preflight");
    machine
        .apply(SessionEvent::PreflightFailed)
        .expect("preflight → failed_preflight");
    assert_eq!(machine.state(), SessionState::FailedPreflight);
    assert!(machine.is_terminal());
}

// ===== 非法转移 =====

#[test]
fn illegal_transition_errors_without_changing_state() {
    let mut machine = SessionMachine::new(new_session_id()).expect("创建状态机");

    // idle 只接受 start；stop 在 idle 上没有合法转移。
    let error = machine
        .apply(SessionEvent::Stop)
        .expect_err("非法转移必须报错");
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.exit_code(), 1, "E_INTERNAL 映射退出码 1");
    assert!(
        error.message().contains("非法状态转移"),
        "错误信息应明确指出非法转移：{}",
        error.message()
    );
    assert!(
        error.message().contains("start"),
        "错误信息应列出当前状态允许的事件"
    );
    assert_eq!(machine.state(), SessionState::Idle, "失败后状态不得改变");
    assert!(machine.history().is_empty(), "失败事件不得写入历史");
    assert!(transition_for(SessionState::Idle, SessionEvent::Stop).is_none());
}

#[test]
fn terminal_state_rejects_every_event() {
    let mut machine = SessionMachine::new(new_session_id()).expect("创建状态机");
    for (event, _) in main_path_events() {
        machine.apply(event).expect("沿主干路径推进");
    }
    assert!(machine.is_terminal());

    let error = machine
        .apply(SessionEvent::Start)
        .expect_err("终态不得再接受任何事件");
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(
        error.message().contains("无（已到终态）"),
        "终态错误信息应说明没有可接受事件：{}",
        error.message()
    );
    assert_eq!(machine.state(), SessionState::Done);
}

#[test]
fn checkpoint_has_no_transition_anywhere() {
    // checkpoint 出现在事件清单里，但不属于任何转移；它不能被静默吞掉。
    for state in [
        SessionState::Connected,
        SessionState::Queued,
        SessionState::Idle,
    ] {
        assert!(
            transition_for(state, SessionEvent::Checkpoint).is_none(),
            "{} 不应存在 checkpoint 转移",
            state.as_str()
        );
    }
    let mut machine = SessionMachine::new(new_session_id()).expect("创建状态机");
    let error = machine
        .apply(SessionEvent::Checkpoint)
        .expect_err("checkpoint 无转移必须报错");
    assert_eq!(error.code(), ErrorCode::Internal);
}

#[test]
fn invalid_session_id_is_rejected() {
    let error = SessionMachine::new("not-a-uuid").expect_err("非法会话 id 必须报错");
    assert_eq!(error.code(), ErrorCode::Usage);
    assert_eq!(error.exit_code(), 2);
}

// ===== 状态快照 =====

#[test]
fn snapshot_round_trips_through_state_store() {
    let directory = TempDirectory::new("state-store");
    let session_id = new_session_id();
    let store = StateStore::new(directory.path());

    let mut snapshot =
        SessionSnapshot::new(&session_id, SessionState::Connected).expect("构造快照");
    snapshot.run_id = Some("run-1".to_string());
    snapshot.entry = Some("terminal".to_string());
    snapshot
        .entry_urls
        .insert("terminal".to_string(), "http://127.0.0.1:7681".to_string());
    snapshot.last_heartbeat = Some("2026-11-08T00:00:00Z".to_string());
    snapshot.p2p_provider = Some("frp_xtcp".to_string());

    let path = store.save(&snapshot).expect("保存快照");
    assert!(path.is_file(), "快照文件应落盘");

    let loaded = store
        .load(&session_id)
        .expect("读取快照")
        .expect("快照应存在");
    assert_eq!(loaded, snapshot, "落盘回读必须完全一致");

    let latest = store.latest().expect("读取最近快照").expect("应存在快照");
    assert_eq!(latest.session_id, session_id);
    assert_eq!(
        store.list_ids().expect("列出快照"),
        vec![session_id.clone()]
    );

    // 同名会话再次保存必须原子替换，而不是留下临时文件。
    let directory_path = store.directory();
    let leftovers: Vec<String> = std::fs::read_dir(&directory_path)
        .expect("读取状态目录")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "不应残留临时文件：{leftovers:?}");
}

#[test]
fn unknown_state_name_is_rejected() {
    let value = serde_json::json!({
        "session_id": new_session_id(),
        "state": "mystery_state",
    });
    let error = SessionSnapshot::from_json(&value).expect_err("未知状态名必须报错");
    assert_eq!(error.code(), ErrorCode::Internal);
}

#[test]
fn status_is_idle_without_snapshots() {
    let directory = TempDirectory::new("status-idle");
    let report = read_status(directory.path(), None).expect("无快照时不应报错");
    assert_eq!(report.state, SessionState::Idle);
    assert!(report.session_id.is_none());
    assert!(!report.terminal);
}

#[test]
fn status_for_missing_explicit_session_is_orphan() {
    let directory = TempDirectory::new("status-orphan");
    let session_id = new_session_id();
    let error =
        read_status(directory.path(), Some(&session_id)).expect_err("显式缺失会话必须报 E_ORPHAN");
    assert_eq!(error.code(), ErrorCode::Orphan);
    assert_eq!(error.exit_code(), 12);
}

#[test]
fn status_reads_latest_snapshot() {
    let directory = TempDirectory::new("status-latest");
    let session_id = new_session_id();
    let store = StateStore::new(directory.path());
    let snapshot =
        SessionSnapshot::new(&session_id, SessionState::WaitingForJoin).expect("构造快照");
    store.save(&snapshot).expect("保存快照");

    let report = read_status(directory.path(), Some("latest")).expect("读取最近状态");
    assert_eq!(report.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(report.state, SessionState::WaitingForJoin);
    assert!(!report.terminal);

    let explicit = read_status(directory.path(), Some(&session_id)).expect("读取指定状态");
    assert_eq!(explicit.state, SessionState::WaitingForJoin);
}
