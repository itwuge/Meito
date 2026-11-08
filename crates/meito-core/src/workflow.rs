//! File: `init` 子命令的内核实现。
//!
//! 契约 `cli.json` 只规定 `init` 安装 workflow 模板；模板默认取仓库内的
//! `templates/meito-session.yml`，目标固定为 `.github/workflows/meito-session.yml`。
//! 已存在目标时只有显式 `--force` 才覆盖，避免破坏用户仓库。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{ErrorCode, MeitoError};

// ===== 路径常量 =====

/// 仓库内默认 workflow 模板路径（相对工作目录）。
pub const WORKFLOW_TEMPLATE_PATH: &str = "templates/meito-session.yml";

/// 目标仓库中的 workflow 安装路径。
pub const WORKFLOW_DESTINATION_PATH: &str = ".github/workflows/meito-session.yml";

// ===== 结果模型 =====

/// `init` 执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub template: PathBuf,
    pub destination: PathBuf,
    pub bytes: u64,
}

impl InitReport {
    /// 输出机读结果。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn to_json(&self) -> Value {
        json!({
            "template": self.template.display().to_string(),
            "destination": self.destination.display().to_string(),
            "bytes": self.bytes,
        })
    }
}

// ===== 安装逻辑 =====

/// 把 workflow 模板复制到目标仓库。
///
/// # 参数
/// - `root`: 目标仓库根目录。
/// - `template`: 可选模板路径；缺省使用 `templates/meito-session.yml`。
/// - `force`: 目标已存在时是否覆盖。
///
/// # 错误
/// 模板不存在返回 `E_PREFLIGHT`；目标已存在且未加 `--force` 返回 `E_USAGE`；
/// 目录创建或复制失败返回 `E_PREFLIGHT`。
pub fn init_workflow(
    root: &Path,
    template: Option<&Path>,
    force: bool,
) -> Result<InitReport, MeitoError> {
    let template_path = match template {
        Some(path) => path.to_path_buf(),
        None => root.join(WORKFLOW_TEMPLATE_PATH),
    };
    if !template_path.is_file() {
        return Err(MeitoError::new(
            ErrorCode::Preflight,
            format!("workflow 模板不存在：{}", template_path.display()),
        ));
    }

    let destination = root.join(WORKFLOW_DESTINATION_PATH);
    if destination.exists() && !force {
        return Err(MeitoError::new(
            ErrorCode::Usage,
            format!(
                "目标 workflow 已存在：{}；使用 --force 才会覆盖",
                destination.display()
            ),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            MeitoError::io(
                ErrorCode::Preflight,
                format!("创建 workflow 目录 {}", parent.display()),
                &error,
            )
        })?;
    }
    let bytes = fs::copy(&template_path, &destination).map_err(|error| {
        MeitoError::io(
            ErrorCode::Preflight,
            format!(
                "复制 workflow 模板 {} 到 {}",
                template_path.display(),
                destination.display()
            ),
            &error,
        )
    })?;
    Ok(InitReport {
        template: template_path,
        destination,
        bytes,
    })
}
