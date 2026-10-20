//! 公開APIで、全表現の実量・archive検証・理由付き収支と失敗時の原子性を検査します。
use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::*,
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
use std::{collections::BTreeMap, sync::Arc};
fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn error() -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, "試験の所有状態")
}
#[derive(Clone)]
struct OwnedCell {
    point: CellPoint,
    layer: LayerId,
    material: MaterialHandle,
    epoch: u64,
    mixture: MixtureRef,
    energy: u64,
}
#[derive(Clone, Default)]
struct OwnedCells(Vec<OwnedCell>);
impl StateCodec for OwnedCells {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.0.len() as u32)?;
        for cell in &self.0 {
            cell.point.encode(out)?;
            cell.layer.encode(out)?;
            cell.material.0.encode(out)?;
            out.u64(cell.epoch)?;
            cell.mixture.encode(out)?;
            out.u64(cell.energy)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let count = input.u32()? as usize;
        if count > input.remaining() / 40 {
            return Err(error());
        }
        let mut cells = Vec::new();
        for _ in 0..count {
            cells.push(OwnedCell {
                point: CellPoint::decode(input)?,
                layer: LayerId::decode(input)?,
                material: MaterialHandle(StableKey::decode(input)?),
                epoch: input.u64()?,
                mixture: MixtureRef::decode(input)?,
                energy: input.u64()?,
            });
        }
        Ok(Self(cells))
    }
}
struct Provider {
    name: StableKey,
    record: StableKey,
    block: bool,
    version: u32,
}
impl Provider {
    fn collect(
        &self,
        cells: OwnedCells,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        for cell in cells.0 {
            output.add(
                cell.layer,
                cell.material,
                cell.epoch,
                cell.mixture,
                MaterialQuantity::ONE,
            )?;
        }
        Ok(())
    }
}
impl MaterialInventoryProvider for Provider {
    fn key(&self) -> StableKey {
        self.name
    }
    fn version(&self) -> u32 {
        self.version
    }
    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        if self.block {
            return Ok(());
        }
        self.collect(world.state(self.record)?, output)
    }
    fn collect_block(
        &self,
        block: MaterialInventoryBlock<'_>,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        if !self.block {
            return Ok(());
        }
        match block.state(self.record) {
            Ok(cells) => self.collect(cells, output),
            Err(error) if error.code == ErrorCode::MissingKey => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// body providerと異なるcallback実装で、reservoir所有を独立に収集します。
struct DistinctReservoirProvider {
    version: u32,
}
impl MaterialInventoryProvider for DistinctReservoirProvider {
    fn key(&self) -> StableKey {
        key("test:distinct-reservoir-provider")
    }
    fn version(&self) -> u32 {
        self.version
    }
    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        let cells = world.state::<OwnedCells>(key("test:reservoir"))?;
        if cells.0.iter().any(|cell| cell.energy == u64::MAX) {
            return Err(error());
        }
        for cell in cells.0 {
            output.add(
                cell.layer,
                cell.material,
                cell.epoch,
                cell.mixture,
                MaterialQuantity::ONE,
            )?;
        }
        Ok(())
    }
}

/// Q04で別Providerの最終監査失敗を、複合World取引へ注入します。
struct FaultGuardProvider;
impl MaterialInventoryProvider for FaultGuardProvider {
    fn key(&self) -> StableKey {
        key("test:fault-guard-provider")
    }
    fn version(&self) -> u32 {
        1
    }
    fn collect_world(
        &self,
        world: &World,
        _output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        let body = world.state::<OwnedCells>(key("test:body"))?;
        if body.0.iter().any(|cell| cell.energy == u64::MAX) {
            return Err(error());
        }
        Ok(())
    }
}

#[derive(Default)]
struct Archives {
    values: BTreeMap<[u8; 32], Vec<u8>>,
    reads: usize,
}
impl ArchiveStorage for Archives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        self.values.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndDirectories)
    }
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        self.reads += 1;
        let bytes = self.values.get(&hash).ok_or_else(error)?;
        if bytes.len() > limit {
            return Err(error());
        }
        Ok(bytes.clone())
    }
}
fn fixture(log_bytes: usize) -> (World, LayerId, MaterialInventoryProfile, Archives) {
    fixture_with_profile(log_bytes, profile(1, 1_000_000))
}

