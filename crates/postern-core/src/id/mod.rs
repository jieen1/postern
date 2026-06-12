//! Unified snowflake id generation — the single id source for the whole workspace.
//!
//! Bit layout (64 bit, top bit always 0):
//! `[1 bit unused][41 bit milliseconds since 2026-01-01T00:00:00Z][10 bit node][12 bit sequence]`
//!
//! `IdGen` is the only stateful facility in `postern-core`. Issuance is serialized
//! through an internal `Mutex` (short critical section: read clock, compare millisecond,
//! advance sequence, assemble id). Invariants (module design §3.5):
//! - within one millisecond the sequence strictly increases, never wraps;
//! - sequence exhaustion (4096 ids in one millisecond) spins until the wall clock
//!   enters the next millisecond, then restarts at sequence 0;
//! - a clock reading earlier than the last issued millisecond is a clock rollback:
//!   generation is refused with `IdGenError::ClockRollback` (fail-closed — never
//!   re-issue under the stale millisecond, never panic);
//! - a clock reading at or beyond epoch + 2^41 ms does not fit the 41-bit
//!   timestamp field: generation is refused with `IdGenError::TimestampOverflow`
//!   (never masked into a wrapped, colliding id);
//! - the clock source is injectable for tests.
//!
//! JSON serialization of `SnowflakeId` is ALWAYS a decimal string in both directions
//! (JS `Number` is 53-bit safe only).

use std::fmt;
use std::sync::Mutex;

/// The snowflake epoch `2026-01-01T00:00:00Z`, as milliseconds since the Unix epoch.
pub const EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// Largest node number representable in the 10-bit node field.
pub const MAX_NODE: u16 = 1023;

/// Largest sequence number representable in the 12-bit sequence field
/// (4096 ids per node per millisecond).
pub const MAX_SEQUENCE: u16 = 4095;

/// Bit offset of the 41-bit timestamp field (`NODE_BITS + SEQUENCE_BITS`).
const TIMESTAMP_SHIFT: u32 = 22;
/// Bit offset of the 10-bit node field (`SEQUENCE_BITS`).
const NODE_SHIFT: u32 = 12;
/// Mask of the 41-bit timestamp field (`(1 << 41) - 1`).
const TIMESTAMP_MASK: u64 = 0x1FF_FFFF_FFFF;
/// Mask of the 10-bit node field (`(1 << 10) - 1`).
const NODE_FIELD_MASK: u64 = 0x3FF;
/// Mask of the 12-bit sequence field (`(1 << 12) - 1`).
const SEQUENCE_FIELD_MASK: u64 = 0xFFF;

/// Injectable wall-clock source. Production uses [`SystemClock`]; tests inject fakes.
pub trait Clock: Send + Sync {
    /// Current wall clock as milliseconds since the Unix epoch.
    fn now_unix_ms(&self) -> u64;
}

/// Production clock backed by `std::time::SystemTime`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> u64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            // u64 milliseconds overflow only ~584 million years from now; saturate
            // rather than panic (the generator then refuses via its own checks).
            Ok(elapsed) => u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            // Wall clock before the Unix epoch: report 0, which the generator
            // refuses as `ClockBeforeEpoch` (fail-closed, never a panic).
            Err(_) => 0,
        }
    }
}

/// A 64-bit snowflake id.
///
/// JSON representation is a decimal string in both directions; this type must never
/// gain a plain numeric `Serialize`/`Deserialize` (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnowflakeId(u64);

impl SnowflakeId {
    /// Width of the millisecond-timestamp field.
    pub const TIMESTAMP_BITS: u32 = 41;
    /// Width of the node field.
    pub const NODE_BITS: u32 = 10;
    /// Width of the intra-millisecond sequence field.
    pub const SEQUENCE_BITS: u32 = 12;

    /// Reconstruct an id from its raw 64-bit value (e.g. read back from storage).
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw 64-bit value (top bit always 0, so it also fits an `i64` column).
    pub fn as_raw(self) -> u64 {
        self.0
    }

    /// Milliseconds since the snowflake epoch (the 41-bit field).
    pub fn timestamp_ms(self) -> u64 {
        // Shift amount is a constant < 64, so `checked_shr` never yields `None`.
        self.0.checked_shr(TIMESTAMP_SHIFT).unwrap_or(0) & TIMESTAMP_MASK
    }

    /// The 10-bit node number.
    pub fn node(self) -> u16 {
        let field = self.0.checked_shr(NODE_SHIFT).unwrap_or(0) & NODE_FIELD_MASK;
        // Masked to 10 bits, so the conversion to u16 never fails.
        u16::try_from(field).unwrap_or(0)
    }

    /// The 12-bit intra-millisecond sequence number.
    pub fn sequence(self) -> u16 {
        let field = self.0 & SEQUENCE_FIELD_MASK;
        // Masked to 12 bits, so the conversion to u16 never fails.
        u16::try_from(field).unwrap_or(0)
    }

    /// Assemble an id from its (pre-validated) fields; each field is explicitly
    /// masked to its width before placement, every shift is `checked_*`.
    fn from_parts(timestamp_ms: u64, node: u16, sequence: u16) -> Self {
        let ts_part = (timestamp_ms & TIMESTAMP_MASK)
            .checked_shl(TIMESTAMP_SHIFT)
            .unwrap_or(0);
        let node_part = (u64::from(node) & NODE_FIELD_MASK)
            .checked_shl(NODE_SHIFT)
            .unwrap_or(0);
        let seq_part = u64::from(sequence) & SEQUENCE_FIELD_MASK;
        Self(ts_part | node_part | seq_part)
    }
}

