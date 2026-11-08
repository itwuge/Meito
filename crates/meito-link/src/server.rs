//! File: meito-link 的 HTTP/1.1 协调服务。
//!
//! 协议解析交给 `hyper`，本模块不再手写请求行与头部解析。单包上限、TTL、读取次数、限流
//! 与状态码全部来自 `contracts/link.json`；服务端只存密文，不解析、不记录 blob，访问日志
//! 只允许 `channel_id`、`size`、`status` 三类字段。
//!
//! 资源约束：头部读取缓冲上限 `MAX_HEADER_BYTES`、并发连接上限 `MAX_CONCURRENT_CONNECTIONS`、
//! 单连接处理超时 `REQUEST_TIMEOUT`；请求体上限取契约的单包上限。
//!
//! 令牌通过 `Authorization: Bearer <token>` 提交；契约只冻结哈希算法，未冻结请求头名称，
//! 该约定属于服务层实现细节。

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::access_log::{append_access_log, AccessLogRecord};
use crate::auth::{is_sha256_hex, verify_token};
use crate::contract::{HttpStatusPolicy, LinkContract, OperationAuth};
use crate::rate_limit::RateLimiter;
use crate::router::{is_valid_channel_id, route, Route};
use crate::store::{ChannelStore, Registration, StoreError};

// ===== 资源约束 =====

/// 请求行与请求头允许占用的最大读取缓冲；超过后由 hyper 中止连接。
const MAX_HEADER_BYTES: usize = 8192;

/// 同时处理的连接上限；超出的连接留在监听队列中，不会无限创建任务。
const MAX_CONCURRENT_CONNECTIONS: usize = 64;

/// 单连接从接收到写回响应的处理超时，用于抵御慢速攻击。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

// ===== 服务端 =====

/// 协调服务实例；绑定成功后由调用方决定线程模型。
pub struct LinkServer {
    listener: std::net::TcpListener,
    state: Arc<ServerState>,
}

/// 服务端共享状态；所有数值都来自契约。
struct ServerState {
    contract: LinkContract,
    store: Arc<dyn ChannelStore>,
    limiter: RateLimiter,
    access_log_path: PathBuf,
    started: Instant,
}

impl LinkServer {
    /// 绑定监听地址并构造服务实例。
    ///
    /// # 参数
    /// - `address`: 监听地址；测试可传 `127.0.0.1:0` 由系统分配端口。
    /// - `contract`: 已校验的协调服务契约，所有数值从这里读取。
    /// - `store`: 通道存储实现。
    /// - `access_log_path`: 访问日志文件路径。
    ///
    /// # 错误
    /// 端口被占用或无权限时返回底层 IO 错误。
    pub fn bind(
        address: SocketAddr,
        contract: LinkContract,
        store: Arc<dyn ChannelStore>,
        access_log_path: PathBuf,
    ) -> io::Result<Self> {
        let listener = std::net::TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        let limiter = RateLimiter::from_contract(&contract.rate_limit);
        Ok(Self {
            listener,
            state: Arc::new(ServerState {
                contract,
                store,
                limiter,
                access_log_path,
                started: Instant::now(),
            }),
        })
    }

    /// 返回实际监听地址。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 监听套接字已失效时返回底层 IO 错误。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// 启动 tokio 运行时并监听连接，直到监听套接字失效。
    ///
    /// # 参数
    /// - 无（消耗 `self`）。
    ///
    /// # 错误
    /// 运行时创建失败、`accept` 失败或监听套接字切换失败时返回底层 IO 错误。
    pub fn run(self) -> io::Result<()> {
        let LinkServer { listener, state } = self;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        runtime.block_on(serve(listener, state))
    }
}

/// 接受连接并按超时、并发上限分发给 hyper。
///
/// # 参数
/// - `listener`: 已绑定的标准库监听套接字。
/// - `state`: 服务端共享状态。
///
/// # 错误
/// 监听套接字失效或无法转换为异步套接字时返回底层 IO 错误。
async fn serve(listener: std::net::TcpListener, state: Arc<ServerState>) -> io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    loop {
        let (stream, remote) = listener.accept().await?;
        let permit = match Arc::clone(&permits).acquire_owned().await {
            Ok(permit) => permit,
            Err(_error) => return Ok(()),
        };
        let state = Arc::clone(&state);
        let remote_ip = remote.ip().to_string();
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |request| {
                let state = Arc::clone(&state);
                let remote_ip = remote_ip.clone();
                async move { Ok::<_, Infallible>(handle_request(state, remote_ip, request).await) }
            });
            let connection = http1::Builder::new()
                .keep_alive(false)
                .max_buf_size(MAX_HEADER_BYTES)
                .serve_connection(TokioIo::new(stream), service);
            match tokio::time::timeout(REQUEST_TIMEOUT, connection).await {
                Ok(Ok(())) => {}
                Ok(Err(_error)) => {}
                Err(_error) => {}
            }
        });
    }
}

