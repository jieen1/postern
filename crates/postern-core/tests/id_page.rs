//! Behavior tests for the `id_page` unit: snowflake `IdGen` + unified pagination.
//!
//! Traceability: each test carries a `// §8-…` comment pointing at its acceptance
//! entry in docs/modules/01-postern-core.md §8 (一F-6 snowflake IdGen, 一F-7 unified
//! pagination, 二L-8 no duplicate ids).
//!
//! All clocks are injected fakes — no test reads the system clock.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use postern_core::id::{Clock, IdGen, IdGenError, SnowflakeId, EPOCH_UNIX_MS};
use postern_core::page::{Page, PageQuery};

/// Settable fake clock: shared `AtomicU64` of unix milliseconds.
struct TestClock(Arc<AtomicU64>);

impl TestClock {
    /// A clock initially reading `unix_ms`, plus a handle to move it later.
    fn at(unix_ms: u64) -> (Self, Arc<AtomicU64>) {
        let handle = Arc::new(AtomicU64::new(unix_ms));
        (TestClock(Arc::clone(&handle)), handle)
    }
}

impl Clock for TestClock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// 一F-6 — snowflake IdGen: epoch, bit layout, default node, rollback, JSON string
// ---------------------------------------------------------------------------

#[test]
fn test_epoch_constant_is_2026_01_01t00_00_00z() {
    // §8-一F-6: the 41-bit timestamp counts from 2026-01-01T00:00:00Z.
    assert_eq!(EPOCH_UNIX_MS, 1_767_225_600_000);
}

#[test]
fn test_id_layout_is_41bit_ms_10bit_node_12bit_sequence() {
    // §8-一F-6: id decomposes into 41-bit ms since epoch + 10-bit node + 12-bit seq.
    // clock = epoch + 7 ms; node = 3; first id in the millisecond → sequence 0.
    let (clock, _handle) = TestClock::at(1_767_225_600_007);
    let gen = match IdGen::with_node(3, clock) {
        Ok(g) => g,
        Err(e) => panic!("node 3 must be accepted, got {e:?}"),
    };
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("generation must succeed, got {e:?}"),
    };
    assert_eq!(id.timestamp_ms(), 7);
    assert_eq!(id.node(), 3);
    assert_eq!(id.sequence(), 0);
    // raw = 7 << 22 | 3 << 12 | 0 = 29_360_128 + 12_288 (hand-computed literal)
    assert_eq!(id.as_raw(), 29_372_416);
}

#[test]
fn test_default_node_is_zero_and_epoch_instant_maps_to_all_zero_fields() {
    // §8-一F-6: `IdGen::new` uses node 0; at exactly the epoch the timestamp field is 0.
    let (clock, _handle) = TestClock::at(1_767_225_600_000);
    let gen = IdGen::new(clock);
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("generation must succeed, got {e:?}"),
    };
    assert_eq!(id.timestamp_ms(), 0);
    assert_eq!(id.node(), 0);
    assert_eq!(id.sequence(), 0);
    assert_eq!(id.as_raw(), 0);
}

#[test]
fn test_node_1023_is_the_largest_accepted_node() {
    // §8-一F-6: 10-bit node field — 1023 fits and lands in bits 12..=21.
    let (clock, _handle) = TestClock::at(1_767_225_600_001);
    let gen = match IdGen::with_node(1023, clock) {
        Ok(g) => g,
        Err(e) => panic!("node 1023 must be accepted, got {e:?}"),
    };
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("generation must succeed, got {e:?}"),
    };
    assert_eq!(id.node(), 1023);
    // raw = 1 << 22 | 1023 << 12 | 0 = 4_194_304 + 4_190_208 (hand-computed literal)
    assert_eq!(id.as_raw(), 8_384_512);
}

#[test]
fn test_node_1024_is_refused_not_masked() {
    // §8-一F-6: a node that does not fit 10 bits is refused (fail-closed), never
    // silently truncated into a colliding node number.
    let (clock, _handle) = TestClock::at(1_767_225_600_001);
    match IdGen::with_node(1024, clock) {
        Err(e) => assert_eq!(e, IdGenError::NodeOutOfRange { node: 1024 }),
        Ok(_) => panic!("node 1024 exceeds 10 bits and must be refused"),
    }
}