fn fixture_with_profile(
    log_bytes: usize,
    profile: MaterialInventoryProfile,
) -> (World, LayerId, MaterialInventoryProfile, Archives) {
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
    for (name, mass) in [
        ("test:a", 5),
        ("test:b", 3),
        ("test:c", 5),
        ("test:d", 3),
        ("test:carrier", 4),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, MaterialBase::Liquid)
                    .unwrap()
                    .mass_per_cell(mass),
            )
            .unwrap();
    }
    registry
        .register(
            MaterialBuilder::new("test:chest", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1)
                .mass_per_cell(10),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:chest-remapped", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1)
                .mass_per_cell(10),
        )
        .unwrap();
    registry
        .schema(Schema::of::<MaterialReceiptLog>(key("test:cell-log"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<OwnedCells>(key("test:owned"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialBalanceLog>(key("test:balance-schema"), 1, log_bytes).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MixtureStore>(key("test:mixtures"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MixtureRef>(key("test:reference"), 1, 32).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:mixture"),
                version: 1,
                value_schema: key("test:reference"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MixtureRef::NONE,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
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
            edit.set_state(key("test:body"), key("test:owned"), &OwnedCells::default())?;
            edit.set_state(
                key("test:reservoir"),
                key("test:owned"),
                &OwnedCells::default(),
            )
        })
        .unwrap();
    (world, layer, profile, Archives::default())
}
fn profile(version: u32, atoms: usize) -> MaterialInventoryProfile {
    MaterialInventoryProfile::new(
        key("test:inventory"),
        1,
        vec![MaterialInventoryMixture {
            layer: key("test:layer"),
            channel: key("test:mixture"),
        }],
        vec![
            Arc::new(Provider {
                name: key("test:body-provider"),
                record: key("test:body"),
                block: false,
                version,
            }),
            Arc::new(DistinctReservoirProvider { version }),
            Arc::new(Provider {
                name: key("test:block-provider"),
                record: key("test:block-owned"),
                block: true,
                version,
            }),
            Arc::new(BodyLocalGridInventoryProvider::new().unwrap()),
            Arc::new(FaultGuardProvider),
        ],
        MaterialInventoryLimits {
            atoms,
            components: 64,
        },
    )
    .unwrap()
}
fn cell(world: &World, material: &str) -> CellCore {
    CellCore {
        material: world
            .content()
            .material_id(MaterialHandle(key(material)))
            .unwrap(),
        ..CellCore::AIR
    }
}
fn owned(layer: LayerId, material: &str, mixture: MixtureRef, count: i32) -> OwnedCells {
    OwnedCells(
        (0..count)
            .map(|x| OwnedCell {
                point: point(x, 0),
                layer,
                material: MaterialHandle(key(material)),
                epoch: 0,
                mixture,
                energy: 900,
            })
            .collect(),
    )
}
fn save(world: &World) -> Vec<u8> {
    world.checkpoint(world.checkpoint_limit().unwrap()).unwrap()
}
fn restore(world: &World) -> World {
    let bytes = save(world);
    World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        bytes.len(),
    )
    .unwrap()
}
fn reason(kind: MaterialReceiptKind) -> MaterialChangeReason {
    MaterialChangeReason {
        key: key("test:rule"),
        version: 1,
        kind,
    }
}
fn init(
    world: &mut World,
    profile: &MaterialInventoryProfile,
    cache: &MaterialArchiveInventory,
    capacity: u32,
) {
    world
        .initialize_material_accounting(
            profile,
            cache,
            key("test:balance"),
            key("test:balance-schema"),
            capacity,
        )
        .unwrap();
}
fn total(report: &MaterialInventoryReport, name: &str) -> MaterialQuantity {
    report
        .quantities()
        .iter()
        .find(|amount| amount.material == key(name))
        .map_or(MaterialQuantity::ZERO, |amount| amount.tokens)
}

#[test]
fn all_real_representations_and_multichunk_archive_are_counted_once_and_remapped() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let b = cell(&world, "test:b");
    world
        .edit(|edit| {
            edit.set_cell(layer, point(1, 1), a)?;
            edit.set_cell(layer, point(2, 1), b)?;
            edit.emit_material_particle(layer, point(2, 1), [1, 2])?;
            edit.set_state(
                key("test:body"),
                key("test:owned"),
                &owned(layer, "test:a", MixtureRef::NONE, 1),
            )?;
            edit.set_state(
                key("test:reservoir"),
                key("test:owned"),
                &owned(layer, "test:b", MixtureRef::NONE, 2),
            )?;
            let block = edit.place_block(
                layer,
                point(5, 1),
                MaterialHandle(key("test:chest")),
                Orientation::Original,
            )?;
            edit.set_block_state(
                block,
                key("test:block-owned"),
                key("test:owned"),
                &owned(layer, "test:a", MixtureRef::NONE, 1),
            )
        })
        .unwrap();
    let mut body_grid = BodyLocalGrid::new(
        1,
        1,
        1.0,
        [0.5, 0.5],
        0,
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 128,
        },
    )
    .unwrap();
    body_grid
        .set_cell(
            0,
            0,
            BodyLocalCell::new(key("test:layer"), key("test:a"), 0, 0, 0, Vec::new())
                .unwrap()
                .with_tokens(MaterialQuantity::new(2, 1).unwrap())
                .unwrap(),
        )
        .unwrap();
    world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                body_grid,
            )
        })
        .unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    let baseline = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(
        total(&baseline, "test:a"),
        MaterialQuantity::new(5, 1).unwrap()
    );
    assert_eq!(
        total(&baseline, "test:b"),
        MaterialQuantity::new(3, 1).unwrap()
    );
    assert_eq!(
        total(&baseline, "test:chest"),
        MaterialQuantity::new(2, 1).unwrap()
    );
    let region = ChunkRegion::new(ChunkCoord::new(0, 0).unwrap(), 2, 1).unwrap();
    let stored = world
        .prepare_eviction(region, 8_000_000)
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert!(world.material_inventory(&profile, &cache).is_err());
    storage.reads = 0;
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    assert_eq!(storage.reads, 1);
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
    let before_remap = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Conversion)),
            |edit| {
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:terrain"),
                    layers: Vec::new(),
                    mappings: vec![
                        (MaterialHandle(key("test:a")), MaterialHandle(key("test:c"))),
                        (MaterialHandle(key("test:b")), MaterialHandle(key("test:d"))),
                    ],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:blocks"),
                    layers: Vec::new(),
                    mappings: vec![(
                        MaterialHandle(key("test:chest")),
                        MaterialHandle(key("test:chest-remapped")),
                    )],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:particles"),
                    layers: Vec::new(),
                    mappings: vec![(MaterialHandle(key("test:b")), MaterialHandle(key("test:d")))],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("test:body-provider"),
                    layers: Vec::new(),
                    mappings: vec![(MaterialHandle(key("test:a")), MaterialHandle(key("test:c")))],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:body-local-grid-inventory"),
                    layers: Vec::new(),
                    mappings: vec![(MaterialHandle(key("test:a")), MaterialHandle(key("test:c")))],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("test:distinct-reservoir-provider"),
                    layers: Vec::new(),
                    mappings: vec![(MaterialHandle(key("test:b")), MaterialHandle(key("test:d")))],
                })?;
                edit.remap_materials(MaterialRemap {
                    owner: key("test:block-provider"),
                    layers: Vec::new(),
                    mappings: vec![(MaterialHandle(key("test:a")), MaterialHandle(key("test:c")))],
                })
            },
        )
        .unwrap();
    let remapped = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(total(&remapped, "test:a"), MaterialQuantity::ZERO);
    assert_eq!(total(&remapped, "test:b"), MaterialQuantity::ZERO);
    assert_eq!(total(&remapped, "test:chest"), MaterialQuantity::ZERO);
    assert_eq!(total(&remapped, "test:c"), total(&before_remap, "test:a"));
    assert_eq!(total(&remapped, "test:d"), total(&before_remap, "test:b"));
    assert_eq!(
        total(&remapped, "test:chest-remapped"),
        total(&before_remap, "test:chest")
    );
    let mut restored = restore(&world);
    let restored_archives = restored
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    assert_eq!(
        restored
            .verify_material_accounting(&profile, &restored_archives, key("test:balance"))
            .unwrap(),
        remapped
    );
    let loaded = restored
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut storage)
        .unwrap();
    restored.edit(|edit| edit.restore_region(loaded)).unwrap();
    let resident_archive_inventory = restored
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    assert_eq!(
        restored
            .verify_material_accounting(&profile, &resident_archive_inventory, key("test:balance"),)
            .unwrap(),
        remapped
    );
}

