//! 公開APIで、物質粒子の所有権・容量失敗・経路・区域退避を検証します。
use charon_engine::foundation::{
    codec::{Decoder, Encoder, StateCodec},
    id::Arena,
};
use charon_engine::{
    foundation::{EngineResult, ErrorCode, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::{collections::BTreeMap, sync::Arc};
fn key(name: &str) -> StableKey {
    StableKey::parse(name).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn fixture(limits: WorldLimits) -> (World, LayerId) {
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
    for name in ["test:sand", "test:wall"] {
        registry
            .register(MaterialBuilder::new(name, MaterialBase::Solid).unwrap())
            .unwrap();
    }
    registry
        .schema(Schema::of::<u64>(key("test:value"), 1, 8).unwrap())
        .unwrap();
    for name in ["test:energy", "test:timer", "test:instance", "test:field"] {
        let definition = ChannelDefinition {
            key: key(name),
            version: 1,
            value_schema: key("test:value"),
            layout: GridLayout {
                origin: p(-2, 1),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0u64,
            limits: GridLimits {
                chunks: 16,
                checkpoint_bytes: 4_000_000,
            },
        };
        if name == "test:field" {
            registry.channel(definition).unwrap();
        } else {
            registry
                .material_channel(key("test:layer"), definition)
                .unwrap();
        }
    }
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        limits,
    )
    .unwrap();
    world
        .edit(|edit| {
            for y in -1..=1 {
                for x in -1..=1 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            Ok(())
        })
        .unwrap();
    (world, layer)
}
fn activate(world: &mut World) -> LeaseRef {
    world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::new(ChunkCoord::new(-1, -1)?, 2, 2)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap()
}
fn core(world: &World, layer: LayerId, point: CellPoint) -> CellCore {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(cell) => cell.core,
        _ => panic!("読込済みcellが必要"),
    }
}
fn material(world: &World, name: &str) -> CellCore {
    CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key(name)))
            .unwrap(),
        flags: 4,
        variation: 9,
    }
}
fn place(world: &mut World, layer: LayerId, point: CellPoint) -> CellCore {
    let value = material(world, "test:sand");
    world
        .edit(|edit| {
            edit.set_cell(layer, point, value)?;
            for (name, value) in [
                ("test:energy", 345u64),
                ("test:timer", 77),
                ("test:instance", 999),
                ("test:field", 123),
            ] {
                edit.set_channel(key(name), p(point.x + 2, point.y - 1), value)?;
            }
            Ok(())
        })
        .unwrap();
    value
}
fn value(world: &World, name: &str, point: CellPoint) -> u64 {
    match world
        .channel_value(key(name), p(point.x + 2, point.y - 1))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("読込済み値が必要"),
    }
}
fn save(world: &World) -> Vec<u8> {
    world.checkpoint(8_000_000).unwrap()
}
fn restore(world: &World) -> World {
    World::restore(
        &save(world),
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap()
}

fn particle_job_roundtrip(job: &MaterialParticleJob) -> MaterialParticleJob {
    let mut out = Encoder::new(4_000_000);
    job.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut input = Decoder::new(&bytes, bytes.len()).unwrap();
    let restored = MaterialParticleJob::decode(&mut input).unwrap();
    input.finish().unwrap();
    restored
}

#[test]
fn all_payload_moves_cell_particle_landing_and_stale_handle_is_rejected() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    let expected = place(&mut world, layer, p(-1, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(-1, 2), [65536, 0]))
        .unwrap();
    assert_eq!(core(&world, layer, p(-1, 2)), CellCore::AIR);
    assert_eq!(value(&world, "test:energy", p(-1, 2)), 0);
    assert_eq!(value(&world, "test:field", p(-1, 2)), 123);
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:timer"))
            .unwrap(),
        77
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:instance"))
            .unwrap(),
        999
    );
    assert!(
        world
            .particle_channel_value::<u64>(handle, key("test:field"))
            .is_err()
    );
    world
        .edit(|edit| edit.step_material_particles(10, 10))
        .unwrap();
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(0, 2));
    world
        .edit(|edit| edit.land_material_particle(handle))
        .unwrap();
    assert_eq!(core(&world, layer, p(0, 2)), expected);
    assert_eq!(value(&world, "test:energy", p(0, 2)), 345);
    assert_eq!(value(&world, "test:timer", p(0, 2)), 77);
    assert_eq!(value(&world, "test:instance", p(0, 2)), 999);
    assert_eq!(
        world.material_particle(handle).unwrap_err().code,
        ErrorCode::StaleId
    );
    let next = world
        .edit(|edit| edit.emit_material_particle(layer, p(0, 2), [0, 0]))
        .unwrap();
    assert_ne!(handle.id(), next.id());
}

#[test]
fn pool_and_payload_capacity_failures_keep_inputs_even_when_ignored() {
    for limits in [
        WorldLimits {
            particles: 1,
            ..Default::default()
        },
        WorldLimits {
            particle_bytes: 23,
            ..Default::default()
        },
    ] {
        let (mut world, layer) = fixture(limits);
        place(&mut world, layer, p(1, 1));
        place(&mut world, layer, p(2, 1));
        if limits.particles == 1 {
            world
                .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [0, 0]))
                .unwrap();
        }
        let before = save(&world);
        let result = world.edit(|edit| {
            let _ = edit.emit_material_particle(layer, p(2, 1), [0, 0]);
            Ok(())
        });
        assert_eq!(result.unwrap_err().code, ErrorCode::CapacityExceeded);
        assert_eq!(save(&world), before);
    }
}

