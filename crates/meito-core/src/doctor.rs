//! File: `doctor` 子命令的内核实现。
//!
//! doctor 是只读诊断：只检查契约文件、模板、目录与状态快照的可读性，不写任何文件；
//! 写权限探针在只读模式下不执行，报告中明确标注“未验证”，避免伪造结论。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{ErrorCode, MeitoError};
use crate::state_store::StateStore;
use crate::workflow::{WORKFLOW_DESTINATION_PATH, WORKFLOW_TEMPLATE_PATH};
use crate::CONTRACT_VERSION;

// ===== 契约文件清单 =====

/// 必须存在的冻结契约文件（0.2.0 共 11 份：版本清单 + 10 份契约/schema）。
pub const CONTRACT_FILES: [&str; 11] = [
    "contracts/contract-version.json",
    "contracts/cli.json",
    "contracts/error-codes.json",
    "contracts/log.json",
    "contracts/state-machine.json",
    "contracts/workflow.json",
    "contracts/artifact-manifest.schema.json",
    "contracts/connection-package.json",
    "contracts/udp-probe.json",
    "contracts/meito-config.schema.json",
    "contracts/tools-lock.schema.json",
];

/// 深度检查时抽取 workflow 输入名的来源文件。
const WORKFLOW_CONTRACT_PATH: &str = "contracts/workflow.json";

/// 深度检查只报告“是否设置”，绝不输出值的环境变量。
const SECRET_ENV_NAMES: [&str; 4] = [
    "FRPS_TOKEN",
    "TAILSCALE_AUTHKEY",
    "GH_TOKEN",
    "TURN_CREDENTIAL",
];

// ===== 结果模型 =====

/// 单项检查状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    /// 返回稳定状态字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

/// 单项诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub id: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// 诊断报告；没有任何 `Fail` 才算通过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub root: PathBuf,
    pub checks: Vec<DoctorCheck>,
    pub ok: bool,
}

impl DoctorReport {
    /// 输出机读结果。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn to_json(&self) -> Value {
        let checks: Vec<Value> = self
            .checks
            .iter()
            .map(|check| {
                json!({
                    "id": check.id,
                    "status": check.status.as_str(),
                    "detail": check.detail,
                })
            })
            .collect();
        json!({
            "root": self.root.display().to_string(),
            "ok": self.ok,
            "checks": checks,
        })
    }
}

/// 收集检查项的小工具，统一维护 `ok` 聚合。
struct CheckBuilder {
    checks: Vec<DoctorCheck>,
}

impl CheckBuilder {
    /// 新建空集合。
    ///
    /// # 参数
    /// - 无。
    ///
    /// # 错误
    /// 不返回错误。
    fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// 追加一条检查。
    ///
    /// # 参数
    /// - `id`: 检查项标识。
    /// - `status`: 结果状态。
    /// - `detail`: 人类可读说明。
    ///
    /// # 错误
    /// 不返回错误。
    fn push(&mut self, id: &str, status: CheckStatus, detail: impl Into<String>) {
        self.checks.push(DoctorCheck {
            id: id.to_string(),
            status,
            detail: detail.into(),
        });
    }

    /// 汇总为报告。
    ///
    /// # 参数
    /// - `root`: 诊断根目录。
    ///
    /// # 错误
    /// 不返回错误。
    fn finish(self, root: &Path) -> DoctorReport {
        let ok = !self
            .checks
            .iter()
            .any(|check| check.status == CheckStatus::Fail);
        DoctorReport {
            root: root.to_path_buf(),
            checks: self.checks,
            ok,
        }
    }
}

// ===== 诊断逻辑 =====

