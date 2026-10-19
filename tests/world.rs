//! E04の素材、占有、負座標、多layerと原子的編集を公開APIから検査します。

use charon_engine::{
    foundation::{
        EngineResult, ErrorCode, StableKey, grid::CellPoint, id::WorldId,
        transaction::CandidateState,
    },
    world::*,
};
use std::sync::Arc;

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}
fn registry() -> ContentRegistry {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    for (name, role) in [
        ("game:front", LayerRole::Terrain),
        ("game:wall", LayerRole::Background),
    ] {
        registry
            .layer(LayerDef {
                key: key(name),
                role,
                render_order: 0,
                collision_group: 1,
                simulated: role == LayerRole::Terrain,
            })
            .unwrap();
    }
    registry
}
fn content(mode: BreakMode, shape: BlockShape) -> Arc<Content> {
    let mut registry = registry();
    registry
        .register(
            MaterialBuilder::new("game:stone", MaterialBase::Solid)
                .unwrap()
                .block_size(3, 2)
                .break_mode(mode)
                .shape(shape),
        )
        .unwrap();
    registry.freeze().unwrap()
}
fn world(content: Arc<Content>) -> World {
    World::new(
        WorldId::from_parts(0, 1).unwrap(),
        42,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            blocks: 16,
            resident_chunks: 16,
            active_chunks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn loaded(world: &World, layer: LayerId, point: CellPoint) -> CellView {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(value) => value,
        _ => panic!("読込済みcellが必要"),
    }
}
fn four_chunks(world: &mut World) {
    world
        .edit(|edit| {
            for x in -1..=0 {
                for y in -1..=0 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn content_ids_are_independent_of_registration_order_and_reject_invalid_definitions() {
    let make = |names: [&str; 2]| {
        let mut registry = registry();
        for name in names {
            registry
                .register(MaterialBuilder::new(name, MaterialBase::Powder).unwrap())
                .unwrap();
        }
        registry.freeze().unwrap()
    };
    let a = make(["game:z", "game:a"]);
    let b = make(["game:a", "game:z"]);
    for name in ["game:a", "game:z"] {
        assert_eq!(
            a.material_id(MaterialHandle(key(name))).unwrap(),
            b.material_id(MaterialHandle(key(name))).unwrap()
        );
    }
    let mut registry = registry();
    assert!(
        registry
            .register(
                MaterialBuilder::new("game:bad", MaterialBase::Solid)
                    .unwrap()
                    .block_size(0, 1)
            )
            .is_err()
    );
    assert!(
        registry
            .register(MaterialBuilder::new("engine:air", MaterialBase::Solid).unwrap())
            .is_err()
    );
    assert!(
        registry
            .register(
                MaterialBuilder::new("game:bad", MaterialBase::Solid)
                    .unwrap()
                    .shape(BlockShape::OccupancyMask(vec![false]))
            )
            .is_err()
    );
    assert!(a.material_id(MaterialHandle(key("game:missing"))).is_err());
    assert_eq!(size_of::<CellCore>(), 8);
}

#[test]
fn world_queries_distinguish_boundary_unavailable_and_loaded_air_without_allocating_the_world() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let bounds = WorldBounds::new(point(-70, -1), 100, 100).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        9,
        bounds,
        content,
        WorldLimits {
            blocks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    assert_eq!(world.resident_chunks(), 0);
    assert!(matches!(
        world.cell(front, point(-71, 0)).unwrap(),
        CellQuery::WorldBoundary
    ));
    assert!(matches!(
        world.cell(front, point(-70, 0)).unwrap(),
        CellQuery::Unavailable(_)
    ));
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(-2, 0)?))
        .unwrap();
    assert_eq!(loaded(&world, front, point(-70, 0)).core, CellCore::AIR);
    assert_eq!(
        ChunkCoord::at(point(-1, -65)),
        ChunkCoord::new(-1, -2).unwrap()
    );
    assert!(ChunkCoord::new(i32::MAX, 0).is_err());
    assert_eq!(
        ChunkCoord::at(point(i32::MAX, i32::MIN)).origin().unwrap(),
        point(i32::MAX - 63, i32::MIN)
    );
}

#[test]
fn rotated_blocks_cross_chunk_boundaries_and_layers_without_overwriting_each_other() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let wall = content.layer_id(key("game:wall")).unwrap();
    let mut world = world(content);
    four_chunks(&mut world);
    let a = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(-1, -1),
                MaterialHandle(key("game:stone")),
                Orientation::Right,
            )
        })
        .unwrap();
    let b = world
        .edit(|edit| {
            edit.place_block(
                wall,
                point(-1, -1),
                MaterialHandle(key("game:stone")),
                Orientation::Original,
            )
        })
        .unwrap();
    assert_ne!(a.id, b.id);
    for y in -1..=1 {
        for x in -1..=0 {
            assert_eq!(loaded(&world, front, point(x, y)).block, Some(a));
        }
    }
    assert_eq!(loaded(&world, wall, point(1, 0)).block, Some(b));
    assert_eq!(world.block(a).unwrap().remaining_cells(), 6);
    world.validate().unwrap();
}

#[test]
fn failed_or_ignored_edit_is_atomic_and_does_not_consume_block_ids() {
    let content = content(BreakMode::Block, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(content);
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let revision = world.revision();
    let bytes = world.terrain_bytes();
    assert!(
        world
            .edit(|edit| {
                let _ = edit.place_block(
                    front,
                    point(63, 0),
                    MaterialHandle(key("game:stone")),
                    Orientation::Original,
                );
                Ok(())
            })
            .is_err()
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.terrain_bytes(), bytes);
    assert_eq!(world.block_count(), 0);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:stone")),
                Orientation::Original,
            )
        })
        .unwrap();
    assert_eq!(block.id.index(), 0);
    assert_eq!(block.id.generation(), 1);
}

#[test]
fn partial_and_whole_block_breaking_preserve_references_and_retire_stale_handles() {
    for mode in [BreakMode::Cell, BreakMode::Block] {
        let content = content(mode, BlockShape::Full);
        let front = content.layer_id(key("game:front")).unwrap();
        let mut world = world(content);
        four_chunks(&mut world);
        let block = world
            .edit(|edit| {
                edit.place_block(
                    front,
                    point(-1, 0),
                    MaterialHandle(key("game:stone")),
                    Orientation::Original,
                )
            })
            .unwrap();
        world
            .edit(|edit| edit.break_at(front, point(-1, 0)))
            .unwrap();
        assert_eq!(loaded(&world, front, point(-1, 0)).core, CellCore::AIR);
        if mode == BreakMode::Cell {
            assert_eq!(world.block(block).unwrap().remaining_cells(), 5);
            for y in 0..2 {
                for x in -1..2 {
                    world
                        .edit(|edit| edit.break_at(front, point(x, y)))
                        .unwrap();
                }
            }
        }
        assert_eq!(world.block(block).err().unwrap().code, ErrorCode::StaleId);
        assert_eq!(world.block_count(), 0);
        world.validate().unwrap();
    }
}

#[test]
fn shapes_use_occupancy_independently_from_their_rectangular_footprint() {
    for shape in [
        BlockShape::Half,
        BlockShape::Triangle,
        BlockShape::Platform,
        BlockShape::OccupancyMask(vec![true, false, false, false, false, true]),
    ] {
        let content = content(BreakMode::Cell, shape);
        let front = content.layer_id(key("game:front")).unwrap();
        let mut world = world(content);
        four_chunks(&mut world);
        let block = world
            .edit(|edit| {
                edit.place_block(
                    front,
                    point(0, 0),
                    MaterialHandle(key("game:stone")),
                    Orientation::Left,
                )
            })
            .unwrap();
        let count = world.block(block).unwrap().remaining_cells();
        assert!(count > 0 && count < 6);
        world.validate().unwrap();
    }
}

#[test]
fn terrain_capacity_and_callback_failure_leave_the_original_world_unchanged() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            terrain_bytes: 2 * 4096 * size_of::<CellCore>(),
            blocks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let revision = world.revision();
    assert!(
        world
            .edit(|edit| edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:stone")),
                Orientation::Original
            ))
            .is_err()
    );
    assert_eq!(world.block_count(), 0);
    assert_eq!(world.revision(), revision);
    let result: EngineResult<()> = world.edit(|edit| {
        edit.break_at(front, point(0, 0))?;
        Err(charon_engine::foundation::EngineError::new(
            ErrorCode::Conflict,
            "作者の取引失敗",
        ))
    });
    assert!(result.is_err());
    assert_eq!(world.revision(), revision);
}

#[test]
fn custom_geometry_requires_registered_validation_and_keeps_exact_record() {
    let mut registry = registry();
    let builder = || {
        MaterialBuilder::new("game:shape", MaterialBase::Solid)
            .unwrap()
            .shape(BlockShape::Custom {
                provider: key("game:curves"),
                version: 1,
                occupancy: vec![true],
                data: vec![8, 9],
            })
    };
    assert_eq!(
        registry.register(builder()).err().unwrap().code,
        ErrorCode::MissingDependency
    );
    registry
        .register_geometry(key("game:curves"), |version, data| {
            if version == 1 && data == [8, 9] {
                Ok(())
            } else {
                Err(charon_engine::foundation::EngineError::new(
                    ErrorCode::SchemaMismatch,
                    "曲線record",
                ))
            }
        })
        .unwrap();
    let handle = registry.register(builder()).unwrap();
    let content = registry.freeze().unwrap();
    assert!(
        matches!(content.material(content.material_id(handle).unwrap()).unwrap().unwrap().shape(),BlockShape::Custom { data, .. } if data == &[8,9])
    );
}

#[test]
fn lease_union_is_atomic_and_distinct_from_visibility_and_residency() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let mut world = world(content);
    four_chunks(&mut world);
    let coord = ChunkCoord::new(-1, 0).unwrap();
    let region = ChunkRegion::single(coord).unwrap();
    let load = world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:loader"),
                region,
                mode: LeaseMode::LoadOnly,
                expires: None,
            })
        })
        .unwrap();
    assert_eq!(
        world.region_state(region, 1).unwrap()[0].status,
        RegionStatus::ResidentInactive
    );
    let active = world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:factory"),
                region,
                mode: LeaseMode::Simulate,
                expires: Some(2),
            })
        })
        .unwrap();
    assert_eq!(world.active_chunks().collect::<Vec<_>>(), vec![coord]);
    world.edit(|edit| edit.advance_tick()).unwrap();
    assert!(world.lease(active).is_ok());
    world.edit(|edit| edit.advance_tick()).unwrap();
    assert_eq!(world.lease(active).err().unwrap().code, ErrorCode::StaleId);
    assert_eq!(world.active_chunks().count(), 0);
    assert!(world.lease(load).is_ok());
    assert_eq!(world.resident_chunks(), 4);
    let revision = world.revision();
    let unavailable = ChunkRegion::new(ChunkCoord::new(0, 0).unwrap(), 2, 1).unwrap();
    assert!(
        world
            .edit(|edit| edit.acquire_lease(RegionLease {
                source: key("game:missing"),
                region: unavailable,
                mode: LeaseMode::Simulate,
                expires: None
            }))
            .is_err()
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.active_chunks().count(), 0);
    world.edit(|edit| edit.release_lease(load)).unwrap();
    assert!(world.lease(load).is_err());
}