#[test]
fn fractional_position_and_all_octants_match_saved_resume() {
    for velocity in [
        [16384, 0],
        [-16384, 0],
        [65536, 131072],
        [-65536, 131072],
        [65536, -131072],
        [-65536, -131072],
        [131072, 65536],
        [-131072, 65536],
        [131072, -65536],
        [-131072, -65536],
    ] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        place(&mut world, layer, p(4, 4));
        let handle = world
            .edit(|edit| edit.emit_material_particle(layer, p(4, 4), velocity))
            .unwrap();
        world
            .edit(|edit| edit.step_material_particles(10, 100))
            .unwrap();
        let mut resumed = restore(&world);
        for target in [&mut world, &mut resumed] {
            for _ in 0..3 {
                target
                    .edit(|edit| edit.step_material_particles(10, 100))
                    .unwrap();
            }
        }
        assert_eq!(save(&world), save(&resumed));
        let position = world.material_particle(handle).unwrap().position_q16();
        assert_eq!(
            position,
            [
                4 * 65536 + 32768 + 4 * velocity[0],
                4 * 65536 + 32768 + 4 * velocity[1]
            ]
        );
    }
}

#[test]
fn wall_and_corner_do_not_tunnel_and_land_with_payload() {
    for wall in [p(2, 1), p(1, 2), p(2, 2)] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        place(&mut world, layer, p(1, 1));
        let rock = material(&world, "test:wall");
        world.edit(|edit| edit.set_cell(layer, wall, rock)).unwrap();
        world
            .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [4 * 65536, 4 * 65536]))
            .unwrap();
        let report = world
            .edit(|edit| edit.step_material_particles(10, 100))
            .unwrap();
        assert_eq!(report.landed, 1);
        assert_eq!(value(&world, "test:energy", p(1, 1)), 345);
        assert_eq!(core(&world, layer, wall), rock);
        assert_eq!(world.material_particles().count(), 0);
    }
}

#[test]
fn step_capacity_and_landing_payload_conflict_do_not_consume_particle() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [10 * 65536, 0]))
        .unwrap();
    let before = save(&world);
    assert!(
        world
            .edit(|edit| edit.step_material_particles(10, 1))
            .is_err()
    );
    assert_eq!(save(&world), before);
    world
        .edit(|edit| edit.set_channel(key("test:energy"), p(3, 0), 88u64))
        .unwrap();
    let before = save(&world);
    assert!(
        world
            .edit(|edit| {
                let _ = edit.land_material_particle(handle);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(save(&world), before);
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
}

#[derive(Default)]
struct Archives(BTreeMap<[u8; 32], Vec<u8>>);
impl ArchiveStorage for Archives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndDirectories)
    }
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        let bytes = &self.0[&hash];
        assert!(bytes.len() <= limit);
        Ok(bytes.clone())
    }
}

#[test]
fn particle_payload_is_evicted_with_region_and_root_restores_stable_handle() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(2, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [16384, -16384]))
        .unwrap();
    let mut archives = Archives::default();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let stored = world
        .prepare_eviction(region, 8_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert!(world.material_particle(handle).unwrap().is_archived());
    assert!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .is_err()
    );
    let mut world = restore(&world);
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert!(!world.material_particle(handle).unwrap().is_archived());
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [16384, -16384]
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
    world
        .edit(|edit| edit.land_material_particle(handle))
        .unwrap();
    assert_eq!(value(&world, "test:instance", p(2, 2)), 999);
}

#[test]
fn saving_eviction_then_changing_velocity_rejects_stale_receipt() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(2, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [0, 0]))
        .unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world
        .edit(|edit| edit.set_particle_velocity(handle, [1, 2]))
        .unwrap();
    let before = save(&world);
    assert!(world.edit(|edit| edit.finish_eviction(stored)).is_err());
    assert_eq!(save(&world), before);
}

#[test]
fn inactive_particles_freeze_and_cross_world_handles_are_rejected() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(2, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [65536, 0]))
        .unwrap();
    assert_eq!(
        world
            .edit(|edit| edit.step_material_particles(10, 100))
            .unwrap()
            .processed,
        0
    );
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(2, 2));
    let other = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        Arc::clone(world.content()),
        WorldLimits::default(),
    )
    .unwrap();
    assert_eq!(
        other.material_particle(handle).unwrap_err().code,
        ErrorCode::WrongScope
    );
}