/// 执行只读环境与权限诊断。
///
/// # 参数
/// - `root`: 工作目录。
/// - `config`: 可选配置路径；显式给出但不存在时按用法错误处理。
/// - `repo`: 可选 `owner/repo`；显式给出但形状非法时按用法错误处理。
/// - `deep`: 是否追加契约细节、状态快照与敏感环境变量“是否存在”的深检。
///
/// # 错误
/// 显式配置不存在或 repo 形状非法返回 `E_USAGE`；不因普通检查失败而中止。
pub fn run_doctor(
    root: &Path,
    config: Option<&Path>,
    repo: Option<&str>,
    deep: bool,
) -> Result<DoctorReport, MeitoError> {
    if let Some(path) = config {
        if !path.is_file() {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("--config 指向的文件不存在：{}", path.display()),
            ));
        }
    }
    if let Some(value) = repo {
        if !is_repo_shape(value) {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("--repo 必须是 owner/repo 形状，收到 {value}"),
            ));
        }
    }

    let mut builder = CheckBuilder::new();

    // 契约文件必须存在且可解析；带 contractVersion 的文件必须与内核常量一致。
    let mut problems: Vec<String> = Vec::new();
    for relative in CONTRACT_FILES {
        let path = root.join(relative);
        if !path.is_file() {
            problems.push(format!("{relative} 不存在"));
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => {
                    if let Some(version) = value.get("contractVersion").and_then(Value::as_str) {
                        if version != CONTRACT_VERSION {
                            problems.push(format!(
                                "{relative} 的 contractVersion={version} 与内核 {CONTRACT_VERSION} 不一致"
                            ));
                        }
                    }
                }
                Err(error) => problems.push(format!("{relative} 不是合法 JSON：{error}")),
            },
            Err(error) => problems.push(format!("{relative} 读取失败：{error}")),
        }
    }
    if problems.is_empty() {
        builder.push(
            "contract.files",
            CheckStatus::Ok,
            format!("{} 个契约文件可解析且版本一致", CONTRACT_FILES.len()),
        );
    } else {
        builder.push("contract.files", CheckStatus::Fail, problems.join("；"));
    }

    let template = root.join(WORKFLOW_TEMPLATE_PATH);
    if template.is_file() {
        builder.push(
            "workflow.template",
            CheckStatus::Ok,
            format!("模板存在：{}", template.display()),
        );
    } else {
        builder.push(
            "workflow.template",
            CheckStatus::Fail,
            format!("模板不存在：{}", template.display()),
        );
    }

    let destination = root.join(WORKFLOW_DESTINATION_PATH);
    if destination.is_file() {
        builder.push(
            "workflow.installed",
            CheckStatus::Ok,
            format!("目标 workflow 已安装：{}", destination.display()),
        );
    } else {
        builder.push(
            "workflow.installed",
            CheckStatus::Warn,
            "目标 workflow 尚未安装；运行 `meito init` 可写入",
        );
    }

    let logs = root.join("logs");
    if logs.is_dir() {
        builder.push(
            "logs.directory",
            CheckStatus::Ok,
            format!("日志目录存在：{}", logs.display()),
        );
    } else {
        builder.push(
            "logs.directory",
            CheckStatus::Warn,
            "日志目录尚未创建；首次写日志时自动创建",
        );
    }
    // 只读诊断不创建探针文件，写权限结论一律标为未验证。
    builder.push(
        "logs.writable",
        CheckStatus::Warn,
        "只读模式未做写探针（未验证）",
    );

    let state_directory = StateStore::new(root).directory();
    if state_directory.is_dir() {
        builder.push(
            "state.directory",
            CheckStatus::Ok,
            format!("状态目录存在：{}", state_directory.display()),
        );
    } else {
        builder.push(
            "state.directory",
            CheckStatus::Warn,
            "状态目录尚未创建（正常）",
        );
    }

    builder.push(
        "config",
        CheckStatus::Ok,
        match config {
            Some(path) => format!("配置存在：{}", path.display()),
            None => "未显式指定配置（可选）".to_string(),
        },
    );

    match repo {
        Some(value) => builder.push(
            "repo.shape",
            CheckStatus::Ok,
            format!("owner/repo 形状合法：{value}"),
        ),
        None => builder.push(
            "repo.shape",
            CheckStatus::Warn,
            "未设置 --repo 或 MEITO_REPO",
        ),
    }

    if deep {
        deep_checks(root, &mut builder);
    }

    Ok(builder.finish(root))
}

