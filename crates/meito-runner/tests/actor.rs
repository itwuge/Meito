//! File: 运行真实 `meito-runner check-actor` 进程的用例。
//!
//! 断言只看契约退出码与 `logs/session-<id>.jsonl` 的结构化事件，覆盖 fail-closed
//! 的四种拒绝原因与 workflow 覆盖优先级。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn temp_root(tag: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("meito-runner-actor-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).expect("创建临时目录");
    directory
}

fn write_config(root: &Path, text: &str) {
    fs::write(root.join("meito.toml"), text).expect("写入配置");
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meito-runner"))
        .current_dir(root)
        .env_remove("GITHUB_ACTOR")
        .env_remove("MEITO_SESSION_ID")
        .args(args)
        .output()
        .expect("启动 runner")
}

fn events(root: &Path, session: &str) -> Vec<serde_json::Value> {
    let path = root.join(format!("logs/session-{session}.jsonl"));
    let text = fs::read_to_string(path).expect("读取日志");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("日志行必须是 JSON"))
        .collect()
}

fn find_event<'a>(events: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    events
        .iter()
        .find(|event| event["event"] == name)
        .unwrap_or_else(|| panic!("缺少事件 {name}：{events:?}"))
}

/// 名单命中时放行，日志记录来源、actor 与结果。
#[test]
fn allows_listed_actor() {
    let root = temp_root("allow");
    write_config(&root, "[security]\nallowed_actors = [\"alice\"]\n");
    let output = run(
        &root,
        &["check-actor", "--session", SESSION, "--actor", "alice"],
    );
    assert_eq!(output.status.code(), Some(0));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["stage"], "preflight");
    assert_eq!(event["allowed"], true);
    assert_eq!(event["source"], "config");
    assert_eq!(event["reason"], serde_json::Value::Null);
    drop(fs::remove_dir_all(&root));
}

/// 不在名单时返回 `E_ACTOR_NOT_ALLOWED`（退出码 3），原因必须可区分。
#[test]
fn denies_actor_not_in_list() {
    let root = temp_root("not-listed");
    write_config(&root, "[security]\nallowed_actors = [\"alice\"]\n");
    let output = run(
        &root,
        &["check-actor", "--session", SESSION, "--actor", "bob"],
    );
    assert_eq!(output.status.code(), Some(3));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["error_code"], "E_ACTOR_NOT_ALLOWED");
    assert_eq!(event["allowed"], false);
    assert_eq!(event["reason"], "not_listed");
    drop(fs::remove_dir_all(&root));
}

/// 缺配置文件时必须 fail-closed 且原因是 config_missing。
#[test]
fn denies_when_config_missing() {
    let root = temp_root("config-missing");
    let output = run(
        &root,
        &["check-actor", "--session", SESSION, "--actor", "alice"],
    );
    assert_eq!(output.status.code(), Some(3));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["reason"], "config_missing");
    drop(fs::remove_dir_all(&root));
}

/// 空名单必须 fail-closed 且原因是 empty_allowlist。
#[test]
fn denies_when_allowlist_empty() {
    let root = temp_root("empty-list");
    write_config(&root, "[security]\nallowed_actors = []\n");
    let output = run(
        &root,
        &["check-actor", "--session", SESSION, "--actor", "alice"],
    );
    assert_eq!(output.status.code(), Some(3));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["reason"], "empty_allowlist");
    drop(fs::remove_dir_all(&root));
}

/// 没有 actor 环境变量也没有 --actor 时原因是 missing_actor。
#[test]
fn denies_when_actor_missing() {
    let root = temp_root("missing-actor");
    write_config(&root, "[security]\nallowed_actors = [\"alice\"]\n");
    let output = run(&root, &["check-actor", "--session", SESSION]);
    assert_eq!(output.status.code(), Some(3));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["reason"], "missing_actor");
    drop(fs::remove_dir_all(&root));
}

/// workflow 覆盖优先，且配置缺失不阻止覆盖放行。
#[test]
fn override_wins_over_missing_config() {
    let root = temp_root("override");
    let output = run(
        &root,
        &[
            "check-actor",
            "--session",
            SESSION,
            "--actor",
            "bob",
            "--allow-actor",
            "bob",
        ],
    );
    assert_eq!(output.status.code(), Some(0));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.checked");
    assert_eq!(event["source"], "override");
    assert_eq!(event["allowed"], true);
    drop(fs::remove_dir_all(&root));
}

/// 配置语法错误返回 `E_CONFIG`（退出码 2），并留下诊断事件。
#[test]
fn rejects_invalid_config_with_diagnostic() {
    let root = temp_root("bad-config");
    write_config(&root, "[security]\nallowed_actors = \"alice\"\n");
    let output = run(
        &root,
        &["check-actor", "--session", SESSION, "--actor", "alice"],
    );
    assert_eq!(output.status.code(), Some(2));

    let log = events(&root, SESSION);
    let event = find_event(&log, "actor.config_invalid");
    assert_eq!(event["error_code"], "E_CONFIG");
    drop(fs::remove_dir_all(&root));
}

/// 会话 id 缺失是用法错误，不能静默生成一个无法关联的 id。
#[test]
fn requires_session_id() {
    let root = temp_root("no-session");
    let output = run(&root, &["check-actor", "--actor", "alice"]);
    assert_eq!(output.status.code(), Some(2));
    drop(fs::remove_dir_all(&root));
}