#[test]
fn overlapping_activity_requests_count_their_union_and_report_partial_coverage() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            active_chunks: 1,
            blocks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    four_chunks(&mut world);
    let a = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    for name in ["game:a", "game:b"] {
        world
            .edit(|edit| {
                edit.acquire_lease(RegionLease {
                    source: key(name),
                    region: a,
                    mode: LeaseMode::Simulate,
                    expires: None,
                })
            })
            .unwrap();
    }
    assert_eq!(world.active_chunks().count(), 1);
    let b = ChunkRegion::single(ChunkCoord::new(-1, 0).unwrap()).unwrap();
    let revision = world.revision();
    assert_eq!(
        world
            .edit(|edit| edit.acquire_lease(RegionLease {
                source: key("game:c"),
                region: b,
                mode: LeaseMode::Simulate,
                expires: None
            }))
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.active_chunks().count(), 1);
    let coverage = world
        .region_state(
            ChunkRegion::new(ChunkCoord::new(0, 0).unwrap(), 2, 1).unwrap(),
            2,
        )
        .unwrap();
    assert_eq!(coverage[0].status, RegionStatus::Active);
    assert_eq!(coverage[1].status, RegionStatus::Unavailable);
    assert!(world.region_state(a, 0).is_err());
}

struct TerrainPass;
impl GenerationPass for TerrainPass {
    fn key(&self) -> StableKey {
        key("game:terrain")
    }
    fn version(&self) -> u32 {
        1
    }
    fn dependencies(&self) -> &[StableKey] {
        &[]
    }
    fn configuration(&self) -> &[u8] {
        b"sample-ground-v1"
    }
    fn generate(
        &self,
        input: &GenerationInput<'_>,
        output: &mut ChunkDraft<'_>,
    ) -> EngineResult<()> {
        let layer = input.content.layer_id(key("game:front"))?;
        let material = input
            .content
            .material_id(MaterialHandle(key("game:stone")))?;
        let origin = input.coordinate.origin()?;
        // 地形を絶対座標から計算します。chunkの生成順や共有乱数には依存しません。
        for y in 0..64 {
            for x in 0..64 {
                let point = point(origin.x + x, origin.y + y);
                if input.bounds.contains(point) && point.y >= 0 {
                    let variation = input.anchor_seed(key("game:variation"), point) as u16;
                    output.set_cell(
                        layer,
                        point,
                        CellCore {
                            material,
                            flags: 0,
                            variation,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}
fn pipeline() -> Arc<GenerationPipeline> {
    GenerationPipeline::new(key("game:generator"), 1, vec![Arc::new(TerrainPass)], 4096).unwrap()
}

#[test]
fn generated_chunks_wait_for_explicit_publication_and_ignore_worker_completion_order() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let pipeline = pipeline();
    let make = |reverse: bool| {
        let mut world = world(Arc::clone(&content));
        let mut requests = Vec::new();
        for x in -1..=0 {
            requests.push(
                world
                    .edit(|edit| {
                        edit.request_generation(
                            key("game:explore"),
                            ChunkRegion::single(ChunkCoord::new(x, 0)?)?,
                            &pipeline,
                            0,
                        )
                    })
                    .unwrap(),
            );
        }
        let tasks = requests
            .iter()
            .map(|request| world.edit(|edit| edit.begin_generation(*request)).unwrap())
            .collect::<Vec<_>>();
        let mut results = tasks
            .into_iter()
            .map(|task| {
                let pipeline = Arc::clone(&pipeline);
                std::thread::spawn(move || task.run(&pipeline))
                    .join()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        if reverse {
            results.reverse();
        }
        for result in results {
            world.edit(|edit| edit.submit_generation(result)).unwrap();
        }
        assert_eq!(world.resident_chunks(), 0);
        assert!(world.edit(|edit| edit.advance_tick()).is_err());
        for request in requests {
            world.edit(|edit| edit.publish_generation(request)).unwrap();
        }
        world.edit(|edit| edit.advance_tick()).unwrap();
        world.validate().unwrap();
        world
    };
    let a = make(false);
    let b = make(true);
    for x in -64..64 {
        for y in 0..64 {
            assert_eq!(
                loaded(&a, front, point(x, y)).core,
                loaded(&b, front, point(x, y)).core
            );
        }
    }
    assert_eq!(a.resident_chunks(), 2);
    assert_eq!(a.tick(), 1);
}

#[test]
fn cancelled_and_reused_generation_requests_reject_late_worker_results() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let pipeline = pipeline();
    let mut world = world(content);
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let old = world
        .edit(|edit| edit.request_generation(key("game:a"), region, &pipeline, 0))
        .unwrap();
    let task = world.edit(|edit| edit.begin_generation(old)).unwrap();
    world.edit(|edit| edit.cancel_generation(old)).unwrap();
    world.edit(|edit| edit.forget_generation(old)).unwrap();
    let next = world
        .edit(|edit| edit.request_generation(key("game:b"), region, &pipeline, 0))
        .unwrap();
    assert_eq!(old.id.index(), next.id.index());
    assert_ne!(old.id.generation(), next.id.generation());
    let revision = world.revision();
    assert!(
        world
            .edit(|edit| edit.submit_generation(task.run(&pipeline)))
            .is_err()
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world.request(next).unwrap().status(),
        RequestStatus::Requested
    );
    assert_eq!(world.resident_chunks(), 0);
}

#[test]
fn generation_reserves_the_union_before_workers_start_and_does_not_overwrite_edits() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let pipeline = pipeline();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            resident_chunks: 1,
            active_chunks: 1,
            blocks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let request = world
        .edit(|edit| edit.request_generation(key("game:a"), region, &pipeline, 0))
        .unwrap();
    assert_eq!(world.reserved_terrain().unwrap().0, 1);
    let revision = world.revision();
    assert!(
        world
            .edit(|edit| edit.request_generation(
                key("game:b"),
                ChunkRegion::single(ChunkCoord::new(1, 0)?)?,
                &pipeline,
                0
            ))
            .is_err()
    );
    assert!(
        world
            .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
            .is_err()
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world.request(request).unwrap().status(),
        RequestStatus::Requested
    );
}

#[test]
fn missing_generation_version_produces_a_failed_request_instead_of_empty_terrain() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let pipeline = pipeline();
    let mut world = world(content);
    let request = world
        .edit(|edit| {
            edit.request_generation(
                key("game:a"),
                ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                &pipeline,
                0,
            )
        })
        .unwrap();
    let task = world.edit(|edit| edit.begin_generation(request)).unwrap();
    let wrong =
        GenerationPipeline::new(key("game:generator"), 2, vec![Arc::new(TerrainPass)], 4096)
            .unwrap();
    world
        .edit(|edit| edit.submit_generation(task.run(&wrong)))
        .unwrap();
    assert_eq!(
        world.request(request).unwrap().status(),
        RequestStatus::Failed
    );
    assert_eq!(
        world.request(request).unwrap().failure().unwrap().code,
        ErrorCode::MissingDependency
    );
    assert!(world.edit(|edit| edit.publish_generation(request)).is_err());
    assert_eq!(world.resident_chunks(), 0);
}

#[test]
fn world_snapshot_preserves_blocks_leases_allocator_and_ready_generation() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let pipeline = pipeline();
    let mut original = world(Arc::clone(&content));
    four_chunks(&mut original);
    let block = original
        .edit(|edit| {
            edit.place_block(
                front,
                point(-1, -1),
                MaterialHandle(key("game:stone")),
                Orientation::Right,
            )
        })
        .unwrap();
    original
        .edit(|edit| edit.break_at(front, point(-1, -1)))
        .unwrap();
    let lease = original
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:factory"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: Some(4),
            })
        })
        .unwrap();
    let request = original
        .edit(|edit| {
            edit.request_generation(
                key("game:explore"),
                ChunkRegion::single(ChunkCoord::new(1, 0)?)?,
                &pipeline,
                0,
            )
        })
        .unwrap();
    let task = original
        .edit(|edit| edit.begin_generation(request))
        .unwrap();
    original
        .edit(|edit| edit.submit_generation(task.run(&pipeline)))
        .unwrap();
    let before = original.canonical_hash(2_000_000).unwrap();
    let bytes = original
        .checkpoint(original.checkpoint_limit().unwrap())
        .unwrap();
    assert_eq!(bytes.len(), original.checkpoint_limit().unwrap());
    let mut restored = World::restore(
        &bytes,
        Arc::clone(&content),
        WorldLimits::default(),
        2_000_000,
    )
    .unwrap();
    assert_eq!(before, restored.canonical_hash(2_000_000).unwrap());
    assert_eq!(restored.block(block).unwrap().remaining_cells(), 5);
    assert_eq!(restored.lease(lease).unwrap().mode, LeaseMode::Simulate);
    assert_eq!(
        restored.request(request).unwrap().status(),
        RequestStatus::Ready
    );
    original
        .edit(|edit| edit.publish_generation(request))
        .unwrap();
    restored
        .edit(|edit| edit.publish_generation(request))
        .unwrap();
    for value in [&mut original, &mut restored] {
        value.edit(|edit| edit.advance_tick()).unwrap();
        value
            .edit(|edit| edit.break_at(front, point(0, 0)))
            .unwrap();
    }
    assert_eq!(
        original.canonical_hash(2_000_000).unwrap(),
        restored.canonical_hash(2_000_000).unwrap()
    );
}

