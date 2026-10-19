//! 粉・液・気の標準scalar輸送と休止最適化のtickごとの正規一致を検査。
use charon_engine::{
    foundation::{
        StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::CellPoint,
        id::WorldId,
    },
    world::*,
};
use std::sync::Arc;

fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn fixture() -> (World, LayerId) {
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
    for (s, base, mass) in [
        ("test:sand", MaterialBase::Powder, 5),
        ("test:liquid", MaterialBase::Liquid, 3),
        ("test:gas", MaterialBase::Gas, 1),
        ("test:solid", MaterialBase::Solid, 100),
    ] {
        registry
            .register(MaterialBuilder::new(s, base).unwrap().mass_per_cell(mass))
            .unwrap();
    }
    let content = registry.freeze().unwrap();
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
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 2, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    (world, layer)
}
fn set(world: &mut World, layer: LayerId, point: CellPoint, s: &str) {
    let material = world.content().material_id(MaterialHandle(key(s))).unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
}
fn restore(world: &World) -> World {
    let bytes = world.checkpoint(8_000_000).unwrap();
    World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap()
}
fn compare(
    reference: &mut World,
    optimized: &mut World,
    cache: &mut ScalarMaterialSleepTracker,
    layer: LayerId,
    sleep: bool,
) {
    let expected = reference
        .edit(|edit| edit.transport_materials(layer, 128))
        .unwrap();
    let actual = optimized
        .step_scalar_materials_sleeping(cache, layer, 128)
        .unwrap();
    assert_eq!(expected, actual.reference);
    assert_eq!(actual.sleeping, sleep);
    assert_eq!(
        reference.checkpoint(8_000_000).unwrap(),
        optimized.checkpoint(8_000_000).unwrap(),
    );
}
fn sealed(world: &mut World, layer: LayerId) {
    for (x, y) in [
        (7, 9),
        (8, 9),
        (9, 9), // sandの落下先
        (20, 9),
        (21, 9),
        (22, 9),
        (20, 8),
        (22, 8), // liquidの左右と下
        (29, 19),
        (30, 19),
        (31, 19),
        (29, 20),
        (31, 20), // gasの左右と上
    ] {
        set(world, layer, p(x, y), "test:solid");
    }
    set(world, layer, p(8, 8), "test:sand");
    set(world, layer, p(21, 8), "test:liquid");
    set(world, layer, p(30, 20), "test:gas");
}

#[test]
fn scalar_sleep_works_for_blocked_powder_liquid_gas_and_matches_every_tick() {
    let (mut plain, layer) = fixture();
    sealed(&mut plain, layer);
    let mut optimized = restore(&plain);
    let mut cache = ScalarMaterialSleepTracker::default();
    for tick in 0..10 {
        compare(&mut plain, &mut optimized, &mut cache, layer, tick != 0);
        assert!(cache.has_proof());
        plain.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }
    // 別chunkを変更した場合でも、古い全域の静止証明は使いません。
    for world in [&mut plain, &mut optimized] {
        set(world, layer, p(70, 30), "test:solid");
    }
    compare(&mut plain, &mut optimized, &mut cache, layer, false);
    assert!(cache.has_proof());
    // 1cellだけを塞ぎから解放すると、液体の横移動が再び起こります。
    for world in [&mut plain, &mut optimized] {
        world
            .edit(|edit| edit.set_cell(layer, p(22, 8), CellCore::AIR))
            .unwrap();
    }
    compare(&mut plain, &mut optimized, &mut cache, layer, false);
    assert!(!cache.has_proof());
}

