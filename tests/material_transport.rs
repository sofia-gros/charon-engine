//! E05の輸送契約を公開APIから確認します。素材と状態を別々に動かす誤り、
//! chunk境界での容量不足、無視された失敗による部分確定を回帰対象にします。

use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    runtime::{
        Engine, StepResult,
        input::TickInput,
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
    },
    world::*,
};
use std::sync::Arc;

fn key(text: &str) -> StableKey {
    StableKey::parse(text).unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn definition(name: &str, origin: CellPoint, chunks: u32) -> ChannelDefinition<u64> {
    ChannelDefinition {
        key: key(name),
        version: 1,
        value_schema: key("game:quantity"),
        layout: GridLayout {
            origin,
            x_scale: GridScale::new(1, 1).unwrap(),
            y_scale: GridScale::new(1, 1).unwrap(),
        },
        default: 0,
        limits: GridLimits {
            chunks,
            checkpoint_bytes: 1_000_000,
        },
    }
}
fn content(chunks: u32, follow: bool) -> Arc<Content> {
    content_with_pressure(chunks, follow, 8, 0)
}
fn content_with_pressure(
    chunks: u32,
    follow: bool,
    pressure_chunks: u32,
    default_pressure: i64,
) -> Arc<Content> {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    for name in ["game:front", "game:back"] {
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
        ("game:sand", MaterialBase::Powder, 10),
        ("game:water", MaterialBase::Liquid, 5),
        ("game:smoke", MaterialBase::Gas, 1),
        ("game:stone", MaterialBase::Solid, 20),
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
        .schema(Schema::of::<u64>(key("game:quantity"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<i64>(key("game:pressure-value"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<PressurePushSearch>(key("game:push-search"), 1, 262144).unwrap())
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("game:fluid-pressure"),
            version: 1,
            value_schema: key("game:pressure-value"),
            layout: GridLayout {
                origin: point(0, 0),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: default_pressure,
            limits: GridLimits {
                chunks: pressure_chunks,
                checkpoint_bytes: 1_000_000,
            },
        })
        .unwrap();
    for name in ["game:energy", "game:timer"] {
        let channel = definition(name, point(-2, 0), chunks);
        if follow {
            registry
                .material_channel(key("game:front"), channel)
                .unwrap();
        } else {
            registry.channel(channel).unwrap();
        }
    }
    registry
        .channel(definition("game:pressure", point(0, 0), 8))
        .unwrap();
    registry.freeze().unwrap()
}
fn world(content: Arc<Content>) -> World {
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            for x in -1..=1 {
                edit.insert_empty_chunk(ChunkCoord::new(x, 0)?)?;
            }
            Ok(())
        })
        .unwrap();
    world
}
fn core(world: &World, layer: LayerId, p: CellPoint) -> CellCore {
    match world.cell(layer, p).unwrap() {
        CellQuery::Loaded(value) => value.core,
        _ => panic!("読込済みcellが必要"),
    }
}
fn value(world: &World, name: &str, p: CellPoint) -> u64 {
    match world.channel_value::<u64>(key(name), p).unwrap() {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("読込済みchannelが必要"),
    }
}

#[test]
fn path_moves_all_material_channels_but_not_position_fields_and_roundtrips() {
    let content = content(8, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let back = content.layer_id(key("game:back")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("game:sand")))
        .unwrap();
    let water = content
        .material_id(MaterialHandle(key("game:water")))
        .unwrap();
    let mut world = world(Arc::clone(&content));
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(-1, 2),
                CellCore {
                    material: sand,
                    flags: 1,
                    variation: 77,
                },
            )?;
            edit.set_cell(
                front,
                point(0, 2),
                CellCore {
                    material: water,
                    flags: 4,
                    variation: 23,
                },
            )?;
            edit.set_cell(
                back,
                point(-1, 2),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            for (name, amount) in [("game:energy", 99u64), ("game:timer", 17u64)] {
                // channel原点が(-2,0)なので、Worldの(-1,2)はchannelの(1,2)です。
                edit.set_channel(key(name), point(1, 2), amount)?;
                edit.set_channel(key(name), point(2, 2), amount + 1)?;
            }
            edit.set_channel(key("game:pressure"), point(-1, 2), 456u64)?;
            edit.transport_path(front, &[point(-1, 2), point(0, 2), point(1, 2)])
        })
        .unwrap();
    assert_eq!(core(&world, front, point(-1, 2)), CellCore::AIR);
    assert_eq!(core(&world, front, point(0, 2)).variation, 77);
    assert_eq!(core(&world, front, point(1, 2)).material, water);
    assert_eq!(core(&world, back, point(-1, 2)).material, water);
    assert_eq!(value(&world, "game:energy", point(1, 2)), 0);
    assert_eq!(value(&world, "game:energy", point(2, 2)), 99);
    assert_eq!(value(&world, "game:timer", point(3, 2)), 18);
    assert_eq!(value(&world, "game:pressure", point(-1, 2)), 456);
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    for target in [&mut world, &mut restored] {
        target
            .edit(|edit| edit.exchange_cells(front, point(0, 2), point(1, 2)))
            .unwrap();
    }
    assert_eq!(
        world.canonical_hash(4_000_000).unwrap(),
        restored.canonical_hash(4_000_000).unwrap()
    );
}

#[test]
fn material_ownership_changes_content_and_is_not_schema_compatible() {
    assert_ne!(
        content(8, true).fingerprint(),
        content(8, false).fingerprint()
    );
    let original = world(content(8, true));
    let bytes = original.checkpoint(4_000_000).unwrap();
    assert!(World::restore(&bytes, content(8, false), WorldLimits::default(), 4_000_000).is_err());
}

#[test]
fn transfer_releases_old_channel_chunk_before_reserving_new_chunk() {
    let content = content(1, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("game:sand")))
        .unwrap();
    let mut world = world(content);
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(61, 2),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("game:energy"), point(63, 2), 19u64)?;
            edit.exchange_cells(front, point(61, 2), point(62, 2))
        })
        .unwrap();
    assert_eq!(value(&world, "game:energy", point(64, 2)), 19);
}