impl serde::Serialize for SnowflakeId {
    /// Serializes as a decimal string (never as a JSON number).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&self.0)
    }
}

/// Visitor accepting only the decimal-string form; any other shape (notably a
/// JSON number) falls through to the visitor defaults and is rejected as a
/// data error.
struct SnowflakeIdVisitor;

impl serde::de::Visitor<'_> for SnowflakeIdVisitor {
    type Value = SnowflakeId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a decimal string holding a 64-bit snowflake id")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        match value.parse::<u64>() {
            Ok(raw) => Ok(SnowflakeId(raw)),
            Err(_) => Err(E::invalid_value(serde::de::Unexpected::Str(value), &self)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for SnowflakeId {
    /// Deserializes from a decimal string only; a JSON number is rejected.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(SnowflakeIdVisitor)
    }
}

/// Why id generation was refused (fail-closed: refusal, never a stale or duplicate id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdGenError {
    /// The clock read an earlier millisecond than the last issued one.
    /// Both fields are milliseconds since the snowflake epoch.
    #[error("clock moved backwards: now {now_ms}ms < last issued {last_ms}ms (snowflake epoch)")]
    ClockRollback { last_ms: u64, now_ms: u64 },
    /// The wall clock reads before `2026-01-01T00:00:00Z`; the 41-bit timestamp
    /// field cannot represent it.
    #[error("clock reads before the snowflake epoch: {now_unix_ms}ms unix")]
    ClockBeforeEpoch { now_unix_ms: u64 },
    /// The wall clock reads at or beyond snowflake epoch + 2^41 ms; the 41-bit
    /// timestamp field cannot represent it (masking would wrap to already-issued
    /// timestamps and mint duplicate ids).
    #[error("clock reads beyond the 41-bit timestamp range: {now_unix_ms}ms unix")]
    TimestampOverflow { now_unix_ms: u64 },
    /// The configured node number does not fit the 10-bit node field.
    #[error("node {node} exceeds the 10-bit maximum 1023")]
    NodeOutOfRange { node: u16 },
}

/// Snowflake id generator — the only mutable-state facility in `postern-core`.
///
/// Safe to share across threads (`&self` issuance, internal `Mutex` serialization).
/// A poisoned mutex is recovered, never unwrapped (issuance state stays consistent
/// because the critical section never panics).
pub struct IdGen {
    node: u16,
    clock: Box<dyn Clock>,
    state: Mutex<IdGenState>,
}

/// Mutable issuance state: last issued millisecond (snowflake epoch) + the next
/// sequence to issue within it (`MAX_SEQUENCE + 1` ⇒ the millisecond is exhausted).
struct IdGenState {
    last_ms: u64,
    sequence: u16,
}

impl IdGen {
    /// Generator with the workspace-default node number `0`.
    pub fn new<C: Clock + 'static>(clock: C) -> Self {
        Self {
            node: 0,
            clock: Box::new(clock),
            state: Mutex::new(IdGenState {
                last_ms: 0,
                sequence: 0,
            }),
        }
    }

    /// Generator with an explicit node number; `node > MAX_NODE` is refused
    /// (never silently masked — truncation would alias distinct nodes).
    pub fn with_node<C: Clock + 'static>(node: u16, clock: C) -> Result<Self, IdGenError> {
        if node > MAX_NODE {
            return Err(IdGenError::NodeOutOfRange { node });
        }
        Ok(Self {
            node,
            clock: Box::new(clock),
            state: Mutex::new(IdGenState {
                last_ms: 0,
                sequence: 0,
            }),
        })
    }

    /// Issue the next id.
    ///
    /// Same millisecond → sequence strictly increases; sequence exhaustion → spin
    /// until the clock enters the next millisecond; clock rollback → `Err` (fail-closed).
    pub fn next_id(&self) -> Result<SnowflakeId, IdGenError> {
        loop {
            let now_unix_ms = self.clock.now_unix_ms();
            let now_ms = match now_unix_ms.checked_sub(EPOCH_UNIX_MS) {
                Some(delta) => delta,
                None => return Err(IdGenError::ClockBeforeEpoch { now_unix_ms }),
            };
            if now_ms > TIMESTAMP_MASK {
                // The millisecond no longer fits the 41-bit field. Refuse
                // (fail-closed): masking would wrap the timestamp back to
                // already-issued values and mint duplicate ids (module design
                // §8-二L-8 "绝不重复").
                return Err(IdGenError::TimestampOverflow { now_unix_ms });
            }
            // A poisoned mutex is recovered, never unwrapped: the critical section
            // below cannot panic, so the recovered state is always consistent.
            let mut state = match self.state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if now_ms < state.last_ms {
                // Clock rollback: refuse (fail-closed). Never re-issue under the
                // stale millisecond, never wait for the clock to catch up.
                return Err(IdGenError::ClockRollback {
                    last_ms: state.last_ms,
                    now_ms,
                });
            }
            if now_ms > state.last_ms {
                state.last_ms = now_ms;
                state.sequence = 0;
            }
            // Here now_ms == state.last_ms.
            if state.sequence > MAX_SEQUENCE {
                // Millisecond exhausted: release the lock and spin until the wall
                // clock enters the next millisecond — never wrap within the same one.
                drop(state);
                std::thread::yield_now();
                continue;
            }
            let sequence = state.sequence;
            // `sequence <= MAX_SEQUENCE = 4095`, so this never saturates in practice;
            // saturating keeps the operation panic-free under all circumstances.
            state.sequence = sequence.saturating_add(1);
            return Ok(SnowflakeId::from_parts(now_ms, self.node, sequence));
        }
    }
}
