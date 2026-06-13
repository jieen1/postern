//! 固定宽度时间戳生成：policy.db 时间列与审计 `ts` 的**唯一**格式化点。
//!
//! [`format`] 把一个墙钟 [`Timestamp`]（Unix 毫秒）渲染为恒
//! `YYYY-MM-DDTHH:MM:SS.sssZ` 的文本：恒 UTC、恒 `Z` 后缀、恒 3 位毫秒、长度恒 24。
//! 保证文本字典序 == 时间先后序（TTL/sweeper 的 `< now` 判定不错序，§7-12）。

use postern_core::domain::Timestamp;

/// 固定宽度时间戳文本的恒定字节长度（`YYYY-MM-DDTHH:MM:SS.sssZ`）。
pub const TIMESTAMP_LEN: usize = 24;

/// 一天的毫秒数。
const MS_PER_DAY: u64 = 86_400_000;

/// 把 [`Timestamp`]（Unix 毫秒）格式化为固定宽度 UTC ISO-8601 文本。
///
/// 输出恒 `YYYY-MM-DDTHH:MM:SS.sssZ`、长度恒 [`TIMESTAMP_LEN`]、恒 UTC、恒 `Z`
/// 结尾、恒 3 位毫秒。字典序与时间序一致。不读系统时钟（`ts` 由调用方传入）、
/// 自带纯算法的历法换算（不引第三方时间库）。
pub fn format(ts: Timestamp) -> String {
    let total_ms = ts.as_unix_ms();
    let days = (total_ms / MS_PER_DAY) as i64;
    let ms_of_day = total_ms % MS_PER_DAY;

    let millis = ms_of_day % 1000;
    let secs_of_day = ms_of_day / 1000;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    let (year, month, day) = civil_from_days(days);

    // 固定宽度、零填充：YYYY-MM-DDTHH:MM:SS.sssZ（恒 24 字节）。
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    )
}

/// 从 Unix 纪元起的天数换算公历 `(year, month, day)`。
///
/// Howard Hinnant 的 `civil_from_days` 算法（纯整数、无第三方依赖）：以 3 月为
/// 年首消化闰年，结果再平移回 1 月。对 [`format`] 的输入域（Unix 毫秒 ≥ 0）恒成立。
fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u64; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u64; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}
