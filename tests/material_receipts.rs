//! cell変更とreceiptが同時に確定し、失敗時にどちらも残らないことを公開APIで検証します。

use charon_engine::{
    foundation::{StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;
fn key(text: &str) -> StableKey {
    StableKey::parse(text).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn journal() -> MaterialReceiptJournal {
    MaterialReceiptJournal {
        key: key("test:journal"),
        schema: key("test:receipt-schema"),
    }
}
fn setup(capacity: u32, schema_bytes: usize, limits: WorldLimits) -> (World, LayerId) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    for name in ["test:front", "test:back"] {
        registry
            .layer(LayerDef {
                key: key(name),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
    }
    for (name, base, mass) in [
        ("test:water", MaterialBase::Liquid, 5),
        ("test:sand", MaterialBase::Powder, 10),
        ("test:stone", MaterialBase::Solid, 20),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, base)
                    .unwrap()
                    .mass_per_cell(mass),
            )
            .unwrap();
    }
    registry
        .schema(Schema::of::<MaterialReceiptLog>(journal().schema, 1, schema_bytes).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:quantity"), 1, 8).unwrap())
        .unwrap();
    for (name, owner) in [
        ("test:energy", Some("test:front")),
        ("test:timer", Some("test:front")),
        ("test:back-energy", Some("test:back")),
        ("test:field", None),
    ] {
        let definition = ChannelDefinition {
            key: key(name),
            version: 1,
            value_schema: key("test:quantity"),
            layout: GridLayout {
                origin: p(-2, 0),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0u64,
            limits: GridLimits {
                chunks: 8,
                checkpoint_bytes: 1_000_000,
            },
        };
        if let Some(owner) = owner {
            registry.material_channel(key(owner), definition).unwrap();
        } else {
            registry.channel(definition).unwrap();
        }
    }
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        content,
        limits,
    )
    .unwrap();
    let log = MaterialReceiptLog::new(&world, journal().key, capacity).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.set_state(journal().key, journal().schema, &log)
        })
        .unwrap();
    (world, layer)
}
fn world() -> (World, LayerId) {
    setup(16, 32_768, WorldLimits::default())
}
fn core(world: &World, name: &str) -> CellCore {
    CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key(name)))
            .unwrap(),
        ..CellCore::AIR
    }
}
fn at(world: &World, layer: LayerId, point: CellPoint) -> CellCore {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(cell) => cell.core,
        _ => panic!("cellが必要"),
    }
}
fn value(world: &World, name: &str, point: CellPoint) -> u64 {
    match world
        .channel_value::<u64>(key(name), p(point.x + 2, point.y))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("channelが必要"),
    }
}
fn reason(kind: MaterialReceiptKind) -> MaterialChangeReason {
    MaterialChangeReason {
        key: key("game:rule"),
        version: 1,
        kind,
    }
}
fn change(point: CellPoint, expected: CellCore, output: CellCore) -> MaterialCellChange {
    MaterialCellChange {
        point,
        expected,
        output,
        payload: MaterialPayloadPolicy::ResetChannels,
    }
}

#[test]
fn source_records_canonical_changes_amounts_and_explicit_channel_reset() {
    let (mut world, layer) = world();
    let mut water = core(&world, "test:water");
    water.flags = 4;
    water.variation = 17;
    world
        .edit(|edit| {
            for name in [
                "test:energy",
                "test:timer",
                "test:back-energy",
                "test:field",
            ] {
                edit.set_channel(key(name), p(3, 2), 91u64)?;
            }
            Ok(())
        })
        .unwrap();
    let receipt = world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[
                    change(p(2, 2), CellCore::AIR, water),
                    change(p(1, 2), CellCore::AIR, water),
                ],
            )
        })
        .unwrap();
    assert_eq!(receipt.sequence(), 0);
    assert_eq!(receipt.tick(), 0);
    assert_eq!(
        receipt
            .changes()
            .iter()
            .map(|change| change.point)
            .collect::<Vec<_>>(),
        [p(1, 2), p(2, 2)]
    );
    assert!(receipt.consumed().is_empty());
    assert_eq!(
        receipt.produced(),
        &[MaterialReceiptAmount {
            material: key("test:water"),
            tokens: 2,
            mass: 10
        }]
    );
    assert_eq!(receipt.produced_mass().unwrap(), 10);
    assert_eq!(at(&world, layer, p(1, 2)), water);
    assert_eq!(value(&world, "test:energy", p(1, 2)), 0);
    assert_eq!(value(&world, "test:timer", p(1, 2)), 0);
    assert_eq!(value(&world, "test:back-energy", p(1, 2)), 91);
    assert_eq!(value(&world, "test:field", p(1, 2)), 91);
    let log = world.state::<MaterialReceiptLog>(journal().key).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log.receipts().next().unwrap().changes(), receipt.changes());
}

