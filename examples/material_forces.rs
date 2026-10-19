//! E05の標準重力・速度端数とDDA輸送を、連続実行と保存再開で照合します。
use charon_engine::{
    foundation::{EngineResult, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;

fn key(text: &str) -> StableKey {
    StableKey::parse(text).expect("作例の固定キー")
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn prepare() -> EngineResult<(World, LayerId)> {
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    registry.layer(LayerDef {
        key: key("demo:front"),
        role: LayerRole::Terrain,
        render_order: 0,
        collision_group: 1,
        simulated: true,
    })?;
    registry.register(
        MaterialBuilder::new("demo:sand", MaterialBase::Powder)?.motion_properties(
            MaterialMotionProperties {
                gravity_scale_q16: 65536,
                drag_q16: 0,
                buoyancy_scale_q16: 0,
                friction_q16: 0,
            },
        ),
    )?;
    registry.schema(Schema::of::<MaterialMotion>(
        key("demo:motion-schema"),
        1,
        24,
    )?)?;
    registry.material_channel(
        key("demo:front"),
        ChannelDefinition {
            key: key("demo:motion"),
            version: 1,
            value_schema: key("demo:motion-schema"),
            layout: GridLayout {
                origin: point(0, 0),
                x_scale: GridScale::new(1, 1)?,
                y_scale: GridScale::new(1, 1)?,
            },
            default: MaterialMotion::default(),
            limits: GridLimits {
                chunks: 4,
                checkpoint_bytes: 500_000,
            },
        },
    )?;
    let content = registry.freeze()?;
    let layer = content.layer_id(key("demo:front"))?;
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )?;
    let material = world
        .content()
        .material_id(MaterialHandle(key("demo:sand")))?;
    world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.acquire_lease(RegionLease {
            source: key("demo:visible"),
            region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
            mode: LeaseMode::Simulate,
            expires: None,
        })?;
        edit.set_cell(
            layer,
            point(8, 8),
            CellCore {
                material,
                ..CellCore::AIR
            },
        )
    })?;
    Ok((world, layer))
}
fn step(world: &mut World, layer: LayerId) -> EngineResult<()> {
    world.edit(|edit| {
        edit.step_material_force_motion(
            layer,
            MaterialForceConfig {
                channel: key("demo:motion"),
                gravity_q16: [0, 32768],
                candidate_limit: 16,
                max_velocity_q16: 65536 * 8,
            },
            MaterialMotionConfig {
                channel: key("demo:motion"),
                candidate_limit: 16,
                max_distance: 8,
                step_limit: 128,
            },
            &StandardMaterialForces,
        )
        .map(|_| ())
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut continuous, layer) = prepare()?;
    step(&mut continuous, layer)?;
    let saved = continuous.checkpoint(4_000_000)?;
    let mut resumed = World::restore(
        &saved,
        Arc::clone(continuous.content()),
        WorldLimits::default(),
        4_000_000,
    )?;
    step(&mut continuous, layer)?;
    step(&mut resumed, layer)?;
    assert_eq!(
        continuous.checkpoint(4_000_000)?,
        resumed.checkpoint(4_000_000)?,
        "同じ2tickでも保存再開と連続実行で正規状態が異なる"
    );
    assert!(matches!(
        resumed.cell(layer, point(8, 9))?,
        CellQuery::Loaded(cell) if cell.core.material != MaterialId::AIR
    ));
    println!("E05標準重力・端数・速度DDAの2tick保存再開が一致しました");
    Ok(())
}