#[test]
fn bounded_world_restore_rejects_truncation_unknown_content_and_capacity_without_touching_source() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let mut world = world(Arc::clone(&content));
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let bytes = world.checkpoint(1_000_000).unwrap();
    let hash = world.canonical_hash(1_000_000).unwrap();
    let mut cuts = (0..200)
        .chain((200..bytes.len()).step_by(701))
        .chain(bytes.len() - 20..bytes.len())
        .collect::<Vec<_>>();
    cuts.sort();
    cuts.dedup();
    for cut in cuts {
        assert!(
            World::restore(
                &bytes[..cut],
                Arc::clone(&content),
                WorldLimits::default(),
                1_000_000
            )
            .is_err(),
            "切断位置{cut}"
        );
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(
        World::restore(
            &extra,
            Arc::clone(&content),
            WorldLimits::default(),
            1_000_000
        )
        .is_err()
    );
    let other = registry().freeze().unwrap();
    assert_eq!(
        World::restore(&bytes, other, WorldLimits::default(), 1_000_000)
            .err()
            .unwrap()
            .code,
        ErrorCode::MissingDependency
    );
    assert!(
        World::restore(
            &bytes,
            Arc::clone(&content),
            WorldLimits {
                blocks: 1,
                ..WorldLimits::default()
            },
            1_000_000
        )
        .is_err()
    );
    assert_eq!(world.canonical_hash(1_000_000).unwrap(), hash);
    use charon_engine::foundation::codec::{Decoder, StateCodec};
    assert!(Option::<u32>::decode(&mut Decoder::new(&[2], 1).unwrap()).is_err());
}

struct SaveDirectory(std::path::PathBuf);
impl SaveDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = format!(
            "charon-e04-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for SaveDirectory {
    fn drop(&mut self) {
        // 試験専用の絶対pathだけを対象にします。リンク等へ置換されていれば削除しません。
        if self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("charon-e04-")
            && std::fs::symlink_metadata(&self.0)
                .is_ok_and(|v| v.is_dir() && !v.file_type().is_symlink())
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn metadata(tick: u64) -> SaveMetadata {
    SaveMetadata {
        profile: key("game:profile"),
        content_hash: [1; 32],
        tick,
        plugins: vec![(key("game:plugin"), 1)],
    }
}
fn records(tick: u64) -> Vec<SaveRecord> {
    vec![SaveRecord {
        scope: key("game:world"),
        kind: key("engine:world"),
        version: 1,
        bytes: tick.to_le_bytes().to_vec(),
    }]
}
struct FaultStorage {
    inner: DirectoryStorage,
    count: usize,
    fail: Option<usize>,
    partial: bool,
}
impl FaultStorage {
    fn check(&mut self) -> EngineResult<()> {
        self.count += 1;
        if self.fail == Some(self.count) {
            Err(charon_engine::foundation::EngineError::new(
                ErrorCode::Io,
                "注入した保存I/O失敗",
            ))
        } else {
            Ok(())
        }
    }
}
impl GenerationStorage for FaultStorage {
    fn lock(&mut self) -> EngineResult<()> {
        self.check()?;
        self.inner.lock()
    }
    fn unlock(&mut self) {
        self.inner.unlock()
    }
    fn generations(&mut self, limit: usize) -> EngineResult<Vec<SaveGeneration>> {
        self.check()?;
        self.inner.generations(limit)
    }
    fn begin(&mut self, generation: u64) -> EngineResult<()> {
        self.check()?;
        self.inner.begin(generation)
    }
    fn write_staged(
        &mut self,
        generation: u64,
        object: SaveObject,
        bytes: &[u8],
    ) -> EngineResult<()> {
        self.check()?;
        if self.partial {
            self.partial = false;
            self.inner
                .write_staged(generation, object, &bytes[..bytes.len() / 2])?;
            return Err(charon_engine::foundation::EngineError::new(
                ErrorCode::Io,
                "途中writeの失敗",
            ));
        }
        self.inner.write_staged(generation, object, bytes)
    }
    fn sync_staged(&mut self, generation: u64) -> EngineResult<SaveDurability> {
        self.check()?;
        self.inner.sync_staged(generation)
    }
    fn publish(&mut self, generation: u64) -> EngineResult<()> {
        self.check()?;
        self.inner.publish(generation)
    }
    fn read(&mut self, generation: u64, object: SaveObject, limit: usize) -> EngineResult<Vec<u8>> {
        self.check()?;
        self.inner.read(generation, object, limit)
    }
    fn publish_current(&mut self, generation: u64) -> EngineResult<()> {
        self.check()?;
        self.inner.publish_current(generation)
    }
    fn remove(&mut self, generation: u64) -> EngineResult<()> {
        self.check()?;
        self.inner.remove(generation)
    }
    fn discard_staged(&mut self, generation: u64) -> EngineResult<()> {
        self.check()?;
        self.inner.discard_staged(generation)
    }
}
#[test]
fn immutable_save_generations_keep_three_valid_backups_and_ignore_broken_current() {
    let directory = SaveDirectory::new();
    let mut repo = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits::default(),
        3,
    )
    .unwrap();
    for tick in 1..=5 {
        assert_eq!(
            repo.save(&metadata(tick), records(tick))
                .unwrap()
                .generation,
            tick
        );
    }
    let mut generations = repo.storage_mut().generations(10).unwrap();
    generations.sort_by_key(|v| v.number);
    assert_eq!(
        generations.iter().map(|v| v.number).collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    std::fs::write(directory.0.join("CURRENT"), b"broken-current").unwrap();
    assert_eq!(repo.load_latest().unwrap().metadata.tick, 5);
    let newest = directory.0.join("gen-00000000000000000005");
    for file in std::fs::read_dir(newest).unwrap() {
        let file = file.unwrap();
        if file.path().extension().is_some_and(|v| v == "blob") {
            std::fs::write(file.path(), b"corrupt").unwrap();
        }
    }
    assert_eq!(repo.load_latest().unwrap().metadata.tick, 4);
}
#[test]
fn every_save_operation_failure_and_partial_write_preserve_the_previous_valid_generation() {
    for fail in 1..=25 {
        let directory = SaveDirectory::new();
        let storage = FaultStorage {
            inner: DirectoryStorage::new(&directory.0).unwrap(),
            count: 0,
            fail: None,
            partial: false,
        };
        let mut repo = SaveRepository::new(storage, SaveLimits::default(), 3).unwrap();
        repo.save(&metadata(1), records(1)).unwrap();
        repo.storage_mut().count = 0;
        repo.storage_mut().fail = Some(fail);
        let _ = repo.save(&metadata(2), records(2));
        repo.storage_mut().fail = None;
        assert_eq!(
            repo.load_generation(1).unwrap().metadata.tick,
            1,
            "失敗点{fail}"
        );
        assert!([1, 2].contains(&repo.load_latest().unwrap().metadata.tick));
    }
    let directory = SaveDirectory::new();
    let storage = FaultStorage {
        inner: DirectoryStorage::new(&directory.0).unwrap(),
        count: 0,
        fail: None,
        partial: false,
    };
    let mut repo = SaveRepository::new(storage, SaveLimits::default(), 3).unwrap();
    repo.save(&metadata(1), records(1)).unwrap();
    repo.storage_mut().partial = true;
    assert!(repo.save(&metadata(2), records(2)).is_err());
    assert_eq!(repo.load_latest().unwrap().metadata.tick, 1);
    assert_eq!(repo.save(&metadata(3), records(3)).unwrap().generation, 3);
}
#[test]
fn independent_writers_cannot_publish_concurrently_and_read_limits_are_enforced() {
    let directory = SaveDirectory::new();
    let mut a = DirectoryStorage::new(&directory.0).unwrap();
    let mut b = DirectoryStorage::new(&directory.0).unwrap();
    a.lock().unwrap();
    assert_eq!(b.lock().unwrap_err().code, ErrorCode::Conflict);
    a.unlock();
    b.lock().unwrap();
    b.unlock();
    let mut repo = SaveRepository::new(a, SaveLimits::default(), 3).unwrap();
    repo.save(&metadata(1), records(1)).unwrap();
    assert!(repo.storage_mut().read(1, SaveObject::Root, 1).is_err());
    assert!(
        repo.save(
            &metadata(2),
            vec![SaveRecord {
                scope: key("game:x"),
                kind: key("engine:world"),
                version: 0,
                bytes: vec![]
            }]
        )
        .is_err()
    );
    assert_eq!(repo.load_latest().unwrap().metadata.tick, 1);
}

fn remap_content() -> Arc<Content> {
    let mut registry = registry();
    for (name, size, base) in [
        ("game:stone", [3, 2], MaterialBase::Solid),
        ("game:sand", [3, 2], MaterialBase::Powder),
        ("game:tiny", [1, 1], MaterialBase::Solid),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, base)
                    .unwrap()
                    .block_size(size[0], size[1]),
            )
            .unwrap();
    }
    registry.freeze().unwrap()
}
fn remap(owner: &str, source: &str, target: &str) -> MaterialRemap {
    MaterialRemap {
        owner: key(owner),
        layers: vec![],
        mappings: vec![(MaterialHandle(key(source)), MaterialHandle(key(target)))],
    }
}
#[test]
fn material_epochs_apply_to_resident_and_later_generated_regions_and_survive_restore() {
    let content = remap_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let stone = content
        .material_id(MaterialHandle(key("game:stone")))
        .unwrap();
    let sand = content
        .material_id(MaterialHandle(key("game:sand")))
        .unwrap();
    let mut world = world(Arc::clone(&content));
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:stone")),
                Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(8, 0),
                CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    assert_eq!(
        world
            .edit(|edit| edit.remap_materials(remap("engine:terrain", "game:stone", "game:sand")))
            .unwrap(),
        1
    );
    assert_eq!(loaded(&world, front, point(8, 0)).core.material, sand);
    assert_eq!(world.block(block).unwrap().material(), stone);
    let pipeline = pipeline();
    let request = world
        .edit(|edit| {
            edit.request_generation(
                key("game:explore"),
                ChunkRegion::single(ChunkCoord::new(1, 0)?)?,
                &pipeline,
                0,
            )
        })
        .unwrap();
    let task = world.edit(|edit| edit.begin_generation(request)).unwrap();
    world
        .edit(|edit| edit.submit_generation(task.run(&pipeline)))
        .unwrap();
    world.edit(|edit| edit.publish_generation(request)).unwrap();
    assert_eq!(loaded(&world, front, point(64, 0)).core.material, sand);
    world
        .edit(|edit| edit.remap_materials(remap("engine:blocks", "game:stone", "game:sand")))
        .unwrap();
    assert_eq!(world.block(block).unwrap().material(), sand);
    assert_eq!(loaded(&world, front, point(0, 0)).core.material, sand);
    let bytes = world.checkpoint(2_000_000).unwrap();
    let restored = World::restore(
        &bytes,
        Arc::clone(&content),
        WorldLimits::default(),
        2_000_000,
    )
    .unwrap();
    assert_eq!(restored.material_remaps().epoch(), 2);
    assert_eq!(
        world.canonical_hash(2_000_000).unwrap(),
        restored.canonical_hash(2_000_000).unwrap()
    );
}
#[test]
fn material_remap_failures_preserve_epochs_and_custom_owners_use_the_same_ledger() {
    let content = remap_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(content);
    four_chunks(&mut world);
    world
        .edit(|edit| {
            edit.place_block(
                front,
                point(-1, 0),
                MaterialHandle(key("game:stone")),
                Orientation::Original,
            )
        })
        .unwrap();
    let before = world.canonical_hash(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.remap_materials(remap("engine:blocks", "game:stone", "game:tiny")))
            .is_err()
    );
    let mut duplicate = remap("engine:terrain", "game:stone", "game:sand");
    duplicate.mappings.push(duplicate.mappings[0]);
    assert!(world.edit(|edit| edit.remap_materials(duplicate)).is_err());
    assert_eq!(world.material_remaps().epoch(), 0);
    assert_eq!(world.canonical_hash(2_000_000).unwrap(), before);
    world
        .edit(|edit| edit.remap_materials(remap("game:liquid", "game:stone", "game:sand")))
        .unwrap();
    assert_eq!(
        world
            .material_remaps()
            .resolve(
                key("game:liquid"),
                key("game:front"),
                0,
                MaterialHandle(key("game:stone"))
            )
            .unwrap(),
        MaterialHandle(key("game:sand"))
    );
    assert_eq!(
        world
            .material_remaps()
            .resolve(
                key("game:liquid"),
                key("game:front"),
                1,
                MaterialHandle(key("game:stone"))
            )
            .unwrap(),
        MaterialHandle(key("game:stone"))
    );
}

