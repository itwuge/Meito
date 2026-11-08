//! M4-1：frpc xtcp visitor 配置渲染与禁中继审计。

use meito_core::{
    audit_visitor_ini, render_visitor_ini, ErrorCode, FrpXtcpProvider, FrpcVisitorSpec, P2pProvider,
};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const SECRET: &str = "abcdefghijklmnop";

/// 构造一份合法输入。
fn spec<'a>(bind_addr: &'a str, secret_key: &'a str) -> FrpcVisitorSpec<'a> {
    FrpcVisitorSpec {
        session_id: SESSION,
        server_addr: "frps.example.com",
        server_port: 7000,
        secret_key,
        bind_addr,
        bind_port: 0,
    }
}

#[test]
fn renders_xtcp_visitor_and_passes_audit() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    assert!(text.contains("type = xtcp"));
    assert!(text.contains("role = visitor"));
    assert!(text.contains("server_name = ssh-11111111-1111-4111-8111-111111111111"));
    assert!(text.contains("bind_addr = 127.0.0.1"));
    assert!(text.contains("bind_port = 0"));
    assert!(!text.contains("fallback"));
    assert!(!text.contains("stcp"));
    audit_visitor_ini(&text).expect("审计必须通过");
}

#[test]
fn rejects_non_loopback_bind() {
    let error = render_visitor_ini(&spec("0.0.0.0", SECRET)).expect_err("通配绑定必须失败");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);
}

#[test]
fn rejects_invalid_session_id() {
    let mut input = spec("127.0.0.1", SECRET);
    input.session_id = "not-a-uuid";
    let error = render_visitor_ini(&input).expect_err("非 UUID 必须失败");
    assert_eq!(error.code(), ErrorCode::Usage);
}

#[test]
fn rejects_secret_with_injection_or_wrong_charset() {
    for secret in [
        "short",
        "has space has space",
        "line\nbreaklinebreak",
        "plus+plusplusplus",
    ] {
        let error =
            render_visitor_ini(&spec("127.0.0.1", secret)).expect_err("非法 secret_key 必须失败");
        assert_eq!(error.code(), ErrorCode::Usage, "secret={secret:?}");
    }
}

#[test]
fn rejects_server_port_zero() {
    let mut input = spec("127.0.0.1", SECRET);
    input.server_port = 0;
    let error = render_visitor_ini(&input).expect_err("端口 0 必须失败");
    assert_eq!(error.code(), ErrorCode::Usage);
}

#[test]
fn audit_rejects_stcp_type() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    let tampered = text.replace("type = xtcp", "type = stcp");
    let error = audit_visitor_ini(&tampered).expect_err("stcp 必须被拒绝");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);
}

#[test]
fn audit_rejects_fallback_keys_case_insensitively() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    for key in ["fallbackTo", "Fallback", "fallback_to"] {
        let tampered = format!("{text}{key} = ssh-other\n");
        let error = audit_visitor_ini(&tampered).expect_err("fallback 必须被拒绝");
        assert_eq!(error.code(), ErrorCode::P2pXtcp, "key={key}");
    }
}

#[test]
fn audit_rejects_duplicate_keys() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    let tampered = format!("{text}bind_port = 0\n");
    let error = audit_visitor_ini(&tampered).expect_err("重复键必须被拒绝");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);
}

#[test]
fn audit_rejects_non_loopback_and_bad_port() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    let wildcard = text.replace("bind_addr = 127.0.0.1", "bind_addr = 0.0.0.0");
    let error = audit_visitor_ini(&wildcard).expect_err("非回环必须被拒绝");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);

    let bad_port = text.replace("bind_port = 0", "bind_port = 70000");
    let error = audit_visitor_ini(&bad_port).expect_err("越界端口必须被拒绝");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);
}

#[test]
fn audit_rejects_unknown_section() {
    let text = render_visitor_ini(&spec("127.0.0.1", SECRET)).expect("渲染必须成功");
    let tampered = format!("{text}[admin]\naddr = 0.0.0.0\n");
    let error = audit_visitor_ini(&tampered).expect_err("未知节必须被拒绝");
    assert_eq!(error.code(), ErrorCode::P2pXtcp);
}

#[test]
fn secret_containing_stcp_substring_is_not_a_false_positive() {
    let secret = "stcpxxxxxxxxxxxx";
    let text = render_visitor_ini(&spec("127.0.0.1", secret)).expect("渲染必须成功");
    audit_visitor_ini(&text).expect("secret 中的 stcp 子串不应被当成中继");
}

#[test]
fn provider_never_allows_relay() {
    let provider = FrpXtcpProvider;
    assert_eq!(P2pProvider::name(&provider), "frp_xtcp");
    assert!(!P2pProvider::relay_allowed(&provider));
}
