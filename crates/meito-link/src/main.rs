//! File: meito 协调服务（M1 stub）。
//!
//! M1 只冻结接口；通道注册、密文下发与限流在后续阶段实现。
//! 本进程只交换 peer 信息与密文，禁止承载会话字节。

fn main() {
    std::hint::black_box(meito_core::CONTRACT_VERSION);
}