struct FailedArchive;
impl ArchiveStorage for FailedArchive {
    fn put(&mut self, _: [u8; 32], _: &[u8]) -> EngineResult<SaveDurability> {
        Err(charon_engine::foundation::EngineError::new(
            ErrorCode::Io,
            "注入したarchive保存失敗",
        ))
    }
    fn read(&mut self, _: [u8; 32], _: usize) -> EngineResult<Vec<u8>> {
        unreachable!("保存に失敗したため再読取へ進みません")
    }
}
#[test]
fn eviction_releases_pages_and_restores_cross_chunk_block_ids_after_snapshot_and_remap() {
    let content = remap_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("game:sand")))
        .unwrap();
    let mut world = world(Arc::clone(&content));
    four_chunks(&mut world);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(-1, -1),
                MaterialHandle(key("game:stone")),
                Orientation::Right,
            )
        })
        .unwrap();
    let region = ChunkRegion::new(ChunkCoord::new(-1, -1).unwrap(), 1, 1).unwrap();
    assert!(
        world
            .prepare_eviction(
                ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
                2_000_000
            )
            .is_err()
    );
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(world.resident_chunks(), 0);
    assert_eq!(world.terrain_bytes(), 0);
    assert_eq!(world.block_count(), 1);
    assert_eq!(
        world.block(block).err().unwrap().code,
        ErrorCode::InvalidState
    );
    assert!(matches!(
        world.cell(front, point(0, 0)).unwrap(),
        CellQuery::Unavailable(_)
    ));
    assert!(
        world
            .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
            .is_err()
    );
    let bytes = world.checkpoint(2_000_000).unwrap();
    let mut world = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    world
        .edit(|edit| edit.remap_materials(remap("engine:blocks", "game:stone", "game:sand")))
        .unwrap();
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    assert_eq!(world.resident_chunks(), 0);
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(world.resident_chunks(), 4);
    assert_eq!(world.block(block).unwrap().remaining_cells(), 6);
    assert_eq!(world.block(block).unwrap().material(), sand);
    assert_eq!(loaded_cell_material(&world, front, point(0, 0)), sand);
    world.validate().unwrap();
}
fn loaded_cell_material(world: &World, layer: LayerId, point: CellPoint) -> MaterialId {
    loaded(world, layer, point).core.material
}
#[test]
fn archive_save_failure_and_intervening_edits_never_discard_resident_state() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let stone = content
        .material_id(MaterialHandle(key("game:stone")))
        .unwrap();
    let mut world = world(content);
    four_chunks(&mut world);
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let before = world.canonical_hash(2_000_000).unwrap();
    assert!(
        world
            .prepare_eviction(region, 2_000_000)
            .unwrap()
            .store(&mut FailedArchive)
            .is_err()
    );
    assert_eq!(world.canonical_hash(2_000_000).unwrap(), before);
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(0, 0),
                CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    assert!(world.edit(|edit| edit.finish_eviction(stored)).is_err());
    assert_eq!(world.resident_chunks(), 4);
    // 別regionの編集や正規tickの進行だけで、この保存済み候補を無効にはしません。
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(-1, 0),
                CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    world.edit(|edit| edit.advance_tick()).unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(world.resident_chunks(), 3);
}
#[test]
fn pinned_regions_cannot_be_evicted_and_capacity_failure_cannot_partially_restore() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            resident_chunks: 4,
            active_chunks: 4,
            blocks: 8,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    four_chunks(&mut world);
    let region = ChunkRegion::new(ChunkCoord::new(-1, -1).unwrap(), 1, 1).unwrap();
    let lease = world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:pin"),
                region,
                mode: LeaseMode::Pin,
                expires: None,
            })
        })
        .unwrap();
    assert!(world.prepare_eviction(region, 2_000_000).is_err());
    world.edit(|edit| edit.release_lease(lease)).unwrap();
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    world
        .edit(|edit| {
            for x in 2..6 {
                edit.insert_empty_chunk(ChunkCoord::new(x, 0)?)?;
            }
            Ok(())
        })
        .unwrap();
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    let before = world.canonical_hash(2_000_000).unwrap();
    assert_eq!(
        world
            .edit(|edit| edit.restore_region(loaded))
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(world.canonical_hash(2_000_000).unwrap(), before);
    assert!(
        world
            .archived_chunk(ChunkCoord::new(0, 0).unwrap())
            .is_some()
    );
}

fn channel_content(scale: (u32, u32)) -> Arc<Content> {
    use charon_engine::foundation::{
        grid::{GridLayout, GridLimits, GridScale},
        schema::Schema,
    };
    let mut registry = registry();
    registry
        .schema(Schema::of::<u8>(key("game:liquid-amount"), 1, 1).unwrap())
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("game:liquid"),
            version: 1,
            value_schema: key("game:liquid-amount"),
            layout: GridLayout {
                origin: point(0, 0),
                x_scale: GridScale::new(scale.0, scale.1).unwrap(),
                y_scale: GridScale::new(scale.0, scale.1).unwrap(),
            },
            default: 0u8,
            limits: GridLimits {
                chunks: 16,
                checkpoint_bytes: 1_000_000,
            },
        })
        .unwrap();
    registry.freeze().unwrap()
}
#[test]
fn fractional_channels_preserve_partial_amounts_negative_coordinates_and_world_coverage() {
    let content = channel_content((1, 2));
    let mut world = world(Arc::clone(&content));
    assert!(matches!(
        world
            .channel_value::<u8>(key("game:liquid"), point(-1, -1))
            .unwrap(),
        ChannelQuery::Unavailable(_)
    ));
    assert!(
        world
            .edit(|edit| edit.set_channel(key("game:liquid"), point(-1, -1), 127u8))
            .is_err()
    );
    four_chunks(&mut world);
    world
        .edit(|edit| {
            edit.set_channel(key("game:liquid"), point(-1, -1), 127u8)?;
            edit.set_channel(key("game:liquid"), point(0, 0), 255u8)
        })
        .unwrap();
    assert!(matches!(
        world
            .channel_value::<u8>(key("game:liquid"), point(-1, -1))
            .unwrap(),
        ChannelQuery::Loaded(127)
    ));
    assert!(matches!(
        world
            .channel_value::<u8>(key("game:liquid"), point(1, 0))
            .unwrap(),
        ChannelQuery::Loaded(0)
    ));
    assert_eq!(
        world
            .channel_value::<u16>(key("game:liquid"), point(0, 0))
            .err()
            .unwrap()
            .code,
        ErrorCode::SchemaMismatch
    );
    let batches = world.channel_batches::<u8>(key("game:liquid"), 16).unwrap();
    assert_eq!(batches.len(), 2);
    assert!(batches.iter().all(|batch| batch.values.len() == 4096));
    drop(batches);
    let bytes = world.checkpoint(2_000_000).unwrap();
    let restored = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    assert_eq!(
        world.canonical_hash(2_000_000).unwrap(),
        restored.canonical_hash(2_000_000).unwrap()
    );
}
#[test]
fn region_archives_transfer_channel_values_without_retaining_their_arrays() {
    let content = channel_content((1, 2));
    let mut world = world(content);
    four_chunks(&mut world);
    world
        .edit(|edit| {
            edit.set_channel(key("game:liquid"), point(1, 1), 89u8)?;
            edit.set_channel(key("game:liquid"), point(2, 2), 255u8)
        })
        .unwrap();
    assert!(world.channel_bytes() > 0);
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(world.channel_bytes(), 0);
    assert!(matches!(
        world
            .channel_value::<u8>(key("game:liquid"), point(1, 1))
            .unwrap(),
        ChannelQuery::Unavailable(_)
    ));
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert!(matches!(
        world
            .channel_value::<u8>(key("game:liquid"), point(1, 1))
            .unwrap(),
        ChannelQuery::Loaded(89)
    ));
    world.validate().unwrap();
}
#[test]
fn coarse_channel_coverage_and_byte_budget_cannot_be_bypassed_by_a_single_sample() {
    let content = channel_content((128, 1));
    let mut world = world(Arc::clone(&content));
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    assert!(
        world
            .edit(|edit| edit.set_channel(key("game:liquid"), point(0, 0), 1u8))
            .is_err()
    );
    world
        .edit(|edit| {
            for coord in [(0, 1), (1, 0), (1, 1)] {
                edit.insert_empty_chunk(ChunkCoord::new(coord.0, coord.1)?)?;
            }
            Ok(())
        })
        .unwrap();
    world
        .edit(|edit| edit.set_channel(key("game:liquid"), point(0, 0), 1u8))
        .unwrap();
    assert!(
        world
            .prepare_eviction(
                ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
                2_000_000
            )
            .is_err()
    );
    let mut small = World::new(
        WorldId::from_parts(1, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits {
            blocks: 8,
            channel_bytes: 4095,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    small
        .edit(|edit| {
            for x in 0..2 {
                for y in 0..2 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        small
            .edit(|edit| edit.set_channel(key("game:liquid"), point(0, 0), 1u8))
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(small.channel_bytes(), 0);
}

fn state_content() -> Arc<Content> {
    let mut registry = registry();
    registry
        .schema(
            charon_engine::foundation::schema::Schema::of::<u32>(key("game:number"), 1, 4).unwrap(),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("game:chest", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 2),
        )
        .unwrap();
    registry.freeze().unwrap()
}
#[test]
fn author_state_and_block_components_share_atomic_edits_and_survive_eviction() {
    let content = state_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&content));
    four_chunks(&mut world);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:chest")),
                Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.set_state(key("game:progress"), key("game:number"), &7u32)?;
            edit.set_block_state(block, key("game:items"), key("game:number"), &83u32)
        })
        .unwrap();
    let before = world.canonical_hash(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.set_state(key("game:progress"), key("game:number"), &8u32)?;
                edit.set_block_state(block, key("game:items"), key("game:number"), &1u8)
            })
            .is_err()
    );
    assert_eq!(world.canonical_hash(2_000_000).unwrap(), before);
    let bytes = world.checkpoint(2_000_000).unwrap();
    let mut world = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    assert_eq!(world.state::<u32>(key("game:progress")).unwrap(), 7);
    assert_eq!(
        world.block_state::<u32>(block, key("game:items")).unwrap(),
        83
    );
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    assert_eq!(world.state::<u32>(key("game:progress")).unwrap(), 7);
    assert!(world.block_state::<u32>(block, key("game:items")).is_err());
    let loaded = world
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    world.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        world.block_state::<u32>(block, key("game:items")).unwrap(),
        83
    );
    world.validate().unwrap();
}