#[test]
fn exact_mixture_components_survive_every_owner_and_archive_without_carrier_duplication() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let carrier = cell(&world, "test:carrier");
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    let mixture = MixtureState::new(&[
        MixturePart {
            material: key("test:a"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:b"),
            ratio: 1,
        },
    ])
    .unwrap();
    world
        .edit(|edit| {
            edit.set_state(MixtureStore::key(), key("test:mixtures"), &store)?;
            let reference = edit.register_mixture(key("test:mixtures"), &mixture)?;
            for x in [1, 2] {
                edit.set_cell(layer, point(x, 1), carrier)?;
                edit.set_channel(key("test:mixture"), point(x, 1), reference)?;
            }
            edit.emit_material_particle(layer, point(2, 1), [0, 0])?;
            edit.set_state(
                key("test:body"),
                key("test:owned"),
                &owned(layer, "test:carrier", reference, 1),
            )?;
            edit.set_state(
                key("test:reservoir"),
                key("test:owned"),
                &owned(layer, "test:carrier", reference, 1),
            )?;
            let block = edit.place_block(
                layer,
                point(5, 1),
                MaterialHandle(key("test:chest")),
                Orientation::Original,
            )?;
            edit.set_block_state(
                block,
                key("test:block-owned"),
                key("test:owned"),
                &owned(layer, "test:carrier", reference, 1),
            )
        })
        .unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    let baseline = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(
        total(&baseline, "test:a"),
        MaterialQuantity::new(5, 2).unwrap()
    );
    assert_eq!(
        total(&baseline, "test:b"),
        MaterialQuantity::new(5, 2).unwrap()
    );
    assert_eq!(total(&baseline, "test:carrier"), MaterialQuantity::ZERO);
    assert_eq!(
        baseline
            .quantities()
            .iter()
            .find(|value| value.material == key("test:a"))
            .unwrap()
            .mass,
        MaterialQuantity::new(25, 2).unwrap()
    );
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    world = restore(&world);
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
}

#[test]
fn source_sink_conversion_and_unrecorded_raw_changes_are_distinguished() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let b = cell(&world, "test:b");
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| {
                edit.set_cell(layer, point(1, 1), a)?;
                edit.set_state(
                    key("test:body"),
                    key("test:owned"),
                    &owned(layer, "test:a", MixtureRef::NONE, 1),
                )
            },
        )
        .unwrap();
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Conversion)),
            |edit| edit.set_cell(layer, point(1, 1), b),
        )
        .unwrap();
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Sink)),
            |edit| {
                edit.set_cell(layer, point(1, 1), CellCore::AIR)?;
                edit.set_state(key("test:body"), key("test:owned"), &OwnedCells::default())
            },
        )
        .unwrap();
    let log: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    assert_eq!(log.receipts().len(), 3);
    assert_eq!(
        log.receipts()[0].produced()[0].tokens,
        MaterialQuantity::new(2, 1).unwrap()
    );
    assert_eq!(log.receipts()[1].consumed()[0].material, key("test:a"));
    assert!(
        world
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .unwrap()
            .quantities()
            .is_empty()
    );
    world = restore(&world);
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    world
        .edit(|edit| edit.set_cell(layer, point(2, 1), a))
        .unwrap();
    assert!(
        world
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .is_err()
    );
    let before = save(&world);
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                Some(reason(MaterialReceiptKind::Source)),
                |edit| edit.set_cell(layer, point(3, 1), a)
            )
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn receipt_capacity_schema_failure_and_wrong_reason_roll_back_the_whole_change() {
    for schema_bytes in [200, 1_000_000] {
        let (mut world, layer, profile, mut storage) = fixture(schema_bytes);
        let a = cell(&world, "test:a");
        let cache = world
            .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
            .unwrap();
        init(&mut world, &profile, &cache, 1);
        let before = save(&world);
        assert!(
            world
                .edit_material_accounted(
                    &profile,
                    &cache,
                    key("test:balance"),
                    key("test:balance-schema"),
                    Some(reason(MaterialReceiptKind::Sink)),
                    |edit| edit.set_cell(layer, point(1, 1), a)
                )
                .is_err()
        );
        assert_eq!(save(&world), before);
        let result = world.edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| edit.set_cell(layer, point(1, 1), a),
        );
        if schema_bytes == 200 {
            assert!(result.is_err());
            assert_eq!(save(&world), before);
        } else {
            result.unwrap();
            let before = save(&world);
            assert!(
                world
                    .edit_material_accounted(
                        &profile,
                        &cache,
                        key("test:balance"),
                        key("test:balance-schema"),
                        Some(reason(MaterialReceiptKind::Source)),
                        |edit| edit.set_cell(layer, point(2, 1), a)
                    )
                    .is_err()
            );
            assert_eq!(save(&world), before);
        }
    }
}

