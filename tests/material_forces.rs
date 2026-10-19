//! Q16の物性・独自力学provider・DDA統合を公開APIから検証します。
use charon_engine::{
    foundation::{EngineResult, ErrorCode, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;

fn key(text: &str) -> StableKey {
    StableKey::parse(text).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn physics(
    base: MaterialBase,
    gravity: u32,
    drag: u32,
    buoyancy: u32,
    friction: u32,
) -> MaterialMotionProperties {
    let _ = base;
    MaterialMotionProperties {
        gravity_scale_q16: gravity,
        drag_q16: drag,
        buoyancy_scale_q16: buoyancy,
        friction_q16: friction,
    }
}
fn content(sand_drag: u32) -> Arc<Content> {
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
        .layer(LayerDef {
            key: key("test:back"),
            role: LayerRole::Terrain,
            render_order: -1,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .layer(LayerDef {
            key: key("test:decor"),
            role: LayerRole::Decoration,
            render_order: -2,
            collision_group: 0,
            simulated: false,
        })
        .unwrap();
    registry
        .layer(LayerDef {
            key: key("test:visual-only"),
            role: LayerRole::Decoration,
            render_order: 2,
            collision_group: 0,
            simulated: false,
        })
        .unwrap();
    for (name, base, mass, properties) in [
        (
            "test:sand",
            MaterialBase::Powder,
            4,
            physics(MaterialBase::Powder, 65536, sand_drag, 0, 32768),
        ),
        (
            "test:liquid",
            MaterialBase::Liquid,
            2,
            physics(MaterialBase::Liquid, 65536, 0, 65536, 0),
        ),
        (
            "test:water",
            MaterialBase::Liquid,
            10,
            physics(MaterialBase::Liquid, 0, 0, 0, 0),
        ),
        (
            "test:wall",
            MaterialBase::Solid,
            100,
            physics(MaterialBase::Solid, 0, 0, 0, 0),
        ),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, base)
                    .unwrap()
                    .mass_per_cell(mass)
                    .motion_properties(properties),
            )
            .unwrap();
    }
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<i64>(key("test:pressure-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:sleep-state-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("test:pressure"),
            version: 1,
            value_schema: key("test:pressure-schema"),
            layout: GridLayout {
                origin: p(0, 0),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0i64,
            limits: GridLimits {
                chunks: 8,
                checkpoint_bytes: 1_000_000,
            },
        })
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-schema"),
                layout: GridLayout {
                    origin: p(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MaterialMotion::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    registry.freeze().unwrap()
}
fn fixture() -> (World, LayerId) {
    let content = content(32768);
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
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    (world, layer)
}
fn place(world: &mut World, layer: LayerId, point: CellPoint, name: &str, velocity: [i64; 2]) {
    let material = world
        .content()
        .material_id(MaterialHandle(key(name)))
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
            edit.set_channel(
                key("test:motion"),
                point,
                MaterialMotion {
                    velocity_q16: velocity,
                    ..Default::default()
                },
            )
        })
        .unwrap();
}
fn motion(world: &World, point: CellPoint) -> MaterialMotion {
    match world.channel_value(key("test:motion"), point).unwrap() {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("速度channelが必要"),
    }
}
fn force(limit: i64) -> MaterialForceConfig {
    MaterialForceConfig {
        channel: key("test:motion"),
        gravity_q16: [0, 65536],
        candidate_limit: 100,
        max_velocity_q16: limit,
    }
}
fn travel() -> MaterialMotionConfig {
    MaterialMotionConfig {
        channel: key("test:motion"),
        candidate_limit: 100,
        max_distance: 8,
        step_limit: 500,
    }
}

struct StencilForceProvider;
impl MaterialForceProvider for StencilForceProvider {
    fn key(&self) -> StableKey {
        key("test:stencil-force")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        let signal = input
            .world
            .channel_value::<i64>(key("test:pressure"), p(input.point.x + 2, input.point.y))?;
        if matches!(signal, ChannelQuery::Loaded(value) if value > 0) {
            Ok(MaterialMotion {
                velocity_q16: [0, 65536],
                remainder_q16: [0, 0],
            })
        } else {
            Ok(input.motion)
        }
    }
    fn sleep_stencil(&self) -> Option<MaterialForceDependencyStencil> {
        Some(MaterialForceDependencyStencil::new(2, 0).unwrap())
    }
    fn sleep_invariant(
        &self,
        world: &World,
        _layer: LayerId,
        _force: MaterialForceConfig,
        _motion: MaterialMotionConfig,
    ) -> EngineResult<bool> {
        match world.state::<u64>(key("test:sleep-state")) {
            Ok(value) => Ok(value == 0),
            Err(error) if error.code == ErrorCode::MissingKey => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[test]
fn provider_without_sleep_contract_always_uses_reference_execution() {
    let (mut base, layer) = fixture();
    place(&mut base, layer, p(8, 8), "test:sand", [0, 0]);
    let snapshot = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &snapshot,
        Arc::clone(base.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut optimized = World::restore(
        &snapshot,
        Arc::clone(base.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let provider = StandardMaterialForces;
    let force = zero_gravity();
    let motion = travel();
    let mut tracker = MaterialProviderSleepTracker::default();

    for _ in 0..2 {
        let expected = reference
            .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
            .unwrap();
        let actual = optimized
            .step_material_force_motion_sleeping_with_provider(
                &mut tracker,
                layer,
                force,
                motion,
                &provider,
            )
            .unwrap();
        assert!(!actual.sleeping);
        assert!(!tracker.has_proof());
        assert_eq!(actual.reference, expected);
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap()
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }
}

#[test]
fn author_provider_sleep_wakes_only_inside_declared_dependency_and_revalidates_restore() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};

    let (mut base, layer) = fixture();
    base.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
        edit.insert_empty_chunk(ChunkCoord::new(2, 0)?)
    })
    .unwrap();
    place(&mut base, layer, p(63, 8), "test:sand", [0, 0]);
    let snapshot = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &snapshot,
        Arc::clone(base.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut optimized = World::restore(
        &snapshot,
        Arc::clone(base.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let provider = StencilForceProvider;
    let force = zero_gravity();
    let motion = travel();
    let mut tracker = MaterialProviderSleepTracker::default();

    for index in 0..3 {
        let expected = reference
            .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
            .unwrap();
        let actual = optimized
            .step_material_force_motion_sleeping_with_provider(
                &mut tracker,
                layer,
                force,
                motion,
                &provider,
            )
            .unwrap();
        assert_eq!(actual.reference, expected);
        assert_eq!(actual.sleeping, index != 0);
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap()
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }

    // 保存復元したtrackerは同じWorldでも一度参照実行してから再証明します。
    let mut encoder = Encoder::new(1_000_000);
    tracker.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut decoder = Decoder::new(&bytes, bytes.len()).unwrap();
    let mut decoded = MaterialProviderSleepTracker::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    let expected = reference
        .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
        .unwrap();
    let actual = optimized
        .step_material_force_motion_sleeping_with_provider(
            &mut decoded,
            layer,
            force,
            motion,
            &provider,
        )
        .unwrap();
    assert!(!actual.sleeping);
    assert_eq!(actual.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );

    // 依存矩形の外側chunkの変更はwakeせず、参照結果のままsleepを続けます。
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| edit.set_channel(key("test:pressure"), p(130, 8), 1i64))
            .unwrap();
    }
    let expected = reference
        .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
        .unwrap();
    let outside = optimized
        .step_material_force_motion_sleeping_with_provider(
            &mut decoded,
            layer,
            force,
            motion,
            &provider,
        )
        .unwrap();
    assert!(outside.sleeping);
    assert_eq!(outside.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );

    // Provider状態の変化はchunk stencil外でも毎tickの不変条件で検出します。
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| {
                edit.set_state(
                    key("test:sleep-state"),
                    key("test:sleep-state-schema"),
                    &1u64,
                )
            })
            .unwrap();
    }
    let expected = reference
        .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
        .unwrap();
    let state_wake = optimized
        .step_material_force_motion_sleeping_with_provider(
            &mut decoded,
            layer,
            force,
            motion,
            &provider,
        )
        .unwrap();
    assert!(!state_wake.sleeping);
    assert!(state_wake.woke);
    assert_eq!(state_wake.reference, expected);
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| {
                edit.set_state(
                    key("test:sleep-state"),
                    key("test:sleep-state-schema"),
                    &0u64,
                )
            })
            .unwrap();
    }
    let expected = reference
        .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
        .unwrap();
    let reproved = optimized
        .step_material_force_motion_sleeping_with_provider(
            &mut decoded,
            layer,
            force,
            motion,
            &provider,
        )
        .unwrap();
    assert!(!reproved.sleeping);
    assert_eq!(reproved.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );

    // source x=63の+2 stencilはchunk 1へ跨る。そこが変わるとwakeして力を再評価します。
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| edit.set_channel(key("test:pressure"), p(65, 8), 1i64))
            .unwrap();
    }
    let expected = reference
        .edit(|edit| edit.step_material_force_motion(layer, force, motion, &provider))
        .unwrap();
    let wake = optimized
        .step_material_force_motion_sleeping_with_provider(
            &mut decoded,
            layer,
            force,
            motion,
            &provider,
        )
        .unwrap();
    assert!(!wake.sleeping);
    assert!(wake.woke);
    assert_eq!(wake.reference, expected);
    let sand = reference
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        optimized.cell(layer, p(63, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn gravity_drag_and_surface_friction_are_deterministic_q16() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [65536, 0]);
    place(&mut world, layer, p(8, 9), "test:wall", [0, 0]);
    let report = world
        .edit(|edit| {
            edit.integrate_material_forces(layer, force(1_000_000), &StandardMaterialForces)
        })
        .unwrap();
    assert_eq!(report.candidates, 2);
    assert_eq!(report.changed, 1);
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [16384, 65536]);
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(motion(&restored, p(8, 8)).velocity_q16, [16384, 65536]);
}

#[test]
fn buoyancy_from_denser_liquid_reduces_gravity_without_rounding_to_zero() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:liquid", [0, 0]);
    place(&mut world, layer, p(8, 9), "test:water", [0, 0]);
    world
        .edit(|edit| {
            edit.integrate_material_forces(layer, force(1_000_000), &StandardMaterialForces)
        })
        .unwrap();
    // 1 - (10 - 2)/10 = 1/5の重力が残る。正規の整数演算で切り捨てます。
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 13108]);
}