#[test]
fn large_edit_jobs_save_their_cursor_and_publish_only_complete_pages() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let stone = content
        .material_id(MaterialHandle(key("game:stone")))
        .unwrap();
    let mut original = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        Arc::clone(&content),
        WorldLimits {
            blocks: 8,
            edit_cells: 2,
            ..WorldLimits::default()
        },
    )
    .unwrap();
    four_chunks(&mut original);
    let edits = (0..64)
        .map(|x| CellEdit {
            layer: front,
            point: point(x, 0),
            value: CellCore {
                material: stone,
                ..CellCore::AIR
            },
        })
        .collect();
    let job = original
        .edit(|edit| edit.submit_cell_job(key("game:explosion"), edits))
        .unwrap();
    assert!(
        original
            .prepare_eviction(
                ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap(),
                2_000_000
            )
            .is_err()
    );
    assert_eq!(
        original.edit(|edit| edit.step_cell_job(job, 7)).unwrap(),
        TerrainJobStatus::Preparing
    );
    assert_eq!(original.terrain_job(job).unwrap().prepared_cells(), 7);
    assert_eq!(loaded(&original, front, point(0, 0)).core, CellCore::AIR);
    let bytes = original
        .checkpoint(original.checkpoint_limit().unwrap())
        .unwrap();
    assert_eq!(bytes.len(), original.checkpoint_limit().unwrap());
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    for world in [&mut original, &mut restored] {
        while world.terrain_job(job).unwrap().status() == TerrainJobStatus::Preparing {
            world.edit(|edit| edit.step_cell_job(job, 11)).unwrap();
        }
        assert_eq!(loaded(world, front, point(63, 0)).core, CellCore::AIR);
        world.edit(|edit| edit.publish_cell_job(job)).unwrap();
        assert_eq!(loaded(world, front, point(63, 0)).core.material, stone);
        assert_eq!(world.job_bytes(), 0);
    }
    assert_eq!(
        original.canonical_hash(2_000_000).unwrap(),
        restored.canonical_hash(2_000_000).unwrap()
    );
}
#[test]
fn job_input_conflicts_and_cancellation_cannot_publish_partial_or_stale_terrain() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let stone = content
        .material_id(MaterialHandle(key("game:stone")))
        .unwrap();
    let mut world = world(content);
    four_chunks(&mut world);
    let make = || {
        vec![
            CellEdit {
                layer: front,
                point: point(0, 0),
                value: CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            },
            CellEdit {
                layer: front,
                point: point(1, 0),
                value: CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            },
        ]
    };
    let job = world
        .edit(|edit| edit.submit_cell_job(key("game:edit"), make()))
        .unwrap();
    world.edit(|edit| edit.step_cell_job(job, 1)).unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                front,
                point(4, 0),
                CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();
    assert_eq!(
        world.edit(|edit| edit.step_cell_job(job, 1)).unwrap(),
        TerrainJobStatus::Invalidated
    );
    assert_eq!(loaded(&world, front, point(0, 0)).core, CellCore::AIR);
    world.edit(|edit| edit.forget_cell_job(job)).unwrap();
    let next = world
        .edit(|edit| edit.submit_cell_job(key("game:edit"), make()))
        .unwrap();
    assert_ne!(job.id.generation(), next.id.generation());
    world.edit(|edit| edit.step_cell_job(next, 2)).unwrap();
    world.edit(|edit| edit.cancel_cell_job(next)).unwrap();
    assert!(world.edit(|edit| edit.publish_cell_job(next)).is_err());
    assert_eq!(loaded(&world, front, point(1, 0)).core, CellCore::AIR);
}

struct NativeWorldSystem {
    fail: bool,
}
impl charon_engine::runtime::schedule::System for NativeWorldSystem {
    fn run(
        &mut self,
        ctx: &mut charon_engine::runtime::schedule::SystemContext<'_>,
    ) -> EngineResult<charon_engine::runtime::schedule::WorkStatus> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("world")?;
        let old = worlds.view().get::<u32>(id, key("game:progress"))?;
        let (front, stone) = {
            let view = worlds.view();
            let world = view.terrain(id)?;
            (
                world.content().layer_id(key("game:front"))?,
                world
                    .content()
                    .material_id(MaterialHandle(key("game:chest")))?,
            )
        };
        worlds.set(id, key("game:progress"), key("game:number"), &(old + 1))?;
        let fail = self.fail;
        worlds.edit_terrain(id, move |edit| {
            edit.set_cell(
                front,
                point(1, 1),
                CellCore {
                    material: stone,
                    ..CellCore::AIR
                },
            )?;
            if fail {
                return Err(charon_engine::foundation::EngineError::new(
                    ErrorCode::Conflict,
                    "注入したWorld更新失敗",
                ));
            }
            Ok(())
        })?;
        Ok(charon_engine::runtime::schedule::WorkStatus::Complete)
    }
}
#[test]
fn runtime_publishes_native_terrain_state_and_tick_atomically_and_exposes_the_full_snapshot() {
    use charon_engine::runtime::{
        Engine, StepResult,
        input::TickInput,
        schedule::{Phase, SystemDescriptor},
    };
    for fail in [false, true] {
        let content = state_content();
        let front = content.layer_id(key("game:front")).unwrap();
        let expected = content
            .material_id(MaterialHandle(key("game:chest")))
            .unwrap();
        let builder = WorldBuilder::new(Arc::clone(&content))
            .limits(WorldLimits {
                blocks: 8,
                ..WorldLimits::default()
            })
            .initialize(|world| {
                world.edit(|edit| {
                    edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                    edit.set_state(key("game:progress"), key("game:number"), &7u32)
                })
            });
        let descriptor = SystemDescriptor::new(key("game:writer"), Phase::Intent)
            .write(key("game:progress"))
            .write(key("engine:terrain"))
            .in_world(key("game:world"))
            .barrier();
        let mut engine = Engine::builder()
            .sandbox_world("world", builder)
            .system(descriptor, NativeWorldSystem { fail })
            .build()
            .unwrap();
        let id = engine.worlds().id("world").unwrap();
        let result = engine.step(TickInput {
            reset: false,
            tick: 1,
            events: vec![],
        });
        if fail {
            assert!(result.is_err());
            assert_eq!(
                engine
                    .worlds()
                    .get::<u32>(id, key("game:progress"))
                    .unwrap(),
                7
            );
            assert_eq!(engine.worlds().tick(id).unwrap(), 0);
            assert_eq!(
                loaded(engine.worlds().terrain(id).unwrap(), front, point(1, 1)).core,
                CellCore::AIR
            );
        } else {
            assert_eq!(result.unwrap(), StepResult::Complete(1));
            assert_eq!(
                engine
                    .worlds()
                    .get::<u32>(id, key("game:progress"))
                    .unwrap(),
                8
            );
            assert_eq!(engine.worlds().terrain(id).unwrap().tick(), 1);
            assert_eq!(
                loaded(engine.worlds().terrain(id).unwrap(), front, point(1, 1))
                    .core
                    .material,
                expected
            );
            let bytes = engine.worlds().checkpoint(id).unwrap();
            let restored =
                World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
            assert_eq!(restored.tick(), 1);
            assert_eq!(restored.state::<u32>(key("game:progress")).unwrap(), 8);
        }
    }
}

#[test]
fn material_anchor_conditions_are_optional_and_validated_before_allocating_instances() {
    let mut registry = registry();
    let front = key("game:front");
    let solid = registry
        .register(MaterialBuilder::new("game:solid", MaterialBase::Solid).unwrap())
        .unwrap();
    let supported = registry
        .register(
            MaterialBuilder::new("game:supported", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1)
                .anchor(AnchorRule::Below {
                    layer: front,
                    mode: SupportMode::All,
                }),
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(front).unwrap();
    let core = CellCore {
        material: content.material_id(solid).unwrap(),
        ..CellCore::AIR
    };
    let mut world = world(content);
    four_chunks(&mut world);
    let revision = world.revision();
    assert!(
        world
            .edit(|edit| edit.place_block(layer, point(0, 0), supported, Orientation::Original))
            .is_err()
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.block_count(), 0);
    world
        .edit(|edit| {
            edit.set_cell(layer, point(0, 1), core)?;
            edit.set_cell(layer, point(1, 1), core)
        })
        .unwrap();
    assert!(
        world
            .edit(|edit| edit.place_block(layer, point(0, 0), supported, Orientation::Original))
            .is_ok()
    );
}
#[test]
fn cache_notifications_report_missed_revisions_and_keep_neighbor_coverage_exact() {
    let content = channel_content((1, 1));
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&content));
    let start = world.revision();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    assert!(
        matches!(world.changes_after(start),ChangeQuery::Delta(change) if change.chunks==vec![ChunkCoord::new(0,0).unwrap()])
    );
    let previous = world.revision();
    world
        .edit(|edit| edit.set_channel(key("game:liquid"), point(0, 0), 12u8))
        .unwrap();
    assert!(
        matches!(world.changes_after(previous),ChangeQuery::Delta(change) if change.chunks.len()==1)
    );
    assert!(matches!(
        world.changes_after(start),
        ChangeQuery::FullRefreshRequired
    ));
    let neighbors = world.neighbors(front, point(0, 0)).unwrap();
    assert!(matches!(neighbors[0], CellQuery::Unavailable(_)));
    assert!(matches!(neighbors[2], CellQuery::Loaded(_)));
    let bytes = world.checkpoint(2_000_000).unwrap();
    let restored = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    assert!(matches!(
        restored.changes_after(previous),
        ChangeQuery::FullRefreshRequired
    ));
}

