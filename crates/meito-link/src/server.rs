//! File: meito-link 的最小 HTTP/1.1 协调服务。
//!
//! 只使用标准库的 `TcpListener`，不引入网络框架。单包上限、TTL、读取次数、限流与状态码
//! 全部来自 `contracts/link.json`；服务端只存密文，不解析、不记录 blob，访问日志只允许
//! `channel_id`、`size`、`status` 三类字段。
//!
//! 令牌通过 `Authorization: Bearer <token>` 提交；契约只冻结哈希算法，未冻结请求头名称，
//! 该约定属于服务层实现细节。

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::access_log::{append_access_log, AccessLogRecord};
use crate::auth::{is_sha256_hex, verify_token};
use crate::contract::{HttpStatusPolicy, LinkContract};
use crate::rate_limit::RateLimiter;
use crate::router::{is_valid_channel_id, route, Route};
use crate::store::{ChannelStore, Registration, StoreError};

/// 单行请求行或请求头的最大字节数；超过即视为报文非法，避免无界读取。
const MAX_HEADER_BYTES: usize = 8192;

/// 允许的请求头数量上限；超过即视为报文非法。
const MAX_HEADER_COUNT: usize = 32;

// ===== 请求模型 =====

/// 已解析的 HTTP 请求；方法统一大写，头名称统一小写。
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    /// 按小写名称查找请求头。
    ///
    /// # 参数
    /// - `name`: 小写请求头名称。
    ///
    /// # 错误
    /// 不返回错误；缺失返回 `None`。
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// 请求解析失败的原因。
enum ParseError {
    /// 请求行、请求头或长度字段不合法。
    Malformed,
    /// 声明的报文体超过契约单包上限。
    TooLarge,
    /// 底层读取失败或连接提前关闭。
    Io(io::Error),
}

/// 读取并解析一个完整的 HTTP/1.1 请求。
///
/// # 参数
/// - `reader`: 建立在已接受连接上的缓冲读取器。
/// - `max_body`: 契约给出的单包上限，超过即返回 `TooLarge`。
///
/// # 错误
/// 报文非法返回 `Malformed`，超限返回 `TooLarge`，底层读取失败返回 `Io`。
fn read_request(
    reader: &mut BufReader<&mut TcpStream>,
    max_body: usize,
) -> Result<Request, ParseError> {
    let mut request_line = String::new();
    let read = reader
        .read_line(&mut request_line)
        .map_err(ParseError::Io)?;
    if read == 0 || request_line.len() > MAX_HEADER_BYTES {
        return Err(ParseError::Malformed);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(ParseError::Malformed)?;
    let target = parts.next().ok_or(ParseError::Malformed)?;
    let version = parts.next().ok_or(ParseError::Malformed)?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(ParseError::Malformed);
    }

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(ParseError::Io)?;
        if read == 0 {
            return Err(ParseError::Malformed);
        }
        if line.len() > MAX_HEADER_BYTES {
            return Err(ParseError::Malformed);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed.split_once(':').ok_or(ParseError::Malformed)?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        if headers.len() > MAX_HEADER_COUNT {
            return Err(ParseError::Malformed);
        }
    }

    let mut content_length = 0usize;
    for (name, value) in &headers {
        if name == "transfer-encoding" {
            return Err(ParseError::Malformed);
        }
        if name == "content-length" {
            content_length = value
                .parse::<usize>()
                .map_err(|_error| ParseError::Malformed)?;
        }
    }
    if content_length > max_body {
        return Err(ParseError::TooLarge);
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                ParseError::Malformed
            } else {
                ParseError::Io(error)
            }
        })?;
    }

    Ok(Request {
        method: method.to_ascii_uppercase(),
        target: target.to_string(),
        headers,
        body,
    })
}

// ===== 响应模型 =====

/// 待写回的一次响应。
struct Response {
    status: u16,
    body: Vec<u8>,
    content_type: Option<&'static str>,
}

/// 把响应写回连接并关闭。
///
/// # 参数
/// - `stream`: 已接受的连接。
/// - `response`: 待写回的状态码与报文。
///
/// # 错误
/// 任何写失败都返回底层 IO 错误。
fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason_phrase(response.status),
        response.body.len()
    );
    if let Some(content_type) = response.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// 返回状态码对应的标准英文短语。