#[test]
fn test_clock_rollback_refuses_generation_with_clock_rollback_error() {
    // §8-一F-6: current ms < last issued ms → exactly Err(ClockRollback{last,now}),
    // never an id issued under the stale millisecond.
    let (clock, handle) = TestClock::at(1_767_225_600_040);
    let gen = IdGen::new(clock);
    match gen.next_id() {
        Ok(id) => assert_eq!(id.timestamp_ms(), 40),
        Err(e) => panic!("healthy clock must issue, got {e:?}"),
    }
    handle.store(1_767_225_600_035, Ordering::SeqCst); // roll back 5 ms
    assert_eq!(
        gen.next_id(),
        Err(IdGenError::ClockRollback {
            last_ms: 40,
            now_ms: 35
        })
    );
}

#[test]
fn test_generation_resumes_once_clock_passes_last_issued_millisecond() {
    // §8-一F-6: the rollback refusal is per-call, not sticky — when the clock again
    // reads beyond the last issued millisecond, issuance resumes at sequence 0.
    let (clock, handle) = TestClock::at(1_767_225_600_040);
    let gen = IdGen::new(clock);
    let first = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("healthy clock must issue, got {e:?}"),
    };
    handle.store(1_767_225_600_035, Ordering::SeqCst);
    assert_eq!(
        gen.next_id(),
        Err(IdGenError::ClockRollback {
            last_ms: 40,
            now_ms: 35
        })
    );
    handle.store(1_767_225_600_041, Ordering::SeqCst);
    let resumed = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("recovered clock must issue, got {e:?}"),
    };
    assert_eq!(resumed.timestamp_ms(), 41);
    assert_eq!(resumed.sequence(), 0);
    assert!(resumed.as_raw() > first.as_raw());
}

#[test]
fn test_clock_before_epoch_is_refused() {
    // §8-一F-6: a wall clock before 2026-01-01T00:00:00Z cannot be represented in
    // the 41-bit field — refused fail-closed, no wrap, no panic.
    let (clock, _handle) = TestClock::at(1_767_225_599_999); // epoch − 1 ms
    let gen = IdGen::new(clock);
    assert_eq!(
        gen.next_id(),
        Err(IdGenError::ClockBeforeEpoch {
            now_unix_ms: 1_767_225_599_999
        })
    );
}

#[test]
fn test_last_representable_millisecond_epoch_plus_2_pow_41_minus_1_is_accepted() {
    // §8-一F-6: upper boundary of the 41-bit timestamp field — the last
    // representable millisecond (epoch + 2^41 − 1 ms) still issues, with the
    // field at exactly its 41-bit maximum.
    let (clock, _handle) = TestClock::at(3_966_248_855_551); // epoch + 2^41 − 1 ms
    let gen = IdGen::new(clock);
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("last representable millisecond must issue, got {e:?}"),
    };
    assert_eq!(id.timestamp_ms(), 2_199_023_255_551); // 2^41 − 1
    assert_eq!(id.node(), 0);
    assert_eq!(id.sequence(), 0);
    // raw = (2^41 − 1) << 22 = 2^63 − 2^22 (hand-computed literal)
    assert_eq!(id.as_raw(), 9_223_372_036_850_581_504);
}

#[test]
fn test_clock_at_epoch_plus_2_pow_41_ms_is_refused_never_wrapped() {
    // §8-一F-6 + §8-二L-8: epoch + 2^41 ms no longer fits the 41-bit timestamp
    // field — issuance is refused (fail-closed), never silently masked. Masking
    // would wrap the timestamp to 0 and re-issue the very ids minted at the
    // epoch instant ("绝不重复", mirror of test_node_1024_is_refused_not_masked).
    let (clock, _handle) = TestClock::at(3_966_248_855_552); // epoch + 2^41 ms
    let gen = IdGen::new(clock);
    assert_eq!(
        gen.next_id(),
        Err(IdGenError::TimestampOverflow {
            now_unix_ms: 3_966_248_855_552
        })
    );
}

#[test]
fn test_clock_at_u64_max_is_refused_not_truncated() {
    // §8-二L-8: even a saturated clock reading (SystemClock saturates to
    // u64::MAX rather than panic) is refused — never truncated into the 41-bit
    // field, so no far-future clock can mint a colliding id.
    let (clock, _handle) = TestClock::at(u64::MAX);
    let gen = IdGen::new(clock);
    assert_eq!(
        gen.next_id(),
        Err(IdGenError::TimestampOverflow {
            now_unix_ms: u64::MAX
        })
    );
}