#[test]
fn capacity_error_in_one_channel_discards_prior_channel_and_terrain_changes() {
    let content = content(1, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("game:sand")))
        .unwrap();
    let mut world = world(content);
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(61, 2),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("game:energy"), point(63, 2), 19u64)?;
            edit.set_channel(key("game:timer"), point(63, 2), 5u64)?;
            edit.set_channel(key("game:timer"), point(0, 2), 7u64)
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.exchange_cells(front, point(61, 2), point(62, 2));
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn invalid_and_unavailable_paths_never_publish_prior_edits() {
    let content = content(8, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(content);
    let before = world.checkpoint(4_000_000).unwrap();
    for path in [
        vec![],
        vec![point(1, 1)],
        vec![point(1, 1), point(1, 1)],
        vec![point(1, 1), point(3, 1)],
        vec![point(1, 0), point(1, -1)],
        vec![point(1, 1), point(2, 1), point(1, 1)],
    ] {
        assert!(
            world
                .edit(|edit| {
                    edit.set_channel(key("game:pressure"), point(1, 1), 71u64)?;
                    let _ = edit.transport_path(front, &path);
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}

#[test]
fn block_cells_are_rejected_without_destroying_instance() {
    let content = content(8, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(content);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(1, 1),
                MaterialHandle(key("game:sand")),
                Orientation::Original,
            )
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.exchange_cells(front, point(1, 1), point(2, 1)))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    assert_eq!(world.block(block).unwrap().remaining_cells(), 1);
}

fn activate(world: &mut World, min: i32, max: i64) {
    world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:simulation"),
                region: ChunkRegion::new(ChunkCoord::new(min, 0)?, max, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
}

/// Runtime経路とWorldEdit参照経路を、同じ標準scalar solverでtickごとに比較します。
struct RuntimeScalarTransportSystem {
    layer: LayerId,
}
impl System for RuntimeScalarTransportSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        let layer = self.layer;
        worlds.edit_terrain(world, move |edit| {
            edit.transport_materials(layer, 8)?;
            Ok(())
        })?;
        Ok(WorkStatus::Complete)
    }
}
fn material(world: &World, name: &str) -> CellCore {
    CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key(name)))
            .unwrap(),
        ..CellCore::AIR
    }
}

#[test]
fn reference_transport_moves_once_and_obeys_activity_and_solid_cells() {
    let mut world = world(content(8, true));
    let front = world.content().layer_id(key("game:front")).unwrap();
    let sand = material(&world, "game:sand");
    let smoke = material(&world, "game:smoke");
    let stone = material(&world, "game:stone");
    world
        .edit(|edit| {
            edit.set_cell(front, point(3, 3), sand)?;
            edit.set_cell(front, point(8, 8), smoke)?;
            edit.set_cell(front, point(11, 11), stone)?;
            edit.set_cell(front, point(-1, 3), sand)?;
            Ok(())
        })
        .unwrap();
    let report = world
        .edit(|edit| edit.transport_materials(front, 16))
        .unwrap();
    assert_eq!(report.candidates, 0);
    activate(&mut world, 0, 1);
    let report = world
        .edit(|edit| edit.transport_materials(front, 16))
        .unwrap();
    assert_eq!(report.candidates, 3);
    assert_eq!(report.exchanges, 2);
    assert_eq!(core(&world, front, point(3, 4)), sand);
    assert_eq!(core(&world, front, point(8, 7)), smoke);
    assert_eq!(core(&world, front, point(11, 11)), stone);
    assert_eq!(core(&world, front, point(-1, 3)), sand);
}

#[test]
fn density_swap_preserves_payload_and_reserves_displaced_token() {
    let mut world = world(content(8, true));
    let front = world.content().layer_id(key("game:front")).unwrap();
    let sand = material(&world, "game:sand");
    let water = material(&world, "game:water");
    let stone = material(&world, "game:stone");
    world
        .edit(|edit| {
            edit.set_cell(front, point(3, 3), sand)?;
            edit.set_cell(front, point(3, 4), water)?;
            for (x, y) in [(3, 5), (2, 4), (4, 4)] {
                edit.set_cell(front, point(x, y), stone)?;
            }
            edit.set_channel(key("game:energy"), point(5, 3), 90u64)?;
            edit.set_channel(key("game:energy"), point(5, 4), 7u64)
        })
        .unwrap();
    activate(&mut world, 0, 1);
    let report = world
        .edit(|edit| edit.transport_materials(front, 16))
        .unwrap();
    assert_eq!(report.exchanges, 1);
    assert_eq!(core(&world, front, point(3, 4)), sand);
    assert_eq!(core(&world, front, point(3, 3)), water);
    assert_eq!(value(&world, "game:energy", point(5, 4)), 90);
    assert_eq!(value(&world, "game:energy", point(5, 3)), 7);
}