#[test]
fn conservation_mode_protects_inputs_and_generic_writes_cannot_rewind_baseline_or_receipts() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    let old: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    let before = save(&world);
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| edit.set_cell(layer, point(1, 1), a)
            )
            .is_err()
    );
    assert_eq!(save(&world), before);
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| edit.set_cell(layer, point(1, 1), a),
        )
        .unwrap();
    let before = save(&world);
    assert!(
        world
            .edit(|edit| edit.set_state(key("test:balance"), key("test:balance-schema"), &old))
            .is_err()
    );
    assert_eq!(save(&world), before);
    let mut current: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    current.set_capacity(16).unwrap();
    world
        .edit(|edit| edit.set_state(key("test:balance"), key("test:balance-schema"), &current))
        .unwrap();
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    let particle = world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| edit.emit_material_particle(layer, point(1, 1), [0, 0]),
        )
        .unwrap();
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| edit.land_material_particle(particle),
        )
        .unwrap();
    assert_eq!(
        world
            .state::<MaterialBalanceLog>(key("test:balance"))
            .unwrap()
            .receipts()
            .len(),
        1
    );
}

#[test]
fn cache_scope_profile_corruption_and_capacity_fail_closed_without_changing_world() {
    let (mut world, layer, profile_a, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    world
        .edit(|edit| edit.set_cell(layer, point(1, 1), a))
        .unwrap();
    let empty = world
        .load_material_archive_inventory(&profile_a, &mut storage, 8_000_000)
        .unwrap();
    assert!(world.material_inventory(&profile(2, 100), &empty).is_err());
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let before = save(&world);
    assert!(world.material_inventory(&profile_a, &empty).is_err());
    storage.values.values_mut().next().unwrap()[0] ^= 1;
    assert!(
        world
            .load_material_archive_inventory(&profile_a, &mut storage, 8_000_000)
            .is_err()
    );
    assert_eq!(save(&world), before);
}

#[test]
fn rational_difference_and_comparison_do_not_round_or_overflow_cross_products() {
    for a in 0u128..12 {
        for b in 1u128..12 {
            for c in 0u128..12 {
                for d in 1u128..12 {
                    let left = MaterialQuantity::new(a, b).unwrap();
                    let right = MaterialQuantity::new(c, d).unwrap();
                    assert_eq!(left.compare(right), (a * d).cmp(&(c * b)));
                    if a * d >= c * b {
                        assert_eq!(
                            left.checked_sub(right).unwrap(),
                            MaterialQuantity::new(a * d - c * b, b * d).unwrap()
                        );
                    } else {
                        assert!(left.checked_sub(right).is_err());
                    }
                }
            }
        }
    }
    let huge = MaterialQuantity::new(u128::MAX, u128::MAX - 1).unwrap();
    assert_eq!(
        huge.compare(MaterialQuantity::ONE),
        std::cmp::Ordering::Greater
    );
    assert_eq!(huge.checked_sub(huge).unwrap(), MaterialQuantity::ZERO);
}

#[test]
fn changed_capacity_inside_accounted_callback_is_retained() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 1);
    let mut log: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    log.set_capacity(2).unwrap();
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| {
                edit.set_state(key("test:balance"), key("test:balance-schema"), &log)?;
                edit.set_cell(layer, point(1, 1), a)
            },
        )
        .unwrap();
    assert_eq!(
        world
            .state::<MaterialBalanceLog>(key("test:balance"))
            .unwrap()
            .capacity(),
        2
    );
}

#[test]
fn native_cell_journal_is_validated_without_counting_its_history_as_material() {
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
    registry
        .register(
            MaterialBuilder::new("test:a", MaterialBase::Powder)
                .unwrap()
                .mass_per_cell(5),
        )
        .unwrap();
    registry
        .schema(Schema::of::<MaterialReceiptLog>(key("test:cell-log"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialBalanceLog>(key("test:balance-schema"), 1, 1_000_000).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let journal = MaterialReceiptJournal {
        key: key("test:cells"),
        schema: key("test:cell-log"),
    };
    let log = MaterialReceiptLog::new(&world, journal.key, 8).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_state(journal.key, journal.schema, &log)
        })
        .unwrap();
    let profile = MaterialInventoryProfile::new(
        key("test:plain"),
        1,
        vec![],
        vec![],
        MaterialInventoryLimits {
            atoms: 16,
            components: 16,
        },
    )
    .unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut Archives::default(), 1_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    let output = cell(&world, "test:a");
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| {
                edit.apply_material_changes(
                    layer,
                    journal,
                    reason(MaterialReceiptKind::Source),
                    &[MaterialCellChange {
                        point: point(1, 1),
                        expected: CellCore::AIR,
                        output,
                        payload: MaterialPayloadPolicy::ResetChannels,
                    }],
                )
            },
        )
        .unwrap();
    let report = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(total(&report, "test:a"), MaterialQuantity::ONE);
    assert_eq!(
        world
            .state::<MaterialReceiptLog>(journal.key)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        world
            .state::<MaterialBalanceLog>(key("test:balance"))
            .unwrap()
            .receipts()
            .len(),
        1
    );
    let restored = restore(&world);
    restored
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn inventory_limits_apply_to_all_archive_groups_and_current_native_atoms_together() {
    let (mut world, layer, normal, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    world
        .edit(|edit| {
            edit.set_cell(layer, point(1, 1), a)?;
            edit.set_cell(layer, point(65, 1), a)
        })
        .unwrap();
    let cache = world
        .load_material_archive_inventory(&normal, &mut storage, 8_000_000)
        .unwrap();
    assert!(world.material_inventory(&profile(1, 1), &cache).is_err());
    for x in [0, 1] {
        let stored = world
            .prepare_eviction(
                ChunkRegion::single(ChunkCoord::new(x, 0).unwrap()).unwrap(),
                8_000_000,
            )
            .unwrap()
            .store(&mut storage)
            .unwrap();
        world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    }
    let before = save(&world);
    assert!(
        world
            .load_material_archive_inventory(&profile(1, 1), &mut storage, 8_000_000)
            .is_err()
    );
    assert_eq!(save(&world), before);
    let cache = world
        .load_material_archive_inventory(&normal, &mut storage, 8_000_000)
        .unwrap();
    assert_eq!(
        total(
            &world.material_inventory(&normal, &cache).unwrap(),
            "test:a"
        ),
        MaterialQuantity::new(2, 1).unwrap()
    );
}

#[test]
fn raw_archive_material_corruption_is_rejected_even_with_updated_hashes() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let core = CellCore {
        flags: 7,
        variation: 31415,
        ..cell(&world, "test:a")
    };
    world
        .edit(|edit| edit.set_cell(layer, point(1, 1), core))
        .unwrap();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let (&old_hash, payload) = storage.values.iter().next().unwrap();
    let mut payload = payload.clone();
    let mut encoded = Encoder::new(8);
    core.encode(&mut encoded).unwrap();
    let encoded = encoded.finish();
    let offset = payload
        .windows(encoded.len())
        .position(|bytes| bytes == encoded)
        .unwrap();
    payload[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let new_hash = *blake3::hash(&payload).as_bytes();
    storage.values.insert(new_hash, payload);
    let mut root = save(&world);
    let positions: Vec<_> = root
        .windows(32)
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == old_hash).then_some(index))
        .collect();
    assert!(!positions.is_empty());
    for index in positions {
        root[index..index + 32].copy_from_slice(&new_hash);
    }
    let world = World::restore(
        &root,
        Arc::clone(world.content()),
        WorldLimits::default(),
        root.len(),
    )
    .unwrap();
    assert!(
        world
            .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
            .is_err()
    );
}

