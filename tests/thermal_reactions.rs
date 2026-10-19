//! E06/Q05の公開World取引と熱channelの保存境界を確認します。

use charon_engine::{
    foundation::{
        StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::*,
};

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

fn thermal_world() -> (World, LayerId, StableKey, EnthalpyFamily) {
    thermal_world_with_limits(WorldLimits::default())
}

fn thermal_world_with_limits(limits: WorldLimits) -> (World, LayerId, StableKey, EnthalpyFamily) {
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
    for material in ["test:ice", "test:water", "test:ash"] {
        registry
            .register(MaterialBuilder::new(material, MaterialBase::Solid).unwrap())
            .unwrap();
    }
    registry
        .schema(Schema::of::<ThermalCellState>(key("game:thermal-state"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ThermalEdgeState>(key("game:thermal-edge"), 1, 18).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("game:quantity"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ReactionSidecar>(key("game:reaction-state"), 1, 18).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ReactionJob>(key("game:reaction-job"), 1, 1_000_000).unwrap())
        .unwrap();
    let channel = key("game:thermal-state");
    registry
        .material_channel(
            key("game:front"),
            ChannelDefinition {
                key: channel,
                version: 1,
                value_schema: key("game:thermal-state"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 2,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("game:thermal-edge"),
            version: 1,
            value_schema: key("game:thermal-edge"),
            layout: GridLayout {
                origin: point(0, 0),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: ThermalEdgeState::default(),
            limits: GridLimits {
                chunks: 2,
                checkpoint_bytes: 1_000_000,
            },
        })
        .unwrap();
    registry
        .material_channel(
            key("game:front"),
            ChannelDefinition {
                key: key("game:quantity"),
                version: 1,
                value_schema: key("game:quantity"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0_u64,
                limits: GridLimits {
                    chunks: 2,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    registry
        .material_channel(
            key("game:front"),
            ChannelDefinition {
                key: key("game:reaction"),
                version: 1,
                value_schema: key("game:reaction-state"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ReactionSidecar::default(),
                limits: GridLimits {
                    chunks: 2,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("game:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        8,
        WorldBounds::expandable(),
        content.clone(),
        limits,
    )
    .unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("game:thermal-test"),
                region,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                point(0, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ice")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_cell(
                layer,
                point(1, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ice")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(channel, point(0, 0), ThermalCellState { energy: 80 })?;
            edit.set_channel(channel, point(1, 0), ThermalCellState { energy: 20 })?;
            Ok(())
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        key("test:water-family"),
        vec![
            EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 100,
                material: key("test:ice"),
            },
            EnthalpyBand::Latent {
                energy_start: 100,
                energy_end: 200,
                temperature_mk: 100,
                from: key("test:ice"),
                to: key("test:water"),
            },
            EnthalpyBand::Sensible {
                energy_start: 200,
                energy_end: 300,
                temperature_start_mk: 100,
                temperature_end_mk: 200,
                material: key("test:water"),
            },
            EnthalpyBand::Latent {
                energy_start: 300,
                energy_end: 400,
                temperature_mk: 200,
                from: key("test:water"),
                to: key("test:ash"),
            },
            EnthalpyBand::Sensible {
                energy_start: 400,
                energy_end: 500,
                temperature_start_mk: 200,
                temperature_end_mk: 300,
                material: key("test:ash"),
            },
        ],
    )
    .unwrap();
    (world, layer, channel, family)
}

#[test]
fn world_edit_thermal_step_is_atomic_and_updates_material_channel() {
    let (mut world, layer, channel, family) = thermal_world();
    let report = world
        .edit(|edit| {
            edit.step_thermal_channel(
                layer,
                channel,
                key("game:thermal-edge"),
                &family,
                point(0, 0),
                2,
                1,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
            )
        })
        .unwrap();
    assert_eq!(report.edges, 1);
    let left = match world
        .channel_value::<ThermalCellState>(channel, point(0, 0))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("読込済みthermal値が必要"),
    };
    let right = match world
        .channel_value::<ThermalCellState>(channel, point(1, 0))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("読込済みthermal値が必要"),
    };
    assert_eq!(left.energy + right.energy, 100);
    assert!(left.energy < 80);
    assert!(right.energy > 20);
}

#[test]
fn thermal_enthalpy_crossing_updates_the_cell_phase_material() {
    let (mut world, layer, channel, family) = thermal_world();
    let content = world.content().clone();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(0, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ash")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(channel, point(0, 0), ThermalCellState { energy: 450 })?;
            edit.set_channel(channel, point(1, 0), ThermalCellState { energy: 149 })
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.step_thermal_channel(
                layer,
                channel,
                key("game:thermal-edge"),
                &family,
                point(0, 0),
                2,
                1,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
            )
        })
        .unwrap();
    let expected = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    match world.cell(layer, point(1, 0)).unwrap() {
        CellQuery::Loaded(view) => assert_eq!(view.core.material, expected),
        _ => panic!("phase cellが読込済みであること"),
    }
}

#[test]
fn cooling_reverses_phase_material_without_losing_energy() {
    let (mut world, layer, channel, family) = thermal_world();
    let content = world.content().clone();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(0, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:water")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(channel, point(0, 0), ThermalCellState { energy: 190 })?;
            edit.set_channel(channel, point(1, 0), ThermalCellState { energy: 10 })
        })
        .unwrap();
    for _ in 0..16 {
        world
            .edit(|edit| {
                edit.step_thermal_channel(
                    layer,
                    channel,
                    key("game:thermal-edge"),
                    &family,
                    point(0, 0),
                    2,
                    1,
                    ThermalStepConfig {
                        conductance_numerator: 1,
                        conductance_denominator: 1,
                    },
                )
            })
            .unwrap();
    }
    let total = [point(0, 0), point(1, 0)]
        .into_iter()
        .map(|point| {
            match world
                .channel_value::<ThermalCellState>(channel, point)
                .unwrap()
            {
                ChannelQuery::Loaded(state) => state.energy,
                _ => panic!("冷却対象のenthalpyが読込済みであること"),
            }
        })
        .sum::<i64>();
    assert_eq!(total, 200);
    let ice = content
        .material_id(MaterialHandle(key("test:ice")))
        .unwrap();
    for point in [point(0, 0), point(1, 0)] {
        match world.cell(layer, point).unwrap() {
            CellQuery::Loaded(view) => assert_eq!(view.core.material, ice),
            _ => panic!("冷却後cellが読込済みであること"),
        }
    }
}

#[test]
fn particle_pool_capacity_defers_reaction_without_consuming_source() {
    let limits = WorldLimits {
        particles: 1,
        ..WorldLimits::default()
    };
    let (mut world, layer, channel, family) = thermal_world_with_limits(limits);
    let content = world.content().clone();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(0, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ice")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(channel, point(0, 0), ThermalCellState { energy: 80 })?;
            edit.set_channel(key("game:quantity"), point(0, 0), 1_u64)?;
            edit.set_cell(layer, point(1, 0), CellCore::AIR)?;
            edit.set_channel(key("game:quantity"), point(1, 0), 0_u64)?;
            edit.set_channel(channel, point(1, 0), ThermalCellState::default())?;
            edit.set_channel(
                key("game:reaction"),
                point(0, 0),
                ReactionSidecar {
                    fuel: 2,
                    timer: 2,
                    ..Default::default()
                },
            )?;
            edit.set_channel(
                key("game:reaction"),
                point(1, 0),
                ReactionSidecar::default(),
            )?;
            edit.set_cell(
                layer,
                point(10, 10),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ash")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(key("game:quantity"), point(10, 10), 1_u64)?;
            edit.emit_material_particle(layer, point(10, 10), [0, 0])?;
            Ok(())
        })
        .unwrap();
    let rule = ReactionRule {
        key: key("test:burn-particle"),
        input: key("test:ice"),
        contact: None,
        priority: 1,
        consume: 1,
        output: key("test:ash"),
        produce: 1,
        output_offset: [1, 0],
        particle_velocity_q16: Some([0, 0]),
        minimum_temperature_mk: 0,
        fuel_cost: 1,
        timer_cost: 1,
        energy_delta: 5,
        maximum_wetness: u16::MAX,
        corrosion_delta: 0,
        state_transfer: ReactionStateTransfer::MigrateToOutput,
        chain_policy: ReactionChainPolicy::Deferred,
    };
    rule.validate(&family).unwrap();
    let rules = ReactionRuleSet::new(&family, vec![rule]).unwrap();
    let (_, report) = world
        .edit(|edit| {
            edit.step_reaction_channels(
                layer,
                channel,
                key("game:quantity"),
                key("game:reaction"),
                &rules,
                point(0, 0),
                2,
                1,
                16,
                1,
                None,
                key("game:reaction-job-state"),
                key("game:reaction-job"),
            )
        })
        .unwrap();
    assert_eq!(report.committed, 0);
    assert_eq!(report.deferred, 1);
    assert_eq!(world.material_particles().count(), 1);
    match world.cell(layer, point(0, 0)).unwrap() {
        CellQuery::Loaded(view) => assert_eq!(
            view.core.material,
            content
                .material_id(MaterialHandle(key("test:ice")))
                .unwrap()
        ),
        _ => panic!("reaction sourceが読込済みであること"),
    }
    assert!(matches!(
        world
            .channel_value::<u64>(key("game:quantity"), point(0, 0))
            .unwrap(),
        ChannelQuery::Loaded(1)
    ));
    assert!(matches!(
        world
            .channel_value::<ReactionSidecar>(key("game:reaction"), point(0, 0))
            .unwrap(),
        ChannelQuery::Loaded(value) if value.fuel == 2
    ));
}

#[test]
fn reaction_job_and_material_channels_commit_together_in_world_edit() {
    let (mut world, layer, channel, family) = thermal_world();
    let content = world.content().clone();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(0, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ice")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(channel, point(0, 0), ThermalCellState { energy: 80 })?;
            edit.set_channel(key("game:quantity"), point(0, 0), 1_u64)?;
            edit.set_channel(key("game:quantity"), point(1, 0), 1_u64)?;
            edit.set_channel(
                key("game:reaction"),
                point(0, 0),
                ReactionSidecar {
                    fuel: 2,
                    timer: 2,
                    ..Default::default()
                },
            )?;
            edit.set_channel(
                key("game:reaction"),
                point(1, 0),
                ReactionSidecar::default(),
            )
        })
        .unwrap();
    let rule = ReactionRule {
        key: key("test:burn"),
        input: key("test:ice"),
        contact: None,
        priority: 1,
        consume: 1,
        output: key("test:ash"),
        produce: 1,
        output_offset: [1, 0],
        particle_velocity_q16: Some([256, 0]),
        minimum_temperature_mk: 0,
        fuel_cost: 1,
        timer_cost: 1,
        energy_delta: 5,
        maximum_wetness: u16::MAX,
        corrosion_delta: 0,
        state_transfer: ReactionStateTransfer::MigrateToOutput,
        chain_policy: ReactionChainPolicy::Deferred,
    };
    let rules = ReactionRuleSet::new(&family, vec![rule]).unwrap();
    world
        .edit(|edit| {
            edit.set_cell(layer, point(1, 0), CellCore::AIR)?;
            edit.set_channel(key("game:quantity"), point(1, 0), 0_u64)?;
            edit.set_channel(
                key("game:thermal-state"),
                point(1, 0),
                ThermalCellState::default(),
            )?;
            edit.set_channel(
                key("game:reaction"),
                point(1, 0),
                ReactionSidecar::default(),
            )
        })
        .unwrap();
    let before_failed_edit = world.checkpoint(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.step_reaction_channels(
                layer,
                channel,
                key("game:quantity"),
                key("game:reaction"),
                &rules,
                point(0, 0),
                2,
                1,
                16,
                1,
                None,
                key("game:reaction-job-state"),
                key("game:quantity"),
            ))
            .is_err()
    );
    assert_eq!(world.checkpoint(2_000_000).unwrap(), before_failed_edit);
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(1, 0),
                CellCore {
                    material: content.material_id(MaterialHandle(key("test:ice")))?,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(key("game:quantity"), point(1, 0), 1_u64)?;
            edit.set_channel(
                key("game:thermal-state"),
                point(1, 0),
                ThermalCellState { energy: 20 },
            )
        })
        .unwrap();
    let (_, deferred) = world
        .edit(|edit| {
            edit.step_reaction_channels(
                layer,
                channel,
                key("game:quantity"),
                key("game:reaction"),
                &rules,
                point(0, 0),
                2,
                1,
                16,
                1,
                None,
                key("game:reaction-job-state"),
                key("game:reaction-job"),
            )
        })
        .unwrap();
    assert_eq!(deferred.deferred, 1);
    assert_eq!(
        world
            .state::<ReactionJob>(key("game:reaction-job-state"))
            .unwrap()
            .remaining(),
        1
    );
    let checkpoint = world.checkpoint(2_000_000).unwrap();
    let mut world = World::restore(
        &checkpoint,
        content.clone(),
        WorldLimits::default(),
        2_000_000,
    )
    .unwrap();
    let resume = world
        .state::<ReactionJob>(key("game:reaction-job-state"))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(layer, point(1, 0), CellCore::AIR)?;
            edit.set_channel(key("game:quantity"), point(1, 0), 0_u64)?;
            edit.set_channel(
                key("game:thermal-state"),
                point(1, 0),
                ThermalCellState::default(),
            )?;
            edit.set_channel(
                key("game:reaction"),
                point(1, 0),
                ReactionSidecar::default(),
            )
        })
        .unwrap();
    let (_, report) = world
        .edit(|edit| {
            edit.step_reaction_channels(
                layer,
                channel,
                key("game:quantity"),
                key("game:reaction"),
                &rules,
                point(0, 0),
                2,
                1,
                16,
                1,
                Some(resume),
                key("game:reaction-job-state"),
                key("game:reaction-job"),
            )
        })
        .unwrap();
    assert_eq!(report.committed, 1);
    match world.cell(layer, point(0, 0)).unwrap() {
        CellQuery::Loaded(view) => assert_eq!(view.core, CellCore::AIR),
        _ => panic!("source cellが読込済みであること"),
    }
    match world.cell(layer, point(1, 0)).unwrap() {
        CellQuery::Loaded(view) => assert_eq!(view.core, CellCore::AIR),
        _ => panic!("particle出力先cellが読込済みであること"),
    }
    let particles = world.material_particles().collect::<Vec<_>>();
    assert_eq!(particles.len(), 1);
    assert_eq!(particles[0].1.cell(), point(1, 0));
    assert_eq!(particles[0].1.velocity_q16(), [256, 0]);
    assert_eq!(
        particles[0].1.core().material,
        content
            .material_id(MaterialHandle(key("test:ash")))
            .unwrap()
    );
    assert_eq!(
        world
            .particle_channel_value::<ThermalCellState>(particles[0].0, channel)
            .unwrap()
            .energy,
        85
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(particles[0].0, key("game:quantity"))
            .unwrap(),
        1
    );
    assert_eq!(
        world
            .particle_channel_value::<ReactionSidecar>(particles[0].0, key("game:reaction"))
            .unwrap()
            .fuel,
        1
    );
}
