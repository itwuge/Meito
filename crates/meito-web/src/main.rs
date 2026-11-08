//! File: meito 网页外壳（M1 stub）。
//!
//! M1 只验证网页外壳与本地外壳依赖同一份 `meito-core` 契约。
//! 静态单页在 `web/`；WebSocket 只承载控制/信令，不承载终端字节。

fn main() {
    std::hint::black_box(meito_core::CONTRACT_VERSION);
}