#[test]
fn unavailable_region_holds_particle_and_velocity_until_loading() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(126, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(126, 2), [4 * 65536, 0]))
        .unwrap();
    let report = world
        .edit(|edit| edit.step_material_particles(10, 100))
        .unwrap();
    assert_eq!(report.waiting, 1);
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(127, 2));
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [4 * 65536, 0]
    );
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(2, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:next"),
                region: ChunkRegion::single(ChunkCoord::new(2, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    world
        .edit(|edit| edit.step_material_particles(10, 100))
        .unwrap();
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(131, 2));
}

#[test]
fn landing_capacity_failure_preserves_particle_and_all_payload() {
    let limits = WorldLimits {
        channel_bytes: 4 * 4096 * 8,
        ..Default::default()
    };
    let (mut world, layer) = fixture(limits);
    place(&mut world, layer, p(2, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [0, 0]))
        .unwrap();
    world
        .edit(|edit| {
            for name in ["test:energy", "test:timer", "test:instance"] {
                edit.set_channel(key(name), p(70, 2), 1u64)?;
            }
            Ok(())
        })
        .unwrap();
    let before = save(&world);
    assert_eq!(
        world
            .edit(|edit| edit.land_material_particle(handle))
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(save(&world), before);
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
    world
        .edit(|edit| {
            for name in ["test:energy", "test:timer", "test:instance"] {
                edit.set_channel(key(name), p(70, 2), 0u64)?;
            }
            edit.land_material_particle(handle)
        })
        .unwrap();
    assert_eq!(value(&world, "test:timer", p(2, 2)), 77);
}

#[test]
fn blocked_particle_does_not_expire_and_retries_landing_after_payload_is_cleared() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [65536, 0]))
        .unwrap();
    let wall = material(&world, "test:wall");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(2, 1), wall)?;
            edit.set_channel(key("test:energy"), p(3, 0), 88u64)
        })
        .unwrap();
    for _ in 0..200 {
        world
            .edit(|edit| edit.step_material_particles(10, 100))
            .unwrap();
    }
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
    world
        .edit(|edit| edit.set_channel(key("test:energy"), p(3, 0), 0u64))
        .unwrap();
    assert_eq!(
        world
            .edit(|edit| edit.step_material_particles(10, 100))
            .unwrap()
            .landed,
        1
    );
    assert_eq!(value(&world, "test:energy", p(1, 1)), 345);
}

#[test]
fn particle_codec_rejects_every_truncation_and_restore_checks_pool_caps() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [1, -1]))
        .unwrap();
    let mut out = Encoder::new(1_000_000);
    world
        .material_particle(handle)
        .unwrap()
        .encode(&mut out)
        .unwrap();
    let bytes = out.finish();
    for length in 0..bytes.len() {
        let mut input = Decoder::new(&bytes[..length], 1_000_000).unwrap();
        assert!(MaterialParticle::decode(&mut input).is_err());
    }
    assert!(
        World::restore(
            &save(&world),
            Arc::clone(world.content()),
            WorldLimits {
                particles: 1,
                ..Default::default()
            },
            8_000_000
        )
        .is_err()
    );
}

#[test]
fn old_world_format_migrates_without_inventing_particles() {
    let (world, _) = fixture(WorldLimits::default());
    let mut bytes = save(&world);
    let arena =
        Arena::<MaterialParticle, MaterialParticleKind>::new(WorldLimits::default().particles)
            .unwrap()
            .checkpoint(8_000_000)
            .unwrap();
    bytes.truncate(bytes.len() - arena.len() - 16);
    bytes[..8].copy_from_slice(b"CHWORLD1");
    let migrated = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    assert_eq!(migrated.material_particles().count(), 0);
    assert_eq!(save(&migrated), save(&world));
}

#[test]
fn custom_provider_edits_payload_atomically_and_cannot_write_position_fields() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [1, 2]))
        .unwrap();
    let before = save(&world);
    assert!(
        world
            .edit(|edit| {
                edit.set_particle_channel(handle, key("test:timer"), 22u64)?;
                let _ = edit.set_particle_channel(handle, key("test:field"), 88u64);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(save(&world), before);
    world
        .edit(|edit| {
            edit.set_particle_channel(handle, key("test:timer"), 22u64)?;
            edit.set_particle_velocity(handle, [-1, 7])?;
            edit.land_material_particle(handle)
        })
        .unwrap();
    assert_eq!(value(&world, "test:timer", p(1, 1)), 22);
}

#[test]
fn crossing_paths_reserve_guards_and_postpone_later_particle() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(1, 2));
    place(&mut world, layer, p(2, 1));
    let first = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 2), [2 * 65536, 0]))
        .unwrap();
    let second = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 1), [0, 2 * 65536]))
        .unwrap();
    let report = world
        .edit(|edit| edit.step_material_particles(10, 100))
        .unwrap();
    assert_eq!(report.waiting, 1);
    assert_eq!(world.material_particle(first).unwrap().cell(), p(3, 2));
    assert_eq!(world.material_particle(second).unwrap().cell(), p(2, 1));
    assert_eq!(
        world.material_particle(second).unwrap().velocity_q16(),
        [0, 2 * 65536]
    );
}

