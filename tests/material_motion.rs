//! 速度輸送を公開APIから検証し、途中の障害・保存・失敗時の原子性を確認します。

use charon_engine::{
    foundation::{StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;
fn key(name: &str) -> StableKey {
    StableKey::parse(name).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn content() -> Arc<Content> {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("test:layer"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    for (name, base) in [
        ("test:sand", MaterialBase::Powder),
        ("test:wall", MaterialBase::Solid),
    ] {
        registry
            .register(MaterialBuilder::new(name, base).unwrap())
            .unwrap();
    }
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-value"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:energy-value"), 1, 8).unwrap())
        .unwrap();
    let layout = GridLayout {
        origin: p(-2, 0),
        x_scale: GridScale::new(1, 1).unwrap(),
        y_scale: GridScale::new(1, 1).unwrap(),
    };
    let limits = GridLimits {
        chunks: 8,
        checkpoint_bytes: 1_000_000,
    };
    registry
        .channel(ChannelDefinition {
            key: key("test:position-motion"),
            version: 1,
            value_schema: key("test:motion-value"),
            layout,
            default: MaterialMotion::default(),
            limits,
        })
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-value"),
                layout,
                default: MaterialMotion::default(),
                limits,
            },
        )
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:energy"),
                version: 1,
                value_schema: key("test:energy-value"),
                layout,
                default: 0u64,
                limits,
            },
        )
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("test:field"),
            version: 1,
            value_schema: key("test:energy-value"),
            layout,
            default: 0u64,
            limits,
        })
        .unwrap();
    registry.freeze().unwrap()
}
fn world() -> (World, LayerId) {
    let content = content();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:activity"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    (world, layer)
}
fn config() -> MaterialMotionConfig {
    MaterialMotionConfig {
        channel: key("test:motion"),
        candidate_limit: 100,
        max_distance: 8,
        step_limit: 100,
    }
}
fn motion(velocity_q16: [i64; 2]) -> MaterialMotion {
    MaterialMotion {
        velocity_q16,
        ..Default::default()
    }
}
fn place(world: &mut World, layer: LayerId, point: CellPoint, state: MaterialMotion) {
    let material = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:motion"), p(point.x + 2, point.y), state)
        })
        .unwrap();
}
fn cell(world: &World, layer: LayerId, point: CellPoint) -> CellCore {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(cell) => cell.core,
        _ => panic!("cellが必要"),
    }
}
fn state(world: &World, point: CellPoint) -> MaterialMotion {
    match world
        .channel_value(key("test:motion"), p(point.x + 2, point.y))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("速度が必要"),
    }
}
fn value(world: &World, name: &str, point: CellPoint) -> u64 {
    match world
        .channel_value::<u64>(key(name), p(point.x + 2, point.y))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("値が必要"),
    }
}

#[test]
fn fractional_motion_accumulates_symmetrically_and_survives_save_restore() {
    for direction in [-1, 1] {
        let (mut world, layer) = world();
        let source = p(10, 10);
        place(&mut world, layer, source, motion([direction * 16384, 0]));
        for _ in 0..2 {
            world
                .edit(|edit| edit.transport_material_motion(layer, config()))
                .unwrap();
        }
        assert_eq!(
            state(&world, source).remainder_q16,
            [direction as i32 * 32768, 0]
        );
        let bytes = world.checkpoint(4_000_000).unwrap();
        let mut restored = World::restore(
            &bytes,
            Arc::clone(world.content()),
            WorldLimits::default(),
            4_000_000,
        )
        .unwrap();
        for target in [&mut world, &mut restored] {
            for _ in 0..2 {
                target
                    .edit(|edit| edit.transport_material_motion(layer, config()))
                    .unwrap();
            }
            let destination = p(10 + direction as i32, 10);
            assert_eq!(cell(target, layer, source), CellCore::AIR);
            assert_ne!(cell(target, layer, destination).material, MaterialId::AIR);
            assert_eq!(state(target, destination), motion([direction * 16384, 0]));
        }
        assert_eq!(
            world.checkpoint(4_000_000).unwrap(),
            restored.checkpoint(4_000_000).unwrap()
        );
    }
}

