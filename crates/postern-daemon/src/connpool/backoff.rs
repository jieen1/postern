//! 退避策略：连接新建失败时的重试节流（§3.5 健康与退避状态机）。
//!
//! 每池键一个状态机：通路死亡 → **指数退避**（基数 1s、上限 60s、带抖动）择时重建，退避期
//! 内对该键的 `acquire` 走 deny 或有界等待而非风暴重连。退避不掩盖失败，只控制重试节奏；
//! 退避有上界，绝不无界增长。**非长连接 transport 不入池、不进退避**（即建即用即弃）。
//!
//! 抖动来源：无 `rand` 依赖（白名单外），故由失败档位**确定性**派生一份伪随机偏移，
//! 落在 `[0, exp/4)` 内叠加到指数基值上。指数基值逐档翻倍（封顶 `cap`），抖动幅度恒
//! 不足一档增量，故合成时延**单调不减**且恒落 `[base, cap]`（封顶档直接取 `cap`、不再抖动，
//! 避免越界与回退）。

use std::time::Duration;

/// 退避基数（首次退避时长）：1 秒。
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// 退避上限（封顶时长）：60 秒。指数退避增长到此即不再翻倍（带抖动后亦不超此上界）。
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// 单池键的连接重试退避状态机（指数、基数 1s、上限 60s、带抖动）。
#[derive(Default)]
pub struct Backoff {
    /// 已连续失败次数（决定指数档位）；成功重建后清零。
    attempts: u32,
}

impl Backoff {
    /// 构造退避器（初始无失败，下次失败从基数起退）。
    pub fn new() -> Self {
        Self { attempts: 0 }
    }

    /// 登记一次连接新建失败 → 推进退避档位（指数翻倍，封顶 60s）。
    pub fn record_failure(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    /// 连接重建成功 → 清零退避档位（下次失败重新从基数起退）。
    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    /// 返回当前档位下一次重试前的等待时长（基数×2^档位、封顶 60s、叠加抖动）。
    /// 无失败档位（刚 `reset` / `new`）返回 `None`（可立即重试，不退避）。
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.attempts == 0 {
            return None;
        }

        let base_ms = BACKOFF_BASE.as_millis() as u64;
        let cap_ms = BACKOFF_CAP.as_millis() as u64;

        // 指数基值 base * 2^(attempts-1)，逐档翻倍；溢出或越上限即钳到 cap。
        let shift = self.attempts - 1;
        let exp_ms = base_ms
            .checked_shl(shift)
            .and_then(|v| v.checked_mul(1))
            .map(|v| v.min(cap_ms))
            .unwrap_or(cap_ms);

        // 封顶档：直接取 cap、不再叠抖动（避免越界与单调回退）。
        if exp_ms >= cap_ms {
            return Some(BACKOFF_CAP);
        }

        // 未封顶档：在 [0, exp/4) 内确定性派生抖动叠加。抖动幅度 < 一档增量（exp），
        // 故合成时延对档位单调不减、且恒 < 下一档基值 ≤ cap。
        let jitter_span = exp_ms / 4;
        let jitter = if jitter_span == 0 {
            0
        } else {
            deterministic_jitter(self.attempts) % jitter_span
        };
        let total_ms = (exp_ms + jitter).min(cap_ms);
        Some(Duration::from_millis(total_ms))
    }
}

/// 由失败档位派生一份确定性伪随机偏移（无 `rand` 依赖）。同一档位恒得同值，
/// 仅用于在退避基值上叠加抖动、打散同步重连，不承载安全语义。
fn deterministic_jitter(attempts: u32) -> u64 {
    // 简单确定性混淆（splitmix64 风格的一轮），足以打散档位间的相位。
    let mut z = (attempts as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
