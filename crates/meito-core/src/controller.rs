//! File: 两个外壳共享的控制器接口与会话视图。
//!
//! 这里的类型在 M1 接口冻结时确定；M2 只补内核实现，不改变外壳可依赖的公共形状。
//! Controller 只做命令与事件路由，业务决定必须回到 meito-core。

use crate::error::ErrorCode;
use crate::state::SessionState;

// ===== 冻结命令 =====

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

// ===== 会话视图 =====

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

// ===== 外壳接口 =====

/// Controller 只做命令与事件路由；业务决定回到 Model。
pub trait Controller {
    /// 分发一条冻结命令。
    ///
    /// # 参数
    /// - `command`: 冻结命令。
    ///
    /// # 错误
    /// 返回 `ErrorCode`，调用方必须映射到契约退出码。
    fn dispatch(&self, command: &Command) -> Result<SessionView, ErrorCode>;
}

/// P2P 提供者；默认禁止中继，外壳不得自行放行。
pub trait P2pProvider {
    fn name(&self) -> &'static str;

    fn relay_allowed(&self) -> bool {
        false
    }
}

/// 入口适配器：终端与图形入口都只允许绑定回环。
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
