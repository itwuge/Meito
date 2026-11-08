//! File: actor 白名单判定。
//!
//! 判定只做一件事：给定实际 actor 与生效白名单，返回允许或拒绝及拒绝原因。
//! 缺配置/空白名单一律拒绝（fail-closed），且原因要写进结构化日志，便于 runner 上排查。

use std::fmt;

// ===== 来源与原因 =====

/// 白名单来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorSource {
    /// 来自仓库根 `meito.toml`。
    Config,
    /// 来自 workflow 的 `flags.allowed_actors` 覆盖。
    Override,
}

impl ActorSource {
    /// 返回日志中的稳定字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Override => "override",
        }
    }
}

/// 拒绝原因；写入日志后可直接定位是缺 actor、缺配置还是不在名单。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorDenyReason {
    /// 运行环境没有提供 `GITHUB_ACTOR`，也没有 `--actor`。
    MissingActor,
    /// 仓库根没有 `meito.toml`。
    ConfigMissing,
    /// 白名单为空。
    EmptyAllowlist,
    /// actor 不在白名单。
    NotListed,
}

impl ActorDenyReason {
    /// 返回日志中的稳定字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingActor => "missing_actor",
            Self::ConfigMissing => "config_missing",
            Self::EmptyAllowlist => "empty_allowlist",
            Self::NotListed => "not_listed",
        }
    }
}

// ===== 判定 =====

/// actor 判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorDecision {
    /// 允许进入预检。
    Allowed,
    /// 拒绝，并给出可记录的原因。
    Denied(ActorDenyReason),
}

impl ActorDecision {
    /// 判断是否允许。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// 返回拒绝原因；允许时为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn reason(&self) -> Option<ActorDenyReason> {
        match self {
            Self::Allowed => None,
            Self::Denied(reason) => Some(*reason),
        }
    }
}

/// 按 fail-closed 规则判定 actor。
///
/// # 参数
/// - `actor`: 实际 actor；`None` 或空白视为缺失。
/// - `allowed`: 生效白名单；空列表表示全部拒绝。
/// - `config_missing`: 是否因为仓库根本没有 `meito.toml`。
///
/// # 错误
/// 不返回错误；拒绝原因由 `ActorDecision::Denied` 携带。
pub fn decide_actor(
    actor: Option<&str>,
    allowed: &[String],
    config_missing: bool,
) -> ActorDecision {
    let Some(actor) = actor.map(str::trim).filter(|value| !value.is_empty()) else {
        return ActorDecision::Denied(ActorDenyReason::MissingActor);
    };
    if config_missing {
        return ActorDecision::Denied(ActorDenyReason::ConfigMissing);
    }
    if allowed.is_empty() {
        return ActorDecision::Denied(ActorDenyReason::EmptyAllowlist);
    }
    // GitHub 登录名大小写不敏感，比较时统一忽略 ASCII 大小写。
    if allowed
        .iter()
        .any(|entry| entry.trim().eq_ignore_ascii_case(actor))
    {
        ActorDecision::Allowed
    } else {
        ActorDecision::Denied(ActorDenyReason::NotListed)
    }
}

impl fmt::Display for ActorDenyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.as_str())
    }
}

// ===== 单元测试 =====

#[cfg(test)]
mod tests {
    use super::{decide_actor, ActorDecision, ActorDenyReason};

    fn allowlist(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    /// 名单命中即放行，且 GitHub 登录名大小写不敏感。
    #[test]
    fn allows_listed_actor_case_insensitively() {
        let allowed = allowlist(&["alice"]);
        assert!(decide_actor(Some("alice"), &allowed, false).is_allowed());
        assert!(decide_actor(Some("ALICE"), &allowed, false).is_allowed());
        assert!(decide_actor(Some(" alice "), &allowed, false).is_allowed());
    }

    /// 四种拒绝原因必须各自可区分。
    #[test]
    fn reports_distinct_deny_reasons() {
        let allowed = allowlist(&["alice"]);
        assert_eq!(
            decide_actor(None, &allowed, false).reason(),
            Some(ActorDenyReason::MissingActor)
        );
        assert_eq!(
            decide_actor(Some("   "), &allowed, false).reason(),
            Some(ActorDenyReason::MissingActor)
        );
        assert_eq!(
            decide_actor(Some("alice"), &allowed, true).reason(),
            Some(ActorDenyReason::ConfigMissing)
        );
        assert_eq!(
            decide_actor(Some("alice"), &[], false).reason(),
            Some(ActorDenyReason::EmptyAllowlist)
        );
        assert_eq!(
            decide_actor(Some("bob"), &allowed, false).reason(),
            Some(ActorDenyReason::NotListed)
        );
        assert_eq!(
            decide_actor(Some("bob"), &allowed, false),
            ActorDecision::Denied(ActorDenyReason::NotListed)
        );
    }
}
