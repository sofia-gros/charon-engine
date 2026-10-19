//! 広域圧力の参照一致、分割・復元、気体sourceと公開取引の回帰試験。
use charon_engine::{
    foundation::{
        StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
use std::sync::Arc;
fn key(name: &str) -> StableKey {
    StableKey::parse(name).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn content(pressure_chunks: u32) -> Arc<Content> {
    content_with_properties(pressure_chunks, GasPressureProperties::default())
}
fn content_with_properties(
    pressure_chunks: u32,
    properties: GasPressureProperties,
) -> Arc<Content> {
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
    for (name, base, mass) in [
        ("test:stone", MaterialBase::Solid, 10),
        ("test:water", MaterialBase::Liquid, 5),
        ("test:gas", MaterialBase::Gas, 2),
    ] {
        let builder = MaterialBuilder::new(name, base)
            .unwrap()
            .mass_per_cell(mass);
        registry
            .register(if base == MaterialBase::Gas {
                builder.gas_pressure(properties)
            } else {
                builder
            })
            .unwrap();
    }
    registry
        .register(
            MaterialBuilder::new("test:other-gas", MaterialBase::Gas)
                .unwrap()
                .mass_per_cell(2)
                .gas_pressure(GasPressureProperties {
                    relaxation_weight: 3,
                    ..GasPressureProperties::default()
                }),
        )
        .unwrap();

    registry
        .schema(Schema::of::<i64>(key("test:i64"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<PressureJob>(key("test:job-schema"), 1, 2_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<PressurePushSearch>(key("test:push-schema"), 1, 2_000_000).unwrap())
        .unwrap();
    for (name, origin, chunks) in [
        ("test:pressure", p(-3, 2), pressure_chunks),
        ("test:pressure-two", p(-5, 4), pressure_chunks),
        ("test:temperature", p(0, 0), 8),
        ("test:pump", p(1, -1), 8),
    ] {
        registry
            .channel(ChannelDefinition {
                key: key(name),
                version: 1,
                value_schema: key("test:i64"),
                layout: GridLayout {
                    origin,
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0i64,
                limits: GridLimits {
                    chunks,
                    checkpoint_bytes: 2_000_000,
                },
            })
            .unwrap();
    }
    registry.freeze().unwrap()
}
fn cell(world: &World, name: &str) -> CellCore {
    CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key(name)))
            .unwrap(),
        ..CellCore::AIR
    }
}
fn material_at(world: &World, layer: LayerId, point: CellPoint) -> MaterialId {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(cell) => cell.core.material,
        _ => panic!("このテストではresidentである必要があります"),
    }
}
fn fixture(pressure_chunks: u32) -> World {
    fixture_with_properties(pressure_chunks, GasPressureProperties::default())
}
fn fixture_with_properties(pressure_chunks: u32, properties: GasPressureProperties) -> World {
    fixture_with_job_byte_limit(pressure_chunks, properties, 64 * 1024 * 1024)
}
fn fixture_with_job_byte_limit(
    pressure_chunks: u32,
    properties: GasPressureProperties,
    job_bytes: usize,
) -> World {
    let limits = WorldLimits {
        job_bytes,
        ..WorldLimits::default()
    };
    let mut world = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        9,
        WorldBounds::expandable(),
        content_with_properties(pressure_chunks, properties),
        limits,
    )
    .unwrap();
    let layer = layer(&world);
    let stone = cell(&world, "test:stone");
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for y in 0..64 {
                for x in 0..64 {
                    edit.set_cell(layer, p(x, y), stone)?;
                }
            }
            edit.acquire_lease(RegionLease {
                source: key("test:simulation"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    world
}
fn layer(world: &World) -> LayerId {
    world.content().layer_id(key("test:terrain")).unwrap()
}
fn config() -> PressureJobConfig {
    PressureJobConfig {
        relaxation: PressureRelaxation {
            channel: key("test:pressure"),
            iterations: 4,
            gravity_q16: 65536,
            cell_limit: 100,
        },
        gas: None,
        pump: None,
    }
}
fn config_with_channel(channel: &str) -> PressureJobConfig {
    PressureJobConfig {
        relaxation: PressureRelaxation {
            channel: key(channel),
            ..config().relaxation
        },
        ..config()
    }
}
fn gas_config() -> PressureJobConfig {
    PressureJobConfig {
        gas: Some(GasPressureSource {
            temperature: key("test:temperature"),
        }),
        ..config()
    }
}
fn finish(job: &mut PressureJob, world: &World, budget: usize) {
    for _ in 0..100_000 {
        if job.status() == PressureJobStatus::Ready {
            return;
        }
        job.step(world, budget).unwrap();
    }
    panic!("圧力jobが有限仕事量で完了しない");
}
fn pressure(world: &World, point: CellPoint) -> i64 {
    match world
        .channel_value::<i64>(key("test:pressure"), p(point.x + 3, point.y - 2))
        .unwrap()
    {
        ChannelQuery::Loaded(v) => v,
        _ => panic!("圧力coverage"),
    }
}
fn bytes(job: &PressureJob) -> Vec<u8> {
    let mut out = Encoder::new(2_000_000);
    job.encode(&mut out).unwrap();
    out.finish()
}
fn decoded(bytes: &[u8]) -> Result<PressureJob, charon_engine::foundation::EngineError> {
    let mut input = Decoder::new(bytes, 2_000_000)?;
    let result = PressureJob::decode(&mut input)?;
    input.finish()?;
    Ok(result)
}
#[test]
fn legacy_profile_matches_scalar_for_every_budget_and_preserves_terrain() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            for (point, core, value) in [
                (p(10, 10), water, 0),
                (p(10, 11), water, 90),
                (p(10, 12), water, 30),
                (p(11, 12), gas, 20),
                (p(20, 20), water, 200),
                (p(20, 21), gas, 100),
            ] {
                edit.set_cell(layer, point, core)?;
                edit.set_channel::<i64>(key("test:pressure"), p(point.x + 3, point.y - 2), value)?;
            }
            edit.set_cell(layer, p(10, 9), CellCore::AIR)
        })
        .unwrap();
    let content = Arc::clone(world.content());
    let original = world.checkpoint(4_000_000).unwrap();
    let mut scalar = World::restore(
        &original,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let expected = scalar
        .edit(|edit| edit.relax_pressure(layer, config().relaxation))
        .unwrap();
    for budget in [1, 3, 65536] {
        let mut restored = World::restore(
            &original,
            Arc::clone(&content),
            WorldLimits::default(),
            4_000_000,
        )
        .unwrap();
        let mut job = PressureJob::new(&restored, layer, config()).unwrap();
        let initial = bytes(&job);
        job.step(&restored, 0).unwrap();
        assert_eq!(initial, bytes(&job));
        finish(&mut job, &restored, budget);
        assert_eq!(job.report(), expected);
        let report = restored
            .edit(|edit| edit.publish_pressure_job(&job))
            .unwrap();
        assert_eq!(report, expected);
        let expected_checkpoint = scalar.checkpoint(4_000_000).unwrap();
        let actual_checkpoint = restored.checkpoint(4_000_000).unwrap();
        assert_eq!(
            expected_checkpoint.len(),
            actual_checkpoint.len(),
            "budget {budget}: checkpoint長"
        );
        if let Some(index) = expected_checkpoint
            .iter()
            .zip(&actual_checkpoint)
            .position(|(expected, actual)| expected != actual)
        {
            let pressure_points = [
                p(13, 8),
                p(13, 9),
                p(13, 10),
                p(14, 10),
                p(23, 18),
                p(23, 19),
            ];
            let pressure_values: Vec<_> = pressure_points
                .into_iter()
                .map(|point| {
                    let scalar_value = scalar
                        .channel_value::<i64>(key("test:pressure"), point)
                        .unwrap();
                    let restored_value = restored
                        .channel_value::<i64>(key("test:pressure"), point)
                        .unwrap();
                    (
                        point,
                        match scalar_value {
                            ChannelQuery::Loaded(value) => Some(value),
                            _ => None,
                        },
                        match restored_value {
                            ChannelQuery::Loaded(value) => Some(value),
                            _ => None,
                        },
                    )
                })
                .collect();
            panic!(
                "budget {budget}: checkpoint byte {index}が異なる expected={} actual={} world_revision={}/{} pressure={pressure_values:?}",
                expected_checkpoint[index],
                actual_checkpoint[index],
                scalar.revision(),
                restored.revision()
            );
        }
        assert!(job.step(&restored, 1).is_err());
        assert!(
            restored
                .edit(|edit| edit.publish_pressure_job(&job))
                .is_err()
        );
    }
}
#[test]
fn gas_pressure_uses_connected_mass_temperature_and_available_volume() {
    for (volume, temperature, expected) in [
        (3, 3, 65536),
        (2, 3, 3 * 65536 / 2),
        (3, 6, 2 * 65536),
        (3, 0, 0),
    ] {
        let mut world = fixture(8);
        let layer = layer(&world);
        let gas = cell(&world, "test:gas");
        world
            .edit(|edit| {
                edit.set_cell(layer, p(10, 10), gas)?;
                for x in 11..10 + volume {
                    edit.set_cell(layer, p(x, 10), CellCore::AIR)?;
                }
                edit.set_channel::<i64>(key("test:temperature"), p(10, 10), temperature * 65536)
            })
            .unwrap();
        let before = gas;
        let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
        assert_eq!(job.sample_count(), volume as usize);
        finish(&mut job, &world, 1);
        world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
        assert_eq!(pressure(&world, p(10, 10)), expected);
        assert!(
            matches!(world.cell(layer, p(10, 10)).unwrap(), CellQuery::Loaded(cell) if cell.core == before)
        );
        assert_eq!(job.report().cells, 1);
        assert_eq!(job.report().closed_components, 1);
        for x in 11..10 + volume {
            assert_eq!(pressure(&world, p(x, 10)), 0);
        }
    }
}
#[test]
fn gas_chambers_are_separate_and_liquids_do_not_count_as_gas_volume() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_cell(layer, p(11, 10), water)?;
            edit.set_cell(layer, p(20, 10), gas)?;
            edit.set_cell(layer, p(21, 10), CellCore::AIR)?;
            edit.set_channel::<i64>(key("test:temperature"), p(10, 10), 65536)?;
            edit.set_channel::<i64>(key("test:temperature"), p(20, 10), 4 * 65536)
        })
        .unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    finish(&mut job, &world, 2);
    world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    assert_eq!(pressure(&world, p(10, 10)), 2 * 65536);
    assert_eq!(pressure(&world, p(20, 10)), 2 * 65536);
}
#[test]
fn pump_is_explicit_and_does_not_change_temperature_or_material() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(11, 10), water)?;
            edit.set_cell(layer, p(10, 9), CellCore::AIR)?;
            edit.set_channel::<i64>(key("test:pump"), p(9, 11), 900)
        })
        .unwrap();
    let mut config = config();
    config.relaxation.gravity_q16 = 0;
    config.pump = Some(key("test:pump"));
    let mut job = PressureJob::new(&world, layer, config).unwrap();
    finish(&mut job, &world, 3);
    world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    assert_eq!(pressure(&world, p(10, 10)), 900);
    assert_eq!(pressure(&world, p(11, 10)), 900);
}
#[test]
fn budget_and_every_saved_stage_reproduce_identical_state() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(10, 11), water)?;
            edit.set_channel(key("test:pressure"), p(13, 8), 100i64)?;
            for point in [p(20, 20), p(30, 20)] {
                edit.set_cell(layer, point, gas)?;
                edit.set_channel(key("test:temperature"), point, 3i64 * 65536)?;
            }
            edit.set_cell(layer, p(40, 20), CellCore::AIR)?;
            edit.set_cell(layer, p(41, 20), CellCore::AIR)
        })
        .unwrap();
    let mut original = PressureJob::new(&world, layer, gas_config()).unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let mut stages = 0;
    while original.status() != PressureJobStatus::Ready {
        let work = original.completed_work();
        original.step(&world, 1).unwrap();
        assert_eq!(original.completed_work(), work + 1);
        assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
        let saved = bytes(&original);
        let restored = decoded(&saved).unwrap();
        assert_eq!(restored.status(), original.status());
        assert_eq!(restored.completed_work(), original.completed_work());
        assert_eq!(bytes(&restored), saved);
        stages += 1;
    }
    assert!(stages > 20);
    let saved = bytes(&original);
    let mut restored = decoded(&saved).unwrap();
    finish(&mut restored, &world, 1);
    assert_eq!(bytes(&restored), saved);
}
#[test]
fn job_and_world_checkpoint_resume_together() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_cell(layer, p(11, 10), CellCore::AIR)?;
            edit.set_channel::<i64>(key("test:temperature"), p(10, 10), 5 * 65536)
        })
        .unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    job.step(&world, 12).unwrap();
    world
        .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &job))
        .unwrap();
    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut resumed =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    let mut restored: PressureJob = resumed.state(key("test:job")).unwrap();
    finish(&mut job, &world, 5);
    finish(&mut restored, &resumed, 1);
    assert_eq!(bytes(&job), bytes(&restored));
    world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    resumed
        .edit(|edit| edit.publish_pressure_job(&restored))
        .unwrap();
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        resumed.checkpoint(4_000_000).unwrap()
    );
}
#[test]
fn incomplete_publish_and_ignored_errors_roll_back_outer_edit() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| edit.set_cell(layer, p(10, 10), gas))
        .unwrap();
    let job = PressureJob::new(&world, layer, config()).unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.set_cell(layer, p(20, 20), gas)?;
                let _ = edit.publish_pressure_job(&job);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}