#[test]
fn custom_provider_replaces_standard_gravity_and_atomic_failure_rolls_back() {
    struct NoGravity;
    impl MaterialForceProvider for NoGravity {
        fn key(&self) -> StableKey {
            key("test:no-gravity")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    world
        .edit(|edit| edit.integrate_material_forces(layer, force(1_000_000), &NoGravity))
        .unwrap();
    // 内容が不変でもWorld::editの成功はrevisionを進めます。
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 0]);
    let unchanged = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.integrate_material_forces(layer, force(100), &StandardMaterialForces))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == unchanged);
}

#[test]
fn force_and_motion_publish_once_and_recover_from_checkpoint() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:liquid", [0, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    force(100),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    let report = world
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    assert_eq!(report.force.changed, 1);
    assert_eq!(report.motion.moved, 1);
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material != MaterialId::AIR
    ));
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
}

#[test]
fn motion_properties_are_validated_and_affect_content_identity() {
    let default = {
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
            .register(MaterialBuilder::new("test:sample", MaterialBase::Powder).unwrap())
            .unwrap();
        registry.freeze().unwrap()
    };
    let customized = {
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
        assert!(
            registry
                .register(
                    MaterialBuilder::new("test:bad", MaterialBase::Powder)
                        .unwrap()
                        .motion_properties(physics(MaterialBase::Powder, 65536, 65537, 0, 0))
                )
                .is_err()
        );
        registry
            .register(
                MaterialBuilder::new("test:sample", MaterialBase::Powder)
                    .unwrap()
                    .motion_properties(physics(MaterialBase::Powder, 65536, 32768, 0, 0)),
            )
            .unwrap();
        registry.freeze().unwrap()
    };
    assert_ne!(default.fingerprint(), customized.fingerprint());
}