#[test]
fn mixture_reference_and_exact_components_survive_particle_archive_and_landing() {
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
    for (name, mass) in [("test:water", 5), ("test:oil", 3), ("test:carrier", 4)] {
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
        .schema(Schema::of::<MixtureStore>(key("test:store"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:mixture"),
                version: 1,
                value_schema: key("test:ref"),
                layout: GridLayout {
                    origin: p(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MixtureRef::NONE,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let carrier = content
        .material_id(MaterialHandle(key("test:carrier")))
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    let mixture = MixtureState::new(&[
        MixturePart {
            material: key("test:water"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:oil"),
            ratio: 1,
        },
    ])
    .unwrap();
    let reference = world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_state(MixtureStore::key(), key("test:store"), &store)?;
            let reference = edit.register_mixture(key("test:store"), &mixture)?;
            edit.set_cell(
                layer,
                p(2, 2),
                CellCore {
                    material: carrier,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:mixture"), p(2, 2), reference)?;
            Ok(reference)
        })
        .unwrap();
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [0, 0]))
        .unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let mut world = restore(&world);
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        world
            .particle_channel_value::<MixtureRef>(handle, key("test:mixture"))
            .unwrap(),
        reference
    );
    world
        .edit(|edit| edit.land_material_particle(handle))
        .unwrap();
    assert!(
        matches!(world.channel_value::<MixtureRef>(key("test:mixture"), p(2, 2)).unwrap(), ChannelQuery::Loaded(value) if value == reference)
    );
    let components = world
        .state::<MixtureStore>(MixtureStore::key())
        .unwrap()
        .quantities(world.content(), carrier, reference)
        .unwrap();
    assert_eq!(components.len(), 2);
    for component in components {
        assert_eq!(component.tokens, MaterialQuantity::new(1, 2).unwrap());
    }
}

#[test]
fn particle_owner_remap_applies_to_resident_and_archived_payload_on_return() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(2, 2));
    place(&mut world, layer, p(70, 2));
    let dormant = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [0, 0]))
        .unwrap();
    let resident = world
        .edit(|edit| edit.emit_material_particle(layer, p(70, 2), [0, 0]))
        .unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    world
        .edit(|edit| {
            edit.remap_materials(MaterialRemap {
                owner: key("engine:particles"),
                layers: vec![],
                mappings: vec![(
                    MaterialHandle(key("test:sand")),
                    MaterialHandle(key("test:wall")),
                )],
            })
        })
        .unwrap();
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    assert_eq!(
        world.material_particle(resident).unwrap().core().material,
        wall
    );
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        world.material_particle(dormant).unwrap().core().material,
        wall
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(dormant, key("test:energy"))
            .unwrap(),
        345
    );
}

#[test]
fn moving_particle_belongs_to_destination_region_for_eviction() {
    let (mut world, layer) = fixture(WorldLimits::default());
    let lease = activate(&mut world);
    place(&mut world, layer, p(63, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(63, 2), [2 * 65536, 0]))
        .unwrap();
    world
        .edit(|edit| edit.step_material_particles(10, 100))
        .unwrap();
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(65, 2));
    world.edit(|edit| edit.release_lease(lease)).unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert!(!world.material_particle(handle).unwrap().is_archived());
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(1, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert!(world.material_particle(handle).unwrap().is_archived());
}

#[test]
fn creator_state_can_save_handle_and_reconnect_after_restart() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [1, 2]))
        .unwrap();
    let mut out = Encoder::new(64);
    handle.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut input = Decoder::new(&bytes, 64).unwrap();
    let loaded_handle = MaterialParticleHandle::decode(&mut input).unwrap();
    input.finish().unwrap();
    let loaded_world = restore(&world);
    assert_eq!(loaded_handle, handle);
    assert_eq!(
        loaded_world
            .material_particle(loaded_handle)
            .unwrap()
            .velocity_q16(),
        [1, 2]
    );
}

#[test]
fn old_region_archive_restores_without_particles() {
    let (mut world, layer) = fixture(WorldLimits::default());
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    // 旧版は粒子列を持ちません。同じ地形・channelの旧形式fixtureを作り、
    // その内容hashをrootへ対応付けて、実際の公開load経路を通します。
    let (old_hash, mut bytes) = archives
        .0
        .iter()
        .next()
        .map(|(hash, bytes)| (*hash, bytes.clone()))
        .unwrap();
    bytes.truncate(bytes.len() - 4);
    bytes[..8].copy_from_slice(b"CHREGN01");
    let hash = *blake3::hash(&bytes).as_bytes();
    archives.0.insert(hash, bytes);
    let mut root = save(&world);
    let positions: Vec<_> = root
        .windows(32)
        .enumerate()
        .filter_map(|(i, bytes)| (bytes == old_hash).then_some(i))
        .collect();
    assert_eq!(positions.len(), 1);
    for index in positions {
        root[index..index + 32].copy_from_slice(&hash);
    }
    let mut world = World::restore(
        &root,
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(world.material_particles().count(), 0);
    assert!(matches!(
        world.cell(layer, p(1, 1)),
        Ok(CellQuery::Loaded(_))
    ));
}

#[test]
fn particle_air_remap_rejected_before_archived_region_can_be_stranded() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(2, 2));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [1, 2]))
        .unwrap();
    let mut archives = Archives::default();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let before = save(&world);
    let result = world.edit(|edit| {
        let _ = edit.remap_materials(MaterialRemap {
            owner: key("engine:particles"),
            layers: vec![],
            mappings: vec![(
                MaterialHandle(key("test:sand")),
                MaterialHandle(key("engine:air")),
            )],
        });
        Ok(())
    });
    assert!(
        result.is_err(),
        "退避粒子を空気へ変えるmappingは登録時に拒否する"
    );
    assert_eq!(save(&world), before);
    assert_eq!(world.material_remaps().epoch(), 0);
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [1, 2]
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
}