///
/// # 参数
/// - `status`: 已冻结的 HTTP 状态码。
///
/// # 错误
/// 不返回错误；未知状态码回退为 `Status`。
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

// ===== 服务端 =====

/// 协调服务实例；绑定成功后由调用方决定线程模型。
pub struct LinkServer {
    listener: TcpListener,
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
        let listener = TcpListener::bind(address)?;
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

    /// 接受连接并逐个处理，直到监听套接字失效。
    ///
    /// # 参数
    /// - 无（消耗 `self`）。
    ///
    /// # 错误
    /// `accept` 失败时返回底层 IO 错误。
    pub fn run(self) -> io::Result<()> {
        for incoming in self.listener.incoming() {
            let stream = incoming?;
            handle_connection(stream, &self.state);
        }
        Ok(())
    }
}

// ===== 连接处理 =====

/// 处理单个连接：解析、分发、写访问日志、写回响应。
///
/// # 参数
/// - `stream`: 已接受的连接。
/// - `state`: 服务端共享状态。
///
/// # 错误
/// 不返回错误；任何失败都转化为响应或直接关闭连接。
fn handle_connection(mut stream: TcpStream, state: &ServerState) {
    let remote = match stream.peer_addr() {
        Ok(address) => address.ip().to_string(),
        Err(_error) => String::new(),
    };
    let now = SystemTime::now();
    let elapsed = state.started.elapsed();
    let parsed = {
        let mut reader = BufReader::new(&mut stream);
        read_request(
            &mut reader,
            state.contract.package_limit.max_ciphertext_bytes,
        )
    };
    let outcome = match parsed {
        Ok(request) => dispatch(state, &request, &remote, now, elapsed),
        Err(ParseError::Malformed) => Outcome::new(state.contract.http_status.malformed),
        Err(ParseError::TooLarge) => Outcome::new(state.contract.http_status.too_large),
        Err(ParseError::Io(_error)) => return,
    };
    finish(&mut stream, state, outcome);
}

