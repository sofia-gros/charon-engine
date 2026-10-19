//! 混合の整数比率・正確な成分量、取引と保存の回帰試験。
use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
use std::{collections::BTreeMap, sync::Arc};
fn key(text: &str) -> StableKey {
    StableKey::parse(text).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn limits() -> MixtureLimits {
    MixtureLimits {
        entries: 8,
        components: 8,
        receipts: 16,
    }
}
fn config() -> MixtureEditConfig {
    MixtureEditConfig {
        schema: key("test:mixtures"),
        channel: key("test:composition"),
        journal: MaterialReceiptJournal {
            key: key("test:journal"),
            schema: key("test:receipts"),
        },
    }
}
fn content(chunks: u32, store_bytes: usize) -> Arc<Content> {
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
    for (name, mass) in [
        ("test:water", 5),
        ("test:oil", 3),
        ("test:solution", 4),
        ("test:dense", 8),
        ("test:salt", 2),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, MaterialBase::Liquid)
                    .unwrap()
                    .mass_per_cell(mass),
            )
            .unwrap();
    }
    registry
        .schema(Schema::of::<MixtureRef>(key("test:ref"), 1, 32).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MixtureStore>(config().schema, 1, store_bytes).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialReceiptLog>(config().journal.schema, 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:u64"), 1, 8).unwrap())
        .unwrap();
    let layout = GridLayout {
        origin: p(-2, 0),
        x_scale: GridScale::new(1, 1).unwrap(),
        y_scale: GridScale::new(1, 1).unwrap(),
    };
    let definition = |name| ChannelDefinition {
        key: key(name),
        version: 1,
        value_schema: key("test:ref"),
        layout,
        default: MixtureRef::NONE,
        limits: GridLimits {
            chunks,
            checkpoint_bytes: 1_000_000,
        },
    };
    registry
        .material_channel(key("test:front"), definition("test:composition"))
        .unwrap();
    registry.channel(definition("test:position-ref")).unwrap();
    for (name, follow) in [("test:energy", true), ("test:field", false)] {
        let definition = ChannelDefinition {
            key: key(name),
            version: 1,
            value_schema: key("test:u64"),
            layout,
            default: 0u64,
            limits: GridLimits {
                chunks: 8,
                checkpoint_bytes: 1_000_000,
            },
        };
        if follow {
            registry
                .material_channel(key("test:front"), definition)
                .unwrap();
        } else {
            registry.channel(definition).unwrap();
        }
    }
    registry.freeze().unwrap()
}
fn fixture(
    chunks: u32,
    store_bytes: usize,
    capacity: MixtureLimits,
    journal_capacity: u32,
) -> World {
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content(chunks, store_bytes),
        WorldLimits::default(),
    )
    .unwrap();
    let store = MixtureStore::new(&world, capacity).unwrap();
    let journal = MaterialReceiptLog::new(&world, config().journal.key, journal_capacity).unwrap();
    world
        .edit(|edit| {
            for x in -1..=1 {
                edit.insert_empty_chunk(ChunkCoord::new(x, 0)?)?;
            }
            edit.set_state(MixtureStore::key(), config().schema, &store)?;
            edit.set_state(config().journal.key, config().journal.schema, &journal)
        })
        .unwrap();
    world
}
fn world() -> World {
    fixture(8, 1_000_000, limits(), 32)
}
fn layer(world: &World) -> LayerId {
    world.content().layer_id(key("test:front")).unwrap()
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
fn state(a: u32, b: u32) -> MixtureState {
    MixtureState::new(&[
        MixturePart {
            material: key("test:water"),
            ratio: a,
        },
        MixturePart {
            material: key("test:oil"),
            ratio: b,
        },
    ])
    .unwrap()
}
fn reason(kind: MaterialReceiptKind) -> MaterialChangeReason {
    MaterialChangeReason {
        key: key("test:rule"),
        version: 1,
        kind,
    }
}
fn change(
    point: CellPoint,
    expected: CellCore,
    output: CellCore,
    before: MixtureRef,
    after: MixtureRef,
) -> MixtureCellChange {
    MixtureCellChange {
        cell: MaterialCellChange {
            point,
            expected,
            output,
            payload: MaterialPayloadPolicy::KeepChannels,
        },
        expected: before,
        output: after,
    }
}
fn seed(world: &mut World) {
    let layer = layer(world);
    let water = core(world, "test:water");
    let oil = core(world, "test:oil");
    world
        .edit(|edit| {
            edit.apply_material_changes(
                layer,
                config().journal,
                reason(MaterialReceiptKind::Source),
                &[
                    MaterialCellChange {
                        point: p(0, 2),
                        expected: CellCore::AIR,
                        output: water,
                        payload: MaterialPayloadPolicy::ResetChannels,
                    },
                    MaterialCellChange {
                        point: p(1, 2),
                        expected: CellCore::AIR,
                        output: oil,
                        payload: MaterialPayloadPolicy::ResetChannels,
                    },
                ],
            )?;
            edit.set_channel(key("test:energy"), p(2, 2), 101u64)?;
            edit.set_channel(key("test:energy"), p(3, 2), 202u64)?;
            edit.set_channel(key("test:field"), p(2, 2), 999u64)
        })
        .unwrap();
}
fn mix(world: &mut World, reference: MixtureRef) -> MaterialReceipt {
    let layer = layer(world);
    let water = core(world, "test:water");
    let oil = core(world, "test:oil");
    let solution = core(world, "test:solution");
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[
                    change(p(1, 2), oil, solution, MixtureRef::NONE, reference),
                    change(p(0, 2), water, solution, MixtureRef::NONE, reference),
                ],
            )
        })
        .unwrap()
}
fn reference(world: &mut World) -> MixtureRef {
    world
        .edit(|edit| edit.register_mixture(config().schema, &state(1, 1)))
        .unwrap()
}
fn ref_at(world: &World, point: CellPoint) -> MixtureRef {
    match world
        .channel_value::<MixtureRef>(config().channel, p(point.x + 2, point.y))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("混合参照coverage"),
    }
}
fn quantity(world: &World, name: &str, reference: MixtureRef) -> Vec<MaterialComponentQuantity> {
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    store
        .quantities(world.content(), core(world, name).material, reference)
        .unwrap()
}
#[test]
fn normalization_combines_duplicates_and_interns_identical_states() {
    let mut world = world();
    let a = state(2, 2);
    let b = MixtureState::new(&[
        MixturePart {
            material: key("test:oil"),
            ratio: 2,
        },
        MixturePart {
            material: key("test:water"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:water"),
            ratio: 1,
        },
    ])
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(a.parts()[0].material, key("test:oil"));
    assert_eq!(a.parts()[0].ratio, 1);
    let (first, second) = world
        .edit(|edit| {
            Ok((
                edit.register_mixture(config().schema, &a)?,
                edit.register_mixture(config().schema, &b)?,
            ))
        })
        .unwrap();
    assert_eq!(first, second);
    assert!(
        world
            .edit(|edit| edit.register_mixture(config().journal.schema, &a))
            .is_err()
    );
    assert_eq!(
        world
            .state::<MixtureStore>(MixtureStore::key())
            .unwrap()
            .len(),
        1
    );
    assert!(
        MixtureState::new(&[MixturePart {
            material: key("test:water"),
            ratio: 1
        }])
        .is_err()
    );
    assert!(
        MixtureState::new(&[
            MixturePart {
                material: key("test:water"),
                ratio: 0
            },
            MixturePart {
                material: key("test:oil"),
                ratio: 1
            }
        ])
        .is_err()
    );
}
#[test]
fn fractional_components_mix_separate_and_keep_payloads() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let receipt = mix(&mut world, reference);
    assert_eq!(receipt.consumed_mass().unwrap(), 8);
    assert_eq!(receipt.produced_mass().unwrap(), 8);
    let amounts = quantity(&world, "test:solution", reference);
    for amount in &amounts {
        assert_eq!(amount.tokens, MaterialQuantity::new(1, 2).unwrap());
    }
    assert_eq!(amounts[0].mass, MaterialQuantity::new(3, 2).unwrap());
    assert_eq!(amounts[1].mass, MaterialQuantity::new(5, 2).unwrap());
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let oil = core(&world, "test:oil");
    let solution = core(&world, "test:solution");
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[
                    change(p(0, 2), solution, water, reference, MixtureRef::NONE),
                    change(p(1, 2), solution, oil, reference, MixtureRef::NONE),
                ],
            )
        })
        .unwrap();
    assert_eq!(ref_at(&world, p(0, 2)), MixtureRef::NONE);
    assert_eq!(ref_at(&world, p(1, 2)), MixtureRef::NONE);
    assert!(matches!(
        world
            .channel_value::<u64>(key("test:energy"), p(2, 2))
            .unwrap(),
        ChannelQuery::Loaded(101)
    ));
    assert!(matches!(
        world
            .channel_value::<u64>(key("test:energy"), p(3, 2))
            .unwrap(),
        ChannelQuery::Loaded(202)
    ));
    assert!(matches!(
        world
            .channel_value::<u64>(key("test:field"), p(2, 2))
            .unwrap(),
        ChannelQuery::Loaded(999)
    ));
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    assert_eq!(store.receipts().count(), 2);
    store.validate_world(&world).unwrap();
}
#[test]
fn wrong_composition_and_expected_ref_poison_the_outer_transaction() {
    let mut world = world();
    let reference = world
        .edit(|edit| edit.register_mixture(config().schema, &state(2, 1)))
        .unwrap();
    seed(&mut world);
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let oil = core(&world, "test:oil");
    let solution = core(&world, "test:solution");
    for stale_ref in [false, true] {
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| {
                    edit.set_channel(key("test:field"), p(2, 2), 1u64)?;
                    let _ = edit.apply_mixture_changes(
                        layer,
                        config(),
                        reason(MaterialReceiptKind::Conversion),
                        MixtureConservation::PreserveComponents,
                        &[
                            change(
                                p(0, 2),
                                water,
                                solution,
                                if stale_ref {
                                    reference
                                } else {
                                    MixtureRef::NONE
                                },
                                reference,
                            ),
                            change(p(1, 2), oil, solution, MixtureRef::NONE, reference),
                        ],
                    );
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}
#[test]
fn intentional_conversion_records_component_changes_without_forcing_mass_conservation() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let oil = core(&world, "test:oil");
    let solution = core(&world, "test:solution");
    let changes = [
        change(p(0, 2), water, solution, MixtureRef::NONE, reference),
        change(
            p(1, 2),
            oil,
            CellCore::AIR,
            MixtureRef::NONE,
            MixtureRef::NONE,
        ),
    ];
    assert!(
        world
            .edit(|edit| edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &changes
            ))
            .is_err()
    );
    let receipt = world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::RecordConversion,
                &changes,
            )
        })
        .unwrap();
    assert_eq!(receipt.consumed_mass().unwrap(), 8);
    assert_eq!(receipt.produced_mass().unwrap(), 4);
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}
#[test]
fn source_and_sink_of_composed_material_are_recorded_with_refs() {
    let mut world = world();
    let reference = reference(&mut world);
    let layer = layer(&world);
    let solution = core(&world, "test:solution");
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Source),
                MixtureConservation::RecordConversion,
                &[change(
                    p(-1, 2),
                    CellCore::AIR,
                    solution,
                    MixtureRef::NONE,
                    reference,
                )],
            )
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Sink),
                MixtureConservation::RecordConversion,
                &[change(
                    p(-1, 2),
                    solution,
                    CellCore::AIR,
                    reference,
                    MixtureRef::NONE,
                )],
            )
        })
        .unwrap();
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    assert_eq!(store.receipts().count(), 2);
    store.validate_world(&world).unwrap();
}
#[test]
fn definitions_receipts_and_codec_capacities_never_consume_input() {
    let mut world = fixture(
        8,
        1_000_000,
        MixtureLimits {
            entries: 1,
            components: 2,
            receipts: 1,
        },
        32,
    );
    let reference = reference(&mut world);
    seed(&mut world);
    let unknown = MixtureState::new(&[
        MixturePart {
            material: key("test:missing"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:oil"),
            ratio: 1,
        },
    ])
    .unwrap();
    assert!(
        world
            .edit(|edit| edit.register_mixture(config().schema, &unknown))
            .is_err()
    );
    let three = MixtureState::new(&[
        MixturePart {
            material: key("test:water"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:oil"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:salt"),
            ratio: 1,
        },
    ])
    .unwrap();
    assert!(
        world
            .edit(|edit| edit.register_mixture(config().schema, &three))
            .is_err()
    );
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.register_mixture(config().schema, &state(2, 1)))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    mix(&mut world, reference);
    let layer = layer(&world);
    let solution = core(&world, "test:solution");
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[change(p(0, 2), solution, solution, reference, reference),]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let mut small = fixture(8, 80, limits(), 32);
    let before = small.checkpoint(4_000_000).unwrap();
    assert!(
        small
            .edit(|edit| edit.register_mixture(config().schema, &state(1, 1)))
            .is_err()
    );
    assert_eq!(before, small.checkpoint(4_000_000).unwrap());
}
#[test]
fn channel_capacity_failure_rolls_back_receipt_refs_and_cells() {
    let mut world = fixture(1, 1_000_000, limits(), 32);
    let reference = reference(&mut world);
    seed(&mut world);
    mix(&mut world, reference);
    let layer = layer(&world);
    let solution = core(&world, "test:solution");
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[
                    change(
                        p(0, 2),
                        solution,
                        CellCore::AIR,
                        reference,
                        MixtureRef::NONE
                    ),
                    change(
                        p(63, 2),
                        CellCore::AIR,
                        solution,
                        MixtureRef::NONE,
                        reference
                    ),
                ]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}
