//! 全部插件 trait 定义（依赖反转：各面 crate 实现，core 只声明形状）。

pub mod audit;
pub mod channel;
pub mod sanitize;
