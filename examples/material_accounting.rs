//! cell・粒子・archiveの所有先を変えても、全表現の収支と履歴が一致する作例です。
use charon_engine::{
    foundation::{EngineResult, StableKey, grid::CellPoint, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;
fn key(value: &str) -> StableKey {
    StableKey::parse(value).expect("作例の固定キー")
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
        .register(MaterialBuilder::new("example:water", MaterialBase::Liquid)?.mass_per_cell(5))?;
    registry.schema(Schema::of::<MaterialBalanceLog>(
        key("example:balance-log"),
        1,
        1_000_000,
    )?)?;
    let content = registry.freeze()?;
    let layer = content.layer_id(key("example:terrain"))?;
    let water = content.material_id(MaterialHandle(key("example:water")))?;
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )?;
    world.edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))?;
    let inventory = MaterialInventoryProfile::new(
        key("example:inventory"),
        1,
        vec![],
        vec![],
        MaterialInventoryLimits {
            atoms: 4096,
            components: 16,
        },
    )?;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/material_accounting_example");
    let mut archives = DirectoryArchives::new(root.join("archives"))?;
    let cache = world.load_material_archive_inventory(&inventory, &mut archives, 4_000_000)?;
    let journal = key("example:balance");
    let schema = key("example:balance-log");
    world.initialize_material_accounting(&inventory, &cache, journal, schema, 16)?;
    world.edit_material_accounted(
        &inventory,
        &cache,
        journal,
        schema,
        Some(MaterialChangeReason {
            key: key("example:spring"),
            version: 1,
            kind: MaterialReceiptKind::Source,
        }),
        |edit| {
            edit.set_cell(
                layer,
                CellPoint { x: 2, y: 2 },
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )
        },
    )?;
    let expected = world.verify_material_accounting(&inventory, &cache, journal)?;
    world.edit_material_accounted(&inventory, &cache, journal, schema, None, |edit| {
        edit.emit_material_particle(layer, CellPoint { x: 2, y: 2 }, [0, 0])
    })?;
    let region = ChunkRegion::single(ChunkCoord::new(0, 0)?)?;
    let stored = world
        .prepare_eviction(region, 4_000_000)?
        .store(&mut archives)?;
    world.edit(|edit| edit.finish_eviction(stored))?;
    let world_profile = WorldSaveProfile {
        scope: key("example:world"),
        profile: key("example:headless"),
        plugins: vec![],
    };
    let profile = MaterialAccountingSaveProfile {
        world: &world_profile,
        inventory: &inventory,
        journal,
        archive_byte_limit: 4_000_000,
    };
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(root.join("generations"))?,
        SaveLimits::default(),
        3,
    )?;
    world.save_material_accounted(&mut repository, &mut archives, &profile, 4_000_000)?;
    let mut world = World::load_material_accounted(
        &mut repository,
        &mut archives,
        Arc::clone(world.content()),
        &profile,
        WorldLimits::default(),
        4_000_000,
    )?;
    let cache = world.load_material_archive_inventory(&inventory, &mut archives, 4_000_000)?;
    expected.verify_conserved(&world.verify_material_accounting(&inventory, &cache, journal)?)?;
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0)?, 4_000_000)?
        .load(&mut archives)?;
    world.edit(|edit| edit.restore_region(loaded))?;
    let handle = world.material_particles().next().expect("保存した一粒子").0;
    world.edit_material_accounted(&inventory, &cache, journal, schema, None, |edit| {
        edit.land_material_particle(handle)
    })?;
    let actual = world.verify_material_accounting(&inventory, &cache, journal)?;
    expected.verify_conserved(&actual)?;
    assert_eq!(actual.quantities()[0].tokens, MaterialQuantity::ONE);
    assert_eq!(actual.quantities()[0].mass, MaterialQuantity::new(5, 1)?);
    assert_eq!(
        world.state::<MaterialBalanceLog>(journal)?.receipts().len(),
        1
    );
    println!(
        "cell→粒子→archive→復帰→着地でtoken=1・mass=5を維持し、理由付きreceipt=1を保存しました。"
    );
    Ok(())
}