/// 写访问日志并写回响应；日志失败时改为内部错误状态码。
///
/// # 参数
/// - `stream`: 已接受的连接。
/// - `state`: 服务端共享状态。
/// - `outcome`: 分发结果。
///
/// # 错误
/// 不返回错误；写回失败只关闭连接。
fn finish(stream: &mut TcpStream, state: &ServerState, outcome: Outcome) {
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
    let response = Response {
        status,
        body,
        content_type,
    };
    match write_response(stream, &response) {
        Ok(()) => {}
        Err(_error) => {}
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
/// - `request`: 已解析的请求。
/// - `remote`: 来源 IP 字符串。
/// - `now`: 当前墙钟时间，传给存储层。
/// - `elapsed`: 自服务启动以来的时长，传给限流器。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn dispatch(
    state: &ServerState,
    request: &Request,
    remote: &str,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    let status_map = &state.contract.http_status;
    if !state.limiter.allow_ip(remote, elapsed) {
        return Outcome::new(status_map.rate_limited);
    }
    match route(
        &request.method,
        &request.target,
        state.contract.channel_id.max_length,
    ) {
        Route::RegisterChannel => handle_register(state, request, now),
        Route::PutBlob(channel_id) => handle_put(state, request, &channel_id, now, elapsed),
        Route::GetBlob(channel_id) => handle_get(state, request, &channel_id, now, elapsed),
        Route::DeleteChannel(channel_id) => {
            handle_delete(state, request, &channel_id, now, elapsed)
        }
        Route::BadRequest => Outcome::new(status_map.malformed),
        Route::MethodNotAllowed => Outcome::new(status_map.method_not_allowed),
        Route::NotFound => Outcome::new(status_map.unknown_channel),
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
        StoreError::ChannelExists => status.malformed,
        StoreError::NotFound | StoreError::Expired => status.unknown_channel,
        StoreError::TooLarge => status.too_large,
        StoreError::MaxReadsReached => status.rate_limited,
    }
}

/// 从请求头取出 Bearer 令牌。
///
/// # 参数
/// - `request`: 已解析的请求。
///
/// # 错误
/// 不返回错误；缺失或格式不对返回 `None`。
fn bearer_token(request: &Request) -> Option<&str> {
    let value = request.header("authorization")?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
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
/// - `request`: 已解析的请求，令牌来自其 `Authorization` 头。
/// - `write`: `true` 用写令牌校验，`false` 用读令牌校验。
/// - `now`: 当前墙钟时间。
///
/// # 错误
/// 通道不存在或已过期返回 `unknown_channel` 状态码；令牌缺失或不匹配返回 `bad_token`。
fn authorize(
    state: &ServerState,
    channel_id: &str,
    request: &Request,
    write: bool,
    now: SystemTime,
) -> Result<(), u16> {
    let status_map = &state.contract.http_status;
    let credentials = state
        .store
        .credentials(channel_id, now)
        .ok_or(status_map.unknown_channel)?;
    let expected = if write {
        credentials.write_token_hash.as_str()
    } else {
        credentials.read_token_hash.as_str()
    };
    let presented = bearer_token(request).ok_or(status_map.bad_token)?;
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
    let value: Value = match serde_json::from_slice(body) {
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
        let value = match object.get(field).and_then(Value::as_str) {
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
/// - `request`: 注册请求。
/// - `now`: 当前墙钟时间。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_register(state: &ServerState, request: &Request, now: SystemTime) -> Outcome {
    let registration = match parse_registration(state, &request.body) {
        Ok(registration) => registration,
        Err(status) => return Outcome::new(status),
    };
    let channel_id = registration.channel_id.clone();
    match state.store.register(registration, now) {
        Ok(()) => Outcome::new(200).with_channel(&channel_id, None),
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(&channel_id, None),
    }
}

/// 处理密文写入。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `request`: 写入请求。
/// - `channel_id`: 路径中的通道 id。
/// - `now`: 当前墙钟时间。
/// - `elapsed`: 自服务启动以来的时长。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_put(
    state: &ServerState,
    request: &Request,
    channel_id: &str,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    let size = Some(request.body.len() as u64);
    if !state.limiter.allow_channel(channel_id, elapsed) {
        return Outcome::new(state.contract.http_status.rate_limited)
            .with_channel(channel_id, size);
    }
    if let Err(status) = authorize(state, channel_id, request, true, now) {
        return Outcome::new(status).with_channel(channel_id, size);
    }
    match state
        .store
        .put_ciphertext(channel_id, request.body.clone(), now)
    {
        Ok(()) => Outcome::new(200).with_channel(channel_id, size),
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(channel_id, size),
    }
}

/// 处理密文读取。
///
/// # 参数
/// - `state`: 服务端共享状态。
/// - `request`: 读取请求。
/// - `channel_id`: 路径中的通道 id。
/// - `now`: 当前墙钟时间。
/// - `elapsed`: 自服务启动以来的时长。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_get(
    state: &ServerState,
    request: &Request,
    channel_id: &str,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    if !state.limiter.allow_channel(channel_id, elapsed) {
        return Outcome::new(state.contract.http_status.rate_limited)
            .with_channel(channel_id, None);
    }
    if let Err(status) = authorize(state, channel_id, request, false, now) {
        return Outcome::new(status).with_channel(channel_id, None);
    }
    match state.store.read_ciphertext(channel_id, now) {
        Ok(blob_read) => {
            let size = blob_read.blob.len() as u64;
            Outcome {
                status: 200,
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
/// - `request`: 撤销请求。
/// - `channel_id`: 路径中的通道 id。
/// - `now`: 当前墙钟时间。
/// - `elapsed`: 自服务启动以来的时长。
///
/// # 错误
/// 不返回错误；所有失败都映射为契约状态码。
fn handle_delete(
    state: &ServerState,
    request: &Request,
    channel_id: &str,
    now: SystemTime,
    elapsed: Duration,
) -> Outcome {
    if !state.limiter.allow_channel(channel_id, elapsed) {
        return Outcome::new(state.contract.http_status.rate_limited)
            .with_channel(channel_id, None);
    }
    if let Err(status) = authorize(state, channel_id, request, true, now) {
        return Outcome::new(status).with_channel(channel_id, None);
    }
    match state.store.delete(channel_id, now) {
        Ok(()) => Outcome::new(200).with_channel(channel_id, None),
        Err(error) => Outcome::new(store_status(error, &state.contract.http_status))
            .with_channel(channel_id, None),
    }
}