#[test]
fn provider_failure_and_ignored_callback_errors_preserve_world_and_journal() {
    struct Failing;
    impl MaterialInventoryProvider for Failing {
        fn key(&self) -> StableKey {
            key("test:failing")
        }
        fn version(&self) -> u32 {
            1
        }
        fn collect_world(
            &self,
            _world: &World,
            _out: &mut MaterialInventorySink<'_>,
        ) -> EngineResult<()> {
            Err(error())
        }
    }
    let (mut world, layer, profile_a, mut storage) = fixture(1_000_000);
    let cache = world
        .load_material_archive_inventory(&profile_a, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile_a, &cache, 8);
    let a = cell(&world, "test:a");
    let before = save(&world);
    assert!(
        world
            .edit_material_accounted(
                &profile_a,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                Some(reason(MaterialReceiptKind::Source)),
                |edit| {
                    edit.set_cell(layer, point(1, 1), a)?;
                    let _ = edit.set_cell(layer, point(9999, 9999), a);
                    Ok(())
                }
            )
            .is_err()
    );
    assert_eq!(save(&world), before);
    let failing = MaterialInventoryProfile::new(
        key("test:failing-profile"),
        1,
        vec![],
        vec![Arc::new(Failing)],
        MaterialInventoryLimits {
            atoms: 10,
            components: 10,
        },
    )
    .unwrap();
    let cache = world
        .load_material_archive_inventory(&failing, &mut storage, 8_000_000)
        .unwrap();
    assert!(world.material_inventory(&failing, &cache).is_err());
    assert_eq!(save(&world), before);
}

#[test]
fn remap_of_archived_atoms_uses_original_owner_and_epoch_without_stale_component_totals() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    world
        .edit(|edit| edit.set_cell(layer, point(1, 1), a))
        .unwrap();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Conversion)),
            |edit| {
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:terrain"),
                    layers: vec![],
                    mappings: vec![(MaterialHandle(key("test:a")), MaterialHandle(key("test:b")))],
                })
            },
        )
        .unwrap();
    let report = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(total(&report, "test:a"), MaterialQuantity::ZERO);
    assert_eq!(total(&report, "test:b"), MaterialQuantity::ONE);
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut storage)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    report
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
}

#[test]
fn remap_mixed_carrier_to_air_consumes_resident_and_archive_without_ghost_components() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    let carrier = cell(&world, "test:carrier");
    world
        .edit(|edit| {
            edit.set_state(MixtureStore::key(), key("test:mixtures"), &store)?;
            let reference = edit.register_mixture(
                key("test:mixtures"),
                &MixtureState::new(&[
                    MixturePart {
                        material: key("test:a"),
                        ratio: 1,
                    },
                    MixturePart {
                        material: key("test:b"),
                        ratio: 1,
                    },
                ])?,
            )?;
            for x in [1, 65] {
                edit.set_cell(layer, point(x, 1), carrier)?;
                edit.set_channel(key("test:mixture"), point(x, 1), reference)?;
            }
            Ok(())
        })
        .unwrap();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(1, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    let before = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(total(&before, "test:a"), MaterialQuantity::ONE);
    assert_eq!(total(&before, "test:b"), MaterialQuantity::ONE);
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Sink)),
            |edit| {
                edit.remap_materials(MaterialRemap {
                    owner: key("engine:terrain"),
                    layers: vec![key("test:layer")],
                    mappings: vec![(
                        MaterialHandle(key("test:carrier")),
                        MaterialHandle(key("engine:air")),
                    )],
                })
            },
        )
        .unwrap();
    let empty = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert!(empty.quantities().is_empty());
    assert!(matches!(
        world.channel_value::<MixtureRef>(key("test:mixture"), point(1, 1)).unwrap(),
        ChannelQuery::Loaded(value) if value == MixtureRef::NONE
    ));
    let loaded = world
        .region_load_task(ChunkCoord::new(1, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut storage)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    let world = restore(&world);
    assert!(
        world
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .unwrap()
            .quantities()
            .is_empty()
    );
    assert!(matches!(
        world.channel_value::<MixtureRef>(key("test:mixture"), point(65, 1)).unwrap(),
        ChannelQuery::Loaded(value) if value == MixtureRef::NONE
    ));
}