/// 追加深度检查：workflow 输入、状态快照可读性与敏感环境变量是否存在。
///
/// # 参数
/// - `root`: 工作目录。
/// - `builder`: 检查项收集器。
///
/// # 错误
/// 不返回错误；所有问题都记录为检查项。
fn deep_checks(root: &Path, builder: &mut CheckBuilder) {
    let mut workflow_inputs: Vec<String> = Vec::new();
    let workflow_path = root.join(WORKFLOW_CONTRACT_PATH);
    match fs::read_to_string(&workflow_path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => {
                if let Some(inputs) = value.get("inputs").and_then(Value::as_array) {
                    for input in inputs {
                        if let Some(name) = input.get("name").and_then(Value::as_str) {
                            workflow_inputs.push(name.to_string());
                        }
                    }
                }
            }
            Err(error) => builder.push(
                "workflow.inputs",
                CheckStatus::Fail,
                format!("workflow 契约解析失败：{error}"),
            ),
        },
        Err(error) => builder.push(
            "workflow.inputs",
            CheckStatus::Fail,
            format!("workflow 契约读取失败：{error}"),
        ),
    }

    let template = root.join(WORKFLOW_TEMPLATE_PATH);
    match fs::read_to_string(&template) {
        Ok(text) => {
            let missing: Vec<String> = workflow_inputs
                .iter()
                .filter(|name| {
                    !text
                        .lines()
                        .any(|line| line.trim_start().starts_with(&format!("{name}:")))
                })
                .cloned()
                .collect();
            if missing.is_empty() {
                builder.push(
                    "workflow.inputs",
                    CheckStatus::Ok,
                    format!("模板包含全部 {} 个契约输入", workflow_inputs.len()),
                );
            } else {
                builder.push(
                    "workflow.inputs",
                    CheckStatus::Fail,
                    format!("模板缺少契约输入：{}", missing.join(", ")),
                );
            }
        }
        Err(error) => builder.push(
            "workflow.inputs",
            CheckStatus::Fail,
            format!("模板读取失败：{error}"),
        ),
    }

    let store = StateStore::new(root);
    match store.list_ids() {
        Ok(ids) => {
            let mut corrupt: Vec<String> = Vec::new();
            for id in &ids {
                match store.load(id) {
                    Ok(Some(_)) => {}
                    Ok(None) => corrupt.push(format!("{id} 快照消失")),
                    Err(error) => corrupt.push(format!("{id} 解析失败：{}", error.message())),
                }
            }
            if corrupt.is_empty() {
                builder.push(
                    "state.snapshots",
                    CheckStatus::Ok,
                    format!("{} 个会话快照可读", ids.len()),
                );
            } else {
                builder.push("state.snapshots", CheckStatus::Fail, corrupt.join("；"));
            }
        }
        Err(error) => builder.push(
            "state.snapshots",
            CheckStatus::Fail,
            format!("状态目录读取失败：{}", error.message()),
        ),
    }

    let mut secrets: Vec<String> = Vec::new();
    for name in SECRET_ENV_NAMES {
        let present = std::env::var(name).is_ok();
        secrets.push(format!(
            "{name}={}",
            if present { "present" } else { "absent" }
        ));
    }
    builder.push(
        "env.secrets",
        CheckStatus::Ok,
        format!("仅检查是否设置，不输出值：{}", secrets.join(", ")),
    );

    let link_present = std::env::var("MEITO_LINK_URL").is_ok();
    builder.push(
        "env.link",
        if link_present {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        format!(
            "MEITO_LINK_URL={}",
            if link_present { "present" } else { "absent" }
        ),
    );
}

/// 校验 `owner/repo` 形状。
///
/// # 参数
/// - `value`: 待校验字符串。
///
/// # 错误
/// 不返回错误；非法返回 `false`。
fn is_repo_shape(value: &str) -> bool {
    match value.split_once('/') {
        Some((owner, repo)) => !owner.is_empty() && !repo.is_empty() && !repo.contains('/'),
        None => false,
    }
}