#[test]
fn store_scope_content_and_channel_ownership_are_checked() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let other = World::new(
        WorldId::from_parts(2, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        Arc::clone(world.content()),
        WorldLimits::default(),
    )
    .unwrap();
    let other_store = MixtureStore::new(&other, limits()).unwrap();
    assert!(other_store.resolve(reference).is_err());
    assert!(MixtureStore::new(&world, limits()).is_err());
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    assert!(
        store
            .quantities(
                &content(1, 1_000_000),
                core(&world, "test:solution").material,
                reference
            )
            .is_err()
    );
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let before = world.checkpoint(4_000_000).unwrap();
    let mut wrong = config();
    wrong.channel = key("test:position-ref");
    assert!(
        world
            .edit(|edit| edit.apply_mixture_changes(
                layer,
                wrong,
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[change(
                    p(0, 2),
                    water,
                    water,
                    MixtureRef::NONE,
                    MixtureRef::NONE
                ),]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let back = world.content().layer_id(key("test:back")).unwrap();
    let solution = core(&world, "test:solution");
    assert!(
        world
            .edit(|edit| edit.apply_mixture_changes(
                back,
                config(),
                reason(MaterialReceiptKind::Source),
                MixtureConservation::RecordConversion,
                &[change(
                    p(0, 2),
                    CellCore::AIR,
                    solution,
                    MixtureRef::NONE,
                    reference
                )]
            ))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}
#[test]
fn reference_moves_with_material_and_survives_world_save() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    mix(&mut world, reference);
    let layer = layer(&world);
    world
        .edit(|edit| edit.exchange_cells(layer, p(0, 2), p(-1, 2)))
        .unwrap();
    assert_eq!(ref_at(&world, p(0, 2)), MixtureRef::NONE);
    assert_eq!(ref_at(&world, p(-1, 2)), reference);
    let bytes = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(ref_at(&restored, p(-1, 2)), reference);
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), bytes);
    restored
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&restored)
        .unwrap();
}
#[test]
fn reset_payload_policy_clears_other_material_channels_but_restores_new_composition() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let oil = core(&world, "test:oil");
    let solution = core(&world, "test:solution");
    let mut changes = [
        change(p(0, 2), water, solution, MixtureRef::NONE, reference),
        change(p(1, 2), oil, solution, MixtureRef::NONE, reference),
    ];
    for change in &mut changes {
        change.cell.payload = MaterialPayloadPolicy::ResetChannels;
    }
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &changes,
            )
        })
        .unwrap();
    assert_eq!(ref_at(&world, p(0, 2)), reference);
    assert!(matches!(
        world
            .channel_value::<u64>(key("test:energy"), p(2, 2))
            .unwrap(),
        ChannelQuery::Loaded(0)
    ));
    assert!(matches!(
        world
            .channel_value::<u64>(key("test:field"), p(2, 2))
            .unwrap(),
        ChannelQuery::Loaded(999)
    ));
}
#[test]
fn quantity_arithmetic_and_codecs_are_exact_and_bounded() {
    assert_eq!(
        MaterialQuantity::new(2, 4).unwrap(),
        MaterialQuantity::new(1, 2).unwrap()
    );
    assert_eq!(
        MaterialQuantity::new(1, 2)
            .unwrap()
            .checked_add(MaterialQuantity::new(1, 3).unwrap())
            .unwrap(),
        MaterialQuantity::new(5, 6).unwrap()
    );
    assert_eq!(
        MaterialQuantity::new(u128::MAX, 2)
            .unwrap()
            .checked_mul(2)
            .unwrap(),
        MaterialQuantity::new(u128::MAX, 1).unwrap()
    );
    assert!(
        MaterialQuantity::new(u128::MAX, 1)
            .unwrap()
            .checked_add(MaterialQuantity::ONE)
            .is_err()
    );
    assert!(MaterialQuantity::new(1, 0).is_err());
    let mut out = Encoder::new(32);
    out.raw(&2u128.to_le_bytes()).unwrap();
    out.raw(&4u128.to_le_bytes()).unwrap();
    let bytes = out.finish();
    assert!(MaterialQuantity::decode(&mut Decoder::new(&bytes, 32).unwrap()).is_err());
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    mix(&mut world, reference);
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    let mut out = Encoder::new(1_000_000);
    store.encode(&mut out).unwrap();
    let bytes = out.finish();
    for end in 0..bytes.len() {
        assert!(
            MixtureStore::decode(&mut Decoder::new(&bytes[..end], 1_000_000).unwrap()).is_err()
        );
    }
    let restored = MixtureStore::decode(&mut Decoder::new(&bytes, 1_000_000).unwrap()).unwrap();
    restored.validate_world(&world).unwrap();
}
#[derive(Default)]
struct Archives(BTreeMap<[u8; 32], Vec<u8>>);
impl ArchiveStorage for Archives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndAtomicPublish)
    }
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        let bytes = &self.0[&hash];
        assert!(bytes.len() <= limit);
        Ok(bytes.clone())
    }
}
#[test]
fn archived_cells_keep_shared_composition_without_owning_mass_in_the_pool() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    mix(&mut world, reference);
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(world.channel_bytes(), 0);
    let pool: MixtureStore = world.state(MixtureStore::key()).unwrap();
    assert_eq!(pool.len(), 1);
    pool.validate_world(&world).unwrap();
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 4_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(ref_at(&world, p(0, 2)), reference);
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}

