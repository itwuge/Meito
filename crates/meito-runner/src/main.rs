//! File: meito runner 侧监督进程（M1 stub）。
//!
//! M1 只冻结接口；预检、frpc、ttyd/noVNC、watchdog 与 collect 在后续阶段实现。

fn main() {
    std::hint::black_box(meito_core::CONTRACT_VERSION);
}
