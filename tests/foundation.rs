//! E01の公開APIから行う回帰試験。内部配列への特別な権限は使用しません。

use charon_engine::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    component::{ComponentStore, EntityPool},
    grid::{CellPoint, GridChannel, GridLayout, GridLimits, GridScale},
    id::{Arena, EntityKind, WorldId},
    random::{RandomSource, SplitMix64},
    schema::{Schema, SchemaRegistry},
    state::{StateStore, StoreLimits},
    transaction::{AtomicState, CandidateState},
    units::{CellRect, Tick, TickSpan, Velocity, WorldPoint},
};

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}
fn registry() -> SchemaRegistry {
    let mut registry = SchemaRegistry::default();
    registry
        .register(Schema::of::<u64>(key("test:number"), 1, 8).unwrap())
        .unwrap();
    registry
}
fn limits() -> StoreLimits {
    StoreLimits {
        records: 2,
        payload_bytes: 16,
        checkpoint_bytes: 4096,
    }
}
fn store() -> StateStore {
    StateStore::new(key("test:world"), registry(), limits()).unwrap()
}
fn layout() -> GridLayout {
    GridLayout {
        origin: CellPoint { x: -10, y: 20 },
        x_scale: GridScale::new(16, 1).unwrap(),
        y_scale: GridScale::new(1, 2).unwrap(),
    }
}
fn grid_limits() -> GridLimits {
    GridLimits {
        chunks: 2,
        checkpoint_bytes: 65536,
    }
}

#[test]
fn stable_keys_validate_normalize_and_sort_without_registration_order() {
    assert_eq!(
        StableKey::resolve("test", "sand").unwrap(),
        key("test:sand")
    );
    assert_eq!(
        StableKey::resolve("test", "other:sand").unwrap(),
        key("other:sand")
    );
    for invalid in [
        "sand",
        "test:",
        ":sand",
        "Test:sand",
        "test:sand:a",
        "test:砂",
    ] {
        assert!(StableKey::parse(invalid).is_err());
    }
    let long = format!("{}:{}", "a".repeat(96), "b".repeat(96));
    assert_eq!(StableKey::parse(&long).unwrap().as_str(), long);
    assert!(StableKey::parse(&format!("a:{}", "b".repeat(97))).is_err());
    assert!(key("a:a") < key("a:a-"));
    let mut out = Encoder::new(256);
    key("test:sand").encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut input = Decoder::new(&bytes, 256).unwrap();
    assert_eq!(StableKey::decode(&mut input).unwrap(), key("test:sand"));
    input.finish().unwrap();
}