#[test]
fn temperature_and_activity_changes_invalidate_input() {
    for temperature_change in [true, false] {
        let mut world = fixture(8);
        let layer = layer(&world);
        let gas = cell(&world, "test:gas");
        world
            .edit(|edit| edit.set_cell(layer, p(10, 10), gas))
            .unwrap();
        let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
        world
            .edit(|edit| {
                if temperature_change {
                    edit.set_channel::<i64>(key("test:temperature"), p(10, 10), 65536)
                } else {
                    edit.set_cell(layer, p(11, 10), CellCore::AIR)
                }
            })
            .unwrap();
        assert!(job.step(&world, 1).is_err());
        assert_eq!(job.completed_work(), 0);
    }
}
#[test]
fn capacities_do_not_publish_partial_results() {
    let mut world = fixture(1);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_cell(layer, p(63, 10), gas)?;
            edit.set_channel::<i64>(key("test:temperature"), p(63, 10), 65536)?;
            edit.set_channel::<i64>(key("test:temperature"), p(10, 10), 65536)
        })
        .unwrap();
    let mut config = gas_config();
    config.relaxation.cell_limit = 0;
    assert!(PressureJob::new(&world, layer, config).is_err());
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    finish(&mut job, &world, 10);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(world.edit(|edit| edit.publish_pressure_job(&job)).is_err());
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}
#[test]
fn malformed_records_and_impossible_reports_are_rejected() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_channel::<i64>(key("test:temperature"), p(10, 10), 65536)
        })
        .unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    finish(&mut job, &world, 10);
    let data = bytes(&job);
    for end in 0..data.len() {
        assert!(decoded(&data[..end]).is_err());
    }
    // 流体一cellより大きい閉領域数は構造上不正です。数値状態の計算履歴を再演しません。
    let mut changed = data.clone();
    let at = changed.len() - 4;
    changed[at..].copy_from_slice(&2u32.to_le_bytes());
    assert!(decoded(&changed).is_err());
}
#[test]
fn empty_activity_and_empty_gas_space_finish_without_fluid_output() {
    for gas in [false, true] {
        let mut world = fixture(8);
        let layer = layer(&world);
        world
            .edit(|edit| edit.set_cell(layer, p(10, 10), CellCore::AIR))
            .unwrap();
        let mut job =
            PressureJob::new(&world, layer, if gas { gas_config() } else { config() }).unwrap();
        finish(&mut job, &world, 1);
        assert_eq!(job.report(), PressureRelaxationReport::default());
        world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    }
}