#[test]
fn scalar_sleep_runs_inside_material_phase_and_rolls_back_with_later_phase_failure() {
    let (mut reference, layer) = fixture();
    sealed(&mut reference, layer);
    let mut optimized = restore(&reference);
    let mut tracker = ScalarMaterialSleepTracker::default();
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(128),
        particle: None,
    };
    for tick in 0..8 {
        let expected = reference
            .edit(|edit| edit.step_material_phase(config, &StandardMaterialForces))
            .unwrap();
        let (actual, updated) = optimized
            .edit(|edit| {
                edit.step_material_phase_with_scalar_sleep(
                    config,
                    &StandardMaterialForces,
                    tracker.clone(),
                )
            })
            .unwrap();
        tracker = updated;
        assert_eq!(expected.scalar, actual.scalar);
        assert_eq!(actual.scalar_sleep.unwrap().sleeping, tick != 0);
        assert_eq!(
            reference.checkpoint(8_000_000).unwrap(),
            optimized.checkpoint(8_000_000).unwrap(),
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }

    set(&mut optimized, layer, p(8, 9), "test:sand");
    optimized
        .edit(|edit| edit.emit_material_particle(layer, p(8, 9), [0, 0]))
        .unwrap();
    let before = optimized.checkpoint(8_000_000).unwrap();
    let error = optimized.edit(|edit| {
        let (report, _) = edit.step_material_phase_with_scalar_sleep(
            MaterialPhaseConfig {
                scalar_candidate_limit: Some(128),
                particle: Some((
                    MaterialParticleForceConfig {
                        gravity_q16: [0, 0],
                        candidate_limit: 0,
                        max_velocity_q16: 65536,
                    },
                    1,
                )),
                ..config
            },
            &StandardMaterialForces,
            tracker.clone(),
        )?;
        let _ = report;
        Ok(())
    });
    assert!(error.is_err());
    assert_eq!(optimized.checkpoint(8_000_000).unwrap(), before);
}