#[test]
fn conversion_can_preserve_creator_migrated_payload_and_report_intentional_mass_change() {
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    let sand = core(&world, "test:sand");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(1, 2), water)?;
            edit.set_channel(key("test:energy"), p(3, 2), 50u64)
        })
        .unwrap();
    let receipt = world
        .edit(|edit| {
            // 作者の移行処理も同じ取引へ含め、保持方針をreceiptへ残します。
            edit.set_channel(key("test:energy"), p(3, 2), 75u64)?;
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Conversion),
                &[MaterialCellChange {
                    payload: MaterialPayloadPolicy::KeepChannels,
                    ..change(p(1, 2), water, sand)
                }],
            )
        })
        .unwrap();
    assert_eq!(
        (
            receipt.consumed_mass().unwrap(),
            receipt.produced_mass().unwrap()
        ),
        (5, 10)
    );
    assert_eq!(value(&world, "test:energy", p(1, 2)), 75);
    assert_eq!(at(&world, layer, p(1, 2)), sand);
    assert_eq!(
        receipt.changes()[0].payload,
        MaterialPayloadPolicy::KeepChannels
    );
}

#[test]
fn conversion_and_sink_record_multi_cell_counts_without_forcing_mass_conservation() {
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    let sand = core(&world, "test:sand");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(1, 2), water)?;
            edit.set_cell(layer, p(2, 2), water)
        })
        .unwrap();
    let receipt = world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Conversion),
                &[
                    change(p(1, 2), water, sand),
                    change(p(2, 2), water, CellCore::AIR),
                ],
            )
        })
        .unwrap();
    assert_eq!(receipt.consumed()[0].tokens, 2);
    assert_eq!(receipt.produced()[0].tokens, 1);
    assert_eq!(
        (
            receipt.consumed_mass().unwrap(),
            receipt.produced_mass().unwrap()
        ),
        (10, 10)
    );
    let sink = world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Sink),
                &[change(p(1, 2), sand, CellCore::AIR)],
            )
        })
        .unwrap();
    assert_eq!(sink.sequence(), 1);
    assert!(sink.produced().is_empty());
    assert_eq!(sink.consumed_mass().unwrap(), 10);
    assert_eq!(at(&world, layer, p(1, 2)), CellCore::AIR);
}

#[test]
fn stale_expected_values_poison_the_whole_transaction_even_when_error_is_ignored() {
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.set_channel(key("test:field"), p(3, 2), 99u64)?;
                let _ = edit.apply_material_changes(
                    layer,
                    journal(),
                    reason(MaterialReceiptKind::Conversion),
                    &[change(p(1, 2), water, water)],
                );
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn receipt_capacity_failure_preserves_cells_and_sequence_then_can_resume_after_growth() {
    let (mut world, layer) = setup(1, 32_768, WorldLimits::default());
    let water = core(&world, "test:water");
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)],
            )
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(2, 2), CellCore::AIR, water)]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let mut log = world.state::<MaterialReceiptLog>(journal().key).unwrap();
    log.set_capacity(2).unwrap();
    world
        .edit(|edit| edit.set_state(journal().key, journal().schema, &log))
        .unwrap();
    let receipt = world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(2, 2), CellCore::AIR, water)],
            )
        })
        .unwrap();
    assert_eq!(receipt.sequence(), 1);
    let bytes = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let log = restored.state::<MaterialReceiptLog>(journal().key).unwrap();
    assert_eq!(log.next_sequence(), 2);
    assert_eq!(
        log.receipts()
            .map(|receipt| receipt.sequence())
            .collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn receipt_codec_capacity_failure_prevents_material_publication() {
    let (mut world, layer) = setup(16, 128, WorldLimits::default());
    let water = core(&world, "test:water");
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn duplicate_unavailable_invalid_and_wrong_kind_changes_never_append_receipts() {
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    let valid = change(p(1, 2), CellCore::AIR, water);
    let mut invalid = valid;
    invalid.output.flags = u16::MAX;
    let before = world.checkpoint(4_000_000).unwrap();
    for changes in [
        vec![],
        vec![valid, valid],
        vec![change(p(128, 2), CellCore::AIR, water)],
        vec![invalid],
    ] {
        assert!(
            world
                .edit(|edit| edit.apply_material_changes(
                    layer,
                    journal(),
                    reason(MaterialReceiptKind::Source),
                    &changes
                ))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
    for kind in [MaterialReceiptKind::Sink, MaterialReceiptKind::Conversion] {
        assert!(
            world
                .edit(|edit| edit.apply_material_changes(layer, journal(), reason(kind), &[valid]))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
    assert!(
        world
            .state::<MaterialReceiptLog>(journal().key)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reset_channel_writes_are_included_in_the_edit_budget() {
    let (mut world, layer) = setup(
        16,
        32_768,
        WorldLimits {
            edit_cells: 2,
            ..WorldLimits::default()
        },
    );
    let water = core(&world, "test:water");
    let before = world.checkpoint(4_000_000).unwrap();
    // 地形1件と対象layerの物質channel2件の合計3件を予約する必要があります。
    assert!(
        world
            .edit(|edit| edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[MaterialCellChange {
                    payload: MaterialPayloadPolicy::KeepChannels,
                    ..change(p(1, 2), CellCore::AIR, water)
                }],
            )
        })
        .unwrap();
}

#[test]
fn foreign_world_and_journal_logs_are_rejected() {
    let (mut world, _) = world();
    let other = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        Arc::clone(world.content()),
        WorldLimits::default(),
    )
    .unwrap();
    for log in [
        MaterialReceiptLog::new(&other, journal().key, 16).unwrap(),
        MaterialReceiptLog::new(&world, key("test:other-journal"), 16).unwrap(),
    ] {
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.set_state(journal().key, journal().schema, &log))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}

#[test]
fn receipt_log_codec_rejects_truncation_and_preserves_canonical_record_bytes() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)],
            )
        })
        .unwrap();
    let log = world.state::<MaterialReceiptLog>(journal().key).unwrap();
    let mut out = Encoder::new(32_768);
    log.encode(&mut out).unwrap();
    let bytes = out.finish();
    for cut in 0..bytes.len() {
        assert!(
            MaterialReceiptLog::decode(&mut Decoder::new(&bytes[..cut], 32_768).unwrap()).is_err()
        );
    }
    let restored = MaterialReceiptLog::decode(&mut Decoder::new(&bytes, 32_768).unwrap()).unwrap();
    let mut after = Encoder::new(32_768);
    restored.encode(&mut after).unwrap();
    assert_eq!(bytes, after.finish());
}

