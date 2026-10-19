//! E05のresident収支がcell・粒子の所有権移動と保存復帰で一致することを検証します。

use charon_engine::{
    foundation::{
        EngineError, EngineResult, StableKey, grid::CellPoint, id::WorldId, schema::Schema,
    },
    world::*,
};
use std::{collections::BTreeMap, sync::Arc};

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}

fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

fn fixture() -> (World, LayerId, LayerId) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    for name in ["test:front", "test:back"] {
        registry
            .layer(LayerDef {
                key: key(name),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
    }
    registry
        .register(
            MaterialBuilder::new("test:sand", MaterialBase::Powder)
                .unwrap()
                .mass_per_cell(7),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:water", MaterialBase::Liquid)
                .unwrap()
                .mass_per_cell(3),
        )
        .unwrap();
    registry
        .schema(Schema::of::<MaterialReceiptLog>(key("test:receipt-schema"), 1, 8192).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let front = content.layer_id(key("test:front")).unwrap();
    let back = content.layer_id(key("test:back")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        17,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    (world, front, back)
}

fn amount(inventory: &ResidentMaterialInventory, key_name: &str) -> ResidentMaterialAmount {
    *inventory
        .materials
        .iter()
        .find(|entry| entry.material == key(key_name))
        .unwrap()
}

#[test]
fn all_layers_and_inactive_cells_count_once() {
    let (mut world, front, back) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                back,
                point(1, 1),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                front,
                point(2, 1),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let inventory = world.resident_material_inventory().unwrap();
    assert_eq!(inventory.archived_chunks, 0);
    assert_eq!(inventory.archived_particles, 0);
    assert_eq!(inventory.materials.len(), 2);
    let sand = amount(&inventory, "test:sand");
    assert_eq!(
        (
            sand.cell_tokens,
            sand.particle_tokens,
            sand.total_tokens,
            sand.mass
        ),
        (1, 0, 1, 7)
    );
    let water = amount(&inventory, "test:water");
    assert_eq!(
        (
            water.cell_tokens,
            water.particle_tokens,
            water.total_tokens,
            water.mass
        ),
        (2, 0, 2, 6)
    );
}

#[test]
fn emission_landing_and_checkpoint_keep_resident_total() {
    let (mut world, front, _) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let before = world.resident_material_inventory().unwrap();
    let handle = world
        .edit(|editor| editor.emit_material_particle(front, point(1, 1), [0, 0]))
        .unwrap();
    let emitted = world.resident_material_inventory().unwrap();
    assert_eq!(amount(&emitted, "test:sand").cell_tokens, 0);
    assert_eq!(amount(&emitted, "test:sand").particle_tokens, 1);
    assert_eq!(
        amount(&before, "test:sand").mass,
        amount(&emitted, "test:sand").mass
    );

    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(restored.resident_material_inventory().unwrap(), emitted);
    restored
        .edit(|editor| editor.land_material_particle(handle))
        .unwrap();
    assert_eq!(restored.resident_material_inventory().unwrap(), before);
}

#[test]
fn pr4_transport_moves_carriers_without_changing_inventory() {
    let (mut world, front, back) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                front,
                point(2, 1),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                back,
                point(2, 1),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let initial = world.resident_material_inventory().unwrap();
    world
        .edit(|editor| editor.transport_path(front, &[point(1, 1), point(2, 1), point(3, 1)]))
        .unwrap();
    assert_eq!(world.resident_material_inventory().unwrap(), initial);
    assert!(matches!(
        world.cell(front, point(2, 1)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
}

#[test]
fn pr6_receipt_source_is_distinguished_from_conservative_transport() {
    let (mut world, front, _) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let journal = MaterialReceiptJournal {
        key: key("test:journal"),
        schema: key("test:receipt-schema"),
    };
    let log = MaterialReceiptLog::new(&world, journal.key, 4).unwrap();
    world
        .edit(|editor| {
            editor.set_state(journal.key, journal.schema, &log)?;
            editor.apply_material_changes(
                front,
                journal,
                MaterialChangeReason {
                    key: key("test:source-rule"),
                    version: 1,
                    kind: MaterialReceiptKind::Source,
                },
                &[MaterialCellChange {
                    point: point(4, 1),
                    expected: CellCore::AIR,
                    output: CellCore {
                        material: sand,
                        ..CellCore::AIR
                    },
                    payload: MaterialPayloadPolicy::ResetChannels,
                }],
            )?;
            Ok(())
        })
        .unwrap();
    let inventory = world.resident_material_inventory().unwrap();
    assert_eq!(amount(&inventory, "test:sand").mass, 7);
    let log = world.state::<MaterialReceiptLog>(journal.key).unwrap();
    log.validate_world(&world).unwrap();
    assert_eq!(log.receipts().count(), 1);
}

#[derive(Default)]
struct MemoryArchives(BTreeMap<[u8; 32], Vec<u8>>);

impl ArchiveStorage for MemoryArchives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndDirectories)
    }

    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        let value = &self.0[&hash];
        if value.len() > limit {
            return Err(EngineError::capacity(
                "archive読込byte数",
                value.len() as u64,
                limit as u64,
            ));
        }
        Ok(value.clone())
    }
}

#[test]
fn pr10_archive_is_explicitly_excluded_and_restored_without_loss() {
    let (mut world, front, _) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                front,
                point(2, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let handle = world
        .edit(|editor| editor.emit_material_particle(front, point(1, 1), [0, 0]))
        .unwrap();
    let initial = world.resident_material_inventory().unwrap();
    assert_eq!(amount(&initial, "test:sand").total_tokens, 2);

    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let mut archive = MemoryArchives::default();
    let stored = world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archive)
        .unwrap();
    world.edit(|editor| editor.finish_eviction(stored)).unwrap();
    let evacuated = world.resident_material_inventory().unwrap();
    assert_eq!(evacuated.archived_chunks, 1);
    assert_eq!(evacuated.archived_particles, 1);
    assert_eq!(amount(&evacuated, "test:sand").total_tokens, 0);
    let full = world
        .material_inventory_with_archives(&mut archive, 1, 4_000_000)
        .unwrap();
    assert_eq!(full.archived_groups, 1);
    let sand_full = full
        .materials
        .iter()
        .find(|item| item.material == key("test:sand"))
        .unwrap();
    assert_eq!(sand_full.archived_cell_tokens, 1);
    assert_eq!(sand_full.archived_particle_tokens, 1);
    assert_eq!(sand_full.total_tokens, 2);
    assert_eq!(sand_full.nominal_mass, 14);
    assert!(
        world
            .material_inventory_with_archives(&mut archive, 0, 4_000_000)
            .is_err()
    );
    assert!(
        world
            .material_inventory_with_archives(&mut archive, 1, 1)
            .is_err()
    );

    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let loaded = restored
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 4_000_000)
        .unwrap()
        .load(&mut archive)
        .unwrap();
    restored
        .edit(|editor| editor.restore_region(loaded))
        .unwrap();
    assert!(!restored.material_particle(handle).unwrap().is_archived());
    assert_eq!(restored.resident_material_inventory().unwrap(), initial);
}

#[test]
fn archive_material_remap_is_accounted_at_current_epoch_without_rewriting_archive() {
    let (mut world, front, _) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            editor.set_cell(
                front,
                point(2, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    let handle = world
        .edit(|editor| editor.emit_material_particle(front, point(1, 1), [0, 0]))
        .unwrap();
    let mut archive = MemoryArchives::default();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let receipt = world
        .prepare_eviction(region, 4_000_000)
        .unwrap()
        .store(&mut archive)
        .unwrap();
    world
        .edit(|editor| editor.finish_eviction(receipt))
        .unwrap();
    let before = world
        .material_inventory_with_archives(&mut archive, 1, 4_000_000)
        .unwrap();
    assert_eq!(
        before.materials.iter().map(|m| m.total_tokens).sum::<u64>(),
        2
    );
    world
        .edit(|editor| {
            editor.remap_materials(MaterialRemap {
                owner: key("engine:terrain"),
                layers: vec![key("test:front")],
                mappings: vec![(
                    MaterialHandle(key("test:sand")),
                    MaterialHandle(key("test:water")),
                )],
            })?;
            Ok(())
        })
        .unwrap();
    let expected = world
        .material_inventory_with_archives(&mut archive, 1, 4_000_000)
        .unwrap();
    let sand = expected
        .materials
        .iter()
        .find(|m| m.material == key("test:sand"))
        .unwrap();
    let water = expected
        .materials
        .iter()
        .find(|m| m.material == key("test:water"))
        .unwrap();
    assert_eq!(sand.archived_particle_tokens, 1);
    assert_eq!(sand.archived_cell_tokens, 0);
    assert_eq!(water.archived_cell_tokens, 1);
    assert_eq!(sand.total_tokens + water.total_tokens, 2);
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 4_000_000)
        .unwrap()
        .load(&mut archive)
        .unwrap();
    world.edit(|editor| editor.restore_region(loaded)).unwrap();
    assert!(!world.material_particle(handle).unwrap().is_archived());
    let restored = world.resident_material_inventory().unwrap();
    assert_eq!(amount(&restored, "test:sand").total_tokens, 1);
    assert_eq!(amount(&restored, "test:water").total_tokens, 1);
}