fn particle_config(cap: i64) -> MaterialParticleForceConfig {
    MaterialParticleForceConfig {
        gravity_q16: [0, 32768],
        candidate_limit: 4,
        max_velocity_q16: cap,
    }
}

#[test]
fn particle_sleep_in_material_phase_matches_reference_and_wakes_after_velocity_edit() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};

    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    let handle = reference
        .edit(|edit| edit.emit_material_particle(layer, p(8, 8), [0, 0]))
        .unwrap();
    let checkpoint = reference.checkpoint(4_000_000).unwrap();
    let mut sleeping = World::restore(
        &checkpoint,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();
    let sleeping_handle = sleeping.material_particles().next().unwrap().0;
    // particleは独立した質量所有者です。同じcellに地形を配置すると静止粒子の着地先が
    // ふさがり、zero-velocity粒子がarenaに残る状態を作れます。
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    place(&mut sleeping, layer, p(8, 8), "test:sand", [0, 0]);
    let force = MaterialParticleForceConfig {
        gravity_q16: [0, 0],
        candidate_limit: 4,
        max_velocity_q16: 1_000_000,
    };
    let phase_config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: None,
        particle: Some((force, 100)),
    };
    let mut tracker = MaterialParticleSleepTracker::default();

    for tick in 0..3 {
        let expected = reference
            .edit(|edit| {
                edit.step_material_particle_forces_in_layer(
                    layer,
                    force,
                    &StandardMaterialForces,
                    100,
                )
            })
            .unwrap();
        reference.edit(|edit| edit.advance_tick()).unwrap();
        let (phase_report, next_tracker, actual) = sleeping
            .edit(|edit| {
                edit.step_material_phase_with_particle_sleep(
                    phase_config,
                    &StandardMaterialForces,
                    tracker.clone(),
                )
            })
            .unwrap();
        tracker = next_tracker;
        assert_eq!(phase_report.particle, Some(actual.reference));
        assert!(phase_report.motion.is_none());
        assert!(phase_report.scalar.is_none());
        assert_eq!(actual.reference, expected);
        assert_eq!(actual.sleeping, tick > 0);
        assert!(!actual.woke);
        sleeping.edit(|edit| edit.advance_tick()).unwrap();
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            sleeping.checkpoint(4_000_000).unwrap(),
            "tick {tick}の正規World状態が参照と一致する"
        );
    }

    let mut encoder = Encoder::new(1_000_000);
    tracker.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut decoder = Decoder::new(&bytes, bytes.len()).unwrap();
    let mut malformed = bytes.clone();
    malformed[8] = 2;
    let mut malformed_decoder = Decoder::new(&malformed, malformed.len()).unwrap();
    assert!(MaterialParticleSleepTracker::decode(&mut malformed_decoder).is_err());
    let decoded_tracker = MaterialParticleSleepTracker::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    let expected = reference
        .edit(|edit| {
            edit.step_material_particle_forces_in_layer(layer, force, &StandardMaterialForces, 100)
        })
        .unwrap();
    let (resumed_phase, _resumed_tracker, resumed) = sleeping
        .edit(|edit| {
            edit.step_material_phase_with_particle_sleep(
                phase_config,
                &StandardMaterialForces,
                decoded_tracker,
            )
        })
        .unwrap();
    assert!(resumed.woke);
    assert!(!resumed.sleeping);
    assert_eq!(resumed_phase.particle, Some(resumed.reference));
    assert_eq!(resumed.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );

    reference
        .edit(|edit| edit.set_particle_velocity(handle, [65536, 0]))
        .unwrap();
    sleeping
        .edit(|edit| edit.set_particle_velocity(sleeping_handle, [65536, 0]))
        .unwrap();
    let expected = reference
        .edit(|edit| {
            edit.step_material_particle_forces_in_layer(layer, force, &StandardMaterialForces, 100)
        })
        .unwrap();
    let (_, _, actual) = sleeping
        .edit(|edit| {
            edit.step_material_phase_with_particle_sleep(
                phase_config,
                &StandardMaterialForces,
                tracker.clone(),
            )
        })
        .unwrap();
    assert!(actual.woke);
    assert!(!actual.sleeping);
    assert_eq!(actual.reference, expected);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );
    assert!(reference.material_particle(handle).is_ok());
}