// ===== 请求处理 =====

/// 处理一次请求：限流、路由、存储操作、写访问日志并构造响应。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `remote_ip`: 来源 IP 字符串。
/// - `request`: hyper 提供的请求。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
async fn handle_request(
    state: Arc<ServerState>,
    remote_ip: String,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let now = SystemTime::now();
    let elapsed = state.started.elapsed();
    let outcome = dispatch(&state, &remote_ip, request, now, elapsed).await;
    let record = AccessLogRecord {
        channel_id: outcome.channel_id.clone(),
        size: outcome.size,
        status: outcome.status,
    };
    let mut status = outcome.status;
    let mut body = outcome.body;
    let mut content_type = outcome.content_type;
    match append_access_log(&state.access_log_path, &record) {
        Ok(()) => {}
        Err(_error) => {
            status = state.contract.http_status.internal;
            body = Vec::new();
            content_type = None;
        }
    }
    respond(status, body, content_type)
}

/// 构造响应；状态码来自契约，非法值回退为内部错误。
///
/// # 参数
/// - `status`: 契约映射出的状态码。
/// - `body`: 响应报文。
/// - `content_type`: 可选的 `Content-Type`。
///
/// # 错误
/// 不返回错误；响应构造失败时返回空响应。
fn respond(
    status: u16,
    body: Vec<u8>,
    content_type: Option<&'static str>,
) -> Response<Full<Bytes>> {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder().status(status);
    if let Some(value) = content_type {
        builder = builder.header(CONTENT_TYPE, value);
    }
    match builder.body(Full::new(Bytes::from(body))) {
        Ok(response) => response,
        Err(_error) => Response::new(Full::new(Bytes::new())),
    }
}

// ===== 分发 =====

/// 一次分发的内部结果，同时携带访问日志允许的三类字段。
struct Outcome {
    status: u16,
    body: Vec<u8>,
    content_type: Option<&'static str>,
    channel_id: String,
    size: Option<u64>,
}

impl Outcome {
    /// 构造只有状态码的结果。
    ///
    /// # 参数
    /// - `status`: 契约映射出的状态码。
    ///
    /// # 错误
    /// 不返回错误。
    fn new(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            content_type: None,
            channel_id: String::new(),
            size: None,
        }
    }

    /// 附加通道 id 与大小，供访问日志记录。
    ///
    /// # 参数
    /// - `channel_id`: 本次请求涉及的通道 id。
    /// - `size`: 请求或响应中的密文字节数。
    ///
    /// # 错误
    /// 不返回错误。
    fn with_channel(mut self, channel_id: &str, size: Option<u64>) -> Self {
        self.channel_id = channel_id.to_string();
        self.size = size;
        self
    }
}

/// 先做来源 IP 限流，再按路由分发。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `remote_ip`: 来源 IP 字符串。
/// - `request`: hyper 提供的请求。
/// - `now`: 当前墙钟时间，传给存储层。
/// - `elapsed`: 自服务启动以来的时长，传给限流器。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
async fn dispatch(
    state: &ServerState,
    remote_ip: &str,
    request: Request<Incoming>,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    let status_map = &state.contract.http_status;
    if !state.limiter.allow_ip(remote_ip, elapsed) {
        return Outcome::new(status_map.rate_limited);
    }

    let method = request.method().as_str().to_string();
    let target = request.uri().path().to_string();
    let auth_header_name = state.contract.auth_header.name.to_ascii_lowercase();
    let authorization = request
        .headers()
        .get(auth_header_name.as_str())
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let declared_length = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let max_body = state.contract.package_limit.max_ciphertext_bytes;
    let route = route(&method, &target, state.contract.channel_id.max_length);

    match route {
        Route::RegisterChannel => {
            let body =
                match read_body(request, max_body, declared_length, status_map.too_large).await {
                    Ok(body) => body,
                    Err(outcome) => return outcome,
                };
            handle_register(state, &body, now)
        }
        Route::PutBlob(channel_id) => {
            if !state.limiter.allow_channel(&channel_id, elapsed) {
                return Outcome::new(status_map.rate_limited)
                    .with_channel(&channel_id, Some(declared_length as u64));
            }
            if let Err(status) = authorize(
                state,
                &channel_id,
                authorization.as_deref(),
                state.contract.operations.put.auth,
                now,
            ) {
                return Outcome::new(status)
                    .with_channel(&channel_id, Some(declared_length as u64));
            }
            let body =
                match read_body(request, max_body, declared_length, status_map.too_large).await {
                    Ok(body) => body,
                    Err(outcome) => {
                        return outcome.with_channel(&channel_id, Some(declared_length as u64))
                    }
                };
            let size = Some(body.len() as u64);
            match state.store.put_ciphertext(&channel_id, body, now) {
                Ok(()) => Outcome::new(status_map.success).with_channel(&channel_id, size),
                Err(error) => {
                    Outcome::new(store_status(error, status_map)).with_channel(&channel_id, size)
                }
            }
        }
        Route::GetBlob(channel_id) => {
            handle_get(state, &channel_id, authorization.as_deref(), now, elapsed)
        }
        Route::DeleteChannel(channel_id) => {
            handle_delete(state, &channel_id, authorization.as_deref(), now, elapsed)
        }
        Route::BadRequest => Outcome::new(status_map.malformed),
        Route::MethodNotAllowed => Outcome::new(status_map.method_not_allowed),
        Route::NotFound => Outcome::new(status_map.unknown_channel),
    }
}