#[test]
fn reference_transport_rejects_budget_overflow_atomically() {
    let mut world = world(content(8, true));
    let front = world.content().layer_id(key("game:front")).unwrap();
    let sand = material(&world, "game:sand");
    world
        .edit(|edit| {
            edit.set_cell(front, point(3, 3), sand)?;
            edit.set_cell(front, point(8, 8), sand)
        })
        .unwrap();
    activate(&mut world, 0, 1);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.transport_materials(front, 1);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn scalar_restart_matches_uninterrupted_transport_and_conserves_material_counts() {
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let front = content.layer_id(key("game:front")).unwrap();
    let kinds =
        ["game:sand", "game:water", "game:smoke", "game:stone"].map(|name| material(&world, name));
    world
        .edit(|edit| {
            for index in 0..32 {
                edit.set_cell(
                    front,
                    point(index * 2 - 28, 10 + index % 3),
                    kinds[index as usize % 4],
                )?;
            }
            Ok(())
        })
        .unwrap();
    activate(&mut world, -1, 1);
    for _ in 0..12 {
        world
            .edit(|edit| {
                edit.transport_materials(front, 100)?;
                edit.advance_tick()
            })
            .unwrap();
    }
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    for _ in 0..12 {
        for target in [&mut world, &mut restored] {
            target
                .edit(|edit| {
                    edit.transport_materials(front, 100)?;
                    edit.advance_tick()
                })
                .unwrap();
        }
    }
    assert_eq!(
        world.canonical_hash(4_000_000).unwrap(),
        restored.canonical_hash(4_000_000).unwrap()
    );
    let mut counts = [0usize; 4];
    for y in 0..64 {
        for x in -64..64 {
            let cell = core(&world, front, point(x, y));
            if let Some(index) = kinds.iter().position(|kind| kind.material == cell.material) {
                counts[index] += 1;
            }
        }
    }
    assert_eq!(counts, [8; 4]);
}

fn pressure(world: &World, p: CellPoint) -> i64 {
    match world
        .channel_value::<i64>(key("game:fluid-pressure"), p)
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("読込済み圧力が必要"),
    }
}
fn pressure_config(limit: usize) -> PressureRelaxation {
    PressureRelaxation {
        channel: key("game:fluid-pressure"),
        iterations: 4,
        gravity_q16: 65536,
        cell_limit: limit,
    }
}
fn column(world: &mut World, closed: bool) -> LayerId {
    let layer = world.content().layer_id(key("game:front")).unwrap();
    let water = material(world, "game:water");
    let stone = material(world, "game:stone");
    world
        .edit(|edit| {
            for y in 2..=4 {
                edit.set_cell(layer, point(3, y), water)?;
                for x in [2, 4] {
                    edit.set_cell(layer, point(x, y), stone)?;
                }
            }
            edit.set_cell(layer, point(3, 5), stone)?;
            if closed {
                edit.set_cell(layer, point(3, 1), stone)?;
            }
            Ok(())
        })
        .unwrap();
    activate(world, 0, 1);
    layer
}

