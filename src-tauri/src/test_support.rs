//! 测试专用辅助工具。
//!
//! Rust 单元测试默认在同一进程内多线程并行执行，而 `std::env::set_var` /
//! `remove_var` 作用于整个进程；所有需要临时修改环境变量（HOME / XDG_* /
//! OPENCODE_* 等）的测试都必须持有 [`env_lock`] 返回的全局锁，避免并行测试
//! 之间互相污染环境。

use std::sync::{Mutex, MutexGuard, OnceLock};

/// 获取全局环境变量锁。
///
/// 锁中毒（前一个持锁测试 panic）时继续复用内部值，保证单个测试失败
/// 不会级联毒化后续所有依赖该锁的测试。
pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}