#[test]
fn packed_carrier_can_mix_and_separate_without_changing_component_totals() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let oil = core(&world, "test:oil");
    let dense = core(&world, "test:dense");
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[
                    change(p(0, 2), water, dense, MixtureRef::NONE, reference),
                    change(
                        p(1, 2),
                        oil,
                        CellCore::AIR,
                        MixtureRef::NONE,
                        MixtureRef::NONE,
                    ),
                ],
            )
        })
        .unwrap();
    for amount in quantity(&world, "test:dense", reference) {
        assert_eq!(amount.tokens, MaterialQuantity::ONE);
    }
    world
        .edit(|edit| {
            edit.apply_mixture_changes(
                layer,
                config(),
                reason(MaterialReceiptKind::Conversion),
                MixtureConservation::PreserveComponents,
                &[
                    change(p(0, 2), dense, water, reference, MixtureRef::NONE),
                    change(
                        p(1, 2),
                        CellCore::AIR,
                        oil,
                        MixtureRef::NONE,
                        MixtureRef::NONE,
                    ),
                ],
            )
        })
        .unwrap();
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}
#[test]
fn journal_and_witness_byte_capacity_fail_atomically() {
    for (bytes, journal_capacity) in [(128, 32), (1_000_000, 1)] {
        let mut world = fixture(8, bytes, limits(), journal_capacity);
        let reference = reference(&mut world);
        seed(&mut world);
        let layer = layer(&world);
        let water = core(&world, "test:water");
        let oil = core(&world, "test:oil");
        let solution = core(&world, "test:solution");
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.apply_mixture_changes(
                    layer,
                    config(),
                    reason(MaterialReceiptKind::Conversion),
                    MixtureConservation::PreserveComponents,
                    &[
                        change(p(0, 2), water, solution, MixtureRef::NONE, reference),
                        change(p(1, 2), oil, solution, MixtureRef::NONE, reference),
                    ]
                ))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}