#[test]
fn invalid_numeric_sources_fail_without_modifying_world() {
    for invalid_temperature in [-1i64, i64::MAX] {
        let mut world = fixture(8);
        let layer = layer(&world);
        let gas = cell(&world, "test:gas");
        world
            .edit(|edit| {
                edit.set_cell(layer, p(10, 10), gas)?;
                edit.set_channel(key("test:temperature"), p(10, 10), invalid_temperature)
            })
            .unwrap();
        let before = world.checkpoint(4_000_000).unwrap();
        if invalid_temperature < 0 {
            assert!(PressureJob::new(&world, layer, gas_config()).is_err());
        } else {
            let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
            let mut failed = false;
            for _ in 0..100 {
                if job.step(&world, 1).is_err() {
                    failed = true;
                    break;
                }
            }
            assert!(failed);
            assert_eq!(job.status(), PressureJobStatus::Computing);
        }
        assert_eq!(before, world.checkpoint(4_000_000).unwrap());
    }
}
#[test]
fn connected_gas_space_crosses_chunk_boundaries_without_using_inactive_cells() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    let stone = cell(&world, "test:stone");
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            for y in 0..64 {
                for x in 64..128 {
                    edit.set_cell(layer, p(x, y), stone)?;
                }
            }
            edit.set_cell(layer, p(63, 10), gas)?;
            edit.set_cell(layer, p(64, 10), CellCore::AIR)?;
            edit.set_channel(key("test:temperature"), p(63, 10), 3i64 * 65536)
        })
        .unwrap();
    let mut isolated = PressureJob::new(&world, layer, gas_config()).unwrap();
    finish(&mut isolated, &world, 1);
    world
        .edit(|edit| edit.publish_pressure_job(&isolated))
        .unwrap();
    assert_eq!(pressure(&world, p(63, 10)), 6 * 65536);
    world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("test:second-region"),
                region: ChunkRegion::new(ChunkCoord::new(1, 0)?, 2, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    let mut connected = PressureJob::new(&world, layer, gas_config()).unwrap();
    assert_eq!(connected.sample_count(), 2);
    finish(&mut connected, &world, 2);
    world
        .edit(|edit| edit.publish_pressure_job(&connected))
        .unwrap();
    assert_eq!(pressure(&world, p(63, 10)), 3 * 65536 / 2);
}
#[test]
fn finite_world_does_not_count_outside_partial_chunks_as_gas_volume() {
    let content = content(8);
    let layer = content.layer_id(key("test:terrain")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        0,
        WorldBounds::new(p(10, 10), 13, 11).unwrap(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_channel(key("test:temperature"), p(10, 10), 3i64 * 65536)?;
            edit.acquire_lease(RegionLease {
                source: key("test:simulation"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    assert_eq!(job.sample_count(), 3);
    finish(&mut job, &world, 1);
    world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    assert_eq!(pressure(&world, p(10, 10)), 65536);
}

#[test]
fn gas_target_is_relaxed_with_configured_lambda_instead_of_overwriting_pressure() {
    for (weight, expected) in [(0, 0), (1, 3 * 65536), (3, 9 * 65536 / 2)] {
        let mut world = fixture_with_properties(
            8,
            GasPressureProperties {
                relaxation_weight: weight,
                ..GasPressureProperties::default()
            },
        );
        let layer = layer(&world);
        let gas = cell(&world, "test:gas");
        world
            .edit(|edit| {
                for x in 10..12 {
                    edit.set_cell(layer, p(x, 10), gas)?;
                    edit.set_channel(key("test:temperature"), p(x, 10), 3i64 * 65536)?;
                }
                Ok(())
            })
            .unwrap();
        let mut config = gas_config();
        config.relaxation.iterations = 1;
        let mut job = PressureJob::new(&world, layer, config).unwrap();
        finish(&mut job, &world, 1);
        world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
        for x in 10..12 {
            assert_eq!(pressure(&world, p(x, 10)), expected);
        }
    }
}

#[test]
fn air_stays_zero_and_cannot_bypass_gas_lambda() {
    for (weight, expected) in [(0, 0), (1, 65536), (3, 3 * 65536 / 2)] {
        let mut world = fixture_with_properties(
            8,
            GasPressureProperties {
                relaxation_weight: weight,
                ..GasPressureProperties::default()
            },
        );
        let layer = layer(&world);
        let gas = cell(&world, "test:gas");
        world
            .edit(|edit| {
                edit.set_cell(layer, p(10, 10), gas)?;
                edit.set_cell(layer, p(11, 10), CellCore::AIR)?;
                edit.set_cell(layer, p(12, 10), CellCore::AIR)?;
                edit.set_channel(key("test:temperature"), p(10, 10), 3i64 * 65536)
            })
            .unwrap();
        let mut config = gas_config();
        config.relaxation.iterations = 1;
        let mut job = PressureJob::new(&world, layer, config).unwrap();
        finish(&mut job, &world, 1);
        world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
        assert_eq!(pressure(&world, p(10, 10)), expected);
        assert_eq!(pressure(&world, p(11, 10)), 0);
        assert_eq!(pressure(&world, p(12, 10)), 0);
    }
}

#[test]
fn loading_job_from_state_each_phase_makes_progress_with_budget_one() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(10, 11), water)
        })
        .unwrap();
    let job = PressureJob::new(&world, layer, config()).unwrap();
    world
        .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &job))
        .unwrap();
    for _ in 0..256 {
        let mut resumed: PressureJob = world.state(key("test:job")).unwrap();
        resumed.step(&world, 1).unwrap();
        world
            .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &resumed))
            .unwrap();
        if resumed.status() == PressureJobStatus::Ready {
            world
                .edit(|edit| edit.publish_pressure_job(&resumed))
                .unwrap();
            return;
        }
    }
    panic!("毎phaseの状態復元で検査が最初に戻り、有限jobが完了しない");
}

