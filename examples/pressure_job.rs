//! 有限の気体室で温度・容積から圧力を求め、途中保存から再開します。
use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
fn key(name: &str) -> StableKey {
    StableKey::parse(name).expect("作例の固定キー")
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn main() -> EngineResult<()> {
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    registry.layer(LayerDef {
        key: key("example:terrain"),
        role: LayerRole::Terrain,
        render_order: 0,
        collision_group: 1,
        simulated: true,
    })?;
    registry.register(MaterialBuilder::new("example:gas", MaterialBase::Gas)?.mass_per_cell(2))?;
    registry.schema(Schema::of::<i64>(key("example:q16"), 1, 8)?)?;
    registry.schema(Schema::of::<PressureJob>(
        key("example:job-schema"),
        1,
        8192,
    )?)?;
    for name in ["example:pressure", "example:temperature"] {
        registry.channel(ChannelDefinition {
            key: key(name),
            version: 1,
            value_schema: key("example:q16"),
            layout: GridLayout {
                origin: p(0, 0),
                x_scale: GridScale::new(1, 1)?,
                y_scale: GridScale::new(1, 1)?,
            },
            default: 0i64,
            limits: GridLimits {
                chunks: 1,
                checkpoint_bytes: 100_000,
            },
        })?;
    }
    let content = registry.freeze()?;
    let layer = content.layer_id(key("example:terrain"))?;
    let gas = CellCore {
        material: content.material_id(MaterialHandle(key("example:gas")))?,
        ..CellCore::AIR
    };
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        0,
        WorldBounds::new(p(0, 0), 3, 1)?,
        content.clone(),
        WorldLimits::default(),
    )?;
    world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.set_cell(layer, p(0, 0), gas)?;
        edit.set_channel(key("example:temperature"), p(0, 0), 3i64 * 65536)?;
        edit.acquire_lease(RegionLease {
            source: key("example:simulation"),
            region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
            mode: LeaseMode::Simulate,
            expires: None,
        })?;
        Ok(())
    })?;
    let mut job = PressureJob::new(
        &world,
        layer,
        PressureJobConfig {
            relaxation: PressureRelaxation {
                channel: key("example:pressure"),
                iterations: 4,
                gravity_q16: 65536,
                cell_limit: 3,
            },
            gas: Some(GasPressureSource {
                temperature: key("example:temperature"),
            }),
            pump: None,
        },
    )?;
    job.step(&world, 5)?;
    world.edit(|edit| edit.set_state(key("example:job"), key("example:job-schema"), &job))?;
    let snapshot = world.checkpoint(1_000_000)?;
    let mut resumed = World::restore(&snapshot, content, WorldLimits::default(), 1_000_000)?;
    let mut restored: PressureJob = resumed.state(key("example:job"))?;
    while job.status() != PressureJobStatus::Ready {
        job.step(&world, 2)?;
    }
    while restored.status() != PressureJobStatus::Ready {
        restored.step(&resumed, 1)?;
    }
    world.edit(|edit| edit.publish_pressure_job(&job))?;
    resumed.edit(|edit| edit.publish_pressure_job(&restored))?;
    assert_eq!(world.checkpoint(1_000_000)?, resumed.checkpoint(1_000_000)?);
    let expected = 65536i64;
    assert!(
        matches!(resumed.channel_value::<i64>(key("example:pressure"),p(0,0))?,ChannelQuery::Loaded(value) if value==expected)
    );
    println!(
        "気体mass2・温度3・容積3・lambda1: target Q16=131072、空境界0からの緩和後Q16={}、{}件の計算と途中保存からの再開が一致",
        expected,
        job.completed_work()
    );
    Ok(())
}
