//! 全部插件 trait 定义（依赖反转：各面 crate 实现，core 只声明形状）。

pub mod audit;
pub mod auth;
pub mod channel;
pub mod condition;
pub mod policy;
pub mod sanitize;

pub use audit::{AuditEvent, AuditSink};
pub use auth::Authenticator;
pub use channel::{
    Adapter, CapabilitySurface, Channel, CredentialProvider, RawResponse, Transport,
};
pub use condition::ConditionPredicate;
pub use policy::PolicyView;
pub use sanitize::{MaskRule, SanitizedResponse, Sanitizer, StreamScrubber};
