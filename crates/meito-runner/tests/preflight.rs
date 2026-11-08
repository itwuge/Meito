//! File: 运行真实 `meito-runner preflight` 进程的用例。
//!
//! 覆盖三种 UDP 失败原因的区分（目标未配置 / DNS 解析失败 / 超时未回执）、收到回执的
//! 成功路径，以及“每一步都落结构化日志（阶段、耗时、结果）”的门禁要求。

use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const SESSION: &str = "22222222-2222-4222-8222-222222222222";

fn temp_root(tag: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "meito-runner-preflight-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&directory).expect("创建临时目录");
    fs::write(
        directory.join("meito.toml"),
        "[security]\nallowed_actors = [\"alice\"]\n",
    )
    .expect("写入配置");
    directory
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meito-runner"))
        .current_dir(root)
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

/// 启动一个回执 `MEITOUDP` 报文的本地 UDP 服务。
fn spawn_echo() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("绑定回执服务");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("设置读取超时");
    let address = socket.local_addr().expect("读取本地地址");
    std::thread::spawn(move || {
        let mut buffer = [0u8; 64];
        while let Ok((size, peer)) = socket.recv_from(&mut buffer) {
            if socket.send_to(&buffer[..size], peer).is_err() {
                break;
            }
        }
    });
    address
}

/// 目标未配置：退出 2（`E_CONFIG`），且三步与汇总都要有日志。
#[test]
fn logs_every_step_when_target_missing() {
    let root = temp_root("no-target");
    let output = run(&root, &["preflight", "--session", SESSION]);
    assert_eq!(output.status.code(), Some(2));

    let log = events(&root, SESSION);
    for name in [
        "preflight.udp",
        "preflight.dns",
        "preflight.disk",
        "preflight.summary",
    ] {
        let event = find_event(&log, name);
        assert_eq!(event["stage"], "preflight", "事件 {name} 阶段");
        assert!(event["duration_ms"].is_number(), "事件 {name} 耗时");
        assert!(event["result"].is_string(), "事件 {name} 结果");
    }
    let udp = find_event(&log, "preflight.udp");
    assert_eq!(udp["result"], "target_not_configured");
    assert_eq!(udp["error_code"], "E_CONFIG");
    let dns = find_event(&log, "preflight.dns");
    assert_eq!(dns["result"], "skipped");
    let summary = find_event(&log, "preflight.summary");
    assert_eq!(summary["result"], "failed");
    assert_eq!(summary["error_code"], "E_CONFIG");
    drop(fs::remove_dir_all(&root));
}

/// DNS 解析失败：退出 4（`E_PREFLIGHT`），原因与超时严格区分。
#[test]
fn distinguishes_dns_failure() {
    let root = temp_root("dns-fail");
    let output = run(
        &root,
        &[
            "preflight",
            "--session",
            SESSION,
            "--udp-target",
            "no-such-host.meito.invalid:9000",
        ],
    );
    assert_eq!(output.status.code(), Some(4));

    let log = events(&root, SESSION);
    let udp = find_event(&log, "preflight.udp");
    assert_eq!(udp["result"], "dns_resolution_failed");
    assert_eq!(udp["error_code"], "E_PREFLIGHT");
    drop(fs::remove_dir_all(&root));
}

/// 超时未回执：退出 6（`E_UDP_BLOCKED`），原因不得写成 dns 或 blocked 笼统值。
#[test]
fn distinguishes_timeout() {
    let root = temp_root("timeout");
    // 先占用再释放一个本地端口，得到一个几乎肯定没有监听者的目标。
    let probe = UdpSocket::bind("127.0.0.1:0").expect("占用端口");
    let port = probe.local_addr().expect("读取本地地址").port();
    drop(probe);
    let target = format!("127.0.0.1:{port}");
    let output = run(
        &root,
        &["preflight", "--session", SESSION, "--udp-target", &target],
    );
    assert_eq!(output.status.code(), Some(6));

    let log = events(&root, SESSION);
    let udp = find_event(&log, "preflight.udp");
    assert_eq!(udp["result"], "timeout");
    assert_eq!(udp["error_code"], "E_UDP_BLOCKED");
    assert_eq!(udp["attempts_used"], 3);
    drop(fs::remove_dir_all(&root));
}

/// 收到同 nonce 回执即通过，三步全部成功。
#[test]
fn succeeds_with_echo_receipt() {
    let root = temp_root("echo");
    let address = spawn_echo();
    let target = address.to_string();
    let output = run(
        &root,
        &["preflight", "--session", SESSION, "--udp-target", &target],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let log = events(&root, SESSION);
    let udp = find_event(&log, "preflight.udp");
    assert_eq!(udp["result"], "ok");
    assert_eq!(udp["attempts_used"], 1);
    let summary = find_event(&log, "preflight.summary");
    assert_eq!(summary["result"], "ok");
    assert_eq!(summary["error_code"], "");
    drop(fs::remove_dir_all(&root));
}

/// 某一步失败不会中断后续步骤；磁盘失败必须独立记录。
#[test]
fn records_disk_failure_without_stopping() {
    let root = temp_root("disk-fail");
    let missing = root.join("missing-dir");
    let missing_text = missing.to_string_lossy().to_string();
    let output = run(
        &root,
        &[
            "preflight",
            "--session",
            SESSION,
            "--disk-path",
            &missing_text,
        ],
    );
    // UDP 目标未配置仍是首个错误，退出码 2；磁盘步骤的结果独立记录。
    assert_eq!(output.status.code(), Some(2));

    let log = events(&root, SESSION);
    let disk = find_event(&log, "preflight.disk");
    assert_eq!(disk["result"], "failed");
    assert_eq!(disk["error_code"], "E_PREFLIGHT");
    drop(fs::remove_dir_all(&root));
}