#[test]
fn particle_air_mapping_is_rejected_without_matching_live_particles() {
    for resident in [false, true] {
        let (mut world, layer) = fixture(WorldLimits::default());
        if resident {
            place(&mut world, layer, p(2, 2));
            world
                .edit(|edit| edit.emit_material_particle(layer, p(2, 2), [0, 0]))
                .unwrap();
        }
        let before = save(&world);
        assert!(
            world
                .edit(|edit| edit.remap_materials(MaterialRemap {
                    owner: key("engine:particles"),
                    layers: vec![key("test:layer")],
                    mappings: vec![(
                        MaterialHandle(key("test:sand")),
                        MaterialHandle(key("engine:air"))
                    )]
                }))
                .is_err()
        );
        assert_eq!(save(&world), before);
    }
}

#[test]
fn runtime_checkpoint_limit_covers_many_channels_with_full_particle_pool() {
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
    registry
        .register(MaterialBuilder::new("test:sand", MaterialBase::Powder).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u8>(key("test:byte"), 1, 1).unwrap())
        .unwrap();
    let mut payload_per_particle = 0usize;
    for index in 0..64 {
        let name = format!("{}:{index:02}{}", "x".repeat(96), "y".repeat(94));
        payload_per_particle += name.len() + 8 + 1;
        registry
            .material_channel(
                key("test:layer"),
                ChannelDefinition {
                    key: key(&name),
                    version: 1,
                    value_schema: key("test:byte"),
                    layout: GridLayout {
                        origin: p(0, 0),
                        x_scale: GridScale::new(1, 1).unwrap(),
                        y_scale: GridScale::new(1, 1).unwrap(),
                    },
                    default: 0u8,
                    limits: GridLimits {
                        chunks: 1,
                        checkpoint_bytes: 4096,
                    },
                },
            )
            .unwrap();
    }
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let limits = WorldLimits {
        resident_chunks: 1,
        active_chunks: 0,
        archived_chunks: 0,
        terrain_bytes: 4096 * 8,
        channel_bytes: 0,
        state_records: 1,
        state_bytes: 0,
        block_state_records: 1,
        block_state_bytes: 0,
        blocks: 1,
        leases: 1,
        requests: 1,
        jobs: 1,
        job_cells: 1,
        job_bytes: 0,
        remaps: 0,
        edit_cells: 4096,
        particles: 8,
        particle_bytes: payload_per_particle * 8,
    };
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        limits,
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for x in 0..8 {
                edit.set_cell(
                    layer,
                    p(x, 0),
                    CellCore {
                        material: sand,
                        ..CellCore::AIR
                    },
                )?;
                edit.emit_material_particle(layer, p(x, 0), [0, 0])?;
            }
            Ok(())
        })
        .unwrap();
    let generous = world.checkpoint(4_000_000).unwrap();
    let limit = world.checkpoint_limit().unwrap();
    assert!(
        generous.len() <= limit,
        "実際の保存{}byteを上限{}byteに含める",
        generous.len(),
        limit
    );
    let saved = world.checkpoint(limit).unwrap();
    assert_eq!(saved, generous);
    let restored = World::restore(&saved, Arc::clone(world.content()), limits, limit).unwrap();
    assert_eq!(restored.material_particles().count(), 8);
}

#[test]
fn restored_particle_arena_capacity_must_equal_saved_world_capacity() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(1, 1));
    world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [0, 0]))
        .unwrap();
    let mut bytes = save(&world);
    let arena = bytes
        .windows(8)
        .rposition(|part| part == b"CHARNA01")
        .unwrap();
    bytes[arena + 8..arena + 12].copy_from_slice(&1u32.to_le_bytes());
    assert!(
        World::restore(
            &bytes,
            Arc::clone(world.content()),
            WorldLimits::default(),
            8_000_000
        )
        .is_err(),
        "外側容量より小さいarena容量も不整合として拒否する"
    );
}

