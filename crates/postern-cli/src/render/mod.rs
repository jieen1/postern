//! 结果渲染面（骨架占位）。
//!
//! 职责（07-postern-cli §3.3、F-4/F-5/L-4）：把 daemon 控制面响应按信封类别分三支渲染，
//! 每支只转述、不加工——`Page<T>` 信封（表格）、单条 DTO（纵向字段表）、统一
//! `{error:{code,message}}` 错误信封（逐字符原样）。雪花 id 在 JSON 中恒为字符串，
//! 渲染原样作字符串展示、绝不数值化（>2^53 不丢精度，F-5）。机器形态 `--format jsonl`
//! 逐行打印后端已分页的 `items`，不做客户端重排或聚合。
//!
//! 输出只转述事实（公理六、L-4/L-6）：不展开底层原因、不补"建议"话术、不改写 daemon
//! 已脱敏的常量 `message`。反序列化失败即报错非零退出，不猜测性补全（L-3）。
//!
//! 子模块：`envelope`（信封三分支分流）、`table`（`Page<T>` 对齐表格 + 分页页脚）、
//! `deny_view`（`DenyResponse`/`DeniedFacts` 拒绝事实渲染）。
pub mod deny_view;
pub mod envelope;
pub mod table;

pub use envelope::Format;