#[test]
fn connected_gases_resolve_lambda_from_each_material() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    let other = cell(&world, "test:other-gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_cell(layer, p(11, 10), other)?;
            for x in 10..12 {
                edit.set_channel(key("test:temperature"), p(x, 10), 3i64 * 65536)?;
            }
            Ok(())
        })
        .unwrap();
    let mut config = gas_config();
    config.relaxation.iterations = 1;
    let mut job = PressureJob::new(&world, layer, config).unwrap();
    finish(&mut job, &world, 1);
    world.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
    // 共通target6に対し、素材ごとのlambda1・3を独立して解決します。
    assert_eq!(pressure(&world, p(10, 10)), 3 * 65536);
    assert_eq!(pressure(&world, p(11, 10)), 9 * 65536 / 2);
}
#[test]
fn gas_properties_are_validated_fingerprinted_and_enforced() {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    assert!(
        registry
            .register(
                MaterialBuilder::new("test:invalid", MaterialBase::Solid)
                    .unwrap()
                    .gas_pressure(GasPressureProperties::default())
            )
            .is_err()
    );
    assert!(
        registry
            .register(
                MaterialBuilder::new("test:invalid", MaterialBase::Gas)
                    .unwrap()
                    .gas_pressure(GasPressureProperties {
                        maximum_q16: -1,
                        ..GasPressureProperties::default()
                    })
            )
            .is_err()
    );
    let default = content_with_properties(8, GasPressureProperties::default());
    let properties = GasPressureProperties {
        coefficient_q16: 2 * 65536,
        relaxation_weight: 3,
        maximum_q16: 4 * 65536,
    };
    let different = content_with_properties(8, properties);
    assert_ne!(default.fingerprint(), different.fingerprint());
    for (coefficient, expected) in [(65536, 6 * 65536), (2 * 65536, 12 * 65536)] {
        let mut sample = fixture_with_properties(
            8,
            GasPressureProperties {
                coefficient_q16: coefficient,
                ..GasPressureProperties::default()
            },
        );
        let layer = layer(&sample);
        let gas = cell(&sample, "test:gas");
        sample
            .edit(|edit| {
                edit.set_cell(layer, p(10, 10), gas)?;
                edit.set_channel(key("test:temperature"), p(10, 10), 3i64 * 65536)
            })
            .unwrap();
        let mut job = PressureJob::new(&sample, layer, gas_config()).unwrap();
        finish(&mut job, &sample, 1);
        sample.edit(|edit| edit.publish_pressure_job(&job)).unwrap();
        assert_eq!(pressure(&sample, p(10, 10)), expected);
    }
    let mut world = fixture_with_properties(8, properties);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), gas)?;
            edit.set_channel(key("test:temperature"), p(10, 10), 3i64 * 65536)
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    let mut failed = false;
    for _ in 0..100 {
        if job.step(&world, 1).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert_eq!(before, world.checkpoint(4_000_000).unwrap());
}