#[test]
fn material_particle_forces_keep_q16_position_and_survive_checkpoint_resume() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 8), [0, 0]))
        .unwrap();
    let first = world
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    assert_eq!(first.force.changed, 1);
    assert_eq!(first.particles.processed, 1);
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [0, 32768]
    );
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut resumed = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    resumed
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    assert!(world.checkpoint(4_000_000).unwrap() == resumed.checkpoint(4_000_000).unwrap());
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(8, 9));
}

#[test]
fn particle_force_failure_rolls_back_velocity_and_material_ownership() {
    struct RejectRemainder;
    impl MaterialForceProvider for RejectRemainder {
        fn key(&self) -> StableKey {
            key("test:bad-particle-remainder")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, mut input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            assert!(input.particle.is_some());
            input.motion.remainder_q16 = [1, 0];
            Ok(input.motion)
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 8), [0, 0]))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit
                .integrate_material_particle_forces(particle_config(100), &StandardMaterialForces))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == before);
    assert!(
        world
            .edit(|edit| edit
                .integrate_material_particle_forces(particle_config(1_000_000), &RejectRemainder))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == before);
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [0, 0]
    );
}

fn zero_gravity() -> MaterialForceConfig {
    MaterialForceConfig {
        gravity_q16: [0, 0],
        ..force(1_000_000)
    }
}

