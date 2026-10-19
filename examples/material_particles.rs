//! 一つの物質を全payloadごと放出し、保存復元後も同じ着地結果になることを確認します。
use charon_engine::{
    foundation::{EngineResult, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;

fn key(name: &str) -> StableKey {
    StableKey::parse(name).expect("作例の固定キー")
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn main() -> EngineResult<()> {
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    registry.layer(LayerDef {
        key: key("demo:terrain"),
        role: LayerRole::Terrain,
        render_order: 0,
        collision_group: 1,
        simulated: true,
    })?;
    registry.register(MaterialBuilder::new("demo:water", MaterialBase::Liquid)?)?;
    registry.register(MaterialBuilder::new("demo:wall", MaterialBase::Solid)?)?;
    registry.schema(Schema::of::<u64>(key("demo:energy-value"), 1, 8)?)?;
    registry.material_channel(
        key("demo:terrain"),
        ChannelDefinition {
            key: key("demo:energy"),
            version: 1,
            value_schema: key("demo:energy-value"),
            layout: GridLayout {
                origin: point(0, 0),
                x_scale: GridScale::new(1, 1)?,
                y_scale: GridScale::new(1, 1)?,
            },
            default: 0u64,
            limits: GridLimits {
                chunks: 8,
                checkpoint_bytes: 1_000_000,
            },
        },
    )?;
    let content = registry.freeze()?;
    let layer = content.layer_id(key("demo:terrain"))?;
    let water = content.material_id(MaterialHandle(key("demo:water")))?;
    let wall = content.material_id(MaterialHandle(key("demo:wall")))?;
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )?;
    world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.acquire_lease(RegionLease {
            source: key("demo:activity"),
            region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
            mode: LeaseMode::Simulate,
            expires: None,
        })?;
        edit.set_cell(
            layer,
            point(2, 2),
            CellCore {
                material: water,
                ..CellCore::AIR
            },
        )?;
        edit.set_cell(
            layer,
            point(7, 2),
            CellCore {
                material: wall,
                ..CellCore::AIR
            },
        )?;
        edit.set_channel(key("demo:energy"), point(2, 2), 900u64)
    })?;
    let particle =
        world.edit(|edit| edit.emit_material_particle(layer, point(2, 2), [2 * 65536, 0]))?;
    world.edit(|edit| edit.step_material_particles(16, 64))?;
    assert_eq!(world.material_particle(particle)?.cell(), point(4, 2));
    let saved = world.checkpoint(4_000_000)?;
    let mut resumed = World::restore(
        &saved,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )?;
    for target in [&mut world, &mut resumed] {
        // Runtimeを使わない作例なので、作者がphaseの後に時計を進めます。
        for _ in 0..2 {
            target.edit(|edit| edit.step_material_particles(16, 64))?;
            target.edit(|edit| edit.advance_tick())?;
        }
        assert_eq!(target.material_particles().count(), 0);
        assert!(matches!(
            target.channel_value::<u64>(key("demo:energy"), point(6, 2))?,
            ChannelQuery::Loaded(900)
        ));
    }
    assert_eq!(world.checkpoint(4_000_000)?, resumed.checkpoint(4_000_000)?);
    println!("粒子の放出→途中保存→着地が一致しました。energy=900、寿命による消失なし。");
    Ok(())
}