// CHPRJOB3のnode境界を型codecで読み、訪問bitだけを書き換える回帰試験用の位置を得ます。
fn saved_progress_offsets(data: &[u8]) -> (Vec<usize>, Vec<usize>, usize, usize) {
    let mut input = Decoder::new(data, 2_000_000).unwrap();
    assert_eq!(input.raw(8).unwrap(), b"CHPRJOB3");
    WorldId::decode(&mut input).unwrap();
    input.raw(32).unwrap();
    input.u64().unwrap();
    StableKey::decode(&mut input).unwrap();
    StableKey::decode(&mut input).unwrap();
    input.u16().unwrap();
    input.u32().unwrap();
    input.u32().unwrap();
    if input.u8().unwrap() != 0 {
        StableKey::decode(&mut input).unwrap();
    }
    Option::<StableKey>::decode(&mut input).unwrap();
    for _ in 0..input.u32().unwrap() {
        ChunkCoord::decode(&mut input).unwrap();
        input.u64().unwrap();
    }
    let mut offsets = Vec::new();
    for _ in 0..input.u32().unwrap() {
        CellPoint::decode(&mut input).unwrap();
        MaterialId::decode(&mut input).unwrap();
        input.u8().unwrap();
        input.u32().unwrap();
        for _ in 0..3 {
            input.i64().unwrap();
        }
        input.u8().unwrap();
        input.u32().unwrap();
        input.u32().unwrap();
        input.i64().unwrap();
        for _ in 0..4 {
            Option::<u32>::decode(&mut input).unwrap();
        }
        offsets.push(data.len() - input.remaining());
        input.u8().unwrap();
        Option::<i64>::decode(&mut input).unwrap();
        Option::<i64>::decode(&mut input).unwrap();
        input.i64().unwrap();
        input.i64().unwrap();
    }
    let stage = data.len() - input.remaining();
    input.u8().unwrap();
    let cursor = data.len() - input.remaining();
    input.u32().unwrap();
    input.u32().unwrap();
    let mut queue = Vec::new();
    for _ in 0..input.u32().unwrap() {
        queue.push(data.len() - input.remaining());
        input.u32().unwrap();
    }
    (offsets, queue, stage, cursor)
}
#[test]
fn saved_seen_flags_cannot_skip_unprocessed_components() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            for point in [p(10, 10), p(20, 20)] {
                edit.set_cell(layer, point, gas)?;
                edit.set_channel(key("test:temperature"), point, 3i64 * 65536)?;
            }
            Ok(())
        })
        .unwrap();
    for budget in [3, 8] {
        let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
        job.step(&world, budget).unwrap();
        let mut data = bytes(&job);
        let (offsets, _, _, _) = saved_progress_offsets(&data);
        assert_eq!(data[offsets[1]], 0);
        data[offsets[1]] = 1;
        assert!(
            decoded(&data).is_err(),
            "未処理の別成分を訪問済みとするrecordを受理した"
        );
    }
}