#[test]
fn invalid_outputs_duplicates_unavailable_cells_and_blocks_are_rejected() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let layer = layer(&world);
    let water = core(&world, "test:water");
    let solution = core(&world, "test:solution");
    world
        .edit(|edit| {
            edit.place_block(
                layer,
                p(4, 2),
                MaterialHandle(key("test:water")),
                Orientation::Original,
            )
        })
        .unwrap();
    let cases = vec![
        vec![],
        vec![change(p(0, 2), water, water, MixtureRef::NONE, MixtureRef::NONE); 2],
        vec![change(
            p(0, 2),
            water,
            CellCore::AIR,
            MixtureRef::NONE,
            reference,
        )],
        vec![change(
            p(128, 2),
            CellCore::AIR,
            solution,
            MixtureRef::NONE,
            reference,
        )],
        vec![change(
            p(4, 2),
            water,
            water,
            MixtureRef::NONE,
            MixtureRef::NONE,
        )],
    ];
    for changes in cases {
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.apply_mixture_changes(
                    layer,
                    config(),
                    reason(MaterialReceiptKind::Conversion),
                    MixtureConservation::PreserveComponents,
                    &changes
                ))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}

#[test]
fn stale_store_cannot_replace_definitions_or_receipt_history() {
    let mut world = world();
    let before_definitions: MixtureStore = world.state(MixtureStore::key()).unwrap();
    let reference = reference(&mut world);
    let before_receipts: MixtureStore = world.state(MixtureStore::key()).unwrap();
    seed(&mut world);
    mix(&mut world, reference);
    for stale in [before_definitions, before_receipts] {
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &stale))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.set_state(MixtureStore::key(), key("test:u64"), &0u64);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let mut latest: MixtureStore = world.state(MixtureStore::key()).unwrap();
    latest
        .set_limits(MixtureLimits {
            entries: 16,
            components: 8,
            receipts: 32,
        })
        .unwrap();
    world
        .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &latest))
        .unwrap();
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}
#[test]
fn second_prepared_store_cannot_reset_an_installed_store() {
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content(8, 1_000_000),
        WorldLimits::default(),
    )
    .unwrap();
    let first = MixtureStore::new(&world, limits()).unwrap();
    let second = MixtureStore::new(&world, limits()).unwrap();
    world
        .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &first))
        .unwrap();
    let reference = reference(&mut world);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &second))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let other = world
        .edit(|edit| edit.register_mixture(config().schema, &state(2, 1)))
        .unwrap();
    assert_ne!(reference, other);
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    assert_eq!(store.resolve(reference).unwrap().unwrap(), &state(1, 1));
}