/// 读取请求体并施加契约单包上限；声明长度已超限时不再读取。
///
/// # 参数
/// - `request`: hyper 提供的请求。
/// - `max_body`: 契约给出的单包上限。
/// - `declared_length`: `Content-Length` 声明的长度。
/// - `too_large`: 契约中的超限状态码。
///
/// # 错误
/// 声明长度或实际报文超过上限、读取失败时返回 `too_large` 状态码。
async fn read_body(
    request: Request<Incoming>,
    max_body: usize,
    declared_length: usize,
    too_large: u16,
) -> Result<Vec<u8>, Outcome> {
    if declared_length > max_body {
        return Err(Outcome::new(too_large));
    }
    match Limited::new(request.into_body(), max_body).collect().await {
        Ok(collected) => Ok(collected.to_bytes().to_vec()),
        Err(_error) => Err(Outcome::new(too_large)),
    }
}

/// 把存储错误映射为契约状态码。
///
/// # 参数
/// - `error`: 存储层错误。
/// - `status`: 契约中的状态码映射。
///
/// # 错误
/// 不返回错误。
fn store_status(error: StoreError, status: &HttpStatusPolicy) -> u16 {
    match error {
        StoreError::ChannelExists => status.duplicate_channel,
        StoreError::NotFound | StoreError::Expired => status.unknown_channel,
        StoreError::TooLarge => status.too_large,
        StoreError::MaxReadsReached => status.rate_limited,
    }
}