#[test]
fn saved_queue_must_follow_the_component_root_and_bfs_order() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            for point in [p(10, 10), p(11, 10), p(20, 20)] {
                edit.set_cell(layer, point, gas)?;
            }
            Ok(())
        })
        .unwrap();
    let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
    job.step(&world, 6).unwrap();
    let mut data = bytes(&job);
    let (_, queue, _, _) = saved_progress_offsets(&data);
    assert_eq!(queue.len(), 2);
    // 両nodeの訪問bitと件数を変えず、queueだけをrootと矛盾する順へ入れ替えます。
    let first = data[queue[0]..queue[0] + 4].to_vec();
    let second = data[queue[1]..queue[1] + 4].to_vec();
    data[queue[0]..queue[0] + 4].copy_from_slice(&second);
    data[queue[1]..queue[1] + 4].copy_from_slice(&first);
    assert!(decoded(&data).is_err());
}

#[test]
fn saved_jacobi_and_copy_cursors_cannot_skip_numeric_work() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let gas = cell(&world, "test:gas");
    world
        .edit(|edit| {
            for point in [p(10, 10), p(20, 20)] {
                edit.set_cell(layer, point, gas)?;
                edit.set_channel(key("test:temperature"), point, 3i64 * 65536)?;
            }
            Ok(())
        })
        .unwrap();
    for wanted_stage in [4, 6] {
        let mut job = PressureJob::new(&world, layer, gas_config()).unwrap();
        let mut reached = false;
        for _ in 0..256 {
            let mut data = bytes(&job);
            let (nodes, _, stage, cursor) = saved_progress_offsets(&data);
            let saved_cursor = u32::from_le_bytes(data[cursor..cursor + 4].try_into().unwrap());
            if data[stage] == wanted_stage && saved_cursor == 0 {
                data[cursor..cursor + 4].copy_from_slice(&(nodes.len() as u32).to_le_bytes());
                assert!(
                    decoded(&data).is_err(),
                    "数値処理を飛ばすcursorを受理した: stage={wanted_stage}"
                );
                reached = true;
                break;
            }
            job.step(&world, 1).unwrap();
        }
        assert!(reached);
    }
}

