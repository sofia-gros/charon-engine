//! 半cell/tickの端数蓄積、壁までの輸送、停止状態の保存・復元を確認します。
//! Runtimeの外でWorld時計を明示的に進めるheadless作例です。

use charon_engine::{
    foundation::{EngineResult, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};

fn key(name: &str) -> StableKey {
    StableKey::parse(name).expect("作例の固定キー")
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

fn main() -> EngineResult<()> {
    let layer_key = key("example:terrain");
    let motion_key = key("example:motion");
    let value_key = key("example:motion-value");
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    registry.layer(LayerDef {
        key: layer_key,
        role: LayerRole::Terrain,
        render_order: 0,
        collision_group: 1,
        simulated: true,
    })?;
    registry.register(MaterialBuilder::new("example:grain", MaterialBase::Powder)?)?;
    registry.register(MaterialBuilder::new("example:wall", MaterialBase::Solid)?)?;
    registry.schema(Schema::of::<MaterialMotion>(value_key, 1, 24)?)?;
    registry.material_channel(
        layer_key,
        ChannelDefinition {
            key: motion_key,
            version: 1,
            value_schema: value_key,
            layout: GridLayout {
                origin: p(0, 0),
                x_scale: GridScale::new(1, 1)?,
                y_scale: GridScale::new(1, 1)?,
            },
            default: MaterialMotion::default(),
            limits: GridLimits {
                chunks: 2,
                checkpoint_bytes: 1_000_000,
            },
        },
    )?;
    let content = registry.freeze()?;
    let layer = content.layer_id(layer_key)?;
    let grain = content.material_id(MaterialHandle(key("example:grain")))?;
    let wall = content.material_id(MaterialHandle(key("example:wall")))?;
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        0,
        WorldBounds::new(p(0, 0), 64, 64)?,
        content.clone(),
        WorldLimits::default(),
    )?;
    world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.acquire_lease(RegionLease {
            source: key("example:simulation"),
            region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
            mode: LeaseMode::Simulate,
            expires: None,
        })?;
        edit.set_cell(
            layer,
            p(2, 4),
            CellCore {
                material: grain,
                ..CellCore::AIR
            },
        )?;
        edit.set_cell(
            layer,
            p(10, 4),
            CellCore {
                material: wall,
                ..CellCore::AIR
            },
        )?;
        edit.set_channel(
            motion_key,
            p(2, 4),
            MaterialMotion {
                velocity_q16: [32768, 0],
                remainder_q16: [0, 0],
            },
        )
    })?;
    let config = MaterialMotionConfig {
        channel: motion_key,
        candidate_limit: 16,
        max_distance: 8,
        step_limit: 64,
    };
    let mut crossings = 0;
    let mut collisions = 0;
    for _ in 0..20 {
        let report = world.edit(|edit| {
            let report = edit.transport_material_motion(layer, config)?;
            // この作例はRuntimeを使用しないため、時計の所有者は呼出側です。
            edit.advance_tick()?;
            Ok(report)
        })?;
        crossings += report.crossings;
        collisions += report.collisions;
    }
    let snapshot = world.checkpoint(1_000_000)?;
    let restored = World::restore(&snapshot, content, WorldLimits::default(), 1_000_000)?;
    assert!(
        matches!(restored.cell(layer,p(9,4))?,CellQuery::Loaded(cell) if cell.core.material==grain)
    );
    assert!(
        matches!(restored.channel_value::<MaterialMotion>(motion_key,p(9,4))?,
        ChannelQuery::Loaded(value) if value==MaterialMotion::default())
    );
    assert_eq!((restored.tick(), crossings, collisions), (20, 7, 1));
    println!(
        "{}tick、{}cellを移動し、壁の手前で停止しました（衝突{}回）。",
        restored.tick(),
        crossings,
        collisions
    );
    println!("物質と速度・端数を保存し、同じ停止状態を復元しました。");
    Ok(())
}