/// 从 `Authorization` 头取出 Bearer 令牌。
///
/// # 参数
/// - `authorization`: 请求头原始值。
/// - `scheme`: 契约声明的鉴权方案。
///
/// # 错误
/// 不返回错误；缺失或格式不对返回 `None`。
fn bearer_token<'a>(authorization: Option<&'a str>, scheme: &str) -> Option<&'a str> {
    let value = authorization?;
    let (presented_scheme, token) = value.split_once(' ')?;
    if !presented_scheme.eq_ignore_ascii_case(scheme) {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// 校验通道是否存在、未过期且令牌匹配。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `channel_id`: 请求路径中的通道 id。
/// - `authorization`: 请求头中的令牌原文。
/// - `auth`: 契约声明的鉴权角色，决定使用读 token还是写 token。
/// - `now`: 当前墙钟时间。
///
/// # 错误
/// 通道不存在或已过期返回 `unknown_channel` 状态码；令牌缺失或不匹配返回 `bad_token`。
fn authorize(
    state: &ServerState,
    channel_id: &str,
    authorization: Option<&str>,
    auth: OperationAuth,
    now: SystemTime,
) -> Result<(), u16> {
    let status_map = &state.contract.http_status;
    let credentials = state
        .store
        .credentials(channel_id, now)
        .ok_or(status_map.unknown_channel)?;
    let expected = if auth.uses_write_token() {
        credentials.write_token_hash.as_str()
    } else {
        credentials.read_token_hash.as_str()
    };
    let presented = bearer_token(authorization, &state.contract.auth_header.scheme)
        .ok_or(status_map.bad_token)?;
    if verify_token(presented, expected) {
        Ok(())
    } else {
        Err(status_map.bad_token)
    }
}

/// 解析注册报文并按契约必需字段构造注册材料。
///
/// # 参数
/// - `state`: 服务端共享状态，用于读取必需字段清单与通道 id 上限。
/// - `body`: 注册请求的 JSON 报文。
///
/// # 错误
/// JSON 非法、缺字段、id 非法或摘要形状不对时返回 `malformed` 状态码。
fn parse_registration(state: &ServerState, body: &[u8]) -> Result<Registration, u16> {
    let malformed = state.contract.http_status.malformed;
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_error) => return Err(malformed),
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => return Err(malformed),
    };

    let mut channel_id = None;
    let mut read_token_hash = None;
    let mut write_token_hash = None;
    let mut public_key = None;
    for field in &state.contract.register_payload.required {
        let value = match object.get(field).and_then(serde_json::Value::as_str) {
            Some(text) if !text.is_empty() => text,
            _ => return Err(malformed),
        };
        match field.as_str() {
            "channel_id" => channel_id = Some(value),
            "read_token_hash" => {
                if !is_sha256_hex(value) {
                    return Err(malformed);
                }
                read_token_hash = Some(value);
            }
            "write_token_hash" => {
                if !is_sha256_hex(value) {
                    return Err(malformed);
                }
                write_token_hash = Some(value);
            }
            "public_key" => public_key = Some(value),
            _ => {}
        }
    }

    let channel_id = channel_id.ok_or(malformed)?;
    let read_token_hash = read_token_hash.ok_or(malformed)?;
    let write_token_hash = write_token_hash.ok_or(malformed)?;
    let public_key = public_key.ok_or(malformed)?;
    if !is_valid_channel_id(channel_id, state.contract.channel_id.max_length) {
        return Err(malformed);
    }

    Ok(Registration {
        channel_id: channel_id.to_string(),
        read_token_hash: read_token_hash.to_string(),
        write_token_hash: write_token_hash.to_string(),
        public_key: public_key.to_string(),
    })
}

/// 处理通道注册。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `body`: 注册请求的 JSON 报文。
/// - `now`: 当前墙钟时间。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_register(state: &ServerState, body: &[u8], now: SystemTime) -> Outcome {
    let registration = match parse_registration(state, body) {
        Ok(registration) => registration,
        Err(status) => return Outcome::new(status),
    };
    let channel_id = registration.channel_id.clone();
    match state.store.register(registration, now) {
        Ok(()) => Outcome::new(state.contract.http_status.success).with_channel(&channel_id, None),
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(&channel_id, None),
    }
}

/// 处理密文读取。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `channel_id`: 路径中的通道 id。
/// - `authorization`: 请求头中的令牌原文。
/// - `now`: 当前墙钟时间。
/// - `elapsed`: 自服务启动以来的时长。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_get(
    state: &ServerState,
    channel_id: &str,
    authorization: Option<&str>,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    if !state.limiter.allow_channel(channel_id, elapsed) {
        return Outcome::new(state.contract.http_status.rate_limited)
            .with_channel(channel_id, None);
    }
    if let Err(status) = authorize(
        state,
        channel_id,
        authorization,
        state.contract.operations.get.auth,
        now,
    ) {
        return Outcome::new(status).with_channel(channel_id, None);
    }
    match state.store.read_ciphertext(channel_id, now) {
        Ok(blob_read) => {
            let size = blob_read.blob.len() as u64;
            Outcome {
                status: state.contract.http_status.success,
                body: blob_read.blob,
                content_type: Some("application/octet-stream"),
                channel_id: channel_id.to_string(),
                size: Some(size),
            }
        }
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(channel_id, None),
    }
}

/// 处理通道撤销。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `channel_id`: 路径中的通道 id。
/// - `authorization`: 请求头中的令牌原文。
/// - `now`: 当前墙钟时间。
/// - `elapsed`: 自服务启动以来的时长。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_delete(
    state: &ServerState,
    channel_id: &str,
    authorization: Option<&str>,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    if !state.limiter.allow_channel(channel_id, elapsed) {
        return Outcome::new(state.contract.http_status.rate_limited)
            .with_channel(channel_id, None);
    }
    if let Err(status) = authorize(
        state,
        channel_id,
        authorization,
        state.contract.operations.delete.auth,
        now,
    ) {
        return Outcome::new(status).with_channel(channel_id, None);
    }
    match state.store.delete(channel_id, now) {
        Ok(()) => Outcome::new(state.contract.http_status.success).with_channel(channel_id, None),
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(channel_id, None),
    }
}
