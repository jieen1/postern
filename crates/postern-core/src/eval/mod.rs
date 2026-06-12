//! 纯函数求值管线：零 IO，失败路径一律 fail-closed。

pub mod deny;
pub mod evaluator;
pub mod trace;
