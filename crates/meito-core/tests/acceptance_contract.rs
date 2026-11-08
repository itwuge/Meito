//! File: 验收契约轻量预算测试——三个已定阈值从契约读取，改写后跟随。
//!
//! 六个 `pending-calibration` 项不在这里断言阈值；加载器不读取它们，避免把未标定值
//! 当成验收断言。

use std::path::PathBuf;

use meito_core::{load_lightweight_budget, ErrorCode, LightweightBudget, ACCEPTANCE_CONTRACT_PATH};
use serde_json::{json, Value};

// ===== 测试辅助 =====

/// 返回仓库根目录。
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("解析仓库根目录")
}

/// 读取仓库验收契约原文。
fn contract_text() -> String {
    std::fs::read_to_string(repository_root().join(ACCEPTANCE_CONTRACT_PATH))
        .expect("读取仓库验收契约")
}

/// 生成独立临时目录并写好 `contracts/` 子目录。
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("meito-acceptance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("contracts")).expect("创建临时契约目录");
    root
}

/// 从契约 JSON 取出三个已定阈值，作为测试期望值。
fn expected_budget(value: &Value) -> LightweightBudget {
    let metric = |key: &str| {
        value["lightweight"]["metrics"][key]
            .as_u64()
            .expect("已定轻量阈值必须是整数")
    };
    LightweightBudget {
        runner_resident_process_count_max: metric("runnerResidentProcessCountMax"),
        runner_new_package_count_max: metric("runnerNewPackageCountMax"),
        graphical_component_count_max: metric("graphicalComponentCountMax"),
    }
}

// ===== 用例 =====

#[test]
fn lightweight_budget_matches_contract() {
    let value: Value = serde_json::from_str(&contract_text()).expect("解析仓库验收契约");
    let budget = load_lightweight_budget(&repository_root()).expect("加载轻量预算");
    assert_eq!(
        budget,
        expected_budget(&value),
        "加载结果必须与 contracts/acceptance.json 的已定阈值一致"
    );
}

#[test]
fn loader_follows_rewritten_budget() {
    let root = temp_root("rewritten");
    let mut value: Value = serde_json::from_str(&contract_text()).expect("解析仓库验收契约");
    value["lightweight"]["metrics"]["runnerResidentProcessCountMax"] = json!(5);
    value["lightweight"]["metrics"]["runnerNewPackageCountMax"] = json!(2);
    value["lightweight"]["metrics"]["graphicalComponentCountMax"] = json!(1);
    std::fs::write(
        root.join(ACCEPTANCE_CONTRACT_PATH),
        serde_json::to_string(&value).expect("序列化临时契约"),
    )
    .expect("写入临时契约");

    let budget = load_lightweight_budget(&root).expect("加载改写后的轻量预算");
    assert_eq!(
        budget,
        LightweightBudget {
            runner_resident_process_count_max: 5,
            runner_new_package_count_max: 2,
            graphical_component_count_max: 1,
        },
        "加载器必须读取契约值，不得写死阈值"
    );
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn missing_lightweight_is_rejected() {
    let root = temp_root("missing");
    let mut value: Value = serde_json::from_str(&contract_text()).expect("解析仓库验收契约");
    value
        .as_object_mut()
        .expect("验收契约根对象")
        .remove("lightweight");
    std::fs::write(
        root.join(ACCEPTANCE_CONTRACT_PATH),
        serde_json::to_string(&value).expect("序列化临时契约"),
    )
    .expect("写入临时契约");

    let error = load_lightweight_budget(&root).expect_err("缺少 lightweight 必须拒绝");
    assert_eq!(error.code(), ErrorCode::Config);
    assert_eq!(error.exit_code(), 2);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn pending_calibration_values_are_not_loaded() {
    let value: Value = serde_json::from_str(&contract_text()).expect("解析仓库验收契约");
    let metrics = value["lightweight"]["metrics"]
        .as_object()
        .expect("lightweight.metrics");
    let pending: Vec<&str> = metrics
        .iter()
        .filter(|(_, metric)| metric.as_str() == Some("pending-calibration"))
        .map(|(key, _)| key.as_str())
        .collect();
    assert!(!pending.is_empty(), "契约应保留 pending-calibration 项");
    for key in pending {
        assert!(
            value["lightweight"]["pendingCalibration"][key]["neededMeasurement"]
                .as_str()
                .is_some(),
            "{key} 缺少 neededMeasurement"
        );
        assert!(
            value["lightweight"]["pendingCalibration"][key]["calibratedBy"]
                .as_str()
                .is_some(),
            "{key} 缺少 calibratedBy"
        );
    }
    let budget = load_lightweight_budget(&repository_root()).expect("加载轻量预算");
    assert_eq!(
        budget,
        expected_budget(&value),
        "加载器只能返回三个已定阈值，不能把 pending 项当成数值"
    );
}
