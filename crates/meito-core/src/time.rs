//! File: UTC 时间格式化。
//!
//! 日志契约要求 RFC 3339 的 `ts` 字段；这里只实现 UTC 秒级时间，避免为单一格式引入
//! 额外依赖，并保证输出固定以 `Z` 结尾。

use std::time::{SystemTime, UNIX_EPOCH};

// ===== RFC 3339 =====

/// 返回当前 UTC 时间的 RFC 3339 字符串。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误；系统时钟早于 1970 时按 0 秒处理，并在日志中体现为 `1970-01-01T00:00:00Z`。
pub fn rfc3339_utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    rfc3339_from_unix_seconds(seconds)
}

/// 把 Unix 秒转换为 RFC 3339 UTC 字符串。
///
/// # 参数
/// - `seconds`: 自 1970-01-01T00:00:00Z 起的秒数。
///
/// # 错误
/// 不返回错误。
pub fn rfc3339_from_unix_seconds(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

// ===== 日期换算 =====

/// 把“1970-01-01 起的天数”换算为公历年月日。
///
/// # 参数
/// - `days`: 自 1970-01-01 起的整数天数，可为负。
///
/// # 错误
/// 不返回错误；算法对负天数同样成立。
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    // Howard Hinnant 的 civil_from_days 算法，按 400 年周期处理闰年。
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let normalized_year = if month <= 2 { year + 1 } else { year };
    (normalized_year, month, day)
}
