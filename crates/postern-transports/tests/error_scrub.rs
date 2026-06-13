//! 传输面错误脱敏行为测试 —— L-7 红线 7.2-1（跨 crate 边界错误先脱敏）。
//!
//! 钉死「越出 `postern-transports` 边界前已脱敏」（§5.1、第 7 节红线 1，对齐 §8 L-7）：
//! 喂底层一个含真实地址明文的 [`InnerFault`] 诊断载体（如 `connection refused to
//! 10.0.3.17`），断言 [`sanitize`] 外抛的 `core::TransportError` 的 `Display` / `Debug`
//! 渲染串**均不含**该地址子串、为常量错误码语义、不内插任何外部输入；并逐类钉死
//! 映射完整性（连接 / 握手 / 通路死亡 / 关闭 → 对应四变体）。
//!
//! 雷区遵守：本单元只处理错误，完全不触机密类型 —— 不构造 / 不 import 任何机密侧
//! 解析结果或资源凭据类型，亦不构造连接来源标识（SEC_CONSTRUCTION_SITES 仅 secrets /
//! 外壳 listener）；无裸数据库写标记；不依赖兄弟单元。测试样例地址 `10.0.3.17` 只在
//! 本测试文件内出现，断言其不外泄。

use postern_core::error::{Stage, TransportError};
use postern_transports::error::{sanitize, InnerFault};

/// 测试样例真实地址明文 —— 只在本测试文件内出现，断言脱敏后绝不外泄。
const SECRET_ADDR: &str = "10.0.3.17";
/// 底层 IO 错误串的样例形状（携带真实地址明文）。
const RAW_IO_DETAIL: &str = "connection refused to 10.0.3.17";

// §8 L-7：底层报 "connection refused to 10.0.3.17" → sanitize 后 TransportError 的
// Display 渲染串不含子串 10.0.3.17。
#[test]
fn display_of_sanitized_connect_fault_omits_real_address() {
    let err = sanitize(InnerFault::connect(RAW_IO_DETAIL));
    let rendered = format!("{err}");
    assert!(
        !rendered.contains(SECRET_ADDR),
        "Display 串泄漏真实地址明文: {rendered:?}",
    );
}

// §8 L-7：底层报 "connection refused to 10.0.3.17" → sanitize 后 TransportError 的
// Debug 渲染串不含子串 10.0.3.17（Debug 路径同样脱敏，不拼 InnerFault::Debug）。
#[test]
fn debug_of_sanitized_connect_fault_omits_real_address() {
    let err = sanitize(InnerFault::connect(RAW_IO_DETAIL));
    let rendered = format!("{err:?}");
    assert!(
        !rendered.contains(SECRET_ADDR),
        "Debug 串泄漏真实地址明文: {rendered:?}",
    );
}

// §8 L-7：脱敏输出为**常量化错误码语义** —— Display 恰为 core 固定的常量文案，
// 不内插任何外部输入（detail 一律丢弃，连接类钉 "transport connect failed"）。
#[test]
fn sanitized_connect_display_is_the_constant_code_string() {
    let err = sanitize(InnerFault::connect(RAW_IO_DETAIL));
    assert_eq!(format!("{err}"), "transport connect failed");
}

// §8 L-7 映射完整性：连接类底层失败 → ConnectFailed（恰为该变体，非他变体）。
#[test]
fn connect_fault_maps_exactly_to_connect_failed() {
    assert_eq!(
        sanitize(InnerFault::connect(RAW_IO_DETAIL)),
        TransportError::ConnectFailed
    );
}

// §8 L-7 映射完整性：握手 / 会话协商类 → HandshakeFailed。
#[test]
fn handshake_fault_maps_exactly_to_handshake_failed() {
    let detail = format!("ssh handshake failed against {SECRET_ADDR}");
    assert_eq!(
        sanitize(InnerFault::handshake(detail)),
        TransportError::HandshakeFailed
    );
}

// §8 L-7 映射完整性：通路死亡类（保活僵死 / 桥接泵退出 / 对端 RST）→ ChannelClosed。
#[test]
fn channel_fault_maps_exactly_to_channel_closed() {
    let detail = format!("keepalive lost on channel to {SECRET_ADDR}");
    assert_eq!(
        sanitize(InnerFault::channel(detail)),
        TransportError::ChannelClosed
    );
}

// §8 L-7 映射完整性：关闭 / 释放底层隧道报错类 → CloseFailed。
#[test]
fn close_fault_maps_exactly_to_close_failed() {
    let detail = format!("close failed: peer {SECRET_ADDR} unreachable");
    assert_eq!(
        sanitize(InnerFault::close(detail)),
        TransportError::CloseFailed
    );
}

// §8 L-7：**逐变体**都不泄真实地址 —— 四类失败的 detail 各自携带地址明文，脱敏后
// Display / Debug 渲染串均不含该子串（脱敏是全收口、非仅连接路径生效）。
#[test]
fn every_fault_kind_omits_real_address_in_both_renderings() {
    let detail = format!("io error talking to {SECRET_ADDR}");
    let faults = [
        InnerFault::connect(detail.clone()),
        InnerFault::handshake(detail.clone()),
        InnerFault::channel(detail.clone()),
        InnerFault::close(detail.clone()),
    ];
    for fault in faults {
        let err = sanitize(fault);
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(!display.contains(SECRET_ADDR), "Display 泄漏: {display:?}");
        assert!(!debug.contains(SECRET_ADDR), "Debug 泄漏: {debug:?}");
    }
}

// §8 L-7：凭据明文同样被丢弃 —— detail 携带凭据样例串，脱敏输出不内插之
// （脱敏丢的是「任何注入的真实地址 / 凭据明文」，不止 IP）。
#[test]
fn sanitized_output_omits_credential_plaintext_in_detail() {
    let detail = "auth via private-key BEGIN-OPENSSH-PRIVATE-KEY material";
    let err = sanitize(InnerFault::handshake(detail));
    let rendered = format!("{err}{err:?}");
    assert!(
        !rendered.contains("PRIVATE-KEY"),
        "脱敏输出内插了凭据明文: {rendered:?}",
    );
    assert_eq!(format!("{err}"), "transport handshake failed");
}

// §8 L-7：脱敏后的 TransportError 归因到 [7b] 传输阶段（核心承载的 stage 判别，
// 错误码语义完整 —— 脱敏不破坏阶段归因）。
#[test]
fn sanitized_error_attributes_to_transport_stage() {
    let err = sanitize(InnerFault::connect(RAW_IO_DETAIL));
    assert_eq!(err.stage(), Stage::Transport);
}
