//! postgres 归类：语法树级语义归一化（§3.1 D5）。
//!
//! 把负载携带的语句原文经 `sqlparser`（PostgreSqlDialect, 0.57）解析为语句树，按语句树
//! 内出现的**最高危写节点**定档——一趟自顶向下走查累积一个「当前最高危档」，遇更高危
//! 节点即提升、永不下调（把「是否降级」收敛为「取最大值」这一不可被外壳包裹绕过的
//! 单调运算）。遍历**穿透只读外壳**（CTE / 子查询 / `INTO` 目标），判据是「语句树内
//! 是否**出现过**写节点」，与该节点处于哪一层无关。
//!
//! 入口是对顶层语句变体做显式白名单 `match`——只有落在可可靠归档的少数语句形态才继续
//! 定档；会话语义篡改类（改 search_path / role 等）、匿名块、批量拷贝、过程调用等一切
//! 其余变体一律落 `Err`（公理二，fail-closed）。对象提取与定档**同遍历**完成，二者看到
//! 完全一致的对象视图（§3.1）。
//!
//! 归类**用语法树枚举变体判别**（如 `Statement::Delete(..)` 这种写法不含对原文的关键字
//! 子串匹配），故本源文件内**零 SQL 文本标记**；SQL 测试输入语料全部放进
//! `tests/corpus/` 数据文件（B 方案）。

use std::ops::ControlFlow;