#[test]
fn test_snowflake_id_serializes_to_decimal_string() {
    // §8-一F-6: JSON serialization is a string, even (especially) beyond 2^53.
    let id = SnowflakeId::from_raw(9_007_199_254_740_993); // 2^53 + 1
    let json = match serde_json::to_string(&id) {
        Ok(j) => j,
        Err(e) => panic!("serialization must succeed, got {e:?}"),
    };
    assert_eq!(json, "\"9007199254740993\"");
}

#[test]
fn test_snowflake_id_deserializes_from_decimal_string() {
    // §8-一F-6: deserialization parses the string form.
    let id: SnowflakeId = match serde_json::from_str("\"9007199254740993\"") {
        Ok(id) => id,
        Err(e) => panic!("string form must deserialize, got {e:?}"),
    };
    assert_eq!(id, SnowflakeId::from_raw(9_007_199_254_740_993));
}

#[test]
fn test_snowflake_id_json_roundtrip_preserves_full_63_bit_value() {
    // §8-一F-6: string transport loses no precision anywhere in the 63-bit range.
    let id = SnowflakeId::from_raw(9_223_372_036_854_775_807); // 2^63 − 1
    let json = match serde_json::to_string(&id) {
        Ok(j) => j,
        Err(e) => panic!("serialization must succeed, got {e:?}"),
    };
    let back: SnowflakeId = match serde_json::from_str(&json) {
        Ok(id) => id,
        Err(e) => panic!("roundtrip must deserialize, got {e:?}"),
    };
    assert_eq!(back, id);
    assert_eq!(back.as_raw(), 9_223_372_036_854_775_807);
}

#[test]
fn test_snowflake_id_refuses_json_number() {
    // §8-一F-6: the numeric JSON form is rejected (data error), so a 53-bit-lossy
    // producer can never sneak a number through deserialization.
    let result: Result<SnowflakeId, _> = serde_json::from_str("9007199254740993");
    match result {
        Err(e) => assert_eq!(e.classify(), serde_json::error::Category::Data),
        Ok(id) => panic!("JSON number must be refused, got {id:?}"),
    }
}

#[test]
fn test_snowflake_id_refuses_non_numeric_string() {
    // §8-一F-6: a string that is not a decimal u64 is a data error, not a panic.
    let result: Result<SnowflakeId, _> = serde_json::from_str("\"not-a-number\"");
    match result {
        Err(e) => assert_eq!(e.classify(), serde_json::error::Category::Data),
        Ok(id) => panic!("non-numeric string must be refused, got {id:?}"),
    }
}

// ---------------------------------------------------------------------------
// 二L-8 — no duplicate ids: same-ms strict increase, exhaustion, concurrency
// ---------------------------------------------------------------------------

#[test]
fn test_same_millisecond_sequence_strictly_increases_from_zero() {
    // §8-二L-8: within one millisecond the sequence is exactly 0,1,2,… — strictly
    // increasing, no gap, no repeat.
    let (clock, _handle) = TestClock::at(1_767_225_600_005);
    let gen = IdGen::new(clock);
    let mut prev_raw: Option<u64> = None;
    for expected_seq in 0..100u16 {
        let id = match gen.next_id() {
            Ok(id) => id,
            Err(e) => panic!("generation must succeed at seq {expected_seq}, got {e:?}"),
        };
        assert_eq!(id.timestamp_ms(), 5);
        assert_eq!(id.sequence(), expected_seq);
        if let Some(prev) = prev_raw {
            assert!(id.as_raw() > prev, "ids must strictly increase");
        }
        prev_raw = Some(id.as_raw());
    }
}

#[test]
fn test_sequence_resets_to_zero_when_millisecond_advances() {
    // §8-二L-8: crossing into a new millisecond restarts the sequence at 0 while
    // ids keep strictly increasing (timestamp field dominates).
    let (clock, handle) = TestClock::at(1_767_225_600_010);
    let gen = IdGen::new(clock);
    let mut last_raw = 0u64;
    for expected_seq in 0..2u16 {
        let id = match gen.next_id() {
            Ok(id) => id,
            Err(e) => panic!("generation must succeed, got {e:?}"),
        };
        assert_eq!(id.sequence(), expected_seq);
        last_raw = id.as_raw();
    }
    handle.store(1_767_225_600_011, Ordering::SeqCst);
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("generation must succeed, got {e:?}"),
    };
    assert_eq!(id.timestamp_ms(), 11);
    assert_eq!(id.sequence(), 0);
    assert!(id.as_raw() > last_raw);
}