#[test]
fn sleeping_standard_force_matches_scalar_reference_every_tick_and_wakes_on_edit() {
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    let bytes = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    for index in 0..8 {
        let plain = reference
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    zero_gravity(),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .unwrap();
        let cached = optimized
            .step_standard_material_force_motion_sleeping(
                &mut tracker,
                layer,
                zero_gravity(),
                travel(),
            )
            .unwrap();
        assert_eq!(plain, cached.reference);
        assert_eq!(cached.sleeping, index != 0);
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap()
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }
    // channelを別取引で変更するとchunk revisionが変わるのでwakeします。
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| {
                edit.set_channel(
                    key("test:motion"),
                    p(8, 8),
                    MaterialMotion {
                        velocity_q16: [65536, 0],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
    }
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                zero_gravity(),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let cached = optimized
        .step_standard_material_force_motion_sleeping(&mut tracker, layer, zero_gravity(), travel())
        .unwrap();
    assert!(!cached.sleeping);
    assert!(cached.woke);
    assert_eq!(cached.reference, plain);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn sleeping_solid_is_woken_by_falling_material_and_never_skips_nonzero_gravity() {
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:wall", [0, 0]);
    let bytes = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let first = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert_eq!(plain, first.reference);
    assert!(tracker.has_proof());
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let slept = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(slept.sleeping);
    assert_eq!(plain, slept.reference);
    for world in [&mut reference, &mut optimized] {
        place(world, layer, p(10, 8), "test:sand", [0, 0]);
    }
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let wake = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(!wake.sleeping);
    assert!(wake.woke);
    assert_eq!(plain, wake.reference);
    assert!(!tracker.has_proof());
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn sleeping_proof_is_codec_safe_and_wrong_profile_falls_back_to_reference() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    let mut tracker = MaterialSleepTracker::default();
    reference
        .step_standard_material_force_motion_sleeping(&mut tracker, layer, zero_gravity(), travel())
        .unwrap();
    assert!(tracker.has_proof());
    let mut encoder = Encoder::new(1_000_000);
    tracker.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut reader = Decoder::new(&bytes, bytes.len()).unwrap();
    let mut restored_cache = MaterialSleepTracker::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    let world_bytes = reference.checkpoint(4_000_000).unwrap();
    let mut restored_world = World::restore(
        &world_bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    // 保存復元後はrevision差分の通知がないため、危険なskipをせず再評価。
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                zero_gravity(),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let result = restored_world
        .step_standard_material_force_motion_sleeping(
            &mut restored_cache,
            layer,
            zero_gravity(),
            travel(),
        )
        .unwrap();
    assert!(!result.sleeping);
    assert_eq!(result.reference, plain);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        restored_world.checkpoint(4_000_000).unwrap()
    );
    // 証明は候補予算にも紐づき、より小さい予算へ再利用して失敗を隠さない。
    let unchanged = restored_world.checkpoint(4_000_000).unwrap();
    let mut smaller = travel();
    smaller.candidate_limit = 0;
    assert!(
        restored_world
            .step_standard_material_force_motion_sleeping(
                &mut restored_cache,
                layer,
                zero_gravity(),
                smaller,
            )
            .is_err()
    );
    assert_eq!(restored_world.checkpoint(4_000_000).unwrap(), unchanged);
}

#[test]
fn unavailable_neighbor_does_not_turn_reference_wait_into_false_sleep() {
    let (mut reference, layer) = fixture();
    // 重力倍率が0の静止素材でも、参照forceは重力側の未読込を
    // waiting_for_regionへ記録します。sleepがこの結果を消してはいけません。
    place(&mut reference, layer, p(8, 63), "test:wall", [0, 0]);
    let snapshot = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &snapshot,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    for _ in 0..3 {
        let normal = reference
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    force(1_000_000),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .unwrap();
        assert_eq!(normal.force.waiting_for_region, 1);
        let result = optimized
            .step_standard_material_force_motion_sleeping(
                &mut tracker,
                layer,
                force(1_000_000),
                travel(),
            )
            .unwrap();
        assert_eq!(result.reference, normal);
        assert!(!result.sleeping);
        assert!(!tracker.has_proof());
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap(),
        );
    }
}

#[test]
fn force_proof_from_fork_with_same_world_and_chunk_revision_cannot_skip_motion() {
    let (mut source, layer) = fixture();
    place(&mut source, layer, p(8, 8), "test:wall", [0, 0]);
    let base = source.checkpoint(4_000_000).unwrap();
    let mut stationary = World::restore(
        &base,
        Arc::clone(source.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut falling = World::restore(
        &base,
        Arc::clone(source.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    place(&mut stationary, layer, p(8, 8), "test:wall", [0, 0]);
    place(&mut falling, layer, p(8, 8), "test:sand", [0, 0]);
    let mut proven = MaterialSleepTracker::default();
    stationary
        .step_standard_material_force_motion_sleeping(
            &mut proven,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(proven.has_proof());
    falling.edit(|_| Ok(())).unwrap();
    assert_eq!(stationary.revision(), falling.revision());
    let mut foreign = proven.clone();
    let result = falling
        .step_standard_material_force_motion_sleeping(
            &mut foreign,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(!result.sleeping, "別枝の同revision証明を信頼しない");
    assert!(result.woke);
    let sand = source
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        falling.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
}

fn phase(layer: LayerId) -> MaterialPhaseConfig {
    MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((force(1_000_000), travel())),
        scalar_candidate_limit: Some(100),
        particle: Some((
            MaterialParticleForceConfig {
                gravity_q16: [0, 65536],
                candidate_limit: 100,
                max_velocity_q16: 1_000_000,
            },
            500,
        )),
    }
}

#[test]
fn integrated_material_phase_matches_individual_reference_steps_and_checkpoint() {
    let (mut baseline, layer) = fixture();
    place(&mut baseline, layer, p(8, 8), "test:sand", [0, 0]);
    place(&mut baseline, layer, p(20, 20), "test:liquid", [0, 0]);
    baseline
        .edit(|edit| edit.emit_material_particle(layer, p(20, 20), [0, 0]))
        .unwrap();
    let bytes = baseline.checkpoint(4_000_000).unwrap();
    let mut integrated = World::restore(
        &bytes,
        Arc::clone(baseline.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap();
    let mut reference = World::restore(
        &bytes,
        Arc::clone(baseline.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap();
    // 独立APIとのbyte同値は共有予約が不要な構成で照合する。
    // motionとscalarを同tickに有効にする場合は別の一回移動回帰で検証する。
    let mut config = phase(layer);
    config.scalar_candidate_limit = None;
    for _ in 0..3 {
        let normal = reference
            .edit(|edit| {
                let motion = edit.step_material_force_motion(
                    layer,
                    force(1_000_000),
                    travel(),
                    &StandardMaterialForces,
                )?;
                let particles = edit.step_material_particle_forces(
                    MaterialParticleForceConfig {
                        gravity_q16: [0, 65536],
                        candidate_limit: 100,
                        max_velocity_q16: 1_000_000,
                    },
                    &StandardMaterialForces,
                    500,
                )?;
                Ok(MaterialPhaseReport {
                    pressure: None,
                    motion: Some(motion),
                    scalar: None,
                    scalar_sleep: None,
                    particle: Some(particles),
                })
            })
            .unwrap();
        let combined = integrated
            .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
            .unwrap();
        assert_eq!(combined, normal);
        assert_eq!(
            integrated.checkpoint(4_000_000).unwrap(),
            reference.checkpoint(4_000_000).unwrap(),
        );
        integrated.edit(|edit| edit.advance_tick()).unwrap();
        reference.edit(|edit| edit.advance_tick()).unwrap();
    }
}

#[test]
fn integrated_material_phase_rolls_back_force_and_dda_on_late_scalar_capacity_failure() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    let mut config = phase(layer);
    // force+DDAが実際に物質を移動した後でscalarの候補数制限に失敗する。
    config.scalar_candidate_limit = Some(0);
    assert!(
        world
            .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn integrated_material_phase_ignored_nested_error_poison_still_rolls_back() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    let mut config = phase(layer);
    config.scalar_candidate_limit = Some(0);
    // 呼出側が部分phaseのErrを握り潰してもWorldEdit::runが失敗を記録する。
    assert!(
        world
            .edit(|edit| {
                let _ = edit.step_material_phase(config, &StandardMaterialForces);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

#[test]
fn integrated_phase_never_moves_same_token_twice_via_motion_then_scalar() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let result = world
        .edit(|edit| {
            edit.step_material_phase(
                MaterialPhaseConfig {
                    layer,
                    pressure: None,
                    motion: Some((force(1_000_000), travel())),
                    scalar_candidate_limit: Some(100),
                    particle: None,
                },
                &StandardMaterialForces,
            )
        })
        .unwrap();
    assert_eq!(result.motion.unwrap().motion.moved, 1);
    assert_eq!(result.scalar.unwrap().exchanges, 0);
    assert!(matches!(
        world.cell(layer, p(8, 8)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == MaterialId::AIR
    ));
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
    assert!(matches!(
        world.cell(layer, p(8, 10)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == MaterialId::AIR
    ));
}

#[test]
fn material_phase_rejects_non_simulated_layers_without_world_mutation() {
    let (mut world, _front) = fixture();
    let layer = world.content().layer_id(key("test:decor")).unwrap();
    let snapshot = world.checkpoint(4_000_000).unwrap();
    let particle = MaterialParticleForceConfig {
        gravity_q16: [0, 65536],
        candidate_limit: 16,
        max_velocity_q16: 1_000_000,
    };
    let phase = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: None,
        particle: Some((particle, 16)),
    };
    assert!(
        world
            .edit(|edit| edit.step_material_phase(phase, &StandardMaterialForces))
            .is_err()
    );
    assert!(
        world
            .edit(|edit| edit.integrate_material_particle_forces_in_layer(
                layer,
                particle,
                &StandardMaterialForces
            ))
            .is_err()
    );
    assert!(
        world
            .edit(|edit| edit.step_material_particles_in_layer(layer, 16, 16))
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), snapshot);
}

#[test]
fn world_wide_particle_steps_preserve_non_simulated_layer_owners() {
    let (mut world, _front) = fixture();
    let decorative = world.content().layer_id(key("test:decor")).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let handle = world
        .edit(|edit| {
            edit.set_cell(
                decorative,
                p(8, 8),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.emit_material_particle(decorative, p(8, 8), [0, 65536])
        })
        .unwrap();
    assert_eq!(
        world
            .edit(|edit| edit.integrate_material_particle_forces(
                MaterialParticleForceConfig {
                    gravity_q16: [0, 65536],
                    candidate_limit: 0,
                    max_velocity_q16: 1_000_000,
                },
                &StandardMaterialForces,
            ))
            .unwrap()
            .candidates,
        0,
    );
    let report = world
        .edit(|edit| edit.step_material_particles(0, 0))
        .unwrap();
    assert_eq!(report.processed, 0);
    let particle = world.material_particle(handle).unwrap();
    assert_eq!(particle.cell(), p(8, 8));
    assert_eq!(particle.velocity_q16(), [0, 65536]);
}

#[test]
fn opt_in_legacy_uniform_forces_preserve_old_gas_buoyancy_and_powder_friction() {
    let uniform = UniformMaterialForces {
        gravity_q16: 8000,
        gas_buoyancy_q16: 20000,
        drag_q16: 0,
        powder_friction_q16: 2000,
        maximum_velocity_q16: 100_000,
    };
    let input = LegacyMaterialForceInput {
        point: p(8, 8),
        tick: 7,
        material: key("test:gas"),
        base: MaterialBase::Gas,
        mass_per_cell: 1,
        motion: MaterialMotion::default(),
    };
    assert_eq!(
        LegacyMaterialForceProvider::evaluate(&uniform, input).unwrap(),
        [0, -12000],
        "旧式の一様気体浮力は隣接物質なしで有効"
    );
    let powder = LegacyMaterialForceInput {
        base: MaterialBase::Powder,
        motion: MaterialMotion {
            velocity_q16: [10000, 0],
            ..Default::default()
        },
        ..input
    };
    assert_eq!(
        LegacyMaterialForceProvider::evaluate(&uniform, powder).unwrap(),
        [8000, 8000]
    );
    let invalid = UniformMaterialForces {
        drag_q16: 65537,
        ..uniform
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn legacy_force_entrypoint_keeps_remainder_and_rolls_back_ignored_error() {
    struct FailAfterFirst;
    impl LegacyMaterialForceProvider for FailAfterFirst {
        fn evaluate(&self, input: LegacyMaterialForceInput) -> EngineResult<[i64; 2]> {
            if input.point.x == 9 {
                return Err(charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::InvalidState,
                    "意図的な旧provider失敗",
                ));
            }
            Ok([32768, 0])
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    place(&mut world, layer, p(9, 8), "test:sand", [0, 0]);
    let snapshot = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.apply_material_forces(layer, key("test:motion"), &FailAfterFirst, 10);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), snapshot);
    let uniform = UniformMaterialForces {
        gravity_q16: 8000,
        gas_buoyancy_q16: 0,
        drag_q16: 0,
        powder_friction_q16: 0,
        maximum_velocity_q16: 100_000,
    };
    let updated = world
        .edit(|edit| edit.apply_material_forces(layer, key("test:motion"), &uniform, 10))
        .unwrap();
    assert_eq!(updated.updated, 2);
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 8000]);
    // 新Provider経由でも一様力学の数式は完全に同じ。
    let compatible = world
        .edit(|edit| {
            edit.integrate_material_forces(
                layer,
                MaterialForceConfig {
                    channel: key("test:motion"),
                    gravity_q16: [0, 0],
                    candidate_limit: 10,
                    max_velocity_q16: 100_000,
                },
                &uniform,
            )
        })
        .unwrap();
    assert_eq!(compatible.changed, 2);
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 16000]);
}

#[test]
fn integrated_material_phase_pressure_and_scalar_match_reference_each_tick() {
    let (mut baseline, layer) = fixture();
    place(&mut baseline, layer, p(8, 8), "test:liquid", [0, 0]);
    place(&mut baseline, layer, p(9, 8), "test:water", [0, 0]);
    let bytes = baseline.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &bytes,
        Arc::clone(baseline.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap();
    let mut combined = World::restore(
        &bytes,
        Arc::clone(baseline.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap();
    let pressure = PressureRelaxation {
        channel: key("test:pressure"),
        iterations: 4,
        gravity_q16: 65536,
        cell_limit: 16,
    };
    let mut config = phase(layer);
    config.pressure = Some(pressure);
    config.motion = None;
    for tick in 0..4 {
        let expected = reference
            .edit(|edit| {
                let pressure_report = edit.relax_pressure(layer, pressure)?;
                let scalar_report = edit.transport_materials(layer, 100)?;
                let particle_report = edit.step_material_particle_forces(
                    MaterialParticleForceConfig {
                        gravity_q16: [0, 65536],
                        candidate_limit: 100,
                        max_velocity_q16: 1_000_000,
                    },
                    &StandardMaterialForces,
                    500,
                )?;
                edit.advance_tick()?;
                Ok(MaterialPhaseReport {
                    pressure: Some(pressure_report),
                    motion: None,
                    scalar: Some(scalar_report),
                    scalar_sleep: None,
                    particle: Some(particle_report),
                })
            })
            .unwrap();
        let actual = combined
            .edit(|edit| {
                let report = edit.step_material_phase(config, &StandardMaterialForces)?;
                edit.advance_tick()?;
                Ok(report)
            })
            .unwrap();
        assert_eq!(
            actual, expected,
            "tick {tick}のphase reportが参照と一致する"
        );
        assert_eq!(
            combined.checkpoint(4_000_000).unwrap(),
            reference.checkpoint(4_000_000).unwrap(),
            "tick {tick}の正規World状態が参照と一致する"
        );
    }
}

#[test]
fn particle_stage_is_layer_scoped_and_cannot_integrate_other_layers_twice() {
    let (mut world, front) = fixture();
    let back = world.content().layer_id(key("test:back")).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let front_handle = world
        .edit(|edit| {
            edit.set_cell(
                front,
                p(8, 8),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.emit_material_particle(front, p(8, 8), [0, 0])
        })
        .unwrap();
    let back_handle = world
        .edit(|edit| {
            edit.set_cell(
                back,
                p(16, 16),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.emit_material_particle(back, p(16, 16), [0, 0])
        })
        .unwrap();
    let config = MaterialPhaseConfig {
        layer: front,
        pressure: None,
        motion: None,
        scalar_candidate_limit: None,
        particle: Some((
            MaterialParticleForceConfig {
                gravity_q16: [0, 65536],
                candidate_limit: 1,
                max_velocity_q16: 1_000_000,
            },
            100,
        )),
    };
    let first = world
        .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
        .unwrap();
    assert_eq!(first.particle.unwrap().force.candidates, 1);
    assert_eq!(
        world
            .material_particle(front_handle)
            .unwrap()
            .velocity_q16(),
        [0, 65536]
    );
    assert_eq!(
        world.material_particle(back_handle).unwrap().velocity_q16(),
        [0, 0]
    );
    let front_cell = world.material_particle(front_handle).unwrap().cell();
    let second = world
        .edit(|edit| {
            edit.step_material_phase(
                MaterialPhaseConfig {
                    layer: back,
                    ..config
                },
                &StandardMaterialForces,
            )
        })
        .unwrap();
    assert_eq!(second.particle.unwrap().force.candidates, 1);
    assert_eq!(
        world.material_particle(back_handle).unwrap().velocity_q16(),
        [0, 65536]
    );
    assert_eq!(
        world
            .material_particle(front_handle)
            .unwrap()
            .velocity_q16(),
        [0, 65536]
    );
    assert_eq!(
        world.material_particle(front_handle).unwrap().cell(),
        front_cell
    );
}

#[test]
fn scalar_and_motion_never_move_resident_material_onto_a_canonical_particle() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 9), "test:sand", [0, 0]);
    let particle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 9), [0, 0]))
        .unwrap();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    // 粉体が斜めに逃げた場合も検出できるよう左右斜めを塞ぐ。
    place(&mut world, layer, p(7, 9), "test:wall", [0, 0]);
    place(&mut world, layer, p(9, 9), "test:wall", [0, 0]);
    let scalar = world
        .edit(|edit| edit.transport_materials(layer, 100))
        .unwrap();
    assert_eq!(scalar.exchanges, 0);
    assert!(matches!(
        world.cell(layer, p(8, 8)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material != MaterialId::AIR
    ));
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == MaterialId::AIR
    ));
    world
        .edit(|edit| {
            edit.set_channel(
                key("test:motion"),
                p(8, 8),
                MaterialMotion {
                    velocity_q16: [0, 65536],
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let motion_report = world
        .edit(|edit| edit.transport_material_motion(layer, travel()))
        .unwrap();
    assert_eq!(motion_report.moved, 0);
    assert!(motion_report.collisions >= 1);
    assert_eq!(world.material_particle(particle).unwrap().cell(), p(8, 9));
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == MaterialId::AIR
    ));
}

#[test]
fn integrated_phase_shares_reservations_and_never_moves_a_cell_twice_in_one_tick() {
    let (mut world, layer) = fixture();
    // fixtureの砂は抗力50%なので、初速2cell/tickでDDAに1cell残す。
    place(&mut world, layer, p(8, 8), "test:sand", [0, 2 * 65536]);
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((
            MaterialForceConfig {
                channel: key("test:motion"),
                gravity_q16: [0, 0],
                candidate_limit: 32,
                max_velocity_q16: 1_000_000,
            },
            travel(),
        )),
        scalar_candidate_limit: Some(32),
        particle: None,
    };
    let report = world
        .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
        .unwrap();
    assert_eq!(report.motion.unwrap().motion.moved, 1);
    assert_eq!(report.scalar.unwrap().exchanges, 0);
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(value) if value.core.material == sand
    ));
    assert!(matches!(
        world.cell(layer, p(8, 10)).unwrap(),
        CellQuery::Loaded(value) if value.core.material == MaterialId::AIR
    ));
}

#[test]
fn integrated_particle_motion_respects_cell_motion_and_scalar_reservations() {
    struct NoGravity;
    impl MaterialForceProvider for NoGravity {
        fn key(&self) -> StableKey {
            key("test:phase-no-gravity")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 65536]);
    place(&mut world, layer, p(8, 7), "test:wall", [0, 0]);
    let particle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 7), [0, 65536]))
        .unwrap();
    let mut config = phase(layer);
    config.motion = Some((
        MaterialForceConfig {
            channel: key("test:motion"),
            gravity_q16: [0, 0],
            candidate_limit: 32,
            max_velocity_q16: 1_000_000,
        },
        travel(),
    ));
    config.scalar_candidate_limit = Some(32);
    config.particle = Some((
        MaterialParticleForceConfig {
            gravity_q16: [0, 0],
            candidate_limit: 32,
            max_velocity_q16: 1_000_000,
        },
        64,
    ));

    let report = world
        .edit(|edit| edit.step_material_phase(config, &NoGravity))
        .unwrap();
    assert_eq!(report.motion.unwrap().motion.moved, 1);
    assert_eq!(report.scalar.unwrap().exchanges, 0);
    assert_eq!(report.particle.unwrap().particles.waiting, 1);
    assert_eq!(world.material_particle(particle).unwrap().cell(), p(8, 7));
    assert!(matches!(
        world.cell(layer, p(8, 8)).unwrap(),
        CellQuery::Loaded(value) if value.core.material == MaterialId::AIR
    ));
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(value) if value.core.material == sand
    ));
}

#[test]
fn cell_motion_and_scalar_must_not_move_into_canonical_particle_source() {
    let (mut initial, layer) = fixture();
    place(&mut initial, layer, p(8, 8), "test:sand", [0, 65536]);
    place(&mut initial, layer, p(8, 9), "test:sand", [0, 0]);
    place(&mut initial, layer, p(7, 9), "test:wall", [0, 0]);
    place(&mut initial, layer, p(9, 9), "test:wall", [0, 0]);
    let particle = initial
        .edit(|edit| edit.emit_material_particle(layer, p(8, 9), [0, 0]))
        .unwrap();
    let serialized = initial.checkpoint(4_000_000).unwrap();
    let mut motion_world = World::restore(
        &serialized,
        Arc::clone(initial.content()),
        WorldLimits::default(),
        serialized.len(),
    )
    .unwrap();
    let mut scalar_world = World::restore(
        &serialized,
        Arc::clone(initial.content()),
        WorldLimits::default(),
        serialized.len(),
    )
    .unwrap();
    let report = motion_world
        .edit(|edit| edit.transport_material_motion(layer, travel()))
        .unwrap();
    assert!(report.collisions > 0);
    let scalar = scalar_world
        .edit(|edit| edit.transport_materials(layer, 100))
        .unwrap();
    assert_eq!(scalar.exchanges, 0);
    for world in [&motion_world, &scalar_world] {
        assert!(matches!(
            world.cell(layer, p(8, 9)).unwrap(),
            CellQuery::Loaded(cell) if cell.core.material == MaterialId::AIR,
        ));
        assert_eq!(world.material_particle(particle).unwrap().cell(), p(8, 9));
    }
}

#[test]
fn material_transport_does_not_block_on_a_particle_in_another_layer() {
    let (mut world, layer) = fixture();
    let other = world.content().layer_id(key("test:back")).unwrap();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 65536]);
    place(&mut world, other, p(8, 9), "test:sand", [0, 0]);
    let other_particle = world
        .edit(|edit| edit.emit_material_particle(other, p(8, 9), [0, 0]))
        .unwrap();
    let report = world
        .edit(|edit| edit.transport_material_motion(layer, travel()))
        .unwrap();
    assert_eq!(report.moved, 1);
    assert_eq!(
        world.material_particle(other_particle).unwrap().cell(),
        p(8, 9)
    );
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material != MaterialId::AIR,
    ));
}

