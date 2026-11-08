//! File: meito CLI 外壳（M1 stub）。
//!
//! M1 只验证外壳能依赖同一份 `meito-core` 契约；命令解析与业务在后续阶段实现。
//! 本文件允许使用 `println!`，因为它是 CLI 展示层，不是日志层。

use meito_core::CONTRACT_VERSION;

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("meito {}", CONTRACT_VERSION);
    }
}
