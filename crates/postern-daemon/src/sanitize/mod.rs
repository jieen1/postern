//! 出口脱敏子域。
//!
//! 数据面所有出站载荷（响应体、错误词汇、审计可见字段）在跨边界前统一过一次脱敏，
//! 收口于此。净化失败即 fail-closed：宁可丢弃也不泄露（公理二）。归池前的会话净化亦
//! 复用本子域的 scrubber。
//!
//! 本波次为骨架：子模块声明 + 出口脱敏器类型再导出，零逻辑。

pub mod scrubber;

pub use scrubber::{DaemonSanitizer, DaemonStreamScrubber};