#[test]
fn test_full_4096_ids_in_one_millisecond_are_all_unique_and_increasing() {
    // §8-二L-8: the full 12-bit sequence space of one millisecond yields 4096
    // distinct, strictly increasing ids — the per-ms capacity is exactly 4096.
    let (clock, _handle) = TestClock::at(1_767_225_600_015);
    let gen = IdGen::new(clock);
    let mut seen = HashSet::with_capacity(4096);
    let mut prev_raw: Option<u64> = None;
    let mut last_seq = 0u16;
    for _ in 0..4096u32 {
        let id = match gen.next_id() {
            Ok(id) => id,
            Err(e) => panic!("within-capacity generation must succeed, got {e:?}"),
        };
        assert_eq!(id.timestamp_ms(), 15);
        assert!(seen.insert(id.as_raw()), "duplicate id {:?}", id);
        if let Some(prev) = prev_raw {
            assert!(id.as_raw() > prev, "ids must strictly increase");
        }
        prev_raw = Some(id.as_raw());
        last_seq = id.sequence();
    }
    assert_eq!(seen.len(), 4096);
    assert_eq!(last_seq, 4095, "the millisecond capacity is exactly 4096");
}

#[test]
fn test_sequence_exhaustion_spins_to_next_millisecond_never_wraps_in_place() {
    // §8-二L-8: the 4097th id in one millisecond is NOT a same-ms wraparound and
    // NOT an error — the generator waits for the injected clock to advance, then
    // issues (next ms, sequence 0).
    let (clock, handle) = TestClock::at(1_767_225_600_020);
    let gen = IdGen::new(clock);
    let mut last_raw = 0u64;
    for _ in 0..4096u32 {
        let id = match gen.next_id() {
            Ok(id) => id,
            Err(e) => panic!("within-capacity generation must succeed, got {e:?}"),
        };
        last_raw = id.as_raw();
    }
    // Release the spin from another thread: the clock enters the next millisecond
    // only after a delay, so a correct implementation must block until then.
    let bumper = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        handle.store(1_767_225_600_021, Ordering::SeqCst);
    });
    let id = match gen.next_id() {
        Ok(id) => id,
        Err(e) => panic!("exhaustion must spin to the next millisecond, got {e:?}"),
    };
    assert_eq!(id.timestamp_ms(), 21);
    assert_eq!(id.sequence(), 0);
    assert!(id.as_raw() > last_raw, "carried id must still increase");
    if bumper.join().is_err() {
        panic!("clock bumper thread panicked");
    }
}

#[test]
fn test_concurrent_issuance_in_one_millisecond_yields_4096_distinct_ids() {
    // §8-二L-8: 8 threads × 512 ids against one frozen millisecond — the Mutex
    // serialization must hand out exactly the sequence set {0..=4095}, no repeat.
    let (clock, _handle) = TestClock::at(1_767_225_600_030);
    let gen = Arc::new(IdGen::new(clock));
    let mut workers = Vec::with_capacity(8);
    for _ in 0..8u32 {
        let gen = Arc::clone(&gen);
        workers.push(thread::spawn(move || {
            let mut raws = Vec::with_capacity(512);
            for _ in 0..512u32 {
                match gen.next_id() {
                    Ok(id) => raws.push(id.as_raw()),
                    Err(e) => panic!("within-capacity generation must succeed, got {e:?}"),
                }
            }
            raws
        }));
    }
    let mut all_raws = HashSet::with_capacity(4096);
    let mut all_seqs = HashSet::with_capacity(4096);
    for worker in workers {
        let raws = match worker.join() {
            Ok(raws) => raws,
            Err(_) => panic!("worker thread panicked"),
        };
        for raw in raws {
            let id = SnowflakeId::from_raw(raw);
            assert_eq!(id.timestamp_ms(), 30);
            assert_eq!(id.node(), 0);
            assert!(all_raws.insert(raw), "duplicate id raw {raw}");
            all_seqs.insert(id.sequence());
        }
    }
    assert_eq!(all_raws.len(), 4096);
    // 4096 distinct 12-bit sequence values ⇒ exactly the set {0..=4095}.
    assert_eq!(all_seqs.len(), 4096);
}

// ---------------------------------------------------------------------------
// 一F-7 — unified pagination: constants, clamp boundaries, Page<T> envelope
// ---------------------------------------------------------------------------

