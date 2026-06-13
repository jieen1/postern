//! 接入向导面（骨架占位）。
//!
//! 职责（07-postern-cli §3.7，F-8/F-9，L-8/L-12）：`init` 是 §3 唯一"一条命令、多次顺序
//! 往返"的形态——把"资源接入"编排为一台**呈现—圈选—回写**的人机状态机：建资源
//! `POST /v1/resources` → 触发 `POST /v1/resources/{code}/discover` → 呈现 daemon 报回的
//! 候选对象与缺口清单 → 圈选回写 tier / 细则 / 绑定 → （可选）本地生成 `CLAUDE.md` 片段。
//!
//! 全部判定权在 daemon，CLI 只发起与呈现（§3.7，L-8/L-12）：探测执行在 daemon 侧
//! （`Adapter::discover`，CLI 不直连资源、不解析协议）；tier 子集校验在 daemon；缺口"是否
//! 消解"由 daemon 在后续调用裁决——CLI **不**自行比对"声明 ⊆ 真实权限"、**不**自行判定
//! 缺口消解、**不**触达数据面 `postern_surface`（CONS-20，接入侧 discover 是控制面端点）。
//!
//! 子模块：`wizard`（呈现—圈选—回写状态机编排）、`claude_md`（可选 `CLAUDE.md` 片段本地
//! 渲染，纯客户端文本便利、零安全逻辑）。
pub mod claude_md;
pub mod wizard;
