//! 插件登记册：把下游 crate 的 trait 实现装配成数据面内核可注入的集合。
//!
//! boot 阶段构造一次，按只读视图共享给数据面（kernel/connpool/外壳）。控制面绝不被
//! 注入连接池/Sanitizer，PolicyRepo 句柄绝不进数据面注入集合（红线 7.2-2）——本登记册
//! 只承载数据面所需的插件实现，控制面的写句柄独立持有。
//!
//! 本单元只承载**适配器登记册**与**传输登记册**两张表及其选型助手；Authenticator /
//! ConditionPredicate 登记册由 `core::Evaluator` 自持（boot 构造并交付），不在此处登记。
//!
//! 设计（06 §8）：两张表均以 `&'static str` 为键的确定性 `BTreeMap`（非 `HashMap`）——
//! 适配器以 `protocol()`、传输以 `kind()` 为键。选型未命中返回 `None`（上游映射为
//! fail-closed deny，不降级）。登记册一经 boot 装配即只读，表本身无内部可变性，仅以
//! 共享引用暴露。本单元为纯数据结构，无 async。

use std::collections::BTreeMap;

use postern_core::plugin::{Adapter, Transport};

/// 适配器登记册（06 §8）。
///
/// 以协议键（`postgres` / `docker_logs` / `http`）定位**唯一解释者** [`Adapter`] 实现的
/// 确定性表（`BTreeMap<&'static str, Box<dyn Adapter>>`）。boot 用真实下游实现（
/// `PostgresAdapter` / `DockerLogsAdapter` / `HttpAdapter`）按各自 `protocol()` 装配；
/// 一经装配即只读，仅以共享引用暴露（表本身无内部可变性）。
pub struct AdapterRegistry {
    /// 协议键 → 适配器实现。确定性容器：`BTreeMap` 而非 `HashMap`（workspace 确定性纪律）。
    table: BTreeMap<&'static str, Box<dyn Adapter>>,
}

impl AdapterRegistry {
    /// 从一组适配器实现装配登记册，**以各自 `protocol()` 为键**（06 §8）。
    ///
    /// boot 传入真实下游实现箱化为 trait 对象的集合；同协议重复以末次为准（boot 不应
    /// 提供冲突键，装配序由调用方保证）。装配后表只读。
    pub fn new(adapters: Vec<Box<dyn Adapter>>) -> Self {
        let table = adapters
            .into_iter()
            .map(|adapter| (adapter.protocol(), adapter))
            .collect();
        Self { table }
    }

    /// 选型助手：按协议键定位适配器（06 §8）。
    ///
    /// 未命中返回 `None`——上游据此映射为 fail-closed deny（不降级、不改路）。命中返回
    /// 只读引用（登记册只读，不交出所有权）。
    pub fn adapter_for(&self, protocol: &str) -> Option<&dyn Adapter> {
        self.table.get(protocol).map(Box::as_ref)
    }

    /// 已登记协议键的确定性升序迭代（`BTreeMap` 序）。
    pub fn protocols(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.table.keys().copied()
    }

    /// 已登记适配器数量。
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// 登记册是否为空。
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

/// 传输登记册（06 §8）。
///
/// 以形态键（`direct`，feature 门控的 `ssh` / `ssm`）定位 [`Transport`] 实现的确定性表
/// （`BTreeMap<&'static str, Box<dyn Transport>>`）。boot 用真实下游实现（`DirectTransport`）
/// 按 `kind()` 装配；一经装配即只读，仅以共享引用暴露（表本身无内部可变性）。
pub struct TransportRegistry {
    /// 形态键 → 传输实现。确定性容器：`BTreeMap` 而非 `HashMap`（workspace 确定性纪律）。
    table: BTreeMap<&'static str, Box<dyn Transport>>,
}

impl TransportRegistry {
    /// 从一组传输实现装配登记册，**以各自 `kind()` 为键**（06 §8）。
    ///
    /// boot 传入真实下游实现箱化为 trait 对象的集合（`DirectTransport` 在 `direct` 键；
    /// ssh/ssm 按 feature 门控加入）。装配后表只读。
    pub fn new(transports: Vec<Box<dyn Transport>>) -> Self {
        let table = transports
            .into_iter()
            .map(|transport| (transport.kind(), transport))
            .collect();
        Self { table }
    }

    /// 选型助手：按形态键定位传输（06 §8）。
    ///
    /// 未命中返回 `None`——上游据此映射为 fail-closed deny（不降级、不改路）。命中返回
    /// 只读引用（登记册只读，不交出所有权）。
    pub fn transport_for(&self, kind: &str) -> Option<&dyn Transport> {
        self.table.get(kind).map(Box::as_ref)
    }

    /// 已登记形态键的确定性升序迭代（`BTreeMap` 序）。
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.table.keys().copied()
    }

    /// 已登记传输数量。
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// 登记册是否为空。
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}