#[test]
fn test_page_default_size_is_20() {
    // §8-一F-7
    assert_eq!(PageQuery::DEFAULT_SIZE, 20);
}

#[test]
fn test_page_max_size_is_200() {
    // §8-一F-7
    assert_eq!(PageQuery::MAX_SIZE, 200);
}

#[test]
fn test_clamp_leaves_in_range_query_unchanged() {
    // §8-一F-7: clamp is identity on legal input.
    let clamped = PageQuery {
        page_no: 3,
        page_size: 50,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 3,
            page_size: 50
        }
    );
}

#[test]
fn test_clamp_raises_page_no_zero_to_one() {
    // §8-一F-7: page_no < 1 → 1 (page numbering is 1-based), page_size untouched.
    let clamped = PageQuery {
        page_no: 0,
        page_size: 20,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 1,
            page_size: 20
        }
    );
}

#[test]
fn test_clamp_raises_page_size_zero_to_one() {
    // §8-一F-7: page_size < 1 → 1 (lower legal bound), no error.
    let clamped = PageQuery {
        page_no: 5,
        page_size: 0,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 5,
            page_size: 1
        }
    );
}

#[test]
fn test_clamp_caps_page_size_201_to_200() {
    // §8-一F-7: just past the ceiling clamps to exactly MAX_SIZE — not an error.
    let clamped = PageQuery {
        page_no: 1,
        page_size: 201,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 1,
            page_size: 200
        }
    );
}

#[test]
fn test_clamp_caps_extreme_values_without_error() {
    // §8-一F-7: u32::MAX page_size clamps to 200; page_no has no upper clamp.
    let clamped = PageQuery {
        page_no: u32::MAX,
        page_size: u32::MAX,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: u32::MAX,
            page_size: 200
        }
    );
}

#[test]
fn test_clamp_is_identity_at_lower_bounds_one_one() {
    // §8-一F-7: the exact lower bounds (1, 1) are legal and pass through unchanged.
    let clamped = PageQuery {
        page_no: 1,
        page_size: 1,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 1,
            page_size: 1
        }
    );
}

#[test]
fn test_clamp_keeps_page_size_exactly_200() {
    // §8-一F-7: the exact ceiling 200 is legal — clamp must not shrink it.
    let clamped = PageQuery {
        page_no: 2,
        page_size: 200,
    }
    .clamp();
    assert_eq!(
        clamped,
        PageQuery {
            page_no: 2,
            page_size: 200
        }
    );
}

#[test]
fn test_page_envelope_carries_items_and_paging_facts() {
    // §8-一F-7: Page<T> is the uniform envelope {items, page_no, page_size, total}.
    let page = Page {
        items: vec!["a", "b"],
        page_no: 2,
        page_size: 20,
        total: 41,
    };
    assert_eq!(page.items, vec!["a", "b"]);
    assert_eq!(page.page_no, 2);
    assert_eq!(page.page_size, 20);
    assert_eq!(page.total, 41);
}

#[test]
fn test_page_envelope_serializes_to_exact_snake_case_wire_json() {
    // §8-一F-7: the cli parses the daemon's HTTP/JSON `Page<T>` envelope
    // (01-postern-core.md §6.6), so the serde wire shape IS the contract —
    // exactly the four snake_case fields {items, page_no, page_size, total}.
    let page = Page {
        items: vec!["a", "b"],
        page_no: 2,
        page_size: 20,
        total: 41,
    };
    let json = match serde_json::to_string(&page) {
        Ok(j) => j,
        Err(e) => panic!("envelope serialization must succeed, got {e:?}"),
    };
    assert_eq!(
        json,
        r#"{"items":["a","b"],"page_no":2,"page_size":20,"total":41}"#
    );
}

#[test]
fn test_page_envelope_deserializes_wire_json_with_total_beyond_u32() {
    // §8-一F-7: `total` is u64 in the documented shape (01-postern-core.md §5.4)
    // — a count beyond u32::MAX (here 2^32) must parse exactly, so a silent
    // u32 regression cannot survive this test.
    let wire = r#"{"items":["a","b"],"page_no":2,"page_size":20,"total":4294967296}"#;
    let page: Page<String> = match serde_json::from_str(wire) {
        Ok(p) => p,
        Err(e) => panic!("documented wire JSON must deserialize, got {e:?}"),
    };
    assert_eq!(page.items, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(page.page_no, 2);
    assert_eq!(page.page_size, 20);
    assert_eq!(page.total, 4_294_967_296);
}
