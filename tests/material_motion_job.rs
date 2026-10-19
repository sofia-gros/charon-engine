//! 広域DDAの固定budget・save/restore・公開失効・全payload輸送の回帰試験。
use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::*,
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
use std::sync::Arc;

fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn create() -> (World, LayerId) {
    let (world, layer, _) = create_with_lease();
    (world, layer)
}
fn create_with_lease() -> (World, LayerId, LeaseRef) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("test:terrain"),
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
        .register(MaterialBuilder::new("test:water", MaterialBase::Liquid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:wall", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotionJob>(key("test:job-schema"), 1, 500_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<i64>(key("test:pressure-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<PressureJob>(key("test:pressure-job-schema"), 1, 2_000_000).unwrap())
        .unwrap();
    registry
        .schema(
            Schema::of::<PressurePushSearch>(key("test:push-job-schema"), 1, 2_000_000).unwrap(),
        )
        .unwrap();
    registry
        .material_channel(
            key("test:terrain"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-schema"),
                layout: GridLayout {
                    origin: point(0, 0),
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
    for channel in ["test:pressure", "test:pump"] {
        registry
            .channel(ChannelDefinition {
                key: key(channel),
                version: 1,
                value_schema: key("test:pressure-schema"),
                layout: GridLayout {
                    origin: point(0, 0),
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
    }
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:terrain")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        42,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let lease = world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 2, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    (world, layer, lease)
}
fn place(world: &mut World, layer: LayerId, position: CellPoint, name: &str, speed: [i64; 2]) {
    let material = world
        .content()
        .material_id(MaterialHandle(key(name)))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                position,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                position,
                MaterialMotion {
                    velocity_q16: speed,
                    remainder_q16: [0, 0],
                },
            )
        })
        .unwrap();
}
fn material(world: &World, layer: LayerId, position: CellPoint) -> MaterialId {
    match world.cell(layer, position).unwrap() {
        CellQuery::Loaded(cell) => cell.core.material,
        _ => panic!("このテストではresidentである必要があります"),
    }
}
fn speed(world: &World, position: CellPoint) -> MaterialMotion {
    match world.channel_value(key("test:motion"), position).unwrap() {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("速度channelが必要"),
    }
}
fn job(world: &World, layer: LayerId, start: CellPoint) -> MaterialMotionJob {
    MaterialMotionJob::new(world, layer, start, key("test:motion"), 128, 128, 2).unwrap()
}
fn finish_pressure_job(job: &mut PressureJob, world: &World) {
    for _ in 0..1000 {
        if job.status() == PressureJobStatus::Ready {
            return;
        }
        job.step(world, 128).unwrap();
    }
    panic!("圧力jobが有限仕事量で完了しない");
}
fn pressure_config() -> PressureJobConfig {
    PressureJobConfig {
        relaxation: PressureRelaxation {
            channel: key("test:pressure"),
            iterations: 4,
            gravity_q16: 65536,
            cell_limit: 8192,
        },
        gas: None,
        pump: None,
    }
}
fn pressure_push_fixture() -> (World, LayerId, PressureJob, CellPoint) {
    let (mut world, layer) = create();
    let source = point(10, 10);
    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    world
        .edit(|edit| {
            for position in [
                point(10, 9),
                point(9, 10),
                point(10, 11),
                point(11, 9),
                point(11, 11),
            ] {
                edit.set_cell(
                    layer,
                    position,
                    CellCore {
                        material: wall,
                        ..CellCore::AIR
                    },
                )?;
            }
            edit.set_cell(
                layer,
                source,
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            edit.set_cell(
                layer,
                point(11, 10),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:pump"), source, 900i64)
        })
        .unwrap();
    place(
        &mut world,
        layer,
        point(20, 20),
        "test:sand",
        [2 * 65536, 0],
    );
    let pressure = PressureJob::new(
        &world,
        layer,
        PressureJobConfig {
            relaxation: PressureRelaxation {
                channel: key("test:pressure"),
                iterations: 1,
                gravity_q16: 0,
                cell_limit: 16,
            },
            gas: None,
            pump: Some(key("test:pump")),
        },
    )
    .unwrap();
    let mut pressure = pressure;
    finish_pressure_job(&mut pressure, &world);
    (world, layer, pressure, source)
}
fn save(world: &World) -> Vec<u8> {
    world.checkpoint(8_000_000).unwrap()
}
fn restore(world: &World) -> World {
    let data = save(world);
    World::restore(
        &data,
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap()
}
fn roundtrip(input: &MaterialMotionJob) -> MaterialMotionJob {
    let mut out = Encoder::new(500_000);
    input.encode(&mut out).unwrap();
    let data = out.finish();
    let mut decoder = Decoder::new(&data, data.len()).unwrap();
    let job = MaterialMotionJob::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    job
}

#[derive(Default)]
struct TestArchives(std::collections::BTreeMap<[u8; 32], Vec<u8>>);
impl ArchiveStorage for TestArchives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        assert_eq!(blake3::hash(bytes).as_bytes(), &hash);
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndAtomicPublish)
    }
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        let bytes = self.0.get(&hash).expect("archiveが保存済み");
        if bytes.len() > limit {
            return Err(charon_engine::foundation::EngineError::capacity(
                "テストarchive読込",
                bytes.len() as u64,
                limit as u64,
            ));
        }
        Ok(bytes.clone())
    }
}

#[test]
fn independent_wide_jobs_publish_together_and_conflicts_roll_back_all() {
    let (mut world, layer) = create();
    let first = point(10, 1);
    let second = point(20, 2);
    place(&mut world, layer, first, "test:sand", [2 * 65536, 0]);
    place(&mut world, layer, second, "test:sand", [-2 * 65536, 0]);
    let mut first_job = job(&world, layer, first);
    let mut second_job = job(&world, layer, second);
    while first_job.status() == MaterialMotionJobStatus::Computing {
        first_job.step(&world, 8).unwrap();
    }
    while second_job.status() == MaterialMotionJobStatus::Computing {
        second_job.step(&world, 8).unwrap();
    }
    let report = world
        .edit(|edit| edit.publish_material_motion_jobs(&[first_job, second_job]))
        .unwrap();
    assert_eq!(report.jobs, 2);
    assert_eq!(report.moved_jobs, 2);
    assert_eq!(report.crossed_cells, 4);
    assert_eq!(
        material(&world, layer, point(12, 1)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(
        material(&world, layer, point(18, 2)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );

    let (mut world, layer) = create();
    let left = point(10, 3);
    let right = point(14, 3);
    place(&mut world, layer, left, "test:sand", [2 * 65536, 0]);
    place(&mut world, layer, right, "test:sand", [-2 * 65536, 0]);
    let mut left_job = job(&world, layer, left);
    let mut right_job = job(&world, layer, right);
    while left_job.status() == MaterialMotionJobStatus::Computing {
        left_job.step(&world, 8).unwrap();
    }
    while right_job.status() == MaterialMotionJobStatus::Computing {
        right_job.step(&world, 8).unwrap();
    }
    let before = save(&world);
    assert!(
        world
            .edit(|edit| edit.publish_material_motion_jobs(&[left_job, right_job]))
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn pressure_push_and_motion_jobs_share_atomic_conflict_reservations() {
    let (mut world, layer) = create();
    let sand = point(10, 1);
    let water = point(20, 5);
    place(&mut world, layer, sand, "test:sand", [2 * 65536, 0]);
    place(&mut world, layer, water, "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), water, 1i64))
        .unwrap();
    let mut motion = job(&world, layer, sand);
    while motion.status() == MaterialMotionJobStatus::Computing {
        motion.step(&world, 8).unwrap();
    }
    let mut push = PressurePushSearch::new(
        &world,
        layer,
        water,
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes: 16,
        },
    )
    .unwrap();
    assert_eq!(push.step(&world, 16).unwrap(), PressurePushStatus::Ready);
    let report = world
        .edit(|edit| edit.publish_material_motion_and_pressure_pushes(&[motion], &[push]))
        .unwrap();
    assert_eq!(report.jobs, 1);
    assert_eq!(report.pressure_pushes, 1);
    assert_eq!(report.moved_jobs, 2);
    assert_eq!(
        material(&world, layer, point(12, 1)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(
        material(&world, layer, point(20, 4)),
        world
            .content()
            .material_id(MaterialHandle(key("test:water")))
            .unwrap()
    );

    let (mut world, layer) = create();
    let sand = point(10, 1);
    let water = point(10, 3);
    place(&mut world, layer, sand, "test:sand", [0, 65536]);
    place(&mut world, layer, water, "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), water, 1i64))
        .unwrap();
    let mut motion = job(&world, layer, sand);
    while motion.status() == MaterialMotionJobStatus::Computing {
        motion.step(&world, 8).unwrap();
    }
    let mut push = PressurePushSearch::new(
        &world,
        layer,
        water,
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes: 16,
        },
    )
    .unwrap();
    assert_eq!(push.step(&world, 16).unwrap(), PressurePushStatus::Ready);
    let before = save(&world);
    assert!(
        world
            .edit(|edit| edit.publish_material_motion_and_pressure_pushes(&[motion], &[push]))
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn pressure_and_material_jobs_publish_in_one_atomic_candidate() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(10, 5), "test:sand", [0, 65536]);
    place(&mut world, layer, point(20, 20), "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), point(20, 20), 100i64))
        .unwrap();

    let mut motion = job(&world, layer, point(10, 5));
    let mut pressure = PressureJob::new(&world, layer, pressure_config()).unwrap();
    for _ in 0..1000 {
        if motion.status() == MaterialMotionJobStatus::Ready {
            break;
        }
        motion.step(&world, 128).unwrap();
    }
    assert_eq!(motion.status(), MaterialMotionJobStatus::Ready);
    finish_pressure_job(&mut pressure, &world);

    let (motion_report, pressure_reports) = world
        .edit(|edit| {
            edit.publish_material_motion_pressure_jobs_and_pushes(&[motion], &[], &[pressure])
        })
        .unwrap();
    assert_eq!(motion_report.moved_jobs, 1);
    assert_eq!(pressure_reports.len(), 1);
    assert_eq!(
        material(&world, layer, point(10, 6)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert!(matches!(
        world
            .channel_value::<i64>(key("test:pressure"), point(20, 20))
            .unwrap(),
        ChannelQuery::Loaded(0)
    ));
}

#[test]
fn integrated_material_phase_keeps_wide_job_path_reserved_for_scalar_transport() {
    struct AccelerateDown;
    impl MaterialForceProvider for AccelerateDown {
        fn key(&self) -> StableKey {
            key("test:wide-job-acceleration")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(MaterialMotion {
                velocity_q16: [
                    input.motion.velocity_q16[0],
                    input.motion.velocity_q16[1] + 65536,
                ],
                remainder_q16: input.motion.remainder_q16,
            })
        }
    }

    let (mut world, layer) = create();
    let source = point(10, 5);
    place(&mut world, layer, source, "test:sand", [2 * 65536, 0]);
    let mut moving = job(&world, layer, source);
    while moving.status() == MaterialMotionJobStatus::Computing {
        moving.step(&world, 8).unwrap();
    }
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((
            MaterialForceConfig {
                channel: key("test:motion"),
                gravity_q16: [0, 65536],
                candidate_limit: 256,
                max_velocity_q16: 8 * 65536,
            },
            MaterialMotionConfig {
                channel: key("test:motion"),
                candidate_limit: 256,
                max_distance: 8,
                step_limit: 1024,
            },
        )),
        scalar_candidate_limit: Some(256),
        particle: None,
    };

    let (jobs, phase) = world
        .edit(|edit| edit.step_material_phase_with_jobs(config, &AccelerateDown, &[moving], &[]))
        .unwrap();

    assert_eq!(jobs.moved_jobs, 1);
    assert_eq!(phase.scalar.unwrap().exchanges, 0);
    assert_eq!(speed(&world, point(12, 5)).velocity_q16, [2 * 65536, 0]);
    assert_eq!(
        material(&world, layer, point(12, 5)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(material(&world, layer, point(12, 6)), MaterialId::AIR);
}

#[test]
fn integrated_material_phase_failure_rolls_back_wide_job_publication() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:wide-job-no-change")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer) = create();
    let source = point(10, 5);
    place(&mut world, layer, source, "test:sand", [2 * 65536, 0]);
    let mut moving = job(&world, layer, source);
    while moving.status() == MaterialMotionJobStatus::Computing {
        moving.step(&world, 8).unwrap();
    }
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(0),
        particle: None,
    };
    let before = save(&world);

    assert!(
        world
            .edit(|edit| edit.step_material_phase_with_jobs(config, &NoChange, &[moving], &[]))
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn integrated_material_phase_keeps_pressure_push_path_reserved_for_scalar_transport() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:pressure-push-no-change")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer) = create();
    let source = point(10, 5);
    place(&mut world, layer, source, "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), source, 1i64))
        .unwrap();
    let mut push = PressurePushSearch::new(
        &world,
        layer,
        source,
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes: 16,
        },
    )
    .unwrap();
    assert_eq!(push.step(&world, 16).unwrap(), PressurePushStatus::Ready);
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(256),
        particle: None,
    };

    let (jobs, phase) = world
        .edit(|edit| edit.step_material_phase_with_jobs(config, &NoChange, &[], &[push]))
        .unwrap();

    assert_eq!(jobs.pressure_pushes, 1);
    assert_eq!(phase.scalar.unwrap().exchanges, 0);
    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    assert_eq!(material(&world, layer, point(10, 4)), water);
    assert_eq!(material(&world, layer, point(10, 6)), MaterialId::AIR);
}

#[test]
fn integrated_material_phase_publishes_pressure_and_motion_jobs_before_standard_solver() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:pressure-phase-no-change")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer) = create();
    let source = point(10, 5);
    let pressure_source = point(20, 20);
    place(&mut world, layer, source, "test:sand", [65536, 0]);
    place(&mut world, layer, pressure_source, "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), pressure_source, 100i64))
        .unwrap();
    let mut moving = job(&world, layer, source);
    while moving.status() == MaterialMotionJobStatus::Computing {
        moving.step(&world, 8).unwrap();
    }
    let mut pressure = PressureJob::new(&world, layer, pressure_config()).unwrap();
    finish_pressure_job(&mut pressure, &world);
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(256),
        particle: None,
    };

    let (jobs, pressure_reports, phase) = world
        .edit(|edit| {
            edit.step_material_phase_with_pressure_jobs(
                config,
                &NoChange,
                &[moving],
                &[],
                &[pressure],
            )
        })
        .unwrap();

    assert_eq!(jobs.moved_jobs, 1);
    assert_eq!(pressure_reports.len(), 1);
    assert_eq!(phase.scalar.unwrap().exchanges, 1);
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(material(&world, layer, point(11, 5)), sand);
    assert_eq!(material(&world, layer, point(11, 6)), MaterialId::AIR);
    assert!(matches!(
        world
            .channel_value::<i64>(key("test:pressure"), pressure_source)
            .unwrap(),
        ChannelQuery::Loaded(0)
    ));
}

#[test]
fn integrated_material_phase_late_failure_rolls_back_pressure_and_motion_jobs() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:pressure-phase-rollback")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer) = create();
    let source = point(10, 5);
    let pressure_source = point(20, 20);
    place(&mut world, layer, source, "test:sand", [65536, 0]);
    place(&mut world, layer, pressure_source, "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), pressure_source, 100i64))
        .unwrap();
    let mut moving = job(&world, layer, source);
    while moving.status() == MaterialMotionJobStatus::Computing {
        moving.step(&world, 8).unwrap();
    }
    let mut pressure = PressureJob::new(&world, layer, pressure_config()).unwrap();
    finish_pressure_job(&mut pressure, &world);
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(0),
        particle: None,
    };
    let before = save(&world);

    assert!(
        world
            .edit(|edit| {
                edit.step_material_phase_with_pressure_jobs(
                    config,
                    &NoChange,
                    &[moving],
                    &[],
                    &[pressure],
                )
            })
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn conflicting_pressure_output_rolls_back_material_job_in_combined_batch() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(10, 5), "test:sand", [0, 65536]);
    place(&mut world, layer, point(20, 20), "test:water", [0, 0]);
    world
        .edit(|edit| edit.set_channel(key("test:pressure"), point(20, 20), 100i64))
        .unwrap();

    let motion = job(&world, layer, point(10, 5));
    let mut first_pressure = PressureJob::new(&world, layer, pressure_config()).unwrap();
    let mut second_pressure = PressureJob::new(&world, layer, pressure_config()).unwrap();
    let mut moving = motion;
    for _ in 0..1000 {
        if moving.status() == MaterialMotionJobStatus::Ready {
            break;
        }
        moving.step(&world, 128).unwrap();
    }
    assert_eq!(moving.status(), MaterialMotionJobStatus::Ready);
    finish_pressure_job(&mut first_pressure, &world);
    finish_pressure_job(&mut second_pressure, &world);

    let before = save(&world);
    assert!(
        world
            .edit(
                |edit| edit.publish_material_motion_pressure_jobs_and_pushes(
                    &[moving],
                    &[],
                    &[first_pressure, second_pressure],
                )
            )
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn wide_job_budget_one_and_save_restore_match_reference_transport() {
    let (mut world, layer) = create();
    let start = point(7, 1);
    let target = point(77, 1);
    place(&mut world, layer, start, "test:sand", [70 * 65536, 0]);
    let mut reference = restore(&world);
    let mut running = job(&world, layer, start);
    assert_eq!(running.status(), MaterialMotionJobStatus::Computing);
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert_eq!(running.report().crossed_cells, 1);
    assert_eq!(
        material(&world, layer, start),
        material(&reference, layer, start)
    );
    for _ in 1..31 {
        assert_eq!(
            running.step(&world, 1).unwrap(),
            MaterialMotionJobStatus::Computing
        );
    }
    // 処理途中も同じWorldのStateStoreへcursorを保存でき、World復元後に続けられる。
    world
        .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &running))
        .unwrap();
    let mut world = restore(&world);
    running = world.state::<MaterialMotionJob>(key("test:job")).unwrap();
    assert_eq!(running.report().crossed_cells, 31);
    running = roundtrip(&running);
    while running.status() == MaterialMotionJobStatus::Computing {
        running.step(&world, 1).unwrap();
    }
    assert_eq!(running.report().checked_steps, 70);
    assert_eq!(running.report().crossed_cells, 70);
    assert_eq!(
        material(&world, layer, start),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    world.edit(|edit| running.publish(edit)).unwrap();
    reference
        .edit(|edit| {
            edit.transport_material_motion(
                layer,
                MaterialMotionConfig {
                    channel: key("test:motion"),
                    candidate_limit: 3,
                    max_distance: 128,
                    step_limit: 200,
                },
            )
        })
        .unwrap();
    assert_eq!(material(&world, layer, start), MaterialId::AIR);
    assert_eq!(
        material(&world, layer, target),
        material(&reference, layer, target)
    );
    assert_eq!(speed(&world, target), speed(&reference, target));
    assert_eq!(running.report().crossed_cells, 70);
}

#[test]
fn archived_region_invalidates_pending_wide_job_after_world_restore() {
    let (mut world, layer, lease) = create_with_lease();
    let start = point(7, 1);
    place(&mut world, layer, start, "test:sand", [70 * 65536, 0]);
    let mut pending = job(&world, layer, start);
    assert_eq!(
        pending.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    world
        .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &pending))
        .unwrap();

    world.edit(|edit| edit.release_lease(lease)).unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let mut archives = TestArchives::default();
    let receipt = world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(receipt)).unwrap();

    let mut restored = restore(&world);
    pending = restored.state(key("test:job")).unwrap();
    let archived_checkpoint = save(&restored);
    assert!(pending.step(&restored, 1).is_err());
    assert!(
        restored
            .edit(|edit| edit.publish_material_motion_jobs(std::slice::from_ref(&pending)))
            .is_err()
    );
    assert_eq!(save(&restored), archived_checkpoint);
    assert!(matches!(
        restored.cell(layer, start).unwrap(),
        CellQuery::Unavailable(_)
    ));
}

#[test]
fn archived_region_invalidates_pending_pressure_job_and_push_search_after_restore() {
    let (mut pressure_world, layer, pressure_lease) = create_with_lease();
    place(
        &mut pressure_world,
        layer,
        point(10, 10),
        "test:water",
        [0, 0],
    );
    let mut pressure_job = PressureJob::new(&pressure_world, layer, pressure_config()).unwrap();
    assert_eq!(
        pressure_job.step(&pressure_world, 1).unwrap(),
        PressureJobStatus::Computing
    );
    pressure_world
        .edit(|edit| {
            edit.set_state(
                key("test:pressure-job"),
                key("test:pressure-job-schema"),
                &pressure_job,
            )
        })
        .unwrap();
    pressure_world
        .edit(|edit| edit.release_lease(pressure_lease))
        .unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let mut archives = TestArchives::default();
    let receipt = pressure_world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    pressure_world
        .edit(|edit| edit.finish_eviction(receipt))
        .unwrap();
    let pressure_world = restore(&pressure_world);
    pressure_job = pressure_world.state(key("test:pressure-job")).unwrap();
    let pressure_checkpoint = save(&pressure_world);
    assert!(pressure_job.step(&pressure_world, 1).is_err());
    assert_eq!(save(&pressure_world), pressure_checkpoint);

    let (mut push_world, layer, push_lease) = create_with_lease();
    let source = point(10, 10);
    let water = push_world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    let wall = push_world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    push_world
        .edit(|edit| {
            for position in [point(10, 9), point(9, 10), point(10, 11)] {
                edit.set_cell(
                    layer,
                    position,
                    CellCore {
                        material: wall,
                        ..CellCore::AIR
                    },
                )?;
            }
            edit.set_cell(
                layer,
                source,
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            edit.set_cell(
                layer,
                point(11, 10),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:pressure"), source, 900i64)
        })
        .unwrap();
    let mut push_search = PressurePushSearch::new(
        &push_world,
        layer,
        source,
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes: 16,
        },
    )
    .unwrap();
    assert_eq!(
        push_search.step(&push_world, 1).unwrap(),
        PressurePushStatus::Searching
    );
    push_world
        .edit(|edit| {
            edit.set_state(
                key("test:push-job"),
                key("test:push-job-schema"),
                &push_search,
            )
        })
        .unwrap();
    push_world
        .edit(|edit| edit.release_lease(push_lease))
        .unwrap();
    let mut archives = TestArchives::default();
    let receipt = push_world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    push_world
        .edit(|edit| edit.finish_eviction(receipt))
        .unwrap();
    let push_world = restore(&push_world);
    push_search = push_world.state(key("test:push-job")).unwrap();
    let push_checkpoint = save(&push_world);
    assert!(push_search.step(&push_world, 1).is_err());
    assert_eq!(save(&push_world), push_checkpoint);
}

#[test]
fn wide_job_collision_returns_partial_path_and_never_jumps_wall() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(8, 8), "test:sand", [70 * 65536, 0]);
    place(&mut world, layer, point(12, 8), "test:wall", [0, 0]);
    let running = job(&world, layer, point(8, 8));
    let mut running = roundtrip(&running);
    assert_eq!(
        running.step(&world, 3).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 3);
    world.edit(|edit| running.publish(edit)).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    assert_eq!(material(&world, layer, point(11, 8)), sand);
    assert_eq!(material(&world, layer, point(12, 8)), wall);
    assert_eq!(speed(&world, point(11, 8)), MaterialMotion::default());
}

#[test]
fn wide_job_treats_same_layer_particle_as_an_occupied_cell() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(8, 8), "test:sand", [5 * 65536, 0]);
    place(&mut world, layer, point(10, 8), "test:sand", [0, 0]);
    let particle = world
        .edit(|edit| edit.emit_material_particle(layer, point(10, 8), [0, 0]))
        .unwrap();
    let mut running = job(&world, layer, point(8, 8));

    assert_eq!(
        running.step(&world, 8).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 1);
    world.edit(|edit| running.publish(edit)).unwrap();

    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(material(&world, layer, point(9, 8)), sand);
    assert_eq!(material(&world, layer, point(10, 8)), MaterialId::AIR);
    assert_eq!(
        world.material_particle(particle).unwrap().cell(),
        point(10, 8)
    );
}

#[test]
fn wide_job_diagonal_guard_treats_same_layer_particle_as_occupied() {
    let (mut world, layer) = create();
    place(
        &mut world,
        layer,
        point(8, 8),
        "test:sand",
        [2 * 65536, 2 * 65536],
    );
    place(&mut world, layer, point(9, 8), "test:sand", [0, 0]);
    let particle = world
        .edit(|edit| edit.emit_material_particle(layer, point(9, 8), [0, 0]))
        .unwrap();
    let mut running = job(&world, layer, point(8, 8));

    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 0);
    world.edit(|edit| running.publish(edit)).unwrap();

    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(material(&world, layer, point(8, 8)), sand);
    assert_eq!(material(&world, layer, point(9, 8)), MaterialId::AIR);
    assert_eq!(
        world.material_particle(particle).unwrap().cell(),
        point(9, 8)
    );
}

#[test]
fn stale_input_unfinished_publish_and_capacity_fail_without_mutating_world() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(8, 8), "test:sand", [5 * 65536, 0]);
    let mut running = job(&world, layer, point(8, 8));
    let baseline = save(&world);
    assert!(world.edit(|edit| running.publish(edit)).is_err());
    assert_eq!(save(&world), baseline);
    let mut too_small =
        MaterialMotionJob::new(&world, layer, point(8, 8), key("test:motion"), 64, 2, 2).unwrap();
    assert_eq!(
        too_small.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert!(too_small.step(&world, 1).is_err());
    assert_eq!(save(&world), baseline);
    running.step(&world, 2).unwrap();
    place(&mut world, layer, point(16, 8), "test:wall", [0, 0]);
    let changed = save(&world);
    assert!(running.step(&world, 1).is_err());
    assert!(world.edit(|edit| running.publish(edit)).is_err());
    assert_eq!(save(&world), changed);
}

#[test]
fn diagonal_job_checks_both_corner_cells_and_cannot_slip_through_a_wall() {
    let (mut world, layer) = create();
    place(
        &mut world,
        layer,
        point(8, 8),
        "test:sand",
        [2 * 65536, 2 * 65536],
    );
    place(&mut world, layer, point(9, 8), "test:wall", [0, 0]);
    let before = save(&world);
    let mut running = job(&world, layer, point(8, 8));
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 0);
    assert_eq!(save(&world), before);
    world.edit(|edit| running.publish(edit)).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(material(&world, layer, point(8, 8)), sand);
    assert_eq!(speed(&world, point(8, 8)), MaterialMotion::default());
}

#[test]
fn restored_job_rejects_forged_path_that_disagrees_with_dda() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(7, 1), "test:sand", [3 * 65536, 0]);
    let mut running = job(&world, layer, point(7, 1));
    running.step(&world, 2).unwrap();
    let mut encoder = Encoder::new(500_000);
    running.encode(&mut encoder).unwrap();
    let mut bytes = encoder.finish();
    // 進捗cursor自体は正しいまま、経路のx座標だけを変更したbyte列を作る。
    // 内部のDDA経路検査は、単なる「隣接した経路」以上を検証する。
    let target = [8u8, 0, 0, 0, 1, 0, 0, 0];
    let candidate = bytes
        .windows(target.len())
        .position(|window| window == target)
        .expect("最初のDDA経路の保存点");
    bytes[candidate] = 9;
    let mut input = Decoder::new(&bytes, bytes.len()).unwrap();
    assert!(MaterialMotionJob::decode(&mut input).is_err());
}