#[test]
fn fast_motion_sweeps_walls_and_moves_every_material_payload() {
    let (mut world, layer) = world();
    place(&mut world, layer, p(1, 2), motion([4 * 65536, 0]));
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                p(4, 2),
                CellCore {
                    material: wall,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:energy"), p(3, 2), 99u64)?;
            edit.set_channel(key("test:field"), p(3, 2), 123u64)
        })
        .unwrap();
    let report = world
        .edit(|edit| edit.transport_material_motion(layer, config()))
        .unwrap();
    assert_eq!(
        (
            report.moved,
            report.crossings,
            report.checked_steps,
            report.collisions
        ),
        (1, 2, 3, 1)
    );
    assert_eq!(cell(&world, layer, p(1, 2)), CellCore::AIR);
    assert_ne!(cell(&world, layer, p(3, 2)).material, MaterialId::AIR);
    assert_eq!(cell(&world, layer, p(4, 2)).material, wall);
    assert_eq!(state(&world, p(3, 2)), MaterialMotion::default());
    assert_eq!(value(&world, "test:energy", p(3, 2)), 99);
    assert_eq!(value(&world, "test:energy", p(1, 2)), 0);
    assert_eq!(value(&world, "test:field", p(1, 2)), 123);
}

#[test]
fn diagonal_sweep_does_not_cut_a_single_blocked_corner() {
    for obstacle in [p(2, 1), p(1, 2), p(2, 2)] {
        let (mut world, layer) = world();
        place(&mut world, layer, p(1, 1), motion([65536, 65536]));
        let wall = world
            .content()
            .material_id(MaterialHandle(key("test:wall")))
            .unwrap();
        world
            .edit(|edit| {
                edit.set_cell(
                    layer,
                    obstacle,
                    CellCore {
                        material: wall,
                        ..CellCore::AIR
                    },
                )
            })
            .unwrap();
        let report = world
            .edit(|edit| edit.transport_material_motion(layer, config()))
            .unwrap();
        assert_eq!((report.moved, report.collisions), (0, 1));
        assert_eq!(state(&world, p(1, 1)), MaterialMotion::default());
        assert_ne!(cell(&world, layer, p(1, 1)).material, MaterialId::AIR);
    }
}

#[test]
fn dda_checks_non_diagonal_intermediate_cells_and_reserves_crossing_paths() {
    let (mut world, layer) = world();
    place(&mut world, layer, p(2, 4), motion([2 * 65536, -65536]));
    // 下のトークンが通った空間へ、後続の別経路が同phaseで進入してはいけません。
    place(&mut world, layer, p(2, 2), motion([2 * 65536, 65536]));
    let report = world
        .edit(|edit| edit.transport_material_motion(layer, config()))
        .unwrap();
    assert_eq!(report.reservation_conflicts, 1);
    assert_eq!(report.collisions, 0);
    assert_eq!(state(&world, p(3, 2)).velocity_q16, [2 * 65536, 65536]);
    assert_eq!(cell(&world, layer, p(2, 4)), CellCore::AIR);
    assert_ne!(cell(&world, layer, p(4, 3)).material, MaterialId::AIR);
}

#[test]
fn unavailable_and_inactive_regions_stop_motion_without_discarding_velocity() {
    for resident in [false, true] {
        let (mut world, layer) = world();
        place(&mut world, layer, p(63, 2), motion([2 * 65536, 0]));
        if resident {
            world
                .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(1, 0)?))
                .unwrap();
        }
        let report = world
            .edit(|edit| edit.transport_material_motion(layer, config()))
            .unwrap();
        assert_eq!((report.moved, report.waiting_for_region), (0, 1));
        assert_eq!(state(&world, p(63, 2)), motion([2 * 65536, 0]));
        world
            .edit(|edit| {
                if !resident {
                    edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
                }
                edit.acquire_lease(RegionLease {
                    source: key("test:next"),
                    region: ChunkRegion::new(ChunkCoord::new(1, 0)?, 2, 1)?,
                    mode: LeaseMode::Simulate,
                    expires: None,
                })
            })
            .unwrap();
        let report = world
            .edit(|edit| edit.transport_material_motion(layer, config()))
            .unwrap();
        assert_eq!(report.crossings, 2);
        assert_ne!(cell(&world, layer, p(65, 2)).material, MaterialId::AIR);
    }
}