#[test]
fn opposing_particles_keep_both_sources_in_either_id_order_and_after_every_save() {
    for reverse in [false, true] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        let mut handles = BTreeMap::new();
        let cell = material(&world, "test:sand");
        world
            .edit(|edit| {
                for x in if reverse { [2, 1] } else { [1, 2] } {
                    edit.set_cell(layer, p(x, 2), cell)?;
                    let handle = edit.emit_material_particle(
                        layer,
                        p(x, 2),
                        [if x == 1 { 65536 } else { -65536 }, 0],
                    )?;
                    edit.set_particle_channel(handle, key("test:energy"), x as u64 * 100)?;
                    handles.insert(x, handle);
                }
                Ok(())
            })
            .unwrap();
        let mut resumed = restore(&world);
        for _ in 0..4 {
            for world in [&mut world, &mut resumed] {
                let report = world
                    .edit(|edit| edit.step_material_particles(2, 10))
                    .unwrap();
                assert_eq!((report.waiting, report.moved, report.landed), (2, 0, 0));
                for (&x, &handle) in &handles {
                    let particle = world.material_particle(handle).unwrap();
                    assert_eq!(particle.cell(), p(x, 2));
                    assert_eq!(
                        world
                            .particle_channel_value::<u64>(handle, key("test:energy"))
                            .unwrap(),
                        x as u64 * 100
                    );
                }
            }
            resumed = restore(&resumed);
            assert_eq!(save(&world), save(&resumed));
        }
        world
            .edit(|edit| {
                for &handle in handles.values() {
                    edit.set_particle_velocity(handle, [0, 0])?;
                }
                edit.step_material_particles(2, 10)
            })
            .unwrap();
        assert_eq!(world.material_particles().count(), 0);
        assert_eq!(value(&world, "test:energy", p(1, 2)), 100);
        assert_eq!(value(&world, "test:energy", p(2, 2)), 200);
    }
}

#[test]
fn following_particles_cannot_take_a_source_vacated_in_the_same_phase() {
    for reverse in [false, true] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        let cell = material(&world, "test:sand");
        let mut handles = BTreeMap::new();
        world
            .edit(|edit| {
                for x in if reverse { [2, 1] } else { [1, 2] } {
                    edit.set_cell(layer, p(x, 2), cell)?;
                    handles.insert(
                        x,
                        edit.emit_material_particle(layer, p(x, 2), [2 * 65536, 0])?,
                    );
                }
                Ok(())
            })
            .unwrap();
        let report = world
            .edit(|edit| edit.step_material_particles(2, 20))
            .unwrap();
        assert_eq!((report.waiting, report.moved), (1, 1));
        assert_eq!(
            world.material_particle(handles[&1]).unwrap().cell(),
            p(1, 2)
        );
        assert_eq!(
            world.material_particle(handles[&2]).unwrap().cell(),
            p(4, 2)
        );
        world
            .edit(|edit| edit.step_material_particles(2, 20))
            .unwrap();
        assert_eq!(
            world.material_particle(handles[&1]).unwrap().cell(),
            p(3, 2)
        );
        assert_eq!(
            world.material_particle(handles[&2]).unwrap().cell(),
            p(6, 2)
        );
    }
}

#[test]
fn diagonal_particle_guard_respects_an_unprocessed_stationary_particle() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(1, 1));
    place(&mut world, layer, p(2, 1));
    let first = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [2 * 65536, 2 * 65536]))
        .unwrap();
    let second = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 1), [0, 0]))
        .unwrap();
    let report = world
        .edit(|edit| edit.step_material_particles(2, 20))
        .unwrap();
    assert_eq!((report.waiting, report.landed), (1, 1));
    assert_eq!(world.material_particle(first).unwrap().cell(), p(1, 1));
    assert!(world.material_particle(second).is_err());
    assert_ne!(core(&world, layer, p(2, 1)), CellCore::AIR);
}

#[test]
fn emission_into_a_particle_source_preserves_both_existing_owners() {
    let (mut world, layer) = fixture(WorldLimits::default());
    place(&mut world, layer, p(1, 1));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [0, 0]))
        .unwrap();
    place(&mut world, layer, p(1, 1));
    let before = save(&world);
    world
        .edit(|edit| {
            let _ = edit.emit_material_particle(layer, p(1, 1), [0, 0]);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(save(&world), before);
    assert_eq!(world.material_particles().count(), 1);
    assert_eq!(
        world
            .particle_channel_value::<u64>(handle, key("test:energy"))
            .unwrap(),
        345
    );
    assert_eq!(value(&world, "test:energy", p(1, 1)), 345);
}

#[test]
fn previously_overlapping_particles_cannot_land_over_one_another_or_disappear() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(1, 1));
    place(&mut world, layer, p(2, 1));
    let first = world
        .edit(|edit| edit.emit_material_particle(layer, p(1, 1), [0, 0]))
        .unwrap();
    let second = world
        .edit(|edit| edit.emit_material_particle(layer, p(2, 1), [0, 0]))
        .unwrap();
    // 修正前に保存された重なりを再現します。payload・ID・保存容量は変更しません。
    let mut encoded = Encoder::new(4096);
    world
        .material_particle(second)
        .unwrap()
        .encode(&mut encoded)
        .unwrap();
    let encoded = encoded.finish();
    let mut input = Decoder::new(&encoded, encoded.len()).unwrap();
    LayerId::decode(&mut input).unwrap();
    CellCore::decode(&mut input).unwrap();
    input.u64().unwrap();
    let offset = encoded.len() - input.remaining();
    let mut bytes = save(&world);
    let start = bytes
        .windows(encoded.len())
        .position(|part| part == encoded)
        .unwrap()
        + offset;
    for (axis, position) in world
        .material_particle(first)
        .unwrap()
        .position_q16()
        .into_iter()
        .enumerate()
    {
        bytes[start + axis * 8..start + (axis + 1) * 8].copy_from_slice(&position.to_le_bytes());
    }
    let mut world = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    assert!(
        world
            .edit(|edit| edit.land_material_particle(first))
            .is_err()
    );
    assert_eq!(save(&world), bytes);
    let report = world
        .edit(|edit| edit.step_material_particles(2, 20))
        .unwrap();
    assert_eq!((report.waiting, report.landed), (2, 0));
    assert_eq!(world.material_particles().count(), 2);
    // 先行する粒子を空き先へ分ければ、残った粒子は次phaseに着地できます。
    world
        .edit(|edit| edit.set_particle_velocity(first, [2 * 65536, 0]))
        .unwrap();
    world
        .edit(|edit| edit.step_material_particles(2, 20))
        .unwrap();
    world
        .edit(|edit| edit.step_material_particles(2, 20))
        .unwrap();
    assert!(world.material_particle(second).is_err());
    assert_eq!(value(&world, "test:energy", p(1, 1)), 345);
    assert_eq!(
        world
            .particle_channel_value::<u64>(first, key("test:energy"))
            .unwrap(),
        345
    );
}

