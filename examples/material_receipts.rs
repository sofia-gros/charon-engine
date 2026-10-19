//! 作者の発行・変換・消費を理由付きreceiptへ記録し、地形とともに保存・復元します。
//! この作例の素材名・変換比率は作者側の規則であり、エンジンの固定recipeではありません。

use charon_engine::{
    foundation::{EngineResult, StableKey, grid::CellPoint, id::WorldId, schema::Schema},
    world::*,
};
fn key(name: &str) -> StableKey {
    StableKey::parse(name).expect("作例の固定キー")
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn change(point: CellPoint, expected: CellCore, output: CellCore) -> MaterialCellChange {
    MaterialCellChange {
        point,
        expected,
        output,
        payload: MaterialPayloadPolicy::ResetChannels,
    }
}
fn reason(name: &str, kind: MaterialReceiptKind) -> MaterialChangeReason {
    MaterialChangeReason {
        key: key(name),
        version: 1,
        kind,
    }
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
    registry
        .register(MaterialBuilder::new("example:fluid", MaterialBase::Liquid)?.mass_per_cell(5))?;
    registry
        .register(MaterialBuilder::new("example:grain", MaterialBase::Powder)?.mass_per_cell(10))?;
    let journal = MaterialReceiptJournal {
        key: key("example:receipts"),
        schema: key("example:receipt-schema"),
    };
    registry.schema(Schema::of::<MaterialReceiptLog>(journal.schema, 1, 8192)?)?;
    let content = registry.freeze()?;
    let layer = content.layer_id(key("example:terrain"))?;
    let fluid = CellCore {
        material: content.material_id(MaterialHandle(key("example:fluid")))?,
        ..CellCore::AIR
    };
    let grain = CellCore {
        material: content.material_id(MaterialHandle(key("example:grain")))?,
        ..CellCore::AIR
    };
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        0,
        WorldBounds::new(p(0, 0), 64, 64)?,
        content.clone(),
        WorldLimits::default(),
    )?;
    let log = MaterialReceiptLog::new(&world, journal.key, 16)?;
    world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.set_state(journal.key, journal.schema, &log)
    })?;
    world.edit(|edit| {
        edit.apply_material_changes(
            layer,
            journal,
            reason("example:spawn", MaterialReceiptKind::Source),
            &[
                change(p(1, 2), CellCore::AIR, fluid),
                change(p(2, 2), CellCore::AIR, fluid),
            ],
        )
    })?;
    world.edit(|edit| {
        edit.apply_material_changes(
            layer,
            journal,
            reason("example:convert", MaterialReceiptKind::Conversion),
            &[
                change(p(1, 2), fluid, grain),
                change(p(2, 2), fluid, CellCore::AIR),
            ],
        )
    })?;
    world.edit(|edit| {
        edit.apply_material_changes(
            layer,
            journal,
            reason("example:consume", MaterialReceiptKind::Sink),
            &[change(p(1, 2), grain, CellCore::AIR)],
        )
    })?;
    let bytes = world.checkpoint(1_000_000)?;
    let restored = World::restore(&bytes, content, WorldLimits::default(), 1_000_000)?;
    let log = restored.state::<MaterialReceiptLog>(journal.key)?;
    log.validate_world(&restored)?;
    assert_eq!(log.len(), 3);
    for point in [p(1, 2), p(2, 2)] {
        assert!(
            matches!(restored.cell(layer,point)?,CellQuery::Loaded(cell) if cell.core==CellCore::AIR)
        );
    }
    let mut consumed = 0;
    let mut produced = 0;
    for receipt in log.receipts() {
        consumed += receipt.consumed_mass()?;
        produced += receipt.produced_mass()?;
        println!(
            "receipt {}：理由={}、消費mass={}、発行mass={}",
            receipt.sequence(),
            receipt.reason().key,
            receipt.consumed_mass()?,
            receipt.produced_mass()?
        );
    }
    assert_eq!((consumed, produced), (20, 20));
    println!("発行・変換・消費の3receiptと空になった地形を同時に復元しました。");
    Ok(())
}
