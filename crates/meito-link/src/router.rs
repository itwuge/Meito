//! File: meito-link 的 HTTP 路由与方法分发骨架。
//!
//! 只做纯逻辑解析，不绑定具体 HTTP 库，也不读取任何契约数值：通道 id 长度上限由调用方
//! 从 `contracts/link.json` 读出后传入。后续接入真实 HTTP 服务时，本模块保持不变。

// ===== 路由模型 =====

/// 一次请求解析出的目标路由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `POST /v1/channels`：注册通道。
    RegisterChannel,
    /// `PUT /v1/channels/{id}/blob`：写入密文。
    PutBlob(String),
    /// `GET /v1/channels/{id}/blob`：读取密文。
    GetBlob(String),
    /// `DELETE /v1/channels/{id}`：撤销通道。
    DeleteChannel(String),
    /// 路径形状正确但 channel id 非法。
    BadRequest,
    /// 路径存在但不接受该方法。
    MethodNotAllowed,
    /// 路径不存在。
    NotFound,
}

// ===== 解析 =====

/// 把方法与路径解析为路由。
///
/// # 参数
/// - `method`: HTTP 方法，大小写不敏感。
/// - `path`: 请求路径，允许带查询串或片段，二者会被忽略。
/// - `max_channel_id_len`: 通道 id 的最大字节数，必须来自契约。
///
/// # 错误
/// 不返回错误；非法输入映射为 `BadRequest`、`MethodNotAllowed` 或 `NotFound`。
pub fn route(method: &str, path: &str, max_channel_id_len: usize) -> Route {
    let method = method.to_ascii_uppercase();
    let path_only = path.split(['?', '#']).next().unwrap_or(path);
    let trimmed = path_only.trim_matches('/');
    let segments: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };

    match segments.as_slice() {
        ["v1", "channels"] => {
            if method == "POST" {
                Route::RegisterChannel
            } else {
                Route::MethodNotAllowed
            }
        }
        ["v1", "channels", channel_id] => {
            if !is_valid_channel_id(channel_id, max_channel_id_len) {
                return Route::BadRequest;
            }
            if method == "DELETE" {
                Route::DeleteChannel(channel_id.to_string())
            } else {
                Route::MethodNotAllowed
            }
        }
        ["v1", "channels", channel_id, "blob"] => {
            if !is_valid_channel_id(channel_id, max_channel_id_len) {
                return Route::BadRequest;
            }
            match method.as_str() {
                "PUT" => Route::PutBlob(channel_id.to_string()),
                "GET" => Route::GetBlob(channel_id.to_string()),
                _ => Route::MethodNotAllowed,
            }
        }
        _ => Route::NotFound,
    }
}

/// 判断通道 id 是否是单个合法路径段。
///
/// # 参数
/// - `channel_id`: 待检查的 id。
/// - `max_len`: 契约给出的最大字节数。
///
/// # 错误
/// 不返回错误；非法返回 `false`。
fn is_valid_channel_id(channel_id: &str, max_len: usize) -> bool {
    !channel_id.is_empty()
        && channel_id.len() <= max_len
        && channel_id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'/')
}
