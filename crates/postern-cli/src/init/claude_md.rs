//! `CLAUDE.md` 片段本地渲染（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.7 步骤 5，F-9，L-6）：向导收尾时把"该 Principal 经哪个 MCP
//! 端点（`data.sock` 的 `/mcp`）、有哪些已授权动词"渲染为可粘贴文本。这是纯客户端文本
//! 渲染便利、零安全逻辑。
//!
//! 红线（§3.7，公理六、L-6）：片段内容**只来自控制面回报的授权事实**——已授权动词集为空
//! 就如实写"暂无已授权动词"，非空就只列该集合；**绝不**附任何固定引导话术或编造建议。
//! 机器部分可验证"输入 = 授权事实、无额外固定文案串"（构造签名检查，有固定话术模板即不
//! 过）；语义余量（措辞确无编造引导话术）须人工评审（L-6 标注）。

/// 控制面回报的**授权事实**（F-9 的输入）：该 Principal 经哪个 MCP 端点、有哪些已授权
/// 动词。片段渲染的**唯一**内容来源——CLI 不补全、不推测、不编造（公理六、L-6）。
///
/// 红线（L-6）：本结构**只**承载控制面回报的两类事实（端点位置 + 已授权动词集），不携任何
/// "固定引导话术"字段——使"片段输入 = 授权事实"成为构造签名可核的事实。`verbs` 为空即如实
/// 表"暂无已授权动词"（步骤 8 尚未绑定时），非空即恰列该集合（顺序原样取自回报）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationFacts {
    /// 该 Principal 的 MCP 端点位置（事实，`data.sock` 的 `/mcp`）。原样取自控制面回报，
    /// CLI 不硬编码、不替换。
    pub mcp_endpoint: String,
    /// 该 Principal 当前**已授权**动词集（事实，取自控制面回报的授权快照）。空集 = 尚无
    /// 任何绑定 → 片段如实写"暂无已授权动词"；非空 = 片段恰列该集合、不增不减。
    pub verbs: Vec<String>,
}

/// 把控制面回报的授权事实渲染为可粘贴的 `CLAUDE.md` 片段文本（§3.7 步骤 5，F-9/L-6）。
///
/// 渲染契约（L-6 机器部分）：
/// - 端点位置原样落入（取自 `facts.mcp_endpoint`，不硬编码）。
/// - `facts.verbs` 为空 → 如实表"暂无已授权动词"（faithful no-op fact），**不**附任何
///   "建议下一步" / "你可以…" 类编造引导话术。
/// - `facts.verbs` 非空 → 恰列该集合，每个动词逐字取自输入，**不**增删、**不**补充集合外
///   的任何动词或固定话术模板。
///
/// 红线（公理六、L-6）：本函数输出的每一处可变内容都源自 `facts`——除"暂无已授权动词"这一
/// 对空集的**如实陈述**与纯结构性标签外，**不**得引入任何输入授权事实集之外的固定散文串
/// （有固定引导话术模板即违反 L-6 机器部分构造签名）。
pub fn render_claude_md(facts: &AuthorizationFacts) -> String {
    // 端点位置事实：原样取自输入授权事实，CLI 不硬编码、不替换。
    let mut fragment = format!("MCP endpoint: {}\n", facts.mcp_endpoint);

    if facts.verbs.is_empty() {
        // 空集 = 尚无任何绑定：如实陈述这一事实，**不**附任何"建议下一步" / "你可以…"类
        // 编造引导话术（公理六、L-6）。
        fragment.push_str("Authorized verbs: no authorized verbs yet\n");
    } else {
        // 非空集 = 恰列该集合，每个动词逐字取自输入、顺序原样，**不**增删、**不**补全集合外
        // 任何动词或固定话术（L-6）。
        fragment.push_str("Authorized verbs:\n");
        for verb in &facts.verbs {
            fragment.push_str("- ");
            fragment.push_str(verb);
            fragment.push('\n');
        }
    }

    fragment
}