#[test]
fn mixture_witness_and_cell_receipt_survive_save_and_global_balance_does_not_double_count_them() {
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let b = cell(&world, "test:b");
    let carrier = cell(&world, "test:carrier");
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    let journal = MaterialReceiptJournal {
        key: key("test:cells"),
        schema: key("test:cell-log"),
    };
    let log = MaterialReceiptLog::new(&world, journal.key, 8).unwrap();
    let reference = world
        .edit(|edit| {
            edit.set_state(MixtureStore::key(), key("test:mixtures"), &store)?;
            edit.set_state(journal.key, journal.schema, &log)?;
            edit.set_cell(layer, point(1, 1), a)?;
            edit.set_cell(layer, point(2, 1), b)?;
            edit.register_mixture(
                key("test:mixtures"),
                &MixtureState::new(&[
                    MixturePart {
                        material: key("test:a"),
                        ratio: 1,
                    },
                    MixturePart {
                        material: key("test:b"),
                        ratio: 1,
                    },
                ])?,
            )
        })
        .unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 8);
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| {
                edit.apply_mixture_changes(
                    layer,
                    MixtureEditConfig {
                        schema: key("test:mixtures"),
                        channel: key("test:mixture"),
                        journal,
                    },
                    reason(MaterialReceiptKind::Conversion),
                    MixtureConservation::PreserveComponents,
                    &[
                        MixtureCellChange {
                            cell: MaterialCellChange {
                                point: point(1, 1),
                                expected: a,
                                output: carrier,
                                payload: MaterialPayloadPolicy::KeepChannels,
                            },
                            expected: MixtureRef::NONE,
                            output: reference,
                        },
                        MixtureCellChange {
                            cell: MaterialCellChange {
                                point: point(2, 1),
                                expected: b,
                                output: carrier,
                                payload: MaterialPayloadPolicy::KeepChannels,
                            },
                            expected: MixtureRef::NONE,
                            output: reference,
                        },
                    ],
                )
            },
        )
        .unwrap();
    let world = restore(&world);
    let report = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(total(&report, "test:a"), MaterialQuantity::ONE);
    assert_eq!(total(&report, "test:b"), MaterialQuantity::ONE);
    assert_eq!(
        world
            .state::<MaterialBalanceLog>(key("test:balance"))
            .unwrap()
            .receipts()
            .len(),
        0
    );
    assert_eq!(
        world
            .state::<MaterialReceiptLog>(journal.key)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        world
            .state::<MixtureStore>(MixtureStore::key())
            .unwrap()
            .receipts()
            .count(),
        1
    );
}

#[test]
fn unrelated_worlds_and_component_capacity_cannot_reuse_a_snapshot_as_an_attestation() {
    let (mut world, layer, profile_a, mut storage) = fixture(1_000_000);
    let cache = world
        .load_material_archive_inventory(&profile_a, &mut storage, 8_000_000)
        .unwrap();
    let mut unrelated = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        Arc::clone(world.content()),
        WorldLimits::default(),
    )
    .unwrap();
    assert!(unrelated.material_inventory(&profile_a, &cache).is_err());
    assert!(
        unrelated
            .initialize_material_accounting(
                &profile_a,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                8
            )
            .is_err()
    );
    let a = cell(&world, "test:a");
    let b = cell(&world, "test:b");
    world
        .edit(|edit| {
            edit.set_cell(layer, point(1, 1), a)?;
            edit.set_cell(layer, point(2, 1), b)
        })
        .unwrap();
    let constrained = MaterialInventoryProfile::new(
        key("test:small"),
        1,
        vec![],
        vec![],
        MaterialInventoryLimits {
            atoms: 10,
            components: 1,
        },
    )
    .unwrap();
    let cache = world
        .load_material_archive_inventory(&constrained, &mut storage, 8_000_000)
        .unwrap();
    let before = save(&world);
    assert!(world.material_inventory(&constrained, &cache).is_err());
    assert_eq!(save(&world), before);
}