#[test]
fn pressure_matches_jacobi_reference_and_does_not_change_materials() {
    let mut world = world(content(8, true));
    let layer = column(&mut world, false);
    let before = (2..=4)
        .map(|y| core(&world, layer, point(3, y)))
        .collect::<Vec<_>>();
    let report = world
        .edit(|edit| edit.relax_pressure(layer, pressure_config(3)))
        .unwrap();
    assert_eq!(
        report,
        PressureRelaxationReport {
            cells: 3,
            open_components: 1,
            closed_components: 0
        }
    );
    assert_eq!(
        (2..=4)
            .map(|y| pressure(&world, point(3, y)))
            .collect::<Vec<_>>(),
        [0, 245760, 491520]
    );
    assert_eq!(
        before,
        (2..=4)
            .map(|y| core(&world, layer, point(3, y)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pressure_sleep_matches_each_reference_tick_and_wakes_after_field_edit() {
    let content = content(8, true);
    let mut reference = world(Arc::clone(&content));
    let mut sleeping = world(content);
    let layer = column(&mut reference, false);
    column(&mut sleeping, false);
    let config = PressureRelaxation {
        gravity_q16: 0,
        ..pressure_config(3)
    };
    let mut tracker = PressureSleepTracker::default();

    for expected_sleep in [false, true, true] {
        let expected = reference
            .edit(|edit| edit.relax_pressure(layer, config))
            .unwrap();
        let actual = sleeping
            .relax_pressure_sleeping(&mut tracker, layer, config)
            .unwrap();
        assert_eq!(actual.reference, expected);
        assert_eq!(actual.sleeping, expected_sleep);
        assert!(!actual.woke);
        let reference_bytes = reference.checkpoint(4_000_000).unwrap();
        let sleeping_bytes = sleeping.checkpoint(4_000_000).unwrap();
        assert_eq!(reference_bytes.len(), sleeping_bytes.len());
        assert_eq!(
            reference_bytes
                .iter()
                .zip(&sleeping_bytes)
                .position(|(left, right)| left != right),
            None
        );
    }

    for target in [&mut reference, &mut sleeping] {
        target
            .edit(|edit| edit.set_channel(key("game:fluid-pressure"), point(3, 3), 777_777i64))
            .unwrap();
    }
    let expected = reference
        .edit(|edit| edit.relax_pressure(layer, config))
        .unwrap();
    let actual = sleeping
        .relax_pressure_sleeping(&mut tracker, layer, config)
        .unwrap();
    assert!(actual.woke);
    assert!(!actual.sleeping);
    assert_eq!(actual.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );

    // 編集後のfieldが固定するまで参照緩和を両Worldへ適用し、各tickを照合する。
    let mut reached_sleep = false;
    for _ in 0..64 {
        let expected = reference
            .edit(|edit| edit.relax_pressure(layer, config))
            .unwrap();
        let actual = sleeping
            .relax_pressure_sleeping(&mut tracker, layer, config)
            .unwrap();
        assert_eq!(actual.reference, expected);
        assert!(!actual.woke);
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            sleeping.checkpoint(4_000_000).unwrap()
        );
        if actual.sleeping {
            reached_sleep = true;
            break;
        }
    }
    assert!(
        reached_sleep,
        "緩和後の圧力fieldが64tick以内に固定しなかった"
    );

    let changed_config = PressureRelaxation {
        gravity_q16: 65536,
        ..config
    };
    let expected = reference
        .edit(|edit| edit.relax_pressure(layer, changed_config))
        .unwrap();
    let actual = sleeping
        .relax_pressure_sleeping(&mut tracker, layer, changed_config)
        .unwrap();
    assert!(actual.woke);
    assert!(!actual.sleeping);
    assert_eq!(actual.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn pressure_sleep_tracker_restore_requires_reference_revalidation() {
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let layer = column(&mut world, false);
    let config = PressureRelaxation {
        gravity_q16: 0,
        ..pressure_config(3)
    };
    let mut tracker = PressureSleepTracker::default();
    world
        .relax_pressure_sleeping(&mut tracker, layer, config)
        .unwrap();
    assert!(
        world
            .relax_pressure_sleeping(&mut tracker, layer, config)
            .unwrap()
            .sleeping
    );

    let mut encoder = Encoder::new(4096);
    tracker.encode(&mut encoder).unwrap();
    let saved = encoder.finish();
    let mut decoder = Decoder::new(&saved, 4096).unwrap();
    let mut restored_tracker = PressureSleepTracker::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut restored_world =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();

    let first = restored_world
        .relax_pressure_sleeping(&mut restored_tracker, layer, config)
        .unwrap();
    assert!(!first.sleeping);
    assert!(
        restored_world
            .relax_pressure_sleeping(&mut restored_tracker, layer, config)
            .unwrap()
            .sleeping
    );
}

#[test]
fn closed_pressure_anchor_keeps_previous_component_average_and_survives_restore() {
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let layer = column(&mut world, true);
    world
        .edit(|edit| {
            for y in 2..=4 {
                edit.set_channel(
                    key("game:fluid-pressure"),
                    point(3, y),
                    i64::from(y) * 65536,
                )?;
            }
            Ok(())
        })
        .unwrap();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    for target in [&mut world, &mut restored] {
        let report = target
            .edit(|edit| edit.relax_pressure(layer, pressure_config(3)))
            .unwrap();
        assert_eq!(report.closed_components, 1);
        assert_eq!(pressure(target, point(3, 2)), 3 * 65536);
    }
    assert_eq!(
        world.canonical_hash(4_000_000).unwrap(),
        restored.canonical_hash(4_000_000).unwrap()
    );
}

#[test]
fn incomplete_pressure_and_negative_input_cannot_publish_partial_results() {
    let mut world = world(content(8, true));
    let layer = column(&mut world, false);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.relax_pressure(layer, pressure_config(2));
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    world
        .edit(|edit| edit.set_channel(key("game:fluid-pressure"), point(3, 3), -1i64))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.relax_pressure(layer, pressure_config(3)))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

fn pipe(world: &mut World) -> (LayerId, Vec<CellPoint>) {
    let layer = world.content().layer_id(key("game:front")).unwrap();
    let water = material(world, "game:water");
    let stone = material(world, "game:stone");
    let path = vec![
        point(3, 4),
        point(4, 4),
        point(5, 4),
        point(5, 3),
        point(5, 2),
        point(5, 1),
    ];
    world
        .edit(|edit| {
            for p in &path[..5] {
                for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                    edit.set_cell(layer, point(p.x + dx, p.y + dy), stone)?;
                }
            }
            for p in &path[..5] {
                edit.set_cell(layer, *p, water)?;
                edit.set_channel(
                    key("game:energy"),
                    point(p.x + 2, p.y),
                    p.x as u64 * 100 + p.y as u64,
                )?;
            }
            edit.set_cell(layer, path[5], CellCore::AIR)?;
            edit.set_channel(key("game:fluid-pressure"), path[0], 20 * 5 * 65536i64)
        })
        .unwrap();
    activate(world, 0, 1);
    (layer, path)
}
fn push_config(nodes: u32) -> PressurePushConfig {
    PressurePushConfig {
        channel: key("game:fluid-pressure"),
        gravity_q16: 65536,
        threshold_q16: 0,
        nodes,
    }
}

#[test]
fn bounded_pressure_search_saves_resumes_and_atomically_pushes_up_a_pipe() {
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let (layer, path) = pipe(&mut world);
    let mut search = PressurePushSearch::new(&world, layer, path[0], push_config(32)).unwrap();
    assert_eq!(
        search.step(&world, 2).unwrap(),
        PressurePushStatus::Searching
    );
    world
        .edit(|edit| edit.set_state(key("game:pending-push"), key("game:push-search"), &search))
        .unwrap();
    let before = (path.iter().map(|p| core(&world, layer, *p))).collect::<Vec<_>>();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    for target in [&mut world, &mut restored] {
        let mut job = target
            .state::<PressurePushSearch>(key("game:pending-push"))
            .unwrap();
        while job.status() == PressurePushStatus::Searching {
            job.step(target, 1).unwrap();
        }
        assert_eq!(job.status(), PressurePushStatus::Ready);
        assert_eq!(
            before,
            path.iter()
                .map(|p| core(target, layer, *p))
                .collect::<Vec<_>>()
        );
        target
            .edit(|edit| edit.publish_pressure_push(&job))
            .unwrap();
        assert_eq!(core(target, layer, path[0]), CellCore::AIR);
        for index in 1..path.len() {
            assert_eq!(core(target, layer, path[index]), before[index - 1]);
            assert_eq!(
                value(
                    target,
                    "game:energy",
                    point(path[index].x + 2, path[index].y)
                ),
                path[index - 1].x as u64 * 100 + path[index - 1].y as u64
            );
        }
        let after = target.checkpoint(4_000_000).unwrap();
        assert!(
            target
                .edit(|edit| edit.publish_pressure_push(&job))
                .is_err()
        );
        assert_eq!(after, target.checkpoint(4_000_000).unwrap());
    }
    assert_eq!(
        world.canonical_hash(4_000_000).unwrap(),
        restored.canonical_hash(4_000_000).unwrap()
    );
}

#[test]
fn pressure_search_capacity_failure_preserves_frontier_and_changed_inputs_invalidate() {
    use charon_engine::foundation::codec::{Encoder, StateCodec};
    let mut world = world(content(8, true));
    let (layer, path) = pipe(&mut world);
    let mut search = PressurePushSearch::new(&world, layer, path[0], push_config(2)).unwrap();
    search.step(&world, 1).unwrap();
    let mut before = Encoder::new(262144);
    search.encode(&mut before).unwrap();
    assert!(search.step(&world, 1).is_err());
    let mut after = Encoder::new(262144);
    search.encode(&mut after).unwrap();
    assert_eq!(before.finish(), after.finish());
    assert_eq!(search.discovered_nodes(), 2);
    assert!(search.set_node_capacity(1).is_err());
    search.set_node_capacity(32).unwrap();
    assert_eq!(search.step(&world, 32).unwrap(), PressurePushStatus::Ready);
    world
        .edit(|edit| edit.set_cell(layer, path[1], CellCore::AIR))
        .unwrap();
    assert!(search.step(&world, 1).is_err());
}

#[test]
fn pressure_search_rejects_wrong_world_truncated_records_and_insufficient_drive() {
    use charon_engine::foundation::{
        codec::{Decoder, Encoder, StateCodec},
        transaction::CandidateState,
    };
    let mut world = world(content(8, true));
    let (layer, path) = pipe(&mut world);
    let mut search = PressurePushSearch::new(&world, layer, path[0], push_config(32)).unwrap();
    let mut encoded = Encoder::new(262144);
    search.encode(&mut encoded).unwrap();
    let bytes = encoded.finish();
    for cut in 0..bytes.len() {
        let mut decoder = Decoder::new(&bytes[..cut], 262144).unwrap();
        assert!(PressurePushSearch::decode(&mut decoder).is_err());
    }
    let mut other = world.try_copy().unwrap();
    // 別World IDは保存形式の先頭直後にあるため、型付きWorld新規作成で検証します。
    other = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        Arc::clone(other.content()),
        WorldLimits::default(),
    )
    .unwrap();
    assert!(search.step(&other, 1).is_err());
    let mut config = push_config(32);
    config.threshold_q16 = u64::MAX;
    search = PressurePushSearch::new(&world, layer, path[0], config).unwrap();
    assert_eq!(
        search.step(&world, 32).unwrap(),
        PressurePushStatus::NoOutlet
    );
    assert!(
        world
            .edit(|edit| edit.publish_pressure_push(&search))
            .is_err()
    );
}

#[test]
fn material_channel_registration_rejects_missing_layer_and_implicit_resampling() {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .schema(Schema::of::<u64>(key("game:quantity"), 1, 8).unwrap())
        .unwrap();
    assert!(
        registry
            .material_channel(
                key("game:missing"),
                definition("game:energy", point(0, 0), 8)
            )
            .is_err()
    );
    registry
        .layer(LayerDef {
            key: key("game:front"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let mut channel = definition("game:energy", point(0, 0), 8);
    channel.layout.x_scale = GridScale::new(1, 2).unwrap();
    assert!(
        registry
            .material_channel(key("game:front"), channel)
            .is_err()
    );
    // 追従を拒否された別解像度の値も、位置固定channelとしては引き続き登録できます。
    let mut channel = definition("game:energy", point(0, 0), 8);
    channel.layout.x_scale = GridScale::new(1, 2).unwrap();
    registry.channel(channel).unwrap();
    registry.freeze().unwrap();
}

#[derive(Clone, Default)]
struct TestArchives {
    objects: std::collections::BTreeMap<[u8; 32], Vec<u8>>,
    read_delay_ms: u64,
    fail_read: bool,
}

/// region archive workerを実filesystemへ接続する回帰用storage。
#[derive(Clone)]
struct FileTestArchives {
    root: std::path::PathBuf,
    read_delay_ms: u64,
    remove_before_read: bool,
}
impl FileTestArchives {
    fn blob_path(&self, hash: [u8; 32]) -> std::path::PathBuf {
        let name = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root.join(format!("{name}.blob"))
    }
}
impl ArchiveStorage for FileTestArchives {
    fn put(
        &mut self,
        hash: [u8; 32],
        bytes: &[u8],
    ) -> charon_engine::foundation::EngineResult<SaveDurability> {
        DirectoryArchives::new(&self.root)?.put(hash, bytes)
    }
    fn read(
        &mut self,
        hash: [u8; 32],
        limit: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<u8>> {
        if self.read_delay_ms != 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.read_delay_ms));
        }
        if self.remove_before_read {
            std::fs::remove_file(self.blob_path(hash)).map_err(|error| {
                charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::Io,
                    "filesystem fault注入用archive削除",
                )
                .detail(error.to_string())
            })?;
        }
        DirectoryArchives::new(&self.root)?.read(hash, limit)
    }
}

struct ArchiveTestDirectory(std::path::PathBuf);
impl ArchiveTestDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = format!(
            "charon-e05-material-archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for ArchiveTestDirectory {
    fn drop(&mut self) {
        let safe_name = self.0.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("charon-e05-material-archive-")
        });
        let real_directory = std::fs::symlink_metadata(&self.0).is_ok_and(|metadata| {
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink()
        });
        if safe_name && real_directory {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

impl ArchiveStorage for TestArchives {
    fn put(
        &mut self,
        hash: [u8; 32],
        bytes: &[u8],
    ) -> charon_engine::foundation::EngineResult<SaveDurability> {
        assert_eq!(blake3::hash(bytes).as_bytes(), &hash);
        self.objects.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndAtomicPublish)
    }
    fn read(
        &mut self,
        hash: [u8; 32],
        limit: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<u8>> {
        if self.read_delay_ms != 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.read_delay_ms));
        }
        if self.fail_read {
            return Err(charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::Io,
                "テスト用archive読込失敗",
            ));
        }
        let bytes = self
            .objects
            .get(&hash)
            .expect("テスト用archiveが保存済みであること");
        assert!(bytes.len() <= limit);
        Ok(bytes.clone())
    }
}