#[test]
fn camera_observers_replace_only_their_own_sources_and_keep_old_activity_when_loading_is_missing() {
    use charon_engine::presentation::Camera2d;
    let content = content(BreakMode::Cell, BlockShape::Full);
    let mut world = world(content);
    four_chunks(&mut world);
    let margins = ObserverMargins {
        simulation: 0,
        resident: 0,
    };
    let mut camera = Camera2d::screen(64, 64);
    let observation =
        CameraObservation::capture(key("game:camera"), &camera, &world, margins).unwrap();
    world
        .edit(|edit| edit.apply_camera_observation(observation))
        .unwrap();
    world
        .edit(|edit| {
            edit.acquire_lease(RegionLease {
                source: key("game:factory"),
                region: ChunkRegion::single(ChunkCoord::new(-1, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    assert_eq!(world.active_chunks().count(), 2);
    camera.position[0] = 96.0;
    let next = CameraObservation::capture(key("game:camera"), &camera, &world, margins).unwrap();
    assert_eq!(
        next.missing_chunks(&world, 16).unwrap(),
        vec![ChunkCoord::new(1, 0).unwrap()]
    );
    let before = world.canonical_hash(2_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.apply_camera_observation(next))
            .is_err()
    );
    assert_eq!(world.canonical_hash(2_000_000).unwrap(), before);
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(1, 0)?))
        .unwrap();
    world
        .edit(|edit| edit.apply_camera_observation(next))
        .unwrap();
    assert_eq!(
        world.active_chunks().collect::<Vec<_>>(),
        vec![
            ChunkCoord::new(-1, 0).unwrap(),
            ChunkCoord::new(1, 0).unwrap()
        ]
    );
    world
        .edit(|edit| edit.apply_camera_observation(CameraObservation::disabled(key("game:camera"))))
        .unwrap();
    assert_eq!(world.active_chunks().count(), 1);
}

#[test]
fn explicit_schema_migrations_cover_world_block_and_archived_state_without_rewriting_the_source() {
    use charon_engine::foundation::{
        codec::{Decoder, StateCodec},
        schema::Schema,
    };
    let old = state_content();
    let front = old.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&old));
    four_chunks(&mut world);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:chest")),
                Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.set_state(key("game:progress"), key("game:number"), &27u32)?;
            edit.set_block_state(block, key("game:items"), key("game:number"), &91u32)
        })
        .unwrap();
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let bytes = world.checkpoint(2_000_000).unwrap();
    let original_bytes = bytes.clone();
    let mut registry = registry();
    let mut schema = Schema::of::<u64>(key("game:number"), 2, 8).unwrap();
    schema
        .add_migration(1, 2, 4, |bytes, out| {
            let mut input = Decoder::new(bytes, 4)?;
            let value = u32::decode(&mut input)?;
            input.finish()?;
            out.u64(u64::from(value))
        })
        .unwrap();
    registry.schema(schema).unwrap();
    registry
        .register(
            MaterialBuilder::new("game:chest", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 2),
        )
        .unwrap();
    registry.schema_migration_from(Arc::clone(&old)).unwrap();
    let current = registry.freeze().unwrap();
    let mut migrated = World::restore(&bytes, current, WorldLimits::default(), 2_000_000).unwrap();
    assert_eq!(migrated.state::<u64>(key("game:progress")).unwrap(), 27);
    let loaded = migrated
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    migrated.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        migrated
            .block_state::<u64>(block, key("game:items"))
            .unwrap(),
        91
    );
    assert_eq!(bytes, original_bytes);
    let previous = World::restore(&bytes, old, WorldLimits::default(), 2_000_000).unwrap();
    assert_eq!(previous.state::<u32>(key("game:progress")).unwrap(), 27);
}
#[test]
fn schema_compatibility_grants_cannot_hide_a_changed_material_footprint() {
    let old = state_content();
    let mut registry = registry();
    registry
        .schema(
            charon_engine::foundation::schema::Schema::of::<u32>(key("game:number"), 1, 4).unwrap(),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("game:chest", MaterialBase::Solid)
                .unwrap()
                .block_size(3, 2),
        )
        .unwrap();
    registry.schema_migration_from(old).unwrap();
    assert_eq!(registry.freeze().err().unwrap().code, ErrorCode::Conflict);
}

#[test]
fn a_world_save_contains_all_archives_and_can_restore_them_into_a_fresh_paging_store() {
    let content = state_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&content));
    four_chunks(&mut world);
    let block = world
        .edit(|edit| {
            edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:chest")),
                Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| edit.set_block_state(block, key("game:items"), key("game:number"), &67u32))
        .unwrap();
    let paging = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&paging.0).unwrap();
    let region = ChunkRegion::single(ChunkCoord::new(0, 0).unwrap()).unwrap();
    let stored = world
        .prepare_eviction(region, 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    let backup = SaveDirectory::new();
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(&backup.0).unwrap(),
        SaveLimits::default(),
        3,
    )
    .unwrap();
    let profile = WorldSaveProfile {
        scope: key("game:world"),
        profile: key("game:profile"),
        plugins: vec![],
    };
    world
        .save(&mut repository, &mut archives, &profile, 2_000_000)
        .unwrap();
    assert_eq!(repository.latest_manifest().unwrap().records.len(), 2);
    // paging用フォルダが別の空保存先でも、保存rootが全参照payloadを所有しています。
    let recovered = SaveDirectory::new();
    let mut recovered_archives = DirectoryArchives::new(&recovered.0).unwrap();
    let mut restored = World::load(
        &mut repository,
        &mut recovered_archives,
        content,
        &profile,
        WorldLimits::default(),
        2_000_000,
    )
    .unwrap();
    let loaded = restored
        .region_load_task(ChunkCoord::new(0, 0).unwrap(), 2_000_000)
        .unwrap()
        .load(&mut recovered_archives)
        .unwrap();
    restored.edit(|edit| edit.restore_region(loaded)).unwrap();
    assert_eq!(
        restored
            .block_state::<u32>(block, key("game:items"))
            .unwrap(),
        67
    );
}
#[test]
fn an_archive_stream_failure_never_publishes_an_incomplete_world_root() {
    let directory = SaveDirectory::new();
    let mut repo = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits::default(),
        3,
    )
    .unwrap();
    repo.save(&metadata(1), records(1)).unwrap();
    let first = records(2).remove(0);
    let stream = std::iter::once(Ok(first)).chain(std::iter::once(Err(
        charon_engine::foundation::EngineError::new(ErrorCode::Io, "archive streamの読込失敗"),
    )));
    assert!(repo.save_stream(&metadata(2), stream).is_err());
    assert_eq!(repo.latest_manifest().unwrap().metadata.tick, 1);
}

#[test]
fn restarted_generation_rejects_results_from_the_previous_attempt_and_resumes_after_save() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let pipeline = pipeline();
    let mut world = world(Arc::clone(&content));
    let request = world
        .edit(|edit| {
            edit.request_generation(
                key("game:loading"),
                ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                &pipeline,
                0,
            )
        })
        .unwrap();
    let old = world.edit(|edit| edit.begin_generation(request)).unwrap();
    let bytes = world.checkpoint(2_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 2_000_000).unwrap();
    restored
        .edit(|edit| edit.restart_generation(request))
        .unwrap();
    let current = restored
        .edit(|edit| edit.begin_generation(request))
        .unwrap();
    assert_eq!(
        restored
            .edit(|edit| edit.submit_generation(old.run(&pipeline)))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    restored
        .edit(|edit| edit.submit_generation(current.run(&pipeline)))
        .unwrap();
    restored
        .edit(|edit| edit.publish_generation(request))
        .unwrap();
    assert_eq!(restored.resident_chunks(), 1);
}
#[test]
fn save_iterator_panics_release_the_writer_and_keep_a_complete_previous_root() {
    let directory = SaveDirectory::new();
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits::default(),
        3,
    )
    .unwrap();
    repository.save(&metadata(1), records(1)).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stream = std::iter::from_fn(|| -> Option<EngineResult<SaveRecord>> {
            panic!("注入したreader panic")
        });
        let _ = repository.save_stream(&metadata(2), stream);
    }));
    assert!(result.is_err());
    assert_eq!(
        repository
            .save(&metadata(3), records(3))
            .unwrap()
            .generation,
        3
    );
    assert_eq!(repository.load_generation(1).unwrap().metadata.tick, 1);
}