#[test]
fn fast_particles_stop_before_other_particle_sources_in_all_octants() {
    for (dx, dy) in [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        let source = p(10, 10);
        let obstacle = p(10 + 3 * dx, 10 + 3 * dy);
        let stopped = p(10 + 2 * dx, 10 + 2 * dy);
        place(&mut world, layer, source);
        place(&mut world, layer, obstacle);
        let first = world
            .edit(|edit| {
                edit.emit_material_particle(
                    layer,
                    source,
                    [i64::from(dx) * 8 * 65536, i64::from(dy) * 8 * 65536],
                )
            })
            .unwrap();
        world
            .edit(|edit| edit.emit_material_particle(layer, obstacle, [0, 0]))
            .unwrap();
        let report = world
            .edit(|edit| edit.step_material_particles(2, 100))
            .unwrap();
        assert_eq!((report.waiting, report.landed), (1, 1));
        assert_eq!(world.material_particle(first).unwrap().cell(), stopped);
        world = restore(&world);
        world
            .edit(|edit| edit.step_material_particles(2, 100))
            .unwrap();
        assert_eq!(world.material_particles().count(), 0);
        assert_eq!(value(&world, "test:energy", stopped), 345);
        assert_eq!(value(&world, "test:energy", obstacle), 345);
    }
}

#[test]
fn saved_particle_job_resumes_by_candidate_count_and_matches_reference_atomically() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(0, 0));
    place(&mut world, layer, p(1, 0));
    let wall = CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key("test:wall")))
            .unwrap(),
        flags: 0,
        variation: 0,
    };
    world
        .edit(|edit| edit.set_cell(layer, p(3, 0), wall))
        .unwrap();
    world
        .edit(|edit| {
            edit.emit_material_particle(layer, p(0, 0), [65536, 0])?;
            edit.emit_material_particle(layer, p(1, 0), [2 * 65536, 0])?;
            Ok(())
        })
        .unwrap();
    let mut reference = restore(&world);
    let before = save(&world);
    let mut job = MaterialParticleJob::new(&world, 8, 16).unwrap();
    assert_eq!(job.status(), MaterialParticleJobStatus::Computing);
    assert_eq!(
        job.step(&world, 1).unwrap(),
        MaterialParticleJobStatus::Computing
    );
    assert_eq!(save(&world), before, "Jobの検査中はWorldを書き換えない");

    job = particle_job_roundtrip(&job);
    assert_eq!(job.report().processed, 1);
    assert_eq!(
        job.step(&world, 1).unwrap(),
        MaterialParticleJobStatus::Ready
    );
    let job_report = world.edit(|edit| job.publish(edit)).unwrap();
    let reference_report = reference
        .edit(|edit| edit.step_material_particles(8, 16))
        .unwrap();
    assert_eq!(
        (
            job_report.processed,
            job_report.moved,
            job_report.landed,
            job_report.waiting,
            job_report.checked_steps,
        ),
        (
            reference_report.processed,
            reference_report.moved,
            reference_report.landed,
            reference_report.waiting,
            reference_report.checked_steps,
        )
    );
    assert_eq!(save(&world), save(&reference));
}

#[test]
fn saved_particle_job_matches_particle_phase_across_all_octants() {
    for (dx, dy) in [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        let (mut world, layer) = fixture(WorldLimits::default());
        activate(&mut world);
        let source = p(10, 10);
        let obstacle = p(10 + 3 * dx, 10 + 3 * dy);
        place(&mut world, layer, source);
        place(&mut world, layer, obstacle);
        world
            .edit(|edit| {
                edit.emit_material_particle(
                    layer,
                    source,
                    [i64::from(dx) * 8 * 65536, i64::from(dy) * 8 * 65536],
                )?;
                edit.emit_material_particle(layer, obstacle, [0, 0])?;
                Ok(())
            })
            .unwrap();
        let mut reference = restore(&world);
        let mut job = MaterialParticleJob::new(&world, 4, 100).unwrap();
        assert_eq!(
            job.step(&world, 1).unwrap(),
            MaterialParticleJobStatus::Computing
        );
        job = particle_job_roundtrip(&job);
        assert_eq!(
            job.step(&world, 1).unwrap(),
            MaterialParticleJobStatus::Ready
        );
        let job_report = world.edit(|edit| job.publish(edit)).unwrap();
        let reference_report = reference
            .edit(|edit| edit.step_material_particles(4, 100))
            .unwrap();
        assert_eq!(
            (
                job_report.processed,
                job_report.moved,
                job_report.landed,
                job_report.waiting,
                job_report.checked_steps,
            ),
            (
                reference_report.processed,
                reference_report.moved,
                reference_report.landed,
                reference_report.waiting,
                reference_report.checked_steps,
            )
        );
        assert_eq!(save(&world), save(&reference));
    }
}