#[test]
fn material_payload_survives_region_eviction_without_retaining_channel_arrays() {
    let mut world = world(content(8, true));
    let layer = world.content().layer_id(key("game:front")).unwrap();
    let sand = material(&world, "game:sand");
    world
        .edit(|edit| {
            edit.set_cell(layer, point(61, 2), sand)?;
            edit.set_channel(key("game:energy"), point(63, 2), 900u64)?;
            edit.exchange_cells(layer, point(61, 2), point(62, 2))
        })
        .unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let mut archives = TestArchives::default();
    let receipt = world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(receipt)).unwrap();
    assert_eq!(world.channel_bytes(), 0);
    assert!(matches!(
        world
            .channel_value::<u64>(key("game:energy"), point(64, 2))
            .unwrap(),
        ChannelQuery::Unavailable(_)
    ));
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 4_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(core(&world, layer, point(62, 2)), sand);
    assert_eq!(value(&world, "game:energy", point(64, 2)), 900);
    world
        .edit(|edit| edit.exchange_cells(layer, point(62, 2), point(63, 2)))
        .unwrap();
    assert_eq!(value(&world, "game:energy", point(65, 2)), 900);
}

#[test]
fn archive_io_completion_order_preserves_scalar_transport_each_tick() {
    let content = content(8, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let fixture = world(Arc::clone(&content));
    let sand = material(&fixture, "game:sand");
    let stone = material(&fixture, "game:stone");
    let run = |slow_first: bool| {
        let mut world = world(Arc::clone(&content));
        world
            .edit(|edit| {
                edit.set_cell(front, point(-2, 1), sand)?;
                edit.set_cell(front, point(-2, 3), stone)?;
                edit.set_cell(front, point(66, 1), sand)?;
                edit.set_cell(front, point(66, 3), stone)
            })
            .unwrap();
        let directory = ArchiveTestDirectory::new();
        let mut archives = FileTestArchives {
            root: directory.0.clone(),
            read_delay_ms: 0,
            remove_before_read: false,
        };
        for coordinate in [-1, 1] {
            let region = ChunkRegion::single(ChunkCoord::new(coordinate, 0).unwrap()).unwrap();
            let receipt = world
                .prepare_eviction(region, 4_000_000)
                .unwrap()
                .store(&mut archives)
                .unwrap();
            world.edit(|edit| edit.finish_eviction(receipt)).unwrap();
        }

        let tasks = [-1, 1].map(|coordinate| {
            (
                coordinate,
                world
                    .region_load_task(ChunkCoord::new(coordinate, 0).unwrap(), 4_000_000)
                    .unwrap(),
            )
        });
        let (sender, receiver) = std::sync::mpsc::channel();
        let start = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for (coordinate, task) in tasks {
            let mut worker_archives = archives.clone();
            worker_archives.read_delay_ms = if (coordinate == -1) == slow_first {
                40
            } else {
                0
            };
            let sender = sender.clone();
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                start.wait();
                let loaded = task.load(&mut worker_archives);
                sender.send((coordinate, loaded)).unwrap();
            }));
        }
        drop(sender);
        start.wait();
        let mut completion_order = Vec::new();
        let mut ready_regions = Vec::new();
        for (coordinate, loaded) in receiver {
            completion_order.push(coordinate);
            ready_regions.push(loaded.unwrap());
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(completion_order, if slow_first { [1, -1] } else { [-1, 1] });
        // completion順でreadyへ集め、同一barrierの一取引としてまとめて公開します。
        world
            .edit(|edit| {
                for loaded in ready_regions {
                    edit.restore_region(loaded)?;
                }
                Ok(())
            })
            .unwrap();
        activate(&mut world, -1, 2);

        let reference_bytes = world.checkpoint(4_000_000).unwrap();
        let mut reference = World::restore(
            &reference_bytes,
            Arc::clone(&content),
            WorldLimits::default(),
            4_000_000,
        )
        .unwrap();
        let runtime_descriptor =
            SystemDescriptor::new(key("game:runtime-scalar-transport"), Phase::Materials)
                .write(key("engine:terrain"))
                .in_world(key("game:world"))
                .barrier();
        let mut runtime = Engine::builder()
            .sandbox_world("world", WorldBuilder::restored(world))
            .system(
                runtime_descriptor,
                RuntimeScalarTransportSystem { layer: front },
            )
            .build()
            .unwrap();
        let runtime_world = runtime.worlds().id("world").unwrap();
        let mut ticks = Vec::new();
        for tick in 1..=4 {
            assert_eq!(
                runtime.step(TickInput::empty(tick)).unwrap(),
                StepResult::Complete(tick)
            );
            reference
                .edit(|edit| {
                    edit.transport_materials(front, 8)?;
                    Ok(())
                })
                .unwrap();
            reference.edit(|edit| edit.advance_tick()).unwrap();
            let runtime_terrain = runtime.worlds().terrain(runtime_world).unwrap();
            let runtime_checkpoint = runtime_terrain.checkpoint(4_000_000).unwrap();
            assert_eq!(
                runtime_terrain.canonical_hash(4_000_000).unwrap(),
                reference.canonical_hash(4_000_000).unwrap(),
                "tick {tick}, runtime tick {}, reference tick {}",
                runtime.worlds().tick(runtime_world).unwrap(),
                reference.tick()
            );
            ticks.push(runtime_checkpoint);
        }
        ticks
    };

    let negative_chunk_slow = run(true);
    let positive_chunk_slow = run(false);
    assert_eq!(negative_chunk_slow, positive_chunk_slow);
}