#[test]
fn chunk_sleep_keeps_a_blocked_region_asleep_while_adjacent_chunk_moves() {
    let (mut reference, layer) = fixture();
    sealed(&mut reference, layer);
    set(&mut reference, layer, p(70, 8), "test:sand");
    let mut optimized = restore(&reference);
    let mut tracker = ScalarMaterialSleepTracker::default();

    for tick in 0..5 {
        let expected = reference
            .edit(|edit| edit.transport_materials(layer, 128))
            .unwrap();
        let actual = optimized
            .step_scalar_materials_sleeping(&mut tracker, layer, 128)
            .unwrap();
        assert_eq!(expected, actual.reference);
        assert!(
            !actual.sleeping,
            "隣接chunkは動的なのでWorld全体はsleepしない"
        );
        assert!(actual.sleeping_chunks >= 1, "封じたchunkだけ局所sleepする");
        assert_eq!(
            reference.checkpoint(8_000_000).unwrap(),
            optimized.checkpoint(8_000_000).unwrap(),
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
        if tick == 2 {
            // 隣接chunkの内部移動はchunk pageを変えるが、1cell haloは変えない。
            assert!(!actual.woke);
        }
    }

    for world in [&mut reference, &mut optimized] {
        set(world, layer, p(64, 8), "test:sand");
    }
    let expected = reference
        .edit(|edit| edit.transport_materials(layer, 128))
        .unwrap();
    let actual = optimized
        .step_scalar_materials_sleeping(&mut tracker, layer, 128)
        .unwrap();
    assert_eq!(expected, actual.reference);
    assert!(
        actual.woke,
        "隣接1cell haloへ素材が入ると局所証明をwakeする"
    );
    assert_eq!(
        reference.checkpoint(8_000_000).unwrap(),
        optimized.checkpoint(8_000_000).unwrap(),
    );
}

#[test]
fn dynamic_scalar_is_never_falsely_asleep_and_preserves_candidate_limit_failures() {
    let (mut plain, layer) = fixture();
    set(&mut plain, layer, p(8, 8), "test:sand");
    let mut optimized = restore(&plain);
    let mut cache = ScalarMaterialSleepTracker::default();
    for _ in 0..4 {
        compare(&mut plain, &mut optimized, &mut cache, layer, false);
        assert!(!cache.has_proof());
    }
    let before = optimized.checkpoint(8_000_000).unwrap();
    assert!(
        optimized
            .step_scalar_materials_sleeping(&mut cache, layer, 0)
            .is_err()
    );
    assert_eq!(optimized.checkpoint(8_000_000).unwrap(), before);
}

#[test]
fn blocked_particle_keeps_candidate_cache_reusable_across_unchanged_chunks() {
    let (mut plain, layer) = fixture();
    plain
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(2, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:third-active"),
                region: ChunkRegion::single(ChunkCoord::new(2, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    // 第2chunkを候補走査の省略対象にできる静止solidで満たす。
    for x in 64..68 {
        set(&mut plain, layer, p(x, 8), "test:solid");
    }
    // 粉体を置く予定のcellから正規particleを放出し、scalar輸送先を占有させる。
    set(&mut plain, layer, p(8, 9), "test:sand");
    plain
        .edit(|edit| edit.emit_material_particle(layer, p(8, 9), [0, 0]))
        .unwrap();
    set(&mut plain, layer, p(8, 8), "test:sand");
    set(&mut plain, layer, p(7, 9), "test:solid");
    set(&mut plain, layer, p(9, 9), "test:solid");
    // 別chunkのpowderは毎tick移動してWorld全体のcache世代を変えるが、
    // 静止chunk自身のpageは同じなので、その候補走査は局所的に再利用できる。
    set(&mut plain, layer, p(136, 8), "test:sand");
    let mut optimized = restore(&plain);
    let mut cache = ScalarMaterialSleepTracker::default();
    for _ in 0..4 {
        compare(&mut plain, &mut optimized, &mut cache, layer, false);
        assert!(!cache.has_proof());
        plain.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }
    let before = optimized.checkpoint(8_000_000).unwrap();
    assert!(
        optimized
            .step_scalar_materials_sleeping(&mut cache, layer, 6)
            .is_err()
    );
    assert_eq!(optimized.checkpoint(8_000_000).unwrap(), before);
}

#[test]
fn decoded_scalar_sleep_is_not_trusted_without_reference_world_validation() {
    let (mut plain, layer) = fixture();
    sealed(&mut plain, layer);
    let mut cache = ScalarMaterialSleepTracker::default();
    plain
        .step_scalar_materials_sleeping(&mut cache, layer, 128)
        .unwrap();
    assert!(cache.has_proof());
    let mut output = Encoder::new(1_000_000);
    cache.encode(&mut output).unwrap();
    let bytes = output.finish();
    let mut decoder = Decoder::new(&bytes, bytes.len()).unwrap();
    let mut cache = ScalarMaterialSleepTracker::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    let mut optimized = restore(&plain);
    let expected = plain
        .edit(|edit| edit.transport_materials(layer, 128))
        .unwrap();
    let actual = optimized
        .step_scalar_materials_sleeping(&mut cache, layer, 128)
        .unwrap();
    assert!(!actual.sleeping);
    assert_eq!(actual.reference, expected);
    assert_eq!(
        plain.checkpoint(8_000_000).unwrap(),
        optimized.checkpoint(8_000_000).unwrap()
    );
    assert!(
        optimized
            .step_scalar_materials_sleeping(&mut cache, layer, 1)
            .is_err()
    );
}

#[test]
fn cloned_verified_proof_cannot_sleep_a_different_branch_with_matching_revisions() {
    let (mut source, layer) = fixture();
    set(&mut source, layer, p(8, 8), "test:solid");
    let mut static_branch = restore(&source);
    let mut moving_branch = restore(&source);
    // 同じWorldId・世界revision・chunk revisionを作るが内容は異なる。
    set(&mut static_branch, layer, p(8, 8), "test:solid");
    set(&mut moving_branch, layer, p(8, 8), "test:sand");
    let mut proof = ScalarMaterialSleepTracker::default();
    static_branch
        .step_scalar_materials_sleeping(&mut proof, layer, 128)
        .unwrap();
    assert!(proof.has_proof());
    moving_branch.edit(|_| Ok(())).unwrap();
    assert_eq!(static_branch.revision(), moving_branch.revision());
    let mut stolen = proof.clone();
    let moving = moving_branch
        .step_scalar_materials_sleeping(&mut stolen, layer, 128)
        .unwrap();
    assert!(
        !moving.sleeping,
        "異なる地形のcacheでscalar輸送を省略してはならない"
    );
    assert!(moving.woke);
    let sand = source
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        moving_branch.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
}