#[test]
fn journal_snapshot_cannot_remove_witness_targets() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let stale: MaterialReceiptLog = world.state(config().journal.key).unwrap();
    mix(&mut world, reference);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(config().journal.key, config().journal.schema, &stale))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let mut current: MaterialReceiptLog = world.state(config().journal.key).unwrap();
    current.set_capacity(64).unwrap();
    world
        .edit(|edit| edit.set_state(config().journal.key, config().journal.schema, &current))
        .unwrap();
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}

#[test]
fn branched_world_cannot_import_uncommitted_history_or_definitions() {
    let mut world = world();
    let reference = reference(&mut world);
    seed(&mut world);
    let before = world.checkpoint(4_000_000).unwrap();
    let mut branch = World::restore(
        &before,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    mix(&mut branch, reference);
    let store: MixtureStore = branch.state(MixtureStore::key()).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &store))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    let journal: MaterialReceiptLog = branch.state(config().journal.key).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(config().journal.key, config().journal.schema, &journal))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    branch
        .edit(|edit| edit.register_mixture(config().schema, &state(2, 1)))
        .unwrap();
    let definitions: MixtureStore = branch.state(MixtureStore::key()).unwrap();
    assert!(
        world
            .edit(|edit| edit.set_state(MixtureStore::key(), config().schema, &definitions))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    mix(&mut world, reference);
    world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .validate_world(&world)
        .unwrap();
}