#[test]
fn archive_cache_compacts_identical_carriers_without_losing_the_token_count() {
    let (mut world, layer, normal, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    world
        .edit(|edit| {
            for x in 0..32 {
                edit.set_cell(layer, point(x, 1), a)?;
            }
            Ok(())
        })
        .unwrap();
    let stored = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let cache = world
        .load_material_archive_inventory(&normal, &mut storage, 8_000_000)
        .unwrap();
    let report = world.material_inventory(&profile(1, 1), &cache).unwrap();
    assert_eq!(
        total(&report, "test:a"),
        MaterialQuantity::new(32, 1).unwrap()
    );
}

#[test]
fn generation_save_and_load_verify_all_material_before_publishing_a_world() {
    let (mut world, layer, inventory, mut storage) = fixture(1_000_000);
    let a = cell(&world, "test:a");
    let cache = world
        .load_material_archive_inventory(&inventory, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &inventory, &cache, 8);
    world
        .edit_material_accounted(
            &inventory,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            Some(reason(MaterialReceiptKind::Source)),
            |edit| edit.set_cell(layer, point(1, 1), a),
        )
        .unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("accounting-generation-{}", std::process::id()));
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(root).unwrap(),
        SaveLimits::default(),
        3,
    )
    .unwrap();
    let world_profile = WorldSaveProfile {
        scope: key("test:world"),
        profile: key("test:run"),
        plugins: vec![],
    };
    let profile = MaterialAccountingSaveProfile {
        world: &world_profile,
        inventory: &inventory,
        journal: key("test:balance"),
        archive_byte_limit: 8_000_000,
    };
    world
        .save_material_accounted(&mut repository, &mut storage, &profile, 8_000_000)
        .unwrap();
    let expected = save(&world);
    let loaded = World::load_material_accounted(
        &mut repository,
        &mut storage,
        Arc::clone(world.content()),
        &profile,
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    assert_eq!(save(&loaded), expected);
    world
        .edit(|edit| edit.set_cell(layer, point(2, 1), a))
        .unwrap();
    assert!(
        world
            .save_material_accounted(&mut repository, &mut storage, &profile, 8_000_000)
            .is_err()
    );
    let loaded = World::load_material_accounted(
        &mut repository,
        &mut storage,
        Arc::clone(world.content()),
        &profile,
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    assert_eq!(save(&loaded), expected);
    // 通常の保存入口で未記録増分を持つrootを作っても、監査付き読込では公開しません。
    world
        .save(&mut repository, &mut storage, &world_profile, 8_000_000)
        .unwrap();
    assert!(
        World::load_material_accounted(
            &mut repository,
            &mut storage,
            Arc::clone(world.content()),
            &profile,
            WorldLimits::default(),
            8_000_000
        )
        .is_err()
    );
}

#[test]
fn q04_distinct_inventory_providers_exchange_material_in_one_atomic_tick() {
    let profile = MaterialInventoryProfile::new(
        key("test:distinct-provider-inventory"),
        1,
        vec![MaterialInventoryMixture {
            layer: key("test:layer"),
            channel: key("test:mixture"),
        }],
        vec![
            Arc::new(Provider {
                name: key("test:body-provider"),
                record: key("test:body"),
                block: false,
                version: 1,
            }),
            Arc::new(DistinctReservoirProvider { version: 1 }),
            Arc::new(FaultGuardProvider),
        ],
        MaterialInventoryLimits {
            atoms: 128,
            components: 16,
        },
    )
    .unwrap();
    let (mut world, layer, profile, mut storage) = fixture_with_profile(1_000_000, profile);
    world
        .edit(|edit| {
            edit.set_state(
                key("test:body"),
                key("test:owned"),
                &owned(layer, "test:a", MixtureRef::NONE, 1),
            )?;
            edit.set_state(
                key("test:reservoir"),
                key("test:owned"),
                &owned(layer, "test:b", MixtureRef::NONE, 1),
            )
        })
        .unwrap();
    let archive = world
        .load_material_archive_inventory(&profile, &mut storage, 1_000_000)
        .unwrap();
    init(&mut world, &profile, &archive, 8);
    let baseline = world
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    assert_eq!(total(&baseline, "test:a"), MaterialQuantity::ONE);
    assert_eq!(total(&baseline, "test:b"), MaterialQuantity::ONE);

    let before_failure = save(&world);
    let mut invalid_reservoir = owned(layer, "test:a", MixtureRef::NONE, 1);
    invalid_reservoir.0[0].energy = u64::MAX;
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &archive,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    edit.set_state(
                        key("test:body"),
                        key("test:owned"),
                        &owned(layer, "test:b", MixtureRef::NONE, 1),
                    )?;
                    edit.set_state(key("test:reservoir"), key("test:owned"), &invalid_reservoir)
                },
            )
            .is_err(),
        "別実装Providerの最終集計失敗で前段所有権変更も戻る"
    );
    assert_eq!(save(&world), before_failure);

    world
        .edit_material_accounted(
            &profile,
            &archive,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| {
                // 同じ原子候補でbodyのaとreservoirのbを相互に移す。
                edit.set_state(
                    key("test:body"),
                    key("test:owned"),
                    &owned(layer, "test:b", MixtureRef::NONE, 1),
                )?;
                edit.set_state(
                    key("test:reservoir"),
                    key("test:owned"),
                    &owned(layer, "test:a", MixtureRef::NONE, 1),
                )
            },
        )
        .unwrap();
    let after = world
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    baseline.verify_conserved(&after).unwrap();
    assert_eq!(
        world.state::<OwnedCells>(key("test:body")).unwrap().0[0].material,
        MaterialHandle(key("test:b"))
    );
    assert_eq!(
        world.state::<OwnedCells>(key("test:reservoir")).unwrap().0[0].material,
        MaterialHandle(key("test:a"))
    );
}