#[test]
fn saved_particle_job_rejects_stale_particle_inputs_without_partial_publish() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(0, 0));
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(0, 0), [65536, 0]))
        .unwrap();
    let mut job = MaterialParticleJob::new(&world, 4, 8).unwrap();
    assert_eq!(
        job.step(&world, 1).unwrap(),
        MaterialParticleJobStatus::Ready
    );
    world
        .edit(|edit| edit.set_particle_velocity(handle, [0, 65536]))
        .unwrap();
    let before = save(&world);
    assert!(world.edit(|edit| job.publish(edit)).is_err());
    assert_eq!(save(&world), before);
}

#[test]
fn saved_particle_job_rejects_forged_plan_that_jumps_past_a_wall() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(4, 0));
    place(&mut world, layer, p(0, 0));
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                p(3, 0),
                CellCore {
                    material: wall,
                    ..CellCore::AIR
                },
            )?;
            edit.emit_material_particle(layer, p(4, 0), [65536, 0])?;
            edit.emit_material_particle(layer, p(0, 0), [5 * 65536, 0])?;
            Ok(())
        })
        .unwrap();
    let mut job = MaterialParticleJob::new(&world, 4, 16).unwrap();
    assert_eq!(
        job.step(&world, 2).unwrap(),
        MaterialParticleJobStatus::Ready
    );

    let mut encoded = Encoder::new(4_000_000);
    job.encode(&mut encoded).unwrap();
    let mut bytes = encoded.finish();
    // 二件目のplanだけを、壁の向こうにいる先行particleのcellへ改ざんする。
    let plan = bytes.len() - 25;
    bytes[plan..plan + 8].copy_from_slice(&(5i64 * 65536 + 32768).to_le_bytes());
    bytes[plan + 16..plan + 20].copy_from_slice(&5i32.to_le_bytes());
    let mut input = Decoder::new(&bytes, bytes.len()).unwrap();
    let forged = MaterialParticleJob::decode(&mut input).unwrap();
    input.finish().unwrap();

    let before = save(&world);
    assert!(world.edit(|edit| forged.publish(edit)).is_err());
    assert_eq!(save(&world), before);
}

#[test]
fn saved_particle_job_checks_candidate_step_and_byte_budgets() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(0, 0));
    place(&mut world, layer, p(10, 10));
    world
        .edit(|edit| {
            edit.emit_material_particle(layer, p(0, 0), [2 * 65536, 0])?;
            edit.emit_material_particle(layer, p(10, 10), [0, 0])?;
            Ok(())
        })
        .unwrap();
    let before = save(&world);
    let error = MaterialParticleJob::new(&world, 1, 16).unwrap_err();
    assert_eq!(error.code, ErrorCode::CapacityExceeded);
    let mut job = MaterialParticleJob::new(&world, 4, 1).unwrap();
    let error = job.step(&world, 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::CapacityExceeded);
    assert_eq!(
        job.report().processed,
        0,
        "未完了候補の途中状態を保持しない"
    );
    assert_eq!(save(&world), before);

    let low_memory = WorldLimits {
        job_bytes: 256,
        ..WorldLimits::default()
    };
    let (mut limited, layer) = fixture(low_memory);
    activate(&mut limited);
    place(&mut limited, layer, p(0, 0));
    limited
        .edit(|edit| edit.emit_material_particle(layer, p(0, 0), [0, 0]))
        .unwrap();
    let error = MaterialParticleJob::new(&limited, 4, 8).unwrap_err();
    assert_eq!(error.code, ErrorCode::CapacityExceeded);
}

#[test]
fn saved_particle_job_codec_rejects_every_truncation() {
    let (mut world, layer) = fixture(WorldLimits::default());
    activate(&mut world);
    place(&mut world, layer, p(0, 0));
    world
        .edit(|edit| edit.emit_material_particle(layer, p(0, 0), [65_536, 0]))
        .unwrap();
    let mut job = MaterialParticleJob::new(&world, 4, 8).unwrap();
    assert_eq!(
        job.step(&world, 1).unwrap(),
        MaterialParticleJobStatus::Ready
    );

    let mut encoded = Encoder::new(4_000_000);
    job.encode(&mut encoded).unwrap();
    let bytes = encoded.finish();
    for length in 0..bytes.len() {
        let mut input = Decoder::new(&bytes[..length], 4_000_000).unwrap();
        assert!(
            MaterialParticleJob::decode(&mut input).is_err(),
            "prefix {length}"
        );
    }
}