#[test]
fn archive_worker_failure_rolls_back_earlier_ready_region_publication() {
    let content = content(8, true);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&content));
    let sand = material(&world, "game:sand");
    world
        .edit(|edit| {
            edit.set_cell(front, point(-2, 1), sand)?;
            edit.set_cell(front, point(66, 1), sand)
        })
        .unwrap();
    let directory = ArchiveTestDirectory::new();
    let mut archives = FileTestArchives {
        root: directory.0.clone(),
        read_delay_ms: 0,
        remove_before_read: false,
    };
    for coordinate in [-1, 1] {
        let region = ChunkRegion::single(ChunkCoord::new(coordinate, 0).unwrap()).unwrap();
        let receipt = world
            .prepare_eviction(region, 4_000_000)
            .unwrap()
            .store(&mut archives)
            .unwrap();
        world.edit(|edit| edit.finish_eviction(receipt)).unwrap();
    }
    let before = world.checkpoint(4_000_000).unwrap();
    let tasks = [-1, 1].map(|coordinate| {
        (
            coordinate,
            world
                .region_load_task(ChunkCoord::new(coordinate, 0).unwrap(), 4_000_000)
                .unwrap(),
        )
    });
    let (sender, receiver) = std::sync::mpsc::channel();
    let start = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for (coordinate, task) in tasks {
        let mut worker_archives = archives.clone();
        worker_archives.remove_before_read = coordinate == -1;
        worker_archives.read_delay_ms = if coordinate == -1 { 40 } else { 0 };
        let sender = sender.clone();
        let start = Arc::clone(&start);
        workers.push(std::thread::spawn(move || {
            start.wait();
            sender
                .send((coordinate, task.load(&mut worker_archives)))
                .unwrap();
        }));
    }
    drop(sender);
    start.wait();
    let mut completion_order = Vec::new();
    let ready = receiver
        .into_iter()
        .map(|(coordinate, result)| {
            completion_order.push(coordinate);
            result
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(completion_order, [1, -1]);
    assert!(ready[0].is_ok());
    assert!(matches!(
        &ready[1],
        Err(error) if error.code == charon_engine::foundation::ErrorCode::Io
    ));
    let error = world
        .edit(|edit| {
            for loaded in ready {
                edit.restore_region(loaded?)?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Io);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

#[test]
fn transport_reserves_channel_writes_and_checks_total_world_channel_bytes() {
    for limits in [
        WorldLimits {
            edit_cells: 5,
            ..WorldLimits::default()
        },
        WorldLimits {
            channel_bytes: 4096 * 8,
            ..WorldLimits::default()
        },
    ] {
        let content = content(8, true);
        let layer = content.layer_id(key("game:front")).unwrap();
        let mut world = World::new(
            WorldId::from_parts(0, 1).unwrap(),
            3,
            WorldBounds::expandable(),
            content,
            limits,
        )
        .unwrap();
        let sand = material(&world, "game:sand");
        world
            .edit(|edit| {
                edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                edit.set_cell(layer, point(61, 2), sand)?;
                edit.set_channel(key("game:energy"), point(63, 2), 7u64)?;
                // この値が残るので、移動後も元のchannel chunkは必要です。
                edit.set_channel(key("game:energy"), point(3, 2), 9u64)
            })
            .unwrap();
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.exchange_cells(layer, point(61, 2), point(62, 2)))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}

#[test]
fn pressure_push_uses_atmospheric_outlet_despite_stale_position_pressure() {
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let (layer, path) = pipe(&mut world);
    // 空き先が以前高圧だった場合も、現在は大気境界として探索・公開します。
    world
        .edit(|edit| edit.set_channel(key("game:fluid-pressure"), path[5], i64::MAX))
        .unwrap();
    let mut search = PressurePushSearch::new(&world, layer, path[0], push_config(32)).unwrap();
    assert_eq!(search.step(&world, 32).unwrap(), PressurePushStatus::Ready);
    let state_key = key("game:pending-push");
    world
        .edit(|edit| edit.set_state(state_key, key("game:push-search"), &search))
        .unwrap();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    // 新形式は照合済みReadyを維持し、公開では全nodeを実地形と再照合します。
    let mut search = restored.state::<PressurePushSearch>(state_key).unwrap();
    assert_eq!(search.status(), PressurePushStatus::Ready);
    assert_eq!(
        search.step(&restored, 0).unwrap(),
        PressurePushStatus::Ready
    );
    restored
        .edit(|edit| edit.publish_pressure_push(&search))
        .unwrap();
    assert_eq!(
        core(&restored, layer, path[5]).material,
        material(&restored, "game:water").material
    );
}

#[test]
fn world_capacity_rejection_precedes_transport_value_writes() {
    use charon_engine::foundation::{
        EngineResult, ErrorCode,
        codec::{Decoder, Encoder, StateCodec},
        grid::GridValue,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    static VALIDATIONS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Clone, Copy)]
    struct Observed(u64);
    impl StateCodec for Observed {
        fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
            out.u64(self.0)
        }
        fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
            Ok(Self(input.u64()?))
        }
    }
    impl GridValue for Observed {
        fn same(self, other: Self) -> bool {
            self.0 == other.0
        }
        fn validate(self) -> EngineResult<()> {
            if self.0 != 0 {
                VALIDATIONS.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
    }
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("game:front"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .schema(Schema::of::<Observed>(key("game:observed"), 1, 8).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("game:front"),
            ChannelDefinition {
                key: key("game:payload"),
                version: 1,
                value_schema: key("game:observed"),
                layout: definition("game:payload", point(0, 0), 8).layout,
                default: Observed(0),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("game:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            channel_bytes: 4096 * 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.set_channel(key("game:payload"), point(63, 2), Observed(7))?;
            edit.set_channel(key("game:payload"), point(1, 2), Observed(9))
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    VALIDATIONS.store(0, Ordering::Relaxed);
    let error = world
        .edit(|edit| edit.exchange_cells(layer, point(63, 2), point(64, 2)))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::CapacityExceeded);
    // 非既定値のsetは新規配列確保前に検証します。0件なら再配置へ未到達です。
    assert_eq!(VALIDATIONS.load(Ordering::Relaxed), 0);
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn transport_reuses_bytes_freed_by_another_material_channel() {
    let content = content(8, true);
    let layer = content.layer_id(key("game:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            channel_bytes: 3 * 4096 * 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.set_channel(key("game:energy"), point(63, 2), 7u64)?;
            edit.set_channel(key("game:energy"), point(3, 2), 9u64)?;
            edit.set_channel(key("game:timer"), point(65, 2), 11u64)?;
            edit.set_channel(key("game:timer"), point(3, 2), 13u64)
        })
        .unwrap();
    // energyは1→2chunk、timerは2→1chunk。途中もWorld上限を超えず再配置します。
    world
        .edit(|edit| edit.transport_path(layer, &[point(61, 2), point(62, 2), point(63, 2)]))
        .unwrap();
    assert_eq!(world.channel_bytes(), 3 * 4096 * 8);
    assert_eq!(value(&world, "game:energy", point(64, 2)), 7);
    assert_eq!(value(&world, "game:timer", point(63, 2)), 11);
}

#[test]
fn pressure_reuses_released_chunk_capacity_independently_of_coordinate_order() {
    for (channel_limit, byte_limit) in [(1, usize::MAX), (8, 4096 * 8), (1, 4096 * 8)] {
        for (column_x, surface_x) in [(3, 67), (67, 3)] {
            let content = content_with_pressure(8, true, channel_limit, 0);
            let layer = content.layer_id(key("game:front")).unwrap();
            let mut world = World::new(
                WorldId::from_parts(0, 1).unwrap(),
                3,
                WorldBounds::expandable(),
                content,
                WorldLimits {
                    channel_bytes: byte_limit,
                    ..WorldLimits::default()
                },
            )
            .unwrap();
            let water = material(&world, "game:water");
            let stone = material(&world, "game:stone");
            world
                .edit(|edit| {
                    for x in 0..2 {
                        edit.insert_empty_chunk(ChunkCoord::new(x, 0)?)?;
                    }
                    for y in 2..=4 {
                        edit.set_cell(layer, point(column_x, y), water)?;
                        edit.set_cell(layer, point(column_x - 1, y), stone)?;
                        edit.set_cell(layer, point(column_x + 1, y), stone)?;
                    }
                    edit.set_cell(layer, point(column_x, 5), stone)?;
                    edit.set_cell(layer, point(surface_x, 2), water)?;
                    edit.set_channel(key("game:fluid-pressure"), point(surface_x, 2), 77i64)
                })
                .unwrap();
            activate(&mut world, 0, 2);
            assert_eq!(world.channel_bytes(), 4096 * 8);
            world
                .edit(|edit| edit.relax_pressure(layer, pressure_config(4)))
                .unwrap();
            assert_eq!(world.channel_bytes(), 4096 * 8);
            assert_eq!(
                (2..=4)
                    .map(|y| pressure(&world, point(column_x, y)))
                    .collect::<Vec<_>>(),
                [0, 245760, 491520]
            );
            assert_eq!(pressure(&world, point(surface_x, 2)), 0);
        }
    }
}

#[test]
fn pressure_releases_nonzero_default_chunks_before_writing_surface_zeroes() {
    let content = content_with_pressure(8, true, 1, 5);
    let layer = content.layer_id(key("game:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        3,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            channel_bytes: 4096 * 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    let water = material(&world, "game:water");
    let stone = material(&world, "game:stone");
    world
        .edit(|edit| {
            for x in 0..2 {
                edit.insert_empty_chunk(ChunkCoord::new(x, 0)?)?;
            }
            for x in 67..=68 {
                for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                    edit.set_cell(layer, point(x + dx, 2 + dy), stone)?;
                }
            }
            for x in 67..=68 {
                edit.set_cell(layer, point(x, 2), water)?;
            }
            edit.set_cell(layer, point(3, 2), water)?;
            edit.set_channel(key("game:fluid-pressure"), point(67, 2), 0i64)?;
            edit.set_channel(key("game:fluid-pressure"), point(68, 2), 10i64)
        })
        .unwrap();
    activate(&mut world, 0, 2);
    let mut config = pressure_config(3);
    config.gravity_q16 = 0;
    world
        .edit(|edit| edit.relax_pressure(layer, config))
        .unwrap();
    assert_eq!(world.channel_bytes(), 4096 * 8);
    assert_eq!(pressure(&world, point(3, 2)), 0);
    assert_eq!(pressure(&world, point(67, 2)), 5);
    assert_eq!(pressure(&world, point(68, 2)), 5);
}

#[test]
fn pressure_push_progress_survives_a_fresh_state_read_each_phase() {
    use charon_engine::foundation::codec::{Encoder, StateCodec};
    let content = content(8, true);
    let mut world = world(Arc::clone(&content));
    let (layer, path) = pipe(&mut world);
    let mut reference = PressurePushSearch::new(&world, layer, path[0], push_config(32)).unwrap();
    while reference.status() == PressurePushStatus::Searching {
        reference.step(&world, 1).unwrap();
    }
    let state_key = key("game:pending-push");
    let schema = key("game:push-search");
    let search = PressurePushSearch::new(&world, layer, path[0], push_config(32)).unwrap();
    world
        .edit(|edit| edit.set_state(state_key, schema, &search))
        .unwrap();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut saved = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    for target in [&mut world, &mut saved] {
        let mut ready = false;
        for _ in 0..100 {
            let mut job: PressurePushSearch = target.state(state_key).unwrap();
            let status = job.step(target, 1).unwrap();
            target
                .edit(|edit| edit.set_state(state_key, schema, &job))
                .unwrap();
            if status == PressurePushStatus::Ready {
                ready = true;
                let mut expected = Encoder::new(262144);
                reference.encode(&mut expected).unwrap();
                let mut actual = Encoder::new(262144);
                job.encode(&mut actual).unwrap();
                assert_eq!(expected.finish(), actual.finish());
                target
                    .edit(|edit| edit.publish_pressure_push(&job))
                    .unwrap();
                break;
            }
        }
        assert!(ready, "毎phaseの復元でも有限回で押出し経路へ到達する");
    }
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        saved.checkpoint(4_000_000).unwrap()
    );
}