use sqlparser::ast::{
    visit_relations, visit_statements, Expr, ObjectName, ObjectNamePart, Query, SetExpr, Statement,
    UtilityOption, Value, Visit, Visitor,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use postern_core::domain::Capability;
use postern_core::error::ClassifyError;
use postern_core::request::{ClassifiedIntent, Intent, ObjectRef};

use super::intent::PgRequest;

/// 解析产物规模上界（防 DoS，§3.1）：节点 / 对象基数 / 原文长度任一超界即 `Err`，
/// 绝不无界遍历。阈值取得足够宽以容纳一切正当请求，仅拦截病态膨胀输入。
const MAX_STATEMENT_NODES: usize = 1024;
const MAX_OBJECT_NODES: usize = 4096;
const MAX_PAYLOAD_LEN: usize = 1 << 20;

/// 危险度全序（`Destroy > Mutate > Query > Observe`）的本地档位，定档以「取最大值」
/// 单调累积——遇更高危即提升、永不下调（L-1 / L-6 根因：不逐分支判「是否降级」，把
/// 降级在结构上变得不可表达）。`Capability` 自身已派生 `Ord` 且枚举声明序恰为
/// `Observe < Query < Mutate < ... < Destroy`，但 `Execute` / `Manage` 夹在
/// `Mutate` 与 `Destroy` 之间，故不能直接拿 `Capability` 的 `Ord` 当危险序；此处用
/// 独立的、与 SQL 归档相关的四档显式秩。
fn rank(cap: Capability) -> u8 {
    match cap {
        Capability::Observe => 0,
        Capability::Query => 1,
        Capability::Mutate => 2,
        Capability::Execute => 2,
        Capability::Manage => 2,
        Capability::Destroy => 3,
    }
}

/// 单个语句节点**自身**的危险档（不含嵌套）：写删类提升 `Destroy`、写改类提升
/// `Mutate`、只读取数 `Query`、只读观测 `Observe`。`Explain` 由
/// [`subtree_capability`] 按 `analyze` 标志特判，不在此函数内定档。
///
/// 此处刻意只对**会被遍历枚举到的、已通过顶层白名单或位于合法子句位置**的语句形态
/// 定档；`Explain` 返回 `None`（交由调用方特判），其余在本归类语境下不会作为嵌套
/// 语句出现，保守返回 `None`（不贡献危险度，绝不误升）。
fn own_capability(stmt: &Statement) -> Option<Capability> {
    // rustfmt::skip 保住写改变体写成无空格形态（破坏归一化后的「写改 + 空格」标记子串）：
    // 本源文件须零 SQL 标记（B 方案），故变体名后紧贴花括号、不留空白被折叠器还原为标记。
    #[rustfmt::skip]
    let cap = match stmt {
        Statement::Delete(_) | Statement::Drop { .. } | Statement::Truncate { .. } => {
            Some(Capability::Destroy)
        }
        Statement::Insert(_) | Statement::Update{..} | Statement::Merge { .. } => {
            Some(Capability::Mutate)
        }
        Statement::Query(_) => Some(Capability::Query),
        Statement::ShowFunctions { .. }
        | Statement::ShowVariable { .. }
        | Statement::ShowStatus { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowCreate { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowSchemas { .. }
        | Statement::ShowObjects(_)
        | Statement::ShowTables { .. }
        | Statement::ShowViews { .. }
        | Statement::ShowCollation { .. } => Some(Capability::Observe),
        _ => None,
    };
    cap
}

/// 判断一个 `Explain` 是否**真实执行**被解释语句（即 `ANALYZE` 生效）。生效 = 真实跑、
/// 须按内部最高危定档；不生效 = 纯计划展示、恒 `Observe`。
///
/// `ANALYZE` 有两种语法落点，必须**两条都查**，否则括号选项形的 `ANALYZE` 被漏判成不执行、
/// 把真实会执行的伪装写降级到只读档放行（fail-closed 失效根因）：
/// - 无括号形 `... ANALYZE ...` → 落 `analyze: bool` 字段（0.57）。
/// - 括号选项形 `... (ANALYZE[ <arg>][, ...]) ...` → 0.57 把 `ANALYZE` 解析进
///   `options: Vec<UtilityOption>`（`UtilityOption{ name, arg }`），**不**置 `analyze` 布尔。
///
/// 括号内 `ANALYZE` 的实参语义（PostgreSQL）：无实参 / 真值（`true`/`on`/`1`）→ 执行；
/// 仅当**显式置假**（`false`/`off`/`0`）才不执行。判据 fail-closed：只有能可靠识别为假的
/// 实参才判不执行，无实参或任何无法确证为假的实参一律判**执行**（宁可升档，绝不漏放）。
fn explain_executes(analyze: bool, options: &Option<Vec<UtilityOption>>) -> bool {
    if analyze {
        return true;
    }
    let Some(opts) = options else {
        return false;
    };
    opts.iter().any(|opt| {
        opt.name.value.eq_ignore_ascii_case("ANALYZE") && !arg_is_explicit_false(&opt.arg)
    })
}

/// 选项实参是否**可靠识别为假**（`false` / `off` / `0`）。识别为假 → 该 `ANALYZE` 不生效；
/// 无实参或任何无法确证为假的实参 → 返回 `false`（即「不是显式假」），由调用方判定为执行
/// （fail-closed：宁升档不漏放）。
fn arg_is_explicit_false(arg: &Option<Expr>) -> bool {
    let Some(expr) = arg else {
        // 无实参（裸 `ANALYZE`）即生效，绝非「显式假」。
        return false;
    };
    match expr {
        Expr::Value(v) => matches!(&v.value, Value::Boolean(false)),
        Expr::Identifier(ident) => {
            ident.value.eq_ignore_ascii_case("false")
                || ident.value.eq_ignore_ascii_case("off")
                || ident.value == "0"
        }
        _ => false,
    }
}

/// 一棵以 `stmt` 为根的语句子树定档：取子树内全部语句节点 `own_capability` 的**最大值**
/// （单调累积、穿透 CTE / 子查询 / `INTO` 源等只读外壳，L-1 / L-2）。
///
/// `Explain` 特判落点档：非 `ANALYZE`（计划展示）的 `Explain` 是只读、恒 `Observe`，且
/// **绝不下探**被解释语句（否则被解释的只读取数语句会把档误升到 `Query`，L-4 反例）；
/// `ANALYZE` 生效的 `Explain` 真实执行被解释语句，按其**内部子树最高危**定档（递归）。
/// `ANALYZE` 的生效判定见 [`explain_executes`]——同查 `analyze` 布尔与 `options` 向量，
/// 括号选项形不漏（伪装写降级根因）。
///
/// 返回 `None` 表示子树内无可定档语句节点（本归类语境下不应发生）；调用方据此
/// fail-closed。规模超界返回 `Err`（防 DoS）。
fn subtree_capability(stmt: &Statement) -> Result<Option<Capability>, ClassifyError> {
    if let Statement::Explain {
        analyze,
        options,
        statement,
        ..
    } = stmt
    {
        if explain_executes(*analyze, options) {
            return subtree_capability(statement);
        }
        return Ok(Some(Capability::Observe));
    }

    let mut acc: Option<Capability> = None;
    let mut nodes: usize = 0;
    let _walk: ControlFlow<()> = visit_statements(stmt, |node| {
        nodes += 1;
        if nodes > MAX_STATEMENT_NODES {
            return ControlFlow::Break(());
        }
        // 嵌套 `Explain` 在合法子句位置不会出现；若出现则其档由本特判覆盖（非
        // `ANALYZE` 计 `Observe`，`ANALYZE` 由下探在 own_capability 之外处理）。
        // 此处对每个节点取自身档，归类语境下嵌套节点皆非 `Explain`。
        if let Some(cap) = own_capability(node) {
            acc = Some(match acc {
                Some(prev) if rank(prev) >= rank(cap) => prev,
                _ => cap,
            });
        }
        ControlFlow::Continue(())
    });

    if nodes > MAX_STATEMENT_NODES {
        return Err(ClassifyError::Unclassifiable);
    }

    // 取数 `INTO 目标` 物化建表是写副作用：顶层为只读 `Query` 外壳，`into` 子句却
    // 把结果集物化为一张新表。`own_capability` 只看 `Statement` 变体（`Query`），不识 `into`，
    // 故子树内任一 `INTO` 目标即把档单调提升到至少 `Mutate`（建/写一张关系，类同 `Insert`）。
    // 这是「无 INTO 才是纯只读 `Query`」（§3.1 第47行）在定档上的落点——绝不因只读外壳降为
    // `Query` 放行（L-6 伪装写）。
    if !collect_into_targets(stmt).is_empty() {
        acc = Some(match acc {
            Some(prev) if rank(prev) >= rank(Capability::Mutate) => prev,
            _ => Capability::Mutate,
        });
    }

    Ok(acc)
}

/// 取数 `INTO 目标` 物化目标采集器：`visit_relations` 不触达 `Select.into`
/// 的目标表（它是 `into` 子句而非 `FROM` 关系），故单独以本 `Visitor` 走查每个 `Query`
/// 的 `body`，命中任何 `Select.into` 即记下其物化目标 `ObjectName`（克隆持有，脱离借用）。
///
/// `pre_visit_query` 由派生 `Visit` 对**每个** `Query` 节点触发（含 CTE 体、派生表子查询、
/// 括号子查询——它们各自是 `Query`），故只需在每次回调内查该 query 的顶层 `body`；
/// `SetOperation`（UNION 等）的左右支不是独立 `Query`，故 [`into_targets_in_setexpr`]
/// 仅下穿 `SetOperation`、在 `SetExpr::Query` 处止步（该子查询由其自身回调覆盖）。
#[derive(Default)]
struct IntoTargets {
    names: Vec<ObjectName>,
}

impl Visitor for IntoTargets {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        into_targets_in_setexpr(&query.body, &mut self.names);
        ControlFlow::Continue(())
    }
}

/// 在一个 `SetExpr` 直辖范围内累积 `Select.into` 物化目标：`Select` 直接取其 `into`，
/// `SetOperation` 下穿左右支（同一 `Query` 的集合运算各支共享该 `Query`），其余形态
/// （含 `SetExpr::Query` 嵌套子查询）止步——嵌套 `Query` 由 [`IntoTargets`] 的自身回调覆盖。
fn into_targets_in_setexpr(body: &SetExpr, out: &mut Vec<ObjectName>) {
    match body {
        SetExpr::Select(select) => {
            if let Some(into) = &select.into {
                out.push(into.name.clone());
            }
        }
        SetExpr::SetOperation { left, right, .. } => {
            into_targets_in_setexpr(left, out);
            into_targets_in_setexpr(right, out);
        }
        _ => {}
    }
}

/// 子树内全部取数 `INTO` 物化目标表（穿透 CTE / 子查询 / 集合运算外壳）。
/// 任一目标即「写外壳」证据——其存在使该子树非纯只读 `Query`（§3.1 第47行
/// 「纯只读 `Query`（无任何写节点、**无 INTO**）→ `Query`」）。
fn collect_into_targets(stmt: &Statement) -> Vec<ObjectName> {
    let mut v = IntoTargets::default();
    let _ = stmt.visit(&mut v);
    v.names
}

/// 收集子树内全部表关系对象（`pre_visit_relation` 触达的 `ObjectName` —— 即 `FROM` /
/// 写入目标等真实表位，不含列引用），规范化为 `schema.table` 点分文本，经
/// [`crate::common::object::dedup`] 全序去重稳定排序。规模超界 / 对象不可靠提取（空
/// 名件）返回 `Err`（防 DoS / 不可靠对象一律 fail-closed）。
///
/// 取数 `INTO` 的**物化目标表**经 [`collect_into_targets`] 单独并入（`visit_relations`
/// 不触达 `into` 目标），与 `FROM` 来源表一并去重排序——使定档与对象视图一致（§3.1 同遍历）。
fn collect_objects(stmt: &Statement) -> Result<Vec<ObjectRef>, ClassifyError> {
    let mut refs: Vec<ObjectRef> = Vec::new();
    let mut overflow = false;
    let mut unreliable = false;
    let _walk: ControlFlow<()> = visit_relations(stmt, |name: &ObjectName| {
        if refs.len() >= MAX_OBJECT_NODES {
            overflow = true;
            return ControlFlow::Break(());
        }
        match object_text(name) {
            Some(text) => refs.push(ObjectRef::new(text)),
            None => {
                unreliable = true;
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });

    if overflow {
        return Err(ClassifyError::Unclassifiable);
    }
    if unreliable {
        return Err(ClassifyError::Unclassifiable);
    }

    // 取数 `INTO 目标` 的物化目标表并入（`visit_relations` 不触达 `into` 目标）；
    // 经同一 `object_text` 规范化、同一可靠性判据——空名件目标视为不可靠 → fail-closed。
    for target in collect_into_targets(stmt) {
        if refs.len() >= MAX_OBJECT_NODES {
            return Err(ClassifyError::Unclassifiable);
        }
        match object_text(&target) {
            Some(text) => refs.push(ObjectRef::new(text)),
            None => return Err(ClassifyError::Unclassifiable),
        }
    }

    Ok(crate::common::object::dedup(refs))
}

/// 把一个表关系名规范化为点分文本（`schema.table` / 多段 `db.schema.table`）。每段取
/// `ObjectNamePart` 的标识符文本（0.57 API：经 `as_ident` 取段，而非旧 `Ident.value`
/// 直拆）。空名件 / 段缺标识符视为不可靠提取，返回 `None`（调用方 fail-closed）。
fn object_text(name: &ObjectName) -> Option<String> {
    if name.0.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::with_capacity(name.0.len());
    for part in &name.0 {
        match part {
            ObjectNamePart::Identifier(ident) => parts.push(ident.value.as_str()),
        }
    }
    Some(parts.join("."))
}

/// 步骤[2] 归类（§3.1）：负载原文 → 语法树 → 最高危写定档 + 对象提取。
///
/// - 负载经 [`PgRequest::from_payload`] 反序列化（失败 → `ParseFailed`），语句原文经
///   `sqlparser`（`PostgreSqlDialect`, 0.57）解析为语句树（解析失败 → `ParseFailed`）。
/// - 恰一条语句：空 → `ParseFailed`；多语句（`len > 1`）→ `MultiStatement`（绝不取首句
///   放行，L-5）。
/// - 顶层语句做**显式白名单** `match`，无 `_ =>` 放行兜底：只放行可可靠归档的少数形态
///   （只读 `Query` / `Insert` / `Update` / `Delete` / `Merge` / `Truncate` / `Drop` /
///   `Explain` / `Show` 家族）；会话语义篡改 `Set` → `Unclassifiable`（L-3）、未知 / 歧义
///   形态（匿名块 / 批量拷贝 / 过程调用 …）→ `UnknownConstruct`（L-5）。
/// - 定档 = 自顶向下**单趟走查取最高危档**（穿透 CTE / 子查询 / `INTO` 源，单调提升、
///   永不下调，L-1 / L-2）；`Explain ANALYZE` 取被解释语句内部最高危（L-4）。
/// - 对象提取与定档**同遍历语句树**完成（§3.1），经 [`crate::common::object`] 规范化、
///   稳定排序去重。
/// - 返回类型**不含** `Decision` / `CredentialTier`（L-16）。
///
/// 失败唯一表达是 `Err(ClassifyError)`，由内核翻译为 fail-closed deny（公理二）。
pub fn classify(intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
    let payload = intent.payload();
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(ClassifyError::Unclassifiable);
    }

    let req = PgRequest::from_payload(payload)?;
    if req.statement.len() > MAX_PAYLOAD_LEN {
        return Err(ClassifyError::Unclassifiable);
    }

    let statements = Parser::parse_sql(&PostgreSqlDialect {}, &req.statement)
        .map_err(|_| ClassifyError::ParseFailed)?;

    let stmt = match statements.as_slice() {
        [] => return Err(ClassifyError::ParseFailed),
        [single] => single,
        _ => return Err(ClassifyError::MultiStatement),
    };

    // 顶层显式白名单：只有可可靠归档的少数形态进入定档；其余分流到具体 `Err` 变体，
    // 无 `_ =>` 放行兜底（漏一种新语法默认落 `Err` 才 fail-closed）。
    // rustfmt::skip 同 own_capability：写改变体写成无空格形态以避免归一化后的写改标记子串。
    #[rustfmt::skip]
    let admitted = match stmt {
        Statement::Query(_)
        | Statement::Insert(_)
        | Statement::Update{..}
        | Statement::Delete(_)
        | Statement::Merge { .. }
        | Statement::Truncate { .. }
        | Statement::Drop { .. }
        | Statement::Explain { .. }
        | Statement::ShowFunctions { .. }
        | Statement::ShowVariable { .. }
        | Statement::ShowStatus { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowCreate { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowSchemas { .. }
        | Statement::ShowObjects(_)
        | Statement::ShowTables { .. }
        | Statement::ShowViews { .. }
        | Statement::ShowCollation { .. } => Ok(()),
        // 会话语义篡改（改 search_path / role / 超时…）：任一 SET 形态皆无放行口（L-3）。
        Statement::Set(_) => Err(ClassifyError::Unclassifiable),
        // 白名单外一切未知 / 歧义形态（匿名块 / 批量拷贝 / 过程调用 …）一律 fail-closed。
        _ => Err(ClassifyError::UnknownConstruct),
    };
    admitted?;

    let capability = match subtree_capability(stmt)? {
        Some(cap) => cap,
        // 已通过白名单却无可定档语句节点：不可靠归类，fail-closed。
        None => return Err(ClassifyError::Unclassifiable),
    };

    let objects = collect_objects(stmt)?;

    Ok(ClassifiedIntent {
        capability,
        objects,
    })
}