#[test]
fn pressure_jobs_publish_from_one_snapshot_and_reject_overlapping_outputs() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(20, 20), water)
        })
        .unwrap();

    let mut first = PressureJob::new(&world, layer, config()).unwrap();
    let mut second =
        PressureJob::new(&world, layer, config_with_channel("test:pressure-two")).unwrap();
    finish(&mut first, &world, 32);
    finish(&mut second, &world, 32);
    let reports = world
        .edit(|edit| edit.publish_pressure_jobs(&[first, second]))
        .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].cells, 2);
    assert_eq!(reports[1].cells, 2);

    let mut duplicate_a = PressureJob::new(&world, layer, config()).unwrap();
    let mut duplicate_b = PressureJob::new(&world, layer, config()).unwrap();
    finish(&mut duplicate_a, &world, 32);
    finish(&mut duplicate_b, &world, 32);
    let before = world.checkpoint(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.publish_pressure_jobs(&[duplicate_a, duplicate_b]))
            .is_err()
    );
    assert_eq!(world.checkpoint(2_000_000).unwrap(), before);
}

#[test]
fn pressure_job_publication_scratch_obeys_job_byte_limit() {
    let mut world = fixture_with_job_byte_limit(8, GasPressureProperties::default(), 1);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| edit.set_cell(layer, p(10, 10), water))
        .unwrap();
    let mut job = PressureJob::new(&world, layer, config()).unwrap();
    finish(&mut job, &world, 32);
    let before = world.checkpoint(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.publish_pressure_jobs(&[job]))
            .is_err()
    );
    assert_eq!(world.checkpoint(2_000_000).unwrap(), before);
}

#[test]
fn pressure_output_starts_push_search_from_new_snapshot_and_publishes_atomically() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(11, 10), CellCore::AIR)?;
            edit.set_channel(key("test:pump"), p(9, 11), 1000i64)
        })
        .unwrap();
    let mut job = PressureJob::new(
        &world,
        layer,
        PressureJobConfig {
            pump: Some(key("test:pump")),
            ..config()
        },
    )
    .unwrap();
    finish(&mut job, &world, 32);

    let result = world
        .edit(|edit| {
            edit.publish_pressure_job_then_search_push(
                &job,
                layer,
                p(10, 10),
                PressurePushConfig {
                    channel: key("test:pressure"),
                    gravity_q16: 0,
                    threshold_q16: 0,
                    nodes: 4096,
                },
                128,
            )
        })
        .unwrap();
    assert_eq!(result.search_status, PressurePushStatus::Ready);
    assert!(result.published_push);
    assert!(result.pending_search.is_none());
    assert_eq!(
        material_at(&world, layer, p(11, 10)),
        world
            .content()
            .material_id(MaterialHandle(key("test:water")))
            .unwrap()
    );
    assert_eq!(pressure(&world, p(10, 10)), 1000);
}

