//! 二进制 posternd 的库面：外壳、控制面、连接管理、数据面内核、启动序列的组装点。
//!
//! daemon 是依赖图唯一可依赖全部下游库（core/store/secrets/transports/adapters）的
//! 节点；本库把它们的类型/trait/实现装配成两个 UDS 服务端（data.sock 数据面、
//! control.sock 控制面）。子域划分见模块文档 06 §2，子模块各自承载一个子域。
//!
//! 顶层模块在此声明并冻结；任何新增子域以子模块文件形式落在对应目录下。
//!
//! 进程级装配契约（模块文档 06 §1 唯一组装点 / §3.8 并发线程模型 / §8 DoD）落在本库面的
//! [`assemble`] 缝。它把 boot 产出的 [`boot::BootReport`] 转译为数据面 router、控制面 router、
//! sweeper 周期任务三个相互独立的 spawn，并把一次 boot `Err` 映射为非零进程退出码（`main`
//! 据此非零退出、data.sock 不 serving）。本缝以可注入的 [`assemble::PlaneSpawner`] 暴露三处
//! spawn 接线点，使「三平面各自独立 spawn」与「boot Err 到非零退出」在集成测试里以 Fake
//! 见证，而无需起真实二进制（lib 可测、bin 极薄）。`anyhow` 仅 main.rs，本库面只用
//! `DaemonError`/`thiserror`。
#![forbid(unsafe_code)]

pub mod assemble;
pub mod boot;
pub mod connpool;
pub mod control;
pub mod error;
pub mod identity;
pub mod kernel;
pub mod registry;
pub mod sanitize;
pub mod shells;
pub mod sweeper;