#[test]
fn later_transaction_failure_discards_the_receipt_and_materials_together() {
    use charon_engine::foundation::{EngineError, ErrorCode};
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let receipt = edit.apply_material_changes(
                    layer,
                    journal(),
                    reason(MaterialReceiptKind::Source),
                    &[change(p(1, 2), CellCore::AIR, water)],
                )?;
                assert_eq!(receipt.sequence(), 0);
                Err::<(), _>(EngineError::new(
                    ErrorCode::Conflict,
                    "後続の作者処理が失敗",
                ))
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn block_instances_are_not_implicitly_consumed_by_cell_receipts() {
    let (mut world, layer) = world();
    let block = world
        .edit(|edit| {
            edit.place_block(
                layer,
                p(1, 2),
                MaterialHandle(key("test:stone")),
                Orientation::Original,
            )
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let stone = at(&world, layer, p(1, 2));
    assert!(
        world
            .edit(|edit| edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Sink),
                &[change(p(1, 2), stone, CellCore::AIR)]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    assert!(world.block(block).is_ok());
}

#[test]
fn content_mismatch_is_not_accepted_as_the_same_journal() {
    let (first, _) = world();
    let log = first.state::<MaterialReceiptLog>(journal().key).unwrap();
    // schema上限の違いもContentの違いです。同じWorld IDだけでは受け入れません。
    let (mut second, _) = setup(16, 16_384, WorldLimits::default());
    let before = second.checkpoint(4_000_000).unwrap();
    assert!(
        second
            .edit(|edit| edit.set_state(journal().key, journal().schema, &log))
            .is_err()
    );
    assert_eq!(before, second.checkpoint(4_000_000).unwrap());
}

#[derive(Default)]
struct Archives(std::collections::BTreeMap<[u8; 32], Vec<u8>>);
impl ArchiveStorage for Archives {
    fn put(
        &mut self,
        hash: [u8; 32],
        bytes: &[u8],
    ) -> charon_engine::foundation::EngineResult<SaveDurability> {
        assert_eq!(blake3::hash(bytes).as_bytes(), &hash);
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndAtomicPublish)
    }
    fn read(
        &mut self,
        hash: [u8; 32],
        limit: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<u8>> {
        let bytes = self.0.get(&hash).unwrap();
        assert!(bytes.len() <= limit);
        Ok(bytes.clone())
    }
}
#[test]
fn region_eviction_and_restore_preserve_existing_receipts_without_reissuing_tokens() {
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)],
            )
        })
        .unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            4_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(
        world
            .state::<MaterialReceiptLog>(journal().key)
            .unwrap()
            .len(),
        1
    );
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let loaded = restored
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 4_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    restored.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(at(&restored, layer, p(1, 2)), water);
    let log = restored.state::<MaterialReceiptLog>(journal().key).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log.receipts().next().unwrap().produced()[0].tokens, 1);
}

#[test]
fn restored_amounts_must_match_the_recorded_cell_changes_before_reuse() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    let (mut world, layer) = world();
    let water = core(&world, "test:water");
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                journal(),
                reason(MaterialReceiptKind::Source),
                &[change(p(1, 2), CellCore::AIR, water)],
            )
        })
        .unwrap();
    let log = world.state::<MaterialReceiptLog>(journal().key).unwrap();
    let mut out = Encoder::new(32_768);
    log.encode(&mut out).unwrap();
    let mut bytes = out.finish();
    // 最後の素材量だけを、比率が正しい2token・mass10へ変えます。cell変更は1件のままです。
    let length = bytes.len();
    bytes[length - 24..length - 16].copy_from_slice(&2u64.to_le_bytes());
    bytes[length - 16..].copy_from_slice(&10u128.to_le_bytes());
    let forged = MaterialReceiptLog::decode(&mut Decoder::new(&bytes, 32_768).unwrap()).unwrap();
    assert!(forged.validate_world(&world).is_err());
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(journal().key, journal().schema, &forged))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}