#[test]
fn generated_base_and_sparse_edits_survive_snapshot_and_eviction_without_regeneration() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let front = content.layer_id(key("game:front")).unwrap();
    let mut world = world(Arc::clone(&content));
    let pipeline = pipeline();
    let coord = ChunkCoord::new(0, 0).unwrap();
    let request = world
        .edit(|edit| {
            edit.request_generation(
                key("game:explore"),
                ChunkRegion::single(coord)?,
                &pipeline,
                0,
            )
        })
        .unwrap();
    let result = world
        .edit(|edit| edit.begin_generation(request))
        .unwrap()
        .run(&pipeline);
    world.edit(|edit| edit.submit_generation(result)).unwrap();
    world.edit(|edit| edit.publish_generation(request)).unwrap();
    let before = world.checkpoint(2_000_000).unwrap();
    let base = loaded(&world, front, point(2, 3)).core;
    let bytes_before = world.terrain_bytes();
    world
        .edit(|edit| edit.set_cell(front, point(2, 3), CellCore::AIR))
        .unwrap();
    let after = world.checkpoint(2_000_000).unwrap();
    // 一つの変更はu16 indexと8byte cellだけを増やします。生成baseは共有から独立します。
    assert_eq!(after.len(), before.len() + 10);
    assert_eq!(world.terrain_bytes(), bytes_before + 4096 * 8);
    assert_eq!(
        world.generation_base_cell(front, point(2, 3)).unwrap(),
        Some(base)
    );
    let mut restored = World::restore(&after, content, WorldLimits::default(), 2_000_000).unwrap();
    assert_eq!(restored.checkpoint(2_000_000).unwrap(), after);
    assert_eq!(
        restored.generation_stamp(coord).unwrap().unwrap().version,
        1
    );
    let directory = SaveDirectory::new();
    let mut archives = DirectoryArchives::new(&directory.0).unwrap();
    let receipt = restored
        .prepare_eviction(ChunkRegion::single(coord).unwrap(), 2_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    restored.edit(|edit| edit.finish_eviction(receipt)).unwrap();
    let loaded_region = restored
        .region_load_task(coord, 2_000_000)
        .unwrap()
        .load(&mut archives)
        .unwrap();
    restored
        .edit(|edit| edit.restore_region(loaded_region))
        .unwrap();
    assert_eq!(
        restored.generation_base_cell(front, point(2, 3)).unwrap(),
        Some(base)
    );
    assert_eq!(loaded(&restored, front, point(2, 3)).core, CellCore::AIR);
    // 大半が変わる場合の完全page符号化でも、baseと現在地形の区別は維持します。
    restored
        .edit(|edit| {
            for y in 0..64 {
                for x in 0..64 {
                    edit.set_cell(front, point(x, y), CellCore::AIR)?;
                }
            }
            Ok(())
        })
        .unwrap();
    let bytes = restored.checkpoint(2_000_000).unwrap();
    let again = World::restore(
        &bytes,
        Arc::clone(restored.content()),
        WorldLimits::default(),
        2_000_000,
    )
    .unwrap();
    assert_eq!(
        again.generation_base_cell(front, point(2, 3)).unwrap(),
        Some(base)
    );
    assert_eq!(loaded(&again, front, point(1, 1)).core, CellCore::AIR);
}

#[test]
fn next_tick_generation_waits_for_every_region_and_publishes_with_the_clock() {
    let content = content(BreakMode::Cell, BlockShape::Full);
    let pipeline = pipeline();
    let mut world = world(Arc::clone(&content));
    let mut requests = Vec::new();
    for x in 0..2 {
        requests.push(
            world
                .edit(|edit| {
                    edit.request_generation(
                        key("game:required"),
                        ChunkRegion::single(ChunkCoord::new(x, 0)?)?,
                        &pipeline,
                        1,
                    )
                })
                .unwrap(),
        );
    }
    let unchanged = world.checkpoint(2_000_000).unwrap();
    assert!(world.edit(|edit| edit.advance_tick()).is_err());
    assert_eq!(world.checkpoint(2_000_000).unwrap(), unchanged);
    let mut tasks = Vec::new();
    for request in &requests {
        tasks.push(world.edit(|edit| edit.begin_generation(*request)).unwrap());
    }
    assert!(world.edit(|edit| edit.advance_tick()).is_err());
    let first = tasks.remove(0).run(&pipeline);
    world.edit(|edit| edit.submit_generation(first)).unwrap();
    let half_ready = world.checkpoint(2_000_000).unwrap();
    assert!(world.edit(|edit| edit.advance_tick()).is_err());
    assert_eq!(world.checkpoint(2_000_000).unwrap(), half_ready);
    assert_eq!(world.tick(), 0);
    assert_eq!(world.resident_chunks(), 0);
    world
        .edit(|edit| edit.submit_generation(tasks.remove(0).run(&pipeline)))
        .unwrap();
    // due tickより前の公開は引き続き拒否し、保存したReadyも到達時に一括採用します。
    assert!(
        world
            .edit(|edit| edit.publish_generation(requests[0]))
            .is_err()
    );
    let snapshot = world.checkpoint(2_000_000).unwrap();
    let mut restored =
        World::restore(&snapshot, content, WorldLimits::default(), 2_000_000).unwrap();
    restored.edit(|edit| edit.advance_tick()).unwrap();
    assert_eq!(restored.tick(), 1);
    assert_eq!(restored.resident_chunks(), 2);
    assert!(restored.pending_at(1).next().is_none());
    for request in requests {
        assert_eq!(
            restored.request(request).unwrap().status(),
            RequestStatus::Published
        );
    }
}

#[test]
fn future_generation_is_not_published_early_and_cancelled_requirements_do_not_block() {
    let mut world = world(content(BreakMode::Cell, BlockShape::Full));
    let pipeline = pipeline();
    let ready = world
        .edit(|edit| {
            edit.request_generation(
                key("game:future"),
                ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                &pipeline,
                2,
            )
        })
        .unwrap();
    let cancelled = world
        .edit(|edit| {
            edit.request_generation(
                key("game:cancelled"),
                ChunkRegion::single(ChunkCoord::new(1, 0)?)?,
                &pipeline,
                1,
            )
        })
        .unwrap();
    let result = world
        .edit(|edit| edit.begin_generation(ready))
        .unwrap()
        .run(&pipeline);
    world.edit(|edit| edit.submit_generation(result)).unwrap();
    world
        .edit(|edit| edit.cancel_generation(cancelled))
        .unwrap();
    world.edit(|edit| edit.advance_tick()).unwrap();
    assert_eq!(world.tick(), 1);
    assert_eq!(world.resident_chunks(), 0);
    assert_eq!(world.request(ready).unwrap().status(), RequestStatus::Ready);
    world.edit(|edit| edit.advance_tick()).unwrap();
    assert_eq!(world.tick(), 2);
    assert_eq!(world.resident_chunks(), 1);
}

#[test]
fn save_generation_enumeration_requires_and_reuses_one_spare_slot() {
    let directory = SaveDirectory::new();
    assert!(matches!(
        SaveRepository::new(
            DirectoryStorage::new(&directory.0).unwrap(),
            SaveLimits {
                generations: 3,
                ..SaveLimits::default()
            },
            3
        ),
        Err(charon_engine::foundation::EngineError {
            code: ErrorCode::InvalidValue,
            ..
        })
    ));
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits {
            generations: 4,
            ..SaveLimits::default()
        },
        3,
    )
    .unwrap();
    for tick in 1..=9 {
        let receipt = repository.save(&metadata(tick), records(tick)).unwrap();
        assert_eq!(receipt.generation, tick);
        assert!(receipt.cleanup_errors.is_empty());
        assert_eq!(repository.load_latest().unwrap().metadata.tick, tick);
        assert_eq!(
            repository.storage_mut().generations(4).unwrap().len(),
            (tick as usize).min(3)
        );
    }
}

#[test]
fn block_remap_rejects_changed_shapes_before_any_instance_or_archive_is_modified() {
    for (old_shape, new_shape) in [
        (BlockShape::Platform, BlockShape::Full),
        (BlockShape::Full, BlockShape::Platform),
        (BlockShape::Full, BlockShape::OccupancyMask(vec![true; 4])),
    ] {
        // 未配置・resident・退避中を同じ契約で検査します。矩形寸法の一致だけでは不十分です。
        for state in 0..3 {
            let mut registry = registry();
            for (name, shape) in [
                ("game:old", old_shape.clone()),
                ("game:new", new_shape.clone()),
            ] {
                registry
                    .register(
                        MaterialBuilder::new(name, MaterialBase::Solid)
                            .unwrap()
                            .block_size(2, 2)
                            .shape(shape)
                            .break_mode(BreakMode::Cell),
                    )
                    .unwrap();
            }
            let content = registry.freeze().unwrap();
            let front = content.layer_id(key("game:front")).unwrap();
            let mut world = world(content);
            let coord = ChunkCoord::new(0, 0).unwrap();
            world.edit(|edit| edit.insert_empty_chunk(coord)).unwrap();
            if state > 0 {
                world
                    .edit(|edit| {
                        edit.place_block(
                            front,
                            point(0, 0),
                            MaterialHandle(key("game:old")),
                            Orientation::Original,
                        )?;
                        edit.break_at(front, point(0, 0))?;
                        Ok(())
                    })
                    .unwrap();
            }
            let directory = SaveDirectory::new();
            let mut archives = DirectoryArchives::new(&directory.0).unwrap();
            if state == 2 {
                let receipt = world
                    .prepare_eviction(ChunkRegion::single(coord).unwrap(), 2_000_000)
                    .unwrap()
                    .store(&mut archives)
                    .unwrap();
                world.edit(|edit| edit.finish_eviction(receipt)).unwrap();
            }
            let before = world.checkpoint(2_000_000).unwrap();
            let error = world
                .edit(|edit| edit.remap_materials(remap("engine:blocks", "game:old", "game:new")))
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Conflict);
            assert_eq!(world.material_remaps().epoch(), 0);
            assert_eq!(world.checkpoint(2_000_000).unwrap(), before);
        }
    }
}

#[test]
fn interrupted_staging_is_reclaimed_with_the_minimum_generation_budget() {
    for mode in 0..6 {
        let directory = SaveDirectory::new();
        let limits = SaveLimits {
            generations: 4,
            ..SaveLimits::default()
        };
        let mut repository = SaveRepository::new(
            FaultStorage {
                inner: DirectoryStorage::new(&directory.0).unwrap(),
                count: 0,
                fail: None,
                partial: false,
            },
            limits,
            3,
        )
        .unwrap();
        for tick in 1..=3 {
            repository.save(&metadata(tick), records(tick)).unwrap();
        }
        repository.storage_mut().count = 0;
        match mode {
            0 => {
                let stream = std::iter::once(Err(charon_engine::foundation::EngineError::new(
                    ErrorCode::Io,
                    "reader中断",
                )));
                assert!(repository.save_stream(&metadata(4), stream).is_err());
            }
            1 => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let stream = std::iter::from_fn(|| -> Option<EngineResult<SaveRecord>> {
                        panic!("reader中断")
                    });
                    let _ = repository.save_stream(&metadata(4), stream);
                }));
                assert!(result.is_err());
            }
            2 => {
                repository.storage_mut().partial = true;
                assert!(repository.save(&metadata(4), records(4)).is_err());
            }
            _ => {
                // lock・列挙・beginに続くblob/root/syncの各失敗を検査します。
                repository.storage_mut().fail = Some(mode + 1);
                assert!(repository.save(&metadata(4), records(4)).is_err());
            }
        }
        repository.storage_mut().fail = None;
        assert_eq!(repository.load_latest().unwrap().metadata.tick, 3);
        assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 4);
        if mode == 0 {
            // 回収自体が失敗しても新しい世代を作らず、直前の完全rootを読み続けます。
            repository.storage_mut().count = 0;
            repository.storage_mut().fail = Some(3);
            assert!(repository.save(&metadata(5), records(5)).is_err());
            repository.storage_mut().fail = None;
            assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 4);
            assert_eq!(repository.load_latest().unwrap().metadata.tick, 3);
        }
        drop(repository);
        // 保存先を開き直しても未公開世代を識別でき、上限を増やさず継続します。
        let mut restarted =
            SaveRepository::new(DirectoryStorage::new(&directory.0).unwrap(), limits, 3).unwrap();
        for tick in 5..=9 {
            let receipt = restarted.save(&metadata(tick), records(tick)).unwrap();
            assert!(receipt.cleanup_errors.is_empty());
            assert_eq!(receipt.generation, tick);
            assert_eq!(restarted.load_latest().unwrap().metadata.tick, tick);
            let generations = restarted.storage_mut().generations(4).unwrap();
            assert_eq!(generations.len(), 3);
            assert!(generations.iter().all(|generation| generation.published));
        }
    }
}