#[test]
fn q04_scalar_mixture_to_particle_then_archive_preserves_rational_provider_totals() {
    // 専用対象試験を互いに並べるだけでなく、同一Worldと同一収支台帳で
    // scalar輸送→正規粒子所有権へ変換→region退避→World保存復元→復帰を照合。
    let (mut world, layer, profile, mut storage) = fixture(1_000_000);
    let carrier = cell(&world, "test:carrier");
    let components = MixtureState::new(&[
        MixturePart {
            material: key("test:a"),
            ratio: 1,
        },
        MixturePart {
            material: key("test:b"),
            ratio: 3,
        },
    ])
    .unwrap();
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    let reference = world
        .edit(|edit| {
            edit.set_state(MixtureStore::key(), key("test:mixtures"), &store)?;
            let mixture = edit.register_mixture(key("test:mixtures"), &components)?;
            edit.set_cell(layer, point(10, 10), carrier)?;
            edit.set_channel(key("test:mixture"), point(10, 10), mixture)?;
            edit.set_state(
                key("test:body"),
                key("test:owned"),
                &owned(layer, "test:a", MixtureRef::NONE, 1),
            )?;
            edit.set_state(
                key("test:reservoir"),
                key("test:owned"),
                &owned(layer, "test:b", MixtureRef::NONE, 1),
            )?;
            Ok(mixture)
        })
        .unwrap();
    let cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    init(&mut world, &profile, &cache, 16);
    let baseline = world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    assert_eq!(
        total(&baseline, "test:a"),
        MaterialQuantity::new(9, 7).unwrap()
    );
    assert_eq!(
        total(&baseline, "test:b"),
        MaterialQuantity::new(13, 7).unwrap()
    );
    assert_eq!(total(&baseline, "test:carrier"), MaterialQuantity::ZERO);
    let mass_of = |name: &str| {
        baseline
            .quantities()
            .iter()
            .find(|value| value.material == key(name))
            .unwrap()
            .mass
    };
    assert_eq!(mass_of("test:a"), MaterialQuantity::new(45, 7).unwrap());
    assert_eq!(mass_of("test:b"), MaterialQuantity::new(39, 7).unwrap());

    // 異なる所有Provider間の交換後も、後続の粒子化・archive復帰まで
    // 同じWorld・同じ有理量baselineを保つ。
    world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| {
                edit.set_state(
                    key("test:body"),
                    key("test:owned"),
                    &owned(layer, "test:b", MixtureRef::NONE, 1),
                )?;
                edit.set_state(
                    key("test:reservoir"),
                    key("test:owned"),
                    &owned(layer, "test:a", MixtureRef::NONE, 1),
                )
            },
        )
        .unwrap();
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();

    let lease = world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("test:q04-active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    let before = save(&world);
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    // 実際にMixtureRefを追従させる合法な移動が先行しても、
                    // 後続の容量不足でcell/channel/台帳が丸ごと戻る。
                    edit.transport_path(layer, &[point(10, 11), point(10, 10)])?;
                    edit.transport_materials(layer, 0).map(|_| ())
                },
            )
            .is_err()
    );
    assert_eq!(
        save(&world),
        before,
        "容量不足で取引前の混合・台帳まで巻き戻す"
    );
    let before_provider_failure = save(&world);
    let mut provider_fault = world.state::<OwnedCells>(key("test:body")).unwrap();
    provider_fault.0[0].energy = u64::MAX;
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    // scalar移送と正規粒子化の後、別Providerの最終監査を失敗させる。
                    edit.transport_materials(layer, 16)?;
                    edit.emit_material_particle(layer, point(10, 11), [0, 0])?;
                    edit.set_state(key("test:body"), key("test:owned"), &provider_fault)?;
                    Ok(())
                },
            )
            .is_err(),
        "作者Providerの監査失敗を取引全体の失敗へ伝播する"
    );
    assert_eq!(
        save(&world),
        before_provider_failure,
        "Provider失敗でscalar・粒子・所有record・台帳を一括rollbackする"
    );
    let report = world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| edit.transport_materials(layer, 16),
        )
        .unwrap();
    assert_eq!(report.exchanges, 1);
    assert!(matches!(
        world.channel_value::<MixtureRef>(key("test:mixture"), point(10, 11)).unwrap(),
        ChannelQuery::Loaded(value) if value == reference
    ));
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
    let particle = world
        .edit_material_accounted(
            &profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            None,
            |edit| edit.emit_material_particle(layer, point(10, 11), [0, 0]),
        )
        .unwrap();
    assert_eq!(
        world.material_particle(particle).unwrap().cell(),
        point(10, 11)
    );
    let before_particle_failure = save(&world);
    assert!(
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    edit.set_particle_velocity(particle, [65536, 0])?;
                    // 粒子更新が済んだ後にresident外の編集を失敗させる。
                    edit.set_cell(layer, point(4096, 4096), carrier)
                },
            )
            .is_err()
    );
    assert_eq!(save(&world), before_particle_failure);
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();

    world.edit(|edit| edit.release_lease(lease)).unwrap();
    let archive = world
        .prepare_eviction(
            ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
            8_000_000,
        )
        .unwrap()
        .store(&mut storage)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(archive)).unwrap();
    let archive_cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &archive_cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
    world = restore(&world);
    // checkpoint後の派生archive cacheは正規状態ではなく、
    // 元storageを再検証してWorld/Content/hashとの整合を取り直す。
    let archive_hash = *storage.values.keys().next().unwrap();
    let archive_bytes = storage.values.remove(&archive_hash).unwrap();
    assert!(
        world
            .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
            .is_err(),
        "Worldのcheckpointが正常でも、欠落した外部archiveをcache扱いしない"
    );
    storage.values.insert(archive_hash, archive_bytes);
    let reads_before = storage.reads;
    let restored_cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    assert!(storage.reads > reads_before);
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &restored_cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 8_000_000)
        .unwrap()
        .load(&mut storage)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    let resident_cache = world
        .load_material_archive_inventory(&profile, &mut storage, 8_000_000)
        .unwrap();
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &resident_cache, key("test:balance"))
                .unwrap(),
        )
        .unwrap();
}

#[test]
fn accounting_rejects_material_mixture_channel_with_non_none_default() {
    // 独立旧PR #12の検証を保持する。World全域監査へ渡す混合参照
    // channelの既定値がNONE以外なら、未確保cellを純物質と解釈できない。
    let mut encoded = Encoder::new(64);
    Some(WorldId::from_parts(0, 1).unwrap())
        .encode(&mut encoded)
        .unwrap();
    encoded.u32(1).unwrap();
    let bytes = encoded.finish();
    let mut decoder = Decoder::new(&bytes, bytes.len()).unwrap();
    let illegal_default = MixtureRef::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    assert_ne!(illegal_default, MixtureRef::NONE);

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
    registry
        .register(MaterialBuilder::new("test:a", MaterialBase::Liquid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MixtureRef>(key("test:reference"), 1, 32).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:mixture"),
                version: 1,
                value_schema: key("test:reference"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: illegal_default,
                limits: GridLimits {
                    chunks: 4,
                    checkpoint_bytes: 500_000,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let profile = MaterialInventoryProfile::new(
        key("test:inventory"),
        1,
        vec![MaterialInventoryMixture {
            layer: key("test:layer"),
            channel: key("test:mixture"),
        }],
        Vec::new(),
        MaterialInventoryLimits {
            atoms: 16,
            components: 4,
        },
    )
    .unwrap();
    assert!(
        world
            .load_material_archive_inventory(&profile, &mut Archives::default(), 500_000)
            .is_err()
    );
}