#[test]
fn insufficient_motion_budgets_roll_back_prior_moves_and_fractional_updates() {
    for config in [
        MaterialMotionConfig {
            step_limit: 1,
            ..config()
        },
        MaterialMotionConfig {
            candidate_limit: 1,
            ..config()
        },
        MaterialMotionConfig {
            max_distance: 1,
            ..config()
        },
    ] {
        let (mut world, layer) = world();
        place(&mut world, layer, p(2, 10), motion([65536, 0]));
        place(&mut world, layer, p(2, 5), motion([2 * 65536, 0]));
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| {
                    let _ = edit.transport_material_motion(layer, config);
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    }
}

#[test]
fn motion_codec_rejects_out_of_range_remainders_and_truncated_records() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    let record = MaterialMotion {
        velocity_q16: [i64::MIN, i64::MAX],
        remainder_q16: [-65535, 65535],
    };
    let mut out = Encoder::new(24);
    record.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut input = Decoder::new(&bytes, 24).unwrap();
    assert_eq!(MaterialMotion::decode(&mut input).unwrap(), record);
    for cut in 0..24 {
        assert!(MaterialMotion::decode(&mut Decoder::new(&bytes[..cut], 24).unwrap()).is_err());
    }
    for invalid in [-65536, 65536] {
        assert!(
            MaterialMotion {
                remainder_q16: [invalid, 0],
                ..record
            }
            .encode(&mut Encoder::new(24))
            .is_err()
        );
    }
}

#[test]
fn dda_reaches_exact_integer_destinations_in_all_octants() {
    for (dx, dy) in [
        (4i32, 0i32),
        (0, 4),
        (-4, 0),
        (0, -4),
        (4, 2),
        (2, 4),
        (-4, 2),
        (-2, 4),
        (4, -2),
        (2, -4),
        (-4, -2),
        (-2, -4),
        (3, 3),
        (-3, 3),
        (3, -3),
        (-3, -3),
    ] {
        let (mut world, layer) = world();
        let velocity = [i64::from(dx) * 65536, i64::from(dy) * 65536];
        place(&mut world, layer, p(10, 10), motion(velocity));
        let report = world
            .edit(|edit| edit.transport_material_motion(layer, config()))
            .unwrap();
        assert_eq!(report.moved, 1);
        assert_eq!(report.collisions, 0);
        assert!(report.checked_steps <= (dx.abs() + dy.abs()) as usize);
        assert_ne!(
            cell(&world, layer, p(10 + dx, 10 + dy)).material,
            MaterialId::AIR
        );
        assert_eq!(state(&world, p(10 + dx, 10 + dy)), motion(velocity));
        assert_eq!(cell(&world, layer, p(10, 10)), CellCore::AIR);
    }
}

#[test]
fn inactive_occupied_cells_are_waiting_boundaries_without_physical_response() {
    let (mut world, layer) = world();
    place(&mut world, layer, p(63, 2), motion([65536, 0]));
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.set_cell(
                layer,
                p(64, 2),
                CellCore {
                    material: wall,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let report = world
        .edit(|edit| edit.transport_material_motion(layer, config()))
        .unwrap();
    assert_eq!((report.waiting_for_region, report.collisions), (1, 0));
    assert_eq!(state(&world, p(63, 2)), motion([65536, 0]));
}

#[test]
fn extreme_velocity_capacity_failure_does_not_overflow_or_publish_changes() {
    for velocity in [i64::MIN, i64::MAX] {
        let (mut world, layer) = world();
        place(
            &mut world,
            layer,
            p(5, 5),
            MaterialMotion {
                velocity_q16: [velocity, velocity],
                remainder_q16: [-65535, 65535],
            },
        );
        let before = world.checkpoint(4_000_000).unwrap();
        let error = world
            .edit(|edit| edit.transport_material_motion(layer, config()))
            .unwrap_err();
        assert_eq!(
            error.code,
            charon_engine::foundation::ErrorCode::CapacityExceeded
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}

#[test]
fn finite_world_boundary_is_a_collision_instead_of_a_missing_region() {
    let content = content();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::new(p(0, 0), 8, 8).unwrap(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:activity"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    place(&mut world, layer, p(6, 3), motion([3 * 65536, 0]));
    let report = world
        .edit(|edit| edit.transport_material_motion(layer, config()))
        .unwrap();
    assert_eq!(
        (
            report.crossings,
            report.collisions,
            report.waiting_for_region
        ),
        (1, 1, 0)
    );
    assert_eq!(state(&world, p(7, 3)), MaterialMotion::default());
    assert_ne!(cell(&world, layer, p(7, 3)).material, MaterialId::AIR);
}

#[test]
fn phase_rejects_wrong_motion_ownership_type_and_missing_channels_without_changes() {
    for name in ["test:position-motion", "test:field", "test:missing"] {
        let (mut world, layer) = world();
        place(&mut world, layer, p(5, 5), motion([65536, 0]));
        let before = world.checkpoint(4_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.transport_material_motion(
                    layer,
                    MaterialMotionConfig {
                        channel: key(name),
                        ..config()
                    }
                ))
                .is_err()
        );
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}