#[test]
fn staging_reclamation_requires_a_lock_and_preserves_published_and_unknown_files() {
    let directory = SaveDirectory::new();
    let mut repository = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits {
            generations: 4,
            ..SaveLimits::default()
        },
        3,
    )
    .unwrap();
    for tick in 1..=3 {
        repository.save(&metadata(tick), records(tick)).unwrap();
    }
    assert_eq!(
        repository.storage_mut().discard_staged(3).unwrap_err().code,
        ErrorCode::InvalidState
    );
    repository.storage_mut().lock().unwrap();
    assert!(repository.storage_mut().discard_staged(3).is_err());
    repository.storage_mut().begin(4).unwrap();
    repository.storage_mut().unlock();
    let unknown = directory
        .0
        .join("gen-00000000000000000004.tmp")
        .join("creator-note.txt");
    std::fs::write(&unknown, b"preserve").unwrap();
    assert_eq!(
        repository
            .save(&metadata(5), records(5))
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidValue
    );
    assert_eq!(std::fs::read(&unknown).unwrap(), b"preserve");
    assert_eq!(repository.load_latest().unwrap().metadata.tick, 3);
    assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 4);
}

#[test]
fn runtime_terrain_callbacks_cannot_advance_owned_clocks_even_when_paused() {
    use charon_engine::runtime::{
        Engine, Scene, SceneContext,
        input::TickInput,
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
    };
    struct ClockWriter;
    impl System for ClockWriter {
        fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
            let mut worlds = ctx.worlds();
            let id = worlds.view().id("world")?;
            worlds.set(id, key("game:progress"), key("game:number"), &99u32)?;
            worlds.edit_terrain(id, |edit| edit.advance_tick())?;
            Ok(WorkStatus::Complete)
        }
    }
    struct Pause;
    impl Scene for Pause {
        fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
            let mut worlds = ctx.worlds();
            let id = worlds.view().id("world")?;
            worlds.pause(id, true)
        }
    }
    for initial_tick in [0, 3] {
        for paused in [false, true] {
            let native = WorldBuilder::new(state_content()).initialize(move |world| {
                world.edit(|edit| {
                    edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                    edit.set_state(key("game:progress"), key("game:number"), &7u32)?;
                    // Runtimeへ登録する前は作者が初期Worldの時計を構成できます。
                    for _ in 0..initial_tick {
                        edit.advance_tick()?;
                    }
                    Ok(())
                })
            });
            let mut builder = Engine::builder().sandbox_world("world", native).system(
                SystemDescriptor::new(key("game:clock-writer"), Phase::Intent)
                    .write(key("game:progress"))
                    .write(key("engine:terrain"))
                    .barrier(),
                ClockWriter,
            );
            if paused {
                builder = builder.scene("pause", Pause).start_scene("pause");
            }
            let mut engine = builder.build().unwrap();
            let id = engine.worlds().id("world").unwrap();
            let before = engine.worlds().checkpoint(id).unwrap();
            let error = engine.step(TickInput::empty(1)).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidState);
            assert_eq!(engine.tick(), 0);
            assert_eq!(engine.worlds().tick(id).unwrap(), initial_tick);
            assert_eq!(engine.worlds().terrain(id).unwrap().tick(), initial_tick);
            assert_eq!(
                engine
                    .worlds()
                    .get::<u32>(id, key("game:progress"))
                    .unwrap(),
                7
            );
            assert_eq!(engine.worlds().checkpoint(id).unwrap(), before);
        }
    }
}

#[test]
fn a_failed_current_update_can_reclaim_valid_surplus_before_the_next_save() {
    let directory = SaveDirectory::new();
    let mut repository = SaveRepository::new(
        FaultStorage {
            inner: DirectoryStorage::new(&directory.0).unwrap(),
            count: 0,
            fail: None,
            partial: false,
        },
        SaveLimits {
            generations: 4,
            ..SaveLimits::default()
        },
        3,
    )
    .unwrap();
    for tick in 1..=3 {
        repository.save(&metadata(tick), records(tick)).unwrap();
    }
    repository.storage_mut().count = 0;
    // lock/列挙/begin、blob/root/sync/publish、再読込2回の次がCURRENT更新です。
    // root公開後の失敗を作り、四つの有効世代から安全に保存を継続できるか検査します。
    repository.storage_mut().fail = Some(10);
    assert!(repository.save(&metadata(4), records(4)).is_err());
    repository.storage_mut().fail = None;
    assert_eq!(repository.load_latest().unwrap().metadata.tick, 4);
    assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 4);
    let receipt = repository.save(&metadata(5), records(5)).unwrap();
    assert_eq!(receipt.generation, 5);
    assert!(receipt.cleanup_errors.is_empty());
    assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 3);
    assert_eq!(repository.load_latest().unwrap().metadata.tick, 5);
}

#[test]
fn corrupt_published_generations_cannot_overflow_the_budget_and_disable_loading() {
    let directory = SaveDirectory::new();
    let limits = SaveLimits {
        generations: 4,
        ..SaveLimits::default()
    };
    let mut repository =
        SaveRepository::new(DirectoryStorage::new(&directory.0).unwrap(), limits, 3).unwrap();
    for tick in 1..=3 {
        repository.save(&metadata(tick), records(tick)).unwrap();
    }
    repository.storage_mut().lock().unwrap();
    repository.storage_mut().begin(4).unwrap();
    repository
        .storage_mut()
        .write_staged(4, SaveObject::Root, b"broken-root")
        .unwrap();
    repository.storage_mut().sync_staged(4).unwrap();
    repository.storage_mut().publish(4).unwrap();
    repository.storage_mut().unlock();
    assert_eq!(
        repository
            .save(&metadata(5), records(5))
            .err()
            .unwrap()
            .code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(repository.storage_mut().generations(4).unwrap().len(), 4);
    assert_eq!(repository.load_latest().unwrap().metadata.tick, 3);
    // 破損rootは保全したまま、作者が明示的に予算を増やして保存を再開できます。
    drop(repository);
    let mut expanded = SaveRepository::new(
        DirectoryStorage::new(&directory.0).unwrap(),
        SaveLimits {
            generations: 5,
            ..limits
        },
        3,
    )
    .unwrap();
    expanded.save(&metadata(5), records(5)).unwrap();
    assert_eq!(expanded.load_latest().unwrap().metadata.tick, 5);
    assert_eq!(expanded.storage_mut().generations(5).unwrap().len(), 4);
}

/// payload予算の余りへ依存せず、最大長の管理キーと各arenaを保存できることを確認します。
#[test]
fn runtime_checkpoint_limit_counts_management_records_and_staged_pages() {
    let content = state_content();
    let front = content.layer_id(key("game:front")).unwrap();
    let limits = WorldLimits {
        resident_chunks: 2,
        active_chunks: 0,
        terrain_bytes: 262_144,
        blocks: 1,
        particles: 1,
        particle_bytes: 0,
        leases: 1024,
        requests: 1,
        jobs: 1,
        state_records: 128,
        state_bytes: 512,
        block_state_records: 128,
        block_state_bytes: 512,
        channel_bytes: 16_000_000,
        ..WorldLimits::default()
    };
    let mut original = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        Arc::clone(&content),
        limits,
    )
    .unwrap();
    let long_key = |index| key(&format!("{}:{index:04}{}", "x".repeat(96), "y".repeat(92)));
    let pipeline = pipeline();
    let (lease, block, job, request) = original
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            let block = edit.place_block(
                front,
                point(0, 0),
                MaterialHandle(key("game:chest")),
                Orientation::Original,
            )?;
            let mut last = None;
            for index in 0..1024 {
                last = Some(edit.acquire_lease(RegionLease {
                    source: long_key(index),
                    region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                    mode: LeaseMode::LoadOnly,
                    expires: None,
                })?);
            }
            for index in 0u32..128 {
                edit.set_state(long_key(index), key("game:number"), &index)?;
                edit.set_block_state(block, long_key(index), key("game:number"), &index)?;
            }
            let job = edit.submit_cell_job(
                long_key(0),
                vec![
                    CellEdit {
                        layer: front,
                        point: point(9, 9),
                        value: CellCore::AIR,
                    },
                    CellEdit {
                        layer: front,
                        point: point(10, 9),
                        value: CellCore::AIR,
                    },
                ],
            )?;
            edit.step_cell_job(job, 1)?;
            let request = edit.request_generation(
                long_key(0),
                ChunkRegion::single(ChunkCoord::new(1, 0)?)?,
                &pipeline,
                0,
            )?;
            Ok((last.unwrap(), block, job, request))
        })
        .unwrap();
    let generous = original.checkpoint(4_000_000).unwrap();
    let limit = original.checkpoint_limit().unwrap();
    // channelのないWorldでは、共有codecが求める長さは保守値ではなく実測値と一致します。
    assert_eq!(generous.len(), limit);
    let bytes = original.checkpoint(limit).unwrap();
    assert_eq!(bytes, generous);
    let restored = World::restore(&bytes, content, limits, limit).unwrap();
    assert_eq!(restored.checkpoint(limit).unwrap(), bytes);
    assert_eq!(restored.lease(lease).unwrap().mode, LeaseMode::LoadOnly);
    assert_eq!(
        restored.block_state::<u32>(block, long_key(127)).unwrap(),
        127
    );
    assert_eq!(restored.state::<u32>(long_key(127)).unwrap(), 127);
    assert_eq!(
        restored.terrain_job(job).unwrap().status(),
        TerrainJobStatus::Preparing
    );
    assert_eq!(
        restored.request(request).unwrap().status(),
        RequestStatus::Requested
    );
}