#[test]
fn pressure_job_search_resumes_and_shares_push_path_with_material_phase() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:pressure-search-phase-no-change")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer, pressure, source) = pressure_push_fixture();
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(256),
        particle: None,
    };
    let push_config = PressurePushConfig {
        channel: key("test:pressure"),
        gravity_q16: 0,
        threshold_q16: 0,
        nodes: 16,
    };
    let report = world
        .edit(|edit| {
            let report = edit.step_material_phase_with_pressure_job_search(
                config,
                &NoChange,
                &[pressure],
                MaterialPhasePressureSearchInput {
                    layer,
                    source,
                    config: push_config,
                    budget: 1,
                },
            )?;
            assert_eq!(report.push_status, PressurePushStatus::Searching);
            assert!(report.phase.is_none());
            edit.set_state(
                key("test:push-job"),
                key("test:push-job-schema"),
                report.pending_search.as_ref().unwrap(),
            )?;
            Ok(report)
        })
        .unwrap();
    assert_eq!(report.pressure.len(), 1);
    assert!(matches!(
        world
            .channel_value::<i64>(key("test:pressure"), source)
            .unwrap(),
        ChannelQuery::Loaded(900)
    ));

    // World checkpointとStateStoreから探索を復元し、budget=1でterminalまで進める。
    let mut world = restore(&world);
    let mut completed = None;
    for _ in 0..16 {
        // PressureJob公開後のchunk revisionを基準にJobを作り、探索中の地形は変更しない。
        let mut moving = job(&world, layer, point(20, 20));
        while moving.status() == MaterialMotionJobStatus::Computing {
            moving.step(&world, 8).unwrap();
        }
        let search: PressurePushSearch = world.state(key("test:push-job")).unwrap();
        let report = world
            .edit(|edit| {
                let report = edit.resume_material_phase_with_pressure_search(
                    config,
                    &NoChange,
                    std::slice::from_ref(&moving),
                    search,
                    1,
                )?;
                if let Some(pending) = report.pending_search.as_ref() {
                    edit.set_state(key("test:push-job"), key("test:push-job-schema"), pending)?;
                } else {
                    edit.remove_state(key("test:push-job"))?;
                }
                Ok(report)
            })
            .unwrap();
        if report.push_status != PressurePushStatus::Searching {
            completed = Some(report);
            break;
        }
        assert!(report.phase.is_none());
    }
    let report = completed.expect("押出し探索がbudget内で完了する");
    assert_eq!(report.push_status, PressurePushStatus::Ready);
    assert_eq!(report.jobs.pressure_pushes, 1);
    assert_eq!(report.jobs.moved_jobs, 2);
    assert_eq!(report.phase.unwrap().scalar.unwrap().exchanges, 0);
    assert!(
        world
            .state::<PressurePushSearch>(key("test:push-job"))
            .is_err()
    );

    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    assert_eq!(material(&world, layer, source), MaterialId::AIR);
    assert_eq!(material(&world, layer, point(11, 10)), water);
    assert_eq!(material(&world, layer, point(12, 10)), water);
    assert_eq!(
        material(&world, layer, point(22, 20)),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(speed(&world, point(11, 10)), MaterialMotion::default());
    assert_eq!(speed(&world, point(12, 10)), MaterialMotion::default());
}

#[test]
fn pressure_job_search_late_phase_failure_rolls_back_pressure_and_state() {
    struct NoChange;
    impl MaterialForceProvider for NoChange {
        fn key(&self) -> StableKey {
            key("test:pressure-search-phase-rollback")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }

    let (mut world, layer, pressure, source) = pressure_push_fixture();
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: None,
        scalar_candidate_limit: Some(0),
        particle: None,
    };
    let before = save(&world);
    assert!(
        world
            .edit(|edit| edit.step_material_phase_with_pressure_job_search(
                config,
                &NoChange,
                &[pressure],
                MaterialPhasePressureSearchInput {
                    layer,
                    source,
                    config: PressurePushConfig {
                        channel: key("test:pressure"),
                        gravity_q16: 0,
                        threshold_q16: 0,
                        nodes: 16,
                    },
                    budget: 16,
                },
            ))
            .is_err()
    );
    assert_eq!(save(&world), before);
}