#[test]
fn material_phase_and_particle_only_entry_reject_non_simulated_layer_atomically() {
    let (mut world, _) = fixture();
    let visual = world.content().layer_id(key("test:visual-only")).unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let config = MaterialPhaseConfig {
        layer: visual,
        pressure: None,
        motion: None,
        scalar_candidate_limit: None,
        particle: Some((
            MaterialParticleForceConfig {
                gravity_q16: [0, 65536],
                candidate_limit: 8,
                max_velocity_q16: 1_000_000,
            },
            20,
        )),
    };
    assert!(
        world
            .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
            .is_err()
    );
    assert!(
        world
            .edit(|edit| {
                edit.integrate_material_particle_forces_in_layer(
                    visual,
                    config.particle.unwrap().0,
                    &StandardMaterialForces,
                )
            })
            .is_err()
    );
    assert!(
        world
            .edit(|edit| edit.step_material_particles_in_layer(visual, 8, 20))
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

#[test]
fn integrated_pressure_and_author_provider_failure_discard_the_entire_world_candidate() {
    struct RejectSecond;
    impl MaterialForceProvider for RejectSecond {
        fn key(&self) -> StableKey {
            key("test:reject-second")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            if input.point == p(9, 8) {
                return Err(charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::InvalidState,
                    "作者Providerが候補途中で失敗",
                ));
            }
            Ok(MaterialMotion {
                velocity_q16: [65536, 0],
                ..input.motion
            })
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:liquid", [0, 0]);
    place(&mut world, layer, p(9, 8), "test:water", [0, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    let mut config = phase(layer);
    config.pressure = Some(PressureRelaxation {
        channel: key("test:pressure"),
        iterations: 4,
        gravity_q16: 65536,
        cell_limit: 16,
    });
    assert!(
        world
            .edit(|edit| edit.step_material_phase(config, &RejectSecond))
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}