#[test]
fn codec_preserves_float_bits_and_rejects_noncanonical_scalars_and_lengths() {
    let mut out = Encoder::new(32);
    (-0.0_f32).encode(&mut out).unwrap();
    (-0.0_f64).encode(&mut out).unwrap();
    i64::MIN.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut input = Decoder::new(&bytes, 32).unwrap();
    assert_eq!(
        f32::decode(&mut input).unwrap().to_bits(),
        (-0.0_f32).to_bits()
    );
    assert_eq!(
        f64::decode(&mut input).unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(i64::decode(&mut input).unwrap(), i64::MIN);
    input.finish().unwrap();
    assert!(f64::NAN.encode(&mut Encoder::new(8)).is_err());
    assert!(bool::decode(&mut Decoder::new(&[2], 1).unwrap()).is_err());
    let mut input = Decoder::new(&[255, 255, 255, 255], 4).unwrap();
    assert!(input.bytes(16).is_err());
    let mut out = Encoder::new(4);
    assert!(out.bytes(&[1]).is_err());
    assert!(out.finish().is_empty());
}

#[test]
fn arena_roundtrip_preserves_generation_and_reuse_order() {
    let mut arena: Arena<u64, EntityKind> = Arena::new(3).unwrap();
    let first = arena.insert(10).unwrap();
    let second = arena.insert(20).unwrap();
    let third = arena.insert(30).unwrap();
    assert_eq!(arena.remove(first).unwrap(), 10);
    arena.remove(second).unwrap();
    let bytes = arena.checkpoint(4096).unwrap();
    let mut restored = Arena::<u64, EntityKind>::restore(&bytes, 3, 4096).unwrap();
    assert_eq!(*restored.get(third).unwrap(), 30);
    assert_eq!(restored.get(first).unwrap_err().code, ErrorCode::StaleId);
    let replacement = restored.insert(40).unwrap();
    assert_eq!(replacement.index(), second.index());
    assert_eq!(replacement.generation(), second.generation() + 1);
    assert_eq!(restored.checkpoint(4096).unwrap(), {
        arena.insert(40).unwrap();
        arena.checkpoint(4096).unwrap()
    });
    for cut in 0..bytes.len() {
        assert!(Arena::<u64, EntityKind>::restore(&bytes[..cut], 3, 4096).is_err());
    }
    assert!(Arena::<u64, EntityKind>::restore(&bytes, 2, 4096).is_err());
}

#[test]
fn arena_candidate_copy_preserves_logical_capacity_with_live_slots_only() {
    let mut arena: Arena<u64, EntityKind> = Arena::new(8).unwrap();
    let first = arena.insert(10).unwrap();
    let second = arena.insert(20).unwrap();
    let mut candidate = arena.try_copy().unwrap();

    assert_eq!(*candidate.get(first).unwrap(), 10);
    assert_eq!(*candidate.get(second).unwrap(), 20);
    candidate.remove(first).unwrap();
    let reused = candidate.insert(30).unwrap();
    assert_eq!(reused.index(), first.index());
    assert_ne!(reused.generation(), first.generation());
    for value in 40..46 {
        candidate.insert(value).unwrap();
    }
    assert_eq!(candidate.len(), 8);
    assert!(matches!(
        candidate.insert(99).unwrap_err().code,
        ErrorCode::CapacityExceeded
    ));
    assert_eq!(arena.len(), 2, "候補内の追加・削除は元arenaを変更しない");
}

#[test]
fn arena_retires_generation_maximum_without_wrapping() {
    // 上限世代まで何十億回も回さず、明示形式でその直前の正規状態を作ります。
    let mut out = Encoder::new(64);
    out.raw(b"CHARNA01").unwrap();
    out.u32(1).unwrap();
    out.u32(1).unwrap();
    out.u32(u32::MAX).unwrap();
    out.u8(0).unwrap();
    out.u8(1).unwrap();
    out.u8(9).unwrap();
    out.u32(0).unwrap();
    let mut arena = Arena::<u8, EntityKind>::restore(&out.finish(), 1, 64).unwrap();
    let id = arena.iter().next().unwrap().0;
    arena.remove(id).unwrap();
    assert!(arena.is_empty());
    assert!(matches!(
        arena.insert(1),
        Err(EngineError {
            code: ErrorCode::CapacityExceeded,
            ..
        })
    ));
    let restored = Arena::<u8, EntityKind>::restore(&arena.checkpoint(64).unwrap(), 1, 64).unwrap();
    assert!(restored.is_empty());
}

#[test]
fn state_transaction_poisons_after_failure_and_discards_earlier_writes() {
    let mut state = store();
    let initial = state.checkpoint().unwrap();
    let mut change = state.transaction().unwrap();
    change
        .put(key("test:a"), key("test:number"), &10_u64)
        .unwrap();
    change
        .put(key("test:b"), key("test:number"), &20_u64)
        .unwrap();
    assert_eq!(
        change
            .put(key("test:c"), key("test:number"), &30_u64)
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert!(change.commit().is_err());
    assert_eq!(state.checkpoint().unwrap(), initial);
    let mut change = state.transaction().unwrap();
    change
        .put(key("test:a"), key("test:number"), &42_u64)
        .unwrap();
    assert_eq!(change.get::<u64>(key("test:a")).unwrap(), 42);
    change.commit().unwrap();
    let before = state.checkpoint().unwrap();
    let mut change = state.transaction().unwrap();
    change
        .put(key("test:a"), key("test:number"), &99_u64)
        .unwrap();
    assert!(change.remove(key("test:missing")).is_err());
    assert!(change.commit().is_err());
    assert_eq!(state.checkpoint().unwrap(), before);
}

#[test]
fn state_restore_checks_scope_type_truncation_order_and_limits() {
    let mut state = store();
    let mut change = state.transaction().unwrap();
    change
        .put(key("test:b"), key("test:number"), &2_u64)
        .unwrap();
    change
        .put(key("test:a"), key("test:number"), &1_u64)
        .unwrap();
    change.commit().unwrap();
    let bytes = state.checkpoint().unwrap();
    let restored = StateStore::restore(&bytes, key("test:world"), registry(), limits()).unwrap();
    assert_eq!(restored.get::<u64>(key("test:a")).unwrap(), 1);
    assert!(restored.get::<u32>(key("test:a")).is_err());
    assert_eq!(restored.checkpoint().unwrap(), bytes);
    assert!(StateStore::restore(&bytes, key("test:other"), registry(), limits()).is_err());
    for cut in 0..bytes.len() {
        assert!(
            StateStore::restore(&bytes[..cut], key("test:world"), registry(), limits()).is_err()
        );
    }
    let small = StoreLimits {
        payload_bytes: 8,
        ..limits()
    };
    assert!(StateStore::restore(&bytes, key("test:world"), registry(), small).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(StateStore::restore(&trailing, key("test:world"), registry(), limits()).is_err());
}

#[test]
fn schema_migrations_are_explicit_bounded_and_type_checked() {
    fn widen(bytes: &[u8], out: &mut Encoder) -> EngineResult<()> {
        let mut input = Decoder::new(bytes, 4)?;
        let value = input.u32()?;
        input.finish()?;
        out.u64(u64::from(value))
    }
    let mut schema = Schema::of::<u64>(key("test:number"), 2, 8).unwrap();
    schema.add_migration(1, 2, 4, widen).unwrap();
    assert!(schema.add_migration(1, 2, 4, widen).is_err());
    assert!(schema.add_migration(2, 1, 8, widen).is_err());
    assert_eq!(
        schema.upgrade(1, &42_u32.to_le_bytes()).unwrap(),
        42_u64.to_le_bytes()
    );
    assert!(schema.upgrade(3, &42_u64.to_le_bytes()).is_err());
    assert!(schema.upgrade(1, &[0; 5]).is_err());
    let mut registry = SchemaRegistry::default();
    registry.register(schema).unwrap();
    assert!(
        registry
            .register(Schema::of::<u32>(key("test:number"), 1, 4).unwrap())
            .is_err()
    );
}

#[test]
fn grid_negative_coordinates_capacity_reclamation_and_roundtrip() {
    let mut grid = GridChannel::new(key("test:liquid"), 1, layout(), 0_u8, grid_limits()).unwrap();
    let negative = CellPoint { x: -1, y: -1 };
    assert_eq!(negative.chunk_local(), ((-1, -1), 4095));
    grid.set(negative, 127).unwrap();
    grid.set(CellPoint { x: 64, y: 0 }, 255).unwrap();
    let before = grid.checkpoint().unwrap();
    assert!(grid.set(CellPoint { x: 128, y: 0 }, 1).is_err());
    assert_eq!(grid.checkpoint().unwrap(), before);
    let restored = GridChannel::restore(
        &before,
        key("test:liquid"),
        1,
        layout(),
        0_u8,
        grid_limits(),
    )
    .unwrap();
    assert_eq!(restored.get(negative), 127);
    assert_eq!(restored.checkpoint().unwrap(), before);
    grid.set(negative, 0).unwrap();
    assert_eq!(grid.allocated_chunks(), 1);
    grid.set(CellPoint { x: 128, y: 0 }, 1).unwrap();
    for cut in 0..before.len() {
        assert!(
            GridChannel::restore(
                &before[..cut],
                key("test:liquid"),
                1,
                layout(),
                0_u8,
                grid_limits()
            )
            .is_err()
        );
    }
}

#[test]
fn grid_preserves_negative_zero_and_integer_extremes() {
    let mut grid = GridChannel::new(key("test:field"), 1, layout(), 0_f32, grid_limits()).unwrap();
    let point = CellPoint {
        x: i32::MIN,
        y: i32::MAX,
    };
    grid.set(point, -0_f32).unwrap();
    assert_eq!(grid.allocated_chunks(), 1);
    let bytes = grid.checkpoint().unwrap();
    let restored =
        GridChannel::restore(&bytes, key("test:field"), 1, layout(), 0_f32, grid_limits()).unwrap();
    assert_eq!(restored.get(point).to_bits(), (-0_f32).to_bits());
    assert!(grid.set(point, f32::NAN).is_err());
    assert_eq!(grid.checkpoint().unwrap(), bytes);
}

#[test]
fn component_store_validates_world_lifetime_and_reused_slots() {
    let world = WorldId::from_parts(0, 1).unwrap();
    let other_world = WorldId::from_parts(1, 1).unwrap();
    let mut pool = EntityPool::new(world, 2).unwrap();
    let other = EntityPool::new(other_world, 2).unwrap();
    let entity = pool.spawn().unwrap();
    let mut components = ComponentStore::new(&pool, key("test:health"), 1, 2).unwrap();
    components.insert(&pool, entity, 10_u64).unwrap();
    assert!(components.get(&other, entity).is_err());
    pool.despawn(entity).unwrap();
    assert!(components.get(&pool, entity).is_err());
    assert_eq!(components.iter(&pool).unwrap().count(), 0);
    let replacement = pool.spawn().unwrap();
    assert_eq!(replacement.local().index(), entity.local().index());
    assert!(components.get(&pool, replacement).unwrap().is_none());
    components.insert(&pool, replacement, 20_u64).unwrap();
    let saved_pool = pool.checkpoint(4096).unwrap();
    let saved = components.checkpoint(&pool, 4096).unwrap();
    let pool = EntityPool::restore(&saved_pool, world, 2, 4096).unwrap();
    let restored =
        ComponentStore::<u64>::restore(&saved, &pool, key("test:health"), 1, 2, 4096).unwrap();
    assert_eq!(restored.get(&pool, replacement).unwrap(), Some(&20));
    assert_eq!(restored.checkpoint(&pool, 4096).unwrap(), saved);
}

#[test]
fn component_checkpoint_is_independent_of_dense_insertion_order() {
    let world = WorldId::from_parts(0, 1).unwrap();
    let mut pool = EntityPool::new(world, 3).unwrap();
    let first = pool.spawn().unwrap();
    let second = pool.spawn().unwrap();
    let mut a = ComponentStore::new(&pool, key("test:health"), 1, 3).unwrap();
    let mut b = ComponentStore::new(&pool, key("test:health"), 1, 3).unwrap();
    a.insert(&pool, first, 1_u64).unwrap();
    a.insert(&pool, second, 2_u64).unwrap();
    b.insert(&pool, second, 2_u64).unwrap();
    b.insert(&pool, first, 1_u64).unwrap();
    assert_eq!(
        b.ordered(&pool)
            .unwrap()
            .iter()
            .map(|(entity, _)| *entity)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(
        a.checkpoint(&pool, 4096).unwrap(),
        b.checkpoint(&pool, 4096).unwrap()
    );
    pool.despawn(first).unwrap();
    a.compact(&pool).unwrap();
    assert_eq!(a.iter(&pool).unwrap().count(), 1);
}

#[test]
fn atomic_edit_rolls_back_nested_store_and_grid_changes_after_failure() {
    let grid = GridChannel::new(key("test:liquid"), 1, layout(), 0_u8, grid_limits()).unwrap();
    let mut data = AtomicState::new(key("test:world"), (store(), grid)).unwrap();
    let before = data.view().0.checkpoint().unwrap();
    let mut edit = data.transaction().unwrap();
    edit.apply(|(state, grid)| {
        let mut transaction = state.transaction()?;
        transaction.put(key("test:a"), key("test:number"), &1_u64)?;
        transaction.commit()?;
        grid.set(CellPoint { x: 0, y: 0 }, 1)?;
        Err::<(), _>(EngineError::new(
            ErrorCode::CapacityExceeded,
            "後続出力の予約",
        ))
    })
    .unwrap_err();
    assert!(edit.prepare().is_err());
    assert_eq!(data.view().0.checkpoint().unwrap(), before);
    assert_eq!(data.view().1.allocated_chunks(), 0);
    assert_eq!(data.revision(), 0);
    let mut edit = data.transaction().unwrap();
    edit.apply(|(_, grid)| grid.set(CellPoint { x: 0, y: 0 }, 3))
        .unwrap();
    let receipt = edit.prepare().unwrap().commit();
    assert_eq!(receipt.revision, 1);
    assert_eq!(receipt.into_previous().1.allocated_chunks(), 0);
    assert_eq!(data.view().1.get(CellPoint { x: 0, y: 0 }), 3);
}

struct Positive(i32);
impl CandidateState for Positive {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self(self.0))
    }
    fn validate(&self) -> EngineResult<()> {
        if self.0 > 0 {
            Ok(())
        } else {
            Err(EngineError::new(ErrorCode::InvalidValue, "正の状態値"))
        }
    }
}

#[test]
fn atomic_prepare_validates_whole_state_and_drop_discards_candidate() {
    let mut data = AtomicState::new(key("test:state"), Positive(1)).unwrap();
    let mut edit = data.transaction().unwrap();
    edit.apply(|value| {
        value.0 = -1;
        Ok(())
    })
    .unwrap();
    assert!(edit.prepare().is_err());
    assert_eq!(data.view().0, 1);
    let mut edit = data.transaction().unwrap();
    edit.apply(|value| {
        value.0 = 2;
        Ok(())
    })
    .unwrap();
    drop(edit.prepare().unwrap());
    assert_eq!(data.view().0, 1);
    assert_eq!(data.revision(), 0);
}

#[test]
fn random_stream_roundtrip_preserves_next_values_and_reference_vector() {
    let mut random = SplitMix64::new(0);
    // 公開参照実装のseed=0、最初の出力と照合します。
    assert_eq!(random.next_u64(), 0xe220_a839_7b1d_cdaf);
    let mut out = Encoder::new(8);
    random.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut restored = SplitMix64::decode(&mut Decoder::new(&bytes, 8).unwrap()).unwrap();
    for _ in 0..100 {
        assert_eq!(random.next_u64(), restored.next_u64());
    }
    assert!(random.below(0).is_err());
    for _ in 0..100 {
        assert!(random.below(3).unwrap() < 3);
    }
}

#[test]
fn unit_types_validate_finite_values_half_open_edges_and_overflow() {
    assert!(WorldPoint::new(f64::INFINITY, 0.0).is_err());
    assert!(Velocity::new(0.0, f64::NAN).is_err());
    let point = WorldPoint::new(-0.0, 1.25).unwrap();
    let mut out = Encoder::new(16);
    point.encode(&mut out).unwrap();
    let bytes = out.finish();
    let restored = WorldPoint::decode(&mut Decoder::new(&bytes, 16).unwrap()).unwrap();
    assert_eq!(restored.x().to_bits(), (-0_f64).to_bits());
    assert_eq!(restored.y(), 1.25);
    assert!(Tick(u64::MAX).checked_advance(TickSpan(1)).is_err());
    assert_eq!(Tick(5).checked_advance(TickSpan(2)).unwrap(), Tick(7));
    let rectangle =
        CellRect::new(CellPoint { x: i32::MAX, y: -1 }, i64::from(i32::MAX) + 1, 1).unwrap();
    assert!(rectangle.contains(CellPoint { x: i32::MAX, y: 0 }));
    assert!(!rectangle.contains(CellPoint { x: i32::MAX, y: 1 }));
    assert_eq!(rectangle.area().unwrap(), 2);
    let mut out = Encoder::new(24);
    rectangle.encode(&mut out).unwrap();
    let bytes = out.finish();
    assert_eq!(
        CellRect::decode(&mut Decoder::new(&bytes, 24).unwrap()).unwrap(),
        rectangle
    );
    let mut ratio = Encoder::new(8);
    ratio.u32(2).unwrap();
    ratio.u32(2).unwrap();
    let bytes = ratio.finish();
    assert!(GridScale::decode(&mut Decoder::new(&bytes, 8).unwrap()).is_err());
    let all = CellRect::new(
        CellPoint {
            x: i32::MIN,
            y: i32::MIN,
        },
        i64::from(i32::MAX) + 1,
        i64::from(i32::MAX) + 1,
    )
    .unwrap();
    assert!(all.area().is_err());
}

#[test]
fn atomic_snapshot_restores_revision_and_revalidates_author_state() {
    let mut state = AtomicState::new(key("test:state"), Positive(1)).unwrap();
    let mut edit = state.transaction().unwrap();
    edit.apply(|value| {
        value.0 = 5;
        Ok(())
    })
    .unwrap();
    edit.prepare().unwrap().commit();
    let bytes = state
        .checkpoint(1024, |value, out| out.i32(value.0))
        .unwrap();
    let restored = AtomicState::restore(&bytes, key("test:state"), 1024, |input| {
        Ok(Positive(input.i32()?))
    })
    .unwrap();
    assert_eq!(restored.revision(), 1);
    assert_eq!(restored.view().0, 5);
    let mut invalid = bytes;
    let end = invalid.len();
    invalid[end - 4..].copy_from_slice(&(-1_i32).to_le_bytes());
    assert!(
        AtomicState::restore(&invalid, key("test:state"), 1024, |input| Ok(Positive(
            input.i32()?
        )))
        .is_err()
    );
}

#[test]
fn arena_rejects_duplicate_free_slots_before_publishing_candidate() {
    let mut arena: Arena<u8, EntityKind> = Arena::new(2).unwrap();
    let first = arena.insert(1).unwrap();
    let second = arena.insert(2).unwrap();
    arena.remove(first).unwrap();
    arena.remove(second).unwrap();
    let mut bytes = arena.checkpoint(128).unwrap();
    let end = bytes.len();
    // free-listの末尾を先頭と同じindexへ改変して重複を作ります。
    let first_index = bytes[end - 8..end - 4].to_vec();
    bytes[end - 4..].copy_from_slice(&first_index);
    assert!(Arena::<u8, EntityKind>::restore(&bytes, 2, 128).is_err());
}

#[test]
fn prepared_commit_defers_author_destructor_until_after_publication() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Tracked {
        value: u64,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl CandidateState for Tracked {
        fn try_copy(&self) -> EngineResult<Self> {
            Ok(Self {
                value: self.value,
                drops: Arc::clone(&self.drops),
            })
        }
        fn validate(&self) -> EngineResult<()> {
            Ok(())
        }
    }
    // dropsは試験専用の観測値であり、作品の正規状態として共有しません。
    let drops = Arc::new(AtomicUsize::new(0));
    let mut state = AtomicState::new(
        key("test:state"),
        Tracked {
            value: 1,
            drops: Arc::clone(&drops),
        },
    )
    .unwrap();
    let mut edit = state.transaction().unwrap();
    edit.apply(|candidate| {
        candidate.value = 2;
        Ok(())
    })
    .unwrap();
    let receipt = edit.prepare().unwrap().commit();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(state.view().value, 2);
    assert_eq!(state.revision(), receipt.revision);
    drop(receipt);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn grid_batch_failure_does_not_publish_partial_kernel_output() {
    let mut grid = GridChannel::new(key("test:field"), 1, layout(), 0_f32, grid_limits()).unwrap();
    let before = grid.checkpoint().unwrap();
    assert!(
        grid.edit_chunk((0, 0), |values| {
            values[0] = 4.0;
            Err(EngineError::new(
                ErrorCode::InvalidValue,
                "作者kernelの失敗",
            ))
        })
        .is_err()
    );
    assert_eq!(grid.checkpoint().unwrap(), before);
    assert!(
        grid.edit_chunk((0, 0), |values| {
            values[0] = 4.0;
            values[1] = f32::NAN;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(grid.checkpoint().unwrap(), before);
    grid.edit_chunk((0, 0), |values| {
        values[0] = 4.0;
        values[4095] = 5.0;
        Ok(())
    })
    .unwrap();
    assert_eq!(grid.get(CellPoint { x: 0, y: 0 }), 4.0);
    assert_eq!(grid.get(CellPoint { x: 63, y: 63 }), 5.0);
    assert_eq!(grid.revision(), 1);
    grid.edit_chunk((0, 0), |values| {
        values.fill(0.0);
        Ok(())
    })
    .unwrap();
    assert_eq!(grid.allocated_chunks(), 0);
}