#[test]
fn invalid_push_search_rolls_back_pressure_output() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_channel(key("test:pump"), p(9, 11), 1000i64)
        })
        .unwrap();
    let mut job = PressureJob::new(
        &world,
        layer,
        PressureJobConfig {
            pump: Some(key("test:pump")),
            ..config()
        },
    )
    .unwrap();
    finish(&mut job, &world, 32);
    let before = world.checkpoint(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.publish_pressure_job_then_search_push(
                &job,
                layer,
                p(10, 10),
                PressurePushConfig {
                    channel: key("test:pressure"),
                    gravity_q16: 0,
                    threshold_q16: 0,
                    nodes: 0,
                },
                1,
            ))
            .is_err()
    );
    assert_eq!(world.checkpoint(2_000_000).unwrap(), before);
}

#[test]
fn pending_push_search_uses_new_pressure_and_resumes_from_world_state() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let water = cell(&world, "test:water");
    world
        .edit(|edit| {
            edit.set_cell(layer, p(10, 10), water)?;
            edit.set_cell(layer, p(11, 10), water)?;
            edit.set_cell(layer, p(12, 10), CellCore::AIR)?;
            edit.set_channel(key("test:pump"), p(9, 11), 1000i64)
        })
        .unwrap();
    let mut job = PressureJob::new(
        &world,
        layer,
        PressureJobConfig {
            pump: Some(key("test:pump")),
            ..config()
        },
    )
    .unwrap();
    finish(&mut job, &world, 32);
    world
        .edit(|edit| {
            let result = edit.publish_pressure_job_then_search_push(
                &job,
                layer,
                p(10, 10),
                PressurePushConfig {
                    channel: key("test:pressure"),
                    gravity_q16: 0,
                    threshold_q16: 0,
                    nodes: 4096,
                },
                1,
            )?;
            assert_eq!(result.search_status, PressurePushStatus::Searching);
            let search = result.pending_search.as_ref().unwrap();
            edit.set_state(key("test:push"), key("test:push-schema"), search)
        })
        .unwrap();

    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    world = World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    let mut search: PressurePushSearch = world.state(key("test:push")).unwrap();
    assert_eq!(search.status(), PressurePushStatus::Searching);
    for _ in 0..16 {
        if search.status() == PressurePushStatus::Ready {
            break;
        }
        search.step(&world, 1).unwrap();
    }
    assert_eq!(search.status(), PressurePushStatus::Ready);
    world
        .edit(|edit| edit.publish_pressure_push(&search))
        .unwrap();
    assert_eq!(
        material_at(&world, layer, p(12, 10)),
        world
            .content()
            .material_id(MaterialHandle(key("test:water")))
            .unwrap()
    );
}

#[test]
fn pressure_push_skips_cells_occupied_by_same_layer_particles() {
    let mut world = fixture(8);
    let layer = layer(&world);
    let stone = cell(&world, "test:stone");
    let water = cell(&world, "test:water");
    let source = p(10, 10);
    let occupied_outlet = p(11, 10);
    let alternate_source = p(10, 11);
    let alternate_outlet = p(10, 12);
    world
        .edit(|edit| {
            edit.set_cell(layer, source, water)?;
            edit.set_cell(layer, p(10, 9), stone)?;
            edit.set_cell(layer, p(9, 10), stone)?;
            edit.set_cell(layer, occupied_outlet, water)?;
            edit.set_cell(layer, alternate_source, water)?;
            edit.set_cell(layer, p(9, 11), stone)?;
            edit.set_cell(layer, p(11, 11), stone)?;
            edit.set_cell(layer, alternate_outlet, CellCore::AIR)?;
            edit.set_channel(key("test:pressure"), p(source.x + 3, source.y - 2), 1000i64)?;
            edit.emit_material_particle(layer, occupied_outlet, [0, 0])?;
            Ok(())
        })
        .unwrap();

    let mut search = PressurePushSearch::new(
        &world,
        layer,
        source,
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes: 4096,
        },
    )
    .unwrap();
    assert_eq!(
        search.step(&world, 4096).unwrap(),
        PressurePushStatus::Ready
    );
    world
        .edit(|edit| edit.publish_pressure_push(&search))
        .unwrap();

    let (handle, particle) = world.material_particles().next().unwrap();
    assert_eq!(
        world.material_particle(handle).unwrap().cell(),
        occupied_outlet
    );
    assert_eq!(material_at(&world, layer, occupied_outlet), MaterialId::AIR);
    assert_eq!(material_at(&world, layer, alternate_outlet), water.material);
    assert_eq!(particle.cell(), occupied_outlet);
}
