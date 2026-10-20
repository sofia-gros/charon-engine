//! 剛体候補stepの手動profileです。通常のtest suiteでは実行しません。

use charon_engine::{
    foundation::{
        StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::{
        BodyCouplingContext, BodyCouplingResponse, BodyDef, BodyFluidContactSample,
        BodyFluidOccupancySample, BodyLocalCell, BodyLocalGrid, BodyLocalGridLimits, BodyPhysics,
        BodyPhysicsSettings, BodyPhysicsStepContext, BodyPhysicsStepProvider, BodyPose,
        BodyStencil, ChannelDefinition, ChunkCoord, ChunkRegion, ColliderShape,
        CollisionQueriesProvider, CollisionQueryGeometry, CollisionQueryHit, CollisionQueryRequest,
        CollisionQueryResult, CollisionQueryStatus, CollisionQueryTarget, ContentRegistry,
        CouplingProvider, EnthalpyBand, EnthalpyFamily, JointDef, JointKind, LayerDef, LayerId,
        LayerRole, LeaseMode, MaterialBase, MaterialBuilder, MaterialHandle, ReactionChainPolicy,
        ReactionJob, ReactionRule, ReactionRuleSet, ReactionSidecar, ReactionStateTransfer,
        RegionLease, ThermalCellState, ThermalStepConfig, World, WorldBounds, WorldLimits,
    },
};
use std::{
    alloc::{GlobalAlloc, Layout, System as SystemAllocator},
    hint::black_box,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct CountingAllocator;
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_BYTES: AtomicUsize = AtomicUsize::new(0);

struct NoopBodyPhysicsStep;
struct BenchCouplingProvider {
    contacts: Option<Vec<BodyFluidContactSample>>,
    occupancy: Option<Vec<BodyFluidOccupancySample>>,
}
struct BenchCollisionQueries {
    hits: Vec<CollisionQueryHit>,
}

impl BodyPhysicsStepProvider for NoopBodyPhysicsStep {
    fn key(&self) -> StableKey {
        StableKey::parse("bench:noop-body-step").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn step(
        &self,
        _physics_candidate: &mut BodyPhysics,
        _world: &World,
        _context: BodyPhysicsStepContext,
    ) -> charon_engine::foundation::EngineResult<u32> {
        Ok(1)
    }
}

impl CouplingProvider for BenchCouplingProvider {
    fn key(&self) -> StableKey {
        StableKey::parse("bench:custom-body-coupling").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn fluid_contacts(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_contacts: usize,
    ) -> charon_engine::foundation::EngineResult<Option<Vec<BodyFluidContactSample>>> {
        Ok(self.contacts.clone())
    }

    fn fluid_occupancy(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_cells: usize,
    ) -> charon_engine::foundation::EngineResult<Option<Vec<BodyFluidOccupancySample>>> {
        Ok(self.occupancy.clone())
    }

    fn couple(
        &self,
        _physics_candidate: &mut BodyPhysics,
        _world: &World,
        _context: BodyCouplingContext,
    ) -> charon_engine::foundation::EngineResult<BodyCouplingResponse> {
        Ok(BodyCouplingResponse {
            fluid_velocity_delta_q16: [0, 0],
            body_impulse: [0.0, 0.0],
            external_impulse: [0.0, 0.0],
        })
    }
}

impl CollisionQueriesProvider for BenchCollisionQueries {
    fn key(&self) -> StableKey {
        StableKey::parse("bench:collision-query").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn query(
        &self,
        _world: &World,
        _physics: Option<&BodyPhysics>,
        _request: &CollisionQueryRequest,
    ) -> charon_engine::foundation::EngineResult<CollisionQueryResult> {
        Ok(CollisionQueryResult {
            status: CollisionQueryStatus::Hit,
            hits: self.hits.clone(),
            truncated: false,
            geometry_revision: 1,
        })
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

impl CountingAllocator {
    fn allocated(size: usize) {
        ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOCATION_BYTES.fetch_add(size, Ordering::Relaxed);
        let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }
}

// SAFETY: 実際の確保・解放を標準allocatorへ委譲し、観測値だけをatomicで集計します。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: layoutはallocator呼出元が渡した有効な条件です。
        let pointer = unsafe { SystemAllocator.alloc(layout) };
        if !pointer.is_null() {
            Self::allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if !pointer.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: pointerとlayoutはこのallocatorが返した組です。
        unsafe { SystemAllocator.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointerとlayoutはこのallocatorが返した有効な割当です。
        let resized = unsafe { SystemAllocator.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                let increase = new_size - layout.size();
                let live = LIVE_BYTES.fetch_add(increase, Ordering::Relaxed) + increase;
                PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATION_BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        resized
    }
}

fn percentile(samples: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    ordered[(ordered.len() - 1) * numerator / denominator]
}

#[test]
#[ignore = "1024 occupied cellのBodyLocalGrid熱伝導を手動計測"]
fn body_local_thermal_profile() {
    const WIDTH: u32 = 32;
    const HEIGHT: u32 = 32;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("bench:terrain").unwrap();
    let stone_key = StableKey::parse("bench:stone").unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = BodyLocalGrid::new(
        WIDTH,
        HEIGHT,
        1.0,
        [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0],
        0,
        BodyLocalGridLimits {
            cells: WIDTH * HEIGHT,
            geometry_cells: WIDTH * HEIGHT,
            sidecar_bytes: 1024,
        },
    )
    .unwrap();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let energy = if (x + y) % 2 == 0 { 80_000 } else { 20_000 };
            grid.set_cell(
                x,
                y,
                BodyLocalCell::new(layer_key, stone_key, 0, 0, energy, Vec::new()).unwrap(),
            )
            .unwrap();
        }
    }
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("bench:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 0,
            energy_end: 100_000,
            temperature_start_mk: 0,
            temperature_end_mk: 100_000,
            material: stone_key,
        }],
    )
    .unwrap();
    let config = ThermalStepConfig {
        conductance_numerator: 1,
        conductance_denominator: 4,
    };
    for _ in 0..WARMUP {
        world
            .edit(|edit| edit.step_body_local_thermal(body, &family, config))
            .unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(
            world
                .edit(|edit| edit.step_body_local_thermal(body, &family, config))
                .unwrap(),
        );
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=32x32_body_local_thermal/occupied_cells=1024 samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
    black_box(layer);
}

#[test]
#[ignore = "回転body/world熱境界fluxを手動計測"]
fn body_world_thermal_boundary_profile() {
    const WIDTH: u32 = 8;
    const HEIGHT: u32 = 8;
    // 接触する1 chunkと、候補では変更しない15 chunkのchannel配列を用意します。
    const CHANNEL_CHUNKS_SIDE: i32 = 4;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    let layer_key = StableKey::parse("bench:terrain").unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let stone_key = StableKey::parse("bench:stone").unwrap();
    registry
        .register(MaterialBuilder::new(stone_key.as_str(), MaterialBase::Solid).unwrap())
        .unwrap();
    let thermal_key = StableKey::parse("bench:thermal").unwrap();
    registry
        .schema(Schema::of::<ThermalCellState>(thermal_key, 1, 8).unwrap())
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: thermal_key,
                version: 1,
                value_schema: thermal_key,
                layout: GridLayout {
                    origin: CellPoint { x: 0, y: 0 },
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 64,
                    checkpoint_bytes: 8 * 1024,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let stone = content.material_id(MaterialHandle(stone_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = BodyLocalGrid::new(
        WIDTH,
        HEIGHT,
        1.0,
        [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0],
        0,
        BodyLocalGridLimits {
            cells: WIDTH * HEIGHT,
            geometry_cells: WIDTH * HEIGHT,
            sidecar_bytes: 1024,
        },
    )
    .unwrap();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            grid.set_cell(
                x,
                y,
                BodyLocalCell::new(layer_key, stone_key, 0, 0, 1_000_000, Vec::new()).unwrap(),
            )
            .unwrap();
        }
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = BodyPose {
        x: 8.0,
        y: 8.0,
        angle: std::f64::consts::FRAC_PI_8,
    };
    let stencil = BodyStencil::new(
        WIDTH,
        HEIGHT,
        1.0,
        [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0],
        0,
        vec![true; (WIDTH * HEIGHT) as usize],
    )
    .unwrap();
    let mut world_points: Vec<_> = stencil
        .boundary_contacts(definition.pose, 1.0, 256)
        .unwrap()
        .into_iter()
        .map(|contact| contact.world_cell)
        .collect();
    world_points.sort_unstable_by_key(|point| (point.y, point.x));
    world_points.dedup();
    world
        .edit(|edit| {
            for chunk_y in 0..CHANNEL_CHUNKS_SIDE {
                for chunk_x in 0..CHANNEL_CHUNKS_SIDE {
                    edit.insert_empty_chunk(ChunkCoord::new(chunk_x, chunk_y)?)?;
                    let point = CellPoint {
                        x: chunk_x * 64 + 1,
                        y: chunk_y * 64 + 1,
                    };
                    if !world_points.contains(&point) {
                        edit.set_cell(
                            layer,
                            point,
                            charon_engine::world::CellCore {
                                material: stone,
                                flags: 0,
                                variation: 0,
                            },
                        )?;
                        edit.set_channel(thermal_key, point, ThermalCellState { energy: 100_000 })?;
                    }
                }
            }
            for &point in &world_points {
                edit.set_cell(
                    layer,
                    point,
                    charon_engine::world::CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
                edit.set_channel(thermal_key, point, ThermalCellState { energy: 100_000 })?;
            }
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("bench:thermal-active")?,
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("bench:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 0,
            energy_end: 2_000_000,
            temperature_start_mk: 0,
            temperature_end_mk: 2_000_000,
            material: stone_key,
        }],
    )
    .unwrap();
    let config = ThermalStepConfig {
        conductance_numerator: 1,
        conductance_denominator: 16,
    };
    for _ in 0..WARMUP {
        world
            .edit(|edit| {
                edit.step_body_world_thermal_boundary(
                    body,
                    layer,
                    thermal_key,
                    &family,
                    config,
                    1.0,
                )
            })
            .unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(
            world
                .edit(|edit| {
                    edit.step_body_world_thermal_boundary(
                        body,
                        layer,
                        thermal_key,
                        &family,
                        config,
                        1.0,
                    )
                })
                .unwrap(),
        );
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=8x8_rotated_body_world_thermal_boundary/channel_chunks=16/contacts={} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        world_points.len(),
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

#[test]
#[ignore = "256 source cellのbody-local reaction候補生成・commitを手動計測"]
fn body_local_reaction_profile() {
    const WIDTH: u32 = 32;
    const HEIGHT: u32 = 8;
    const SOURCE_COUNT: usize = (WIDTH as usize / 2) * HEIGHT as usize;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;

    let layer_key = StableKey::parse("bench:reaction-layer").unwrap();
    let ice_key = StableKey::parse("bench:reaction-ice").unwrap();
    let ash_key = StableKey::parse("bench:reaction-ash").unwrap();
    let sidecar_key = StableKey::parse("bench:reaction-state").unwrap();
    let sidecar_schema = StableKey::parse("bench:reaction-state-schema").unwrap();
    let job_key = StableKey::parse("bench:reaction-job").unwrap();
    let job_schema = StableKey::parse("bench:reaction-job-schema").unwrap();

    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new(ice_key.as_str(), MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new(ash_key.as_str(), MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ReactionSidecar>(sidecar_schema, 1, 14).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ReactionJob>(job_schema, 1, 65_536).unwrap())
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: sidecar_key,
                version: 1,
                value_schema: sidecar_schema,
                layout: GridLayout {
                    origin: CellPoint { x: 0, y: 0 },
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ReactionSidecar::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = BodyLocalGrid::new(
        WIDTH,
        HEIGHT,
        1.0,
        [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0],
        0,
        BodyLocalGridLimits {
            cells: SOURCE_COUNT as u32,
            geometry_cells: WIDTH * HEIGHT,
            sidecar_bytes: 1024,
        },
    )
    .unwrap();
    for y in 0..HEIGHT {
        for x in (0..WIDTH).step_by(2) {
            grid.set_cell(
                x,
                y,
                BodyLocalCell::new(layer_key, ice_key, 0, 0, 90, Vec::new()).unwrap(),
            )
            .unwrap();
        }
    }
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Cuboid {
                    half_extents: [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0],
                }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("bench:reaction-family").unwrap(),
        vec![
            EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 1000,
                material: ice_key,
            },
            EnthalpyBand::Latent {
                energy_start: 100,
                energy_end: 101,
                temperature_mk: 1000,
                from: ice_key,
                to: ash_key,
            },
            EnthalpyBand::Sensible {
                energy_start: 101,
                energy_end: 200,
                temperature_start_mk: 1000,
                temperature_end_mk: 2000,
                material: ash_key,
            },
        ],
    )
    .unwrap();
    let rules = ReactionRuleSet::new(
        &family,
        vec![
            ReactionRule {
                key: StableKey::parse("bench:reaction-forward").unwrap(),
                input: ice_key,
                contact: None,
                priority: 0,
                consume: 1,
                output: ash_key,
                produce: 1,
                output_offset: [1, 0],
                particle_velocity_q16: None,
                minimum_temperature_mk: 0,
                fuel_cost: 0,
                timer_cost: 0,
                energy_delta: 20,
                maximum_wetness: u16::MAX,
                corrosion_delta: 0,
                state_transfer: ReactionStateTransfer::MigrateToOutput,
                chain_policy: ReactionChainPolicy::Deferred,
            },
            ReactionRule {
                key: StableKey::parse("bench:reaction-reverse").unwrap(),
                input: ash_key,
                contact: None,
                priority: 0,
                consume: 1,
                output: ice_key,
                produce: 1,
                output_offset: [-1, 0],
                particle_velocity_q16: None,
                minimum_temperature_mk: 0,
                fuel_cost: 0,
                timer_cost: 0,
                energy_delta: -20,
                maximum_wetness: u16::MAX,
                corrosion_delta: 0,
                state_transfer: ReactionStateTransfer::MigrateToOutput,
                chain_policy: ReactionChainPolicy::Deferred,
            },
        ],
    )
    .unwrap();
    let layer_id = world.content().layer_id(layer_key).unwrap();

    let run_cycle = |world: &mut World| {
        for _ in 0..2 {
            let job = world
                .edit(|edit| {
                    edit.build_body_local_reaction_job(
                        body,
                        layer_key,
                        sidecar_key,
                        1,
                        &rules,
                        SOURCE_COUNT,
                    )
                })
                .unwrap();
            let (_, report) = world
                .edit(|edit| {
                    edit.step_body_local_reactions(
                        body,
                        layer_key,
                        sidecar_key,
                        1,
                        &rules,
                        SOURCE_COUNT,
                        SOURCE_COUNT,
                        Some(job),
                        job_key,
                        job_schema,
                    )
                })
                .unwrap();
            assert_eq!(report.committed, SOURCE_COUNT as u32);
        }
    };
    for _ in 0..WARMUP {
        run_cycle(&mut world);
    }

    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut build_elapsed = Vec::with_capacity(SAMPLES * 2);
    let mut commit_elapsed = Vec::with_capacity(SAMPLES * 2);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let mut build_allocations = 0usize;
    let mut build_bytes = 0usize;
    let mut commit_allocations = 0usize;
    let mut commit_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..2 {
            let build_count_before = ALLOCATION_COUNT.load(Ordering::Relaxed);
            let build_bytes_before = ALLOCATION_BYTES.load(Ordering::Relaxed);
            let build_started = Instant::now();
            let job = world
                .edit(|edit| {
                    edit.build_body_local_reaction_job(
                        body,
                        layer_key,
                        sidecar_key,
                        1,
                        &rules,
                        SOURCE_COUNT,
                    )
                })
                .unwrap();
            build_elapsed.push(build_started.elapsed());
            let build_count_after = ALLOCATION_COUNT.load(Ordering::Relaxed);
            let build_bytes_after = ALLOCATION_BYTES.load(Ordering::Relaxed);
            build_allocations += build_count_after.saturating_sub(build_count_before);
            build_bytes += build_bytes_after.saturating_sub(build_bytes_before);
            let commit_count_before = ALLOCATION_COUNT.load(Ordering::Relaxed);
            let commit_bytes_before = ALLOCATION_BYTES.load(Ordering::Relaxed);
            let commit_started = Instant::now();
            let (_, report) = world
                .edit(|edit| {
                    edit.step_body_local_reactions(
                        body,
                        layer_key,
                        sidecar_key,
                        1,
                        &rules,
                        SOURCE_COUNT,
                        SOURCE_COUNT,
                        Some(job),
                        job_key,
                        job_schema,
                    )
                })
                .unwrap();
            commit_elapsed.push(commit_started.elapsed());
            let commit_count_after = ALLOCATION_COUNT.load(Ordering::Relaxed);
            let commit_bytes_after = ALLOCATION_BYTES.load(Ordering::Relaxed);
            commit_allocations += commit_count_after.saturating_sub(commit_count_before);
            commit_bytes += commit_bytes_after.saturating_sub(commit_bytes_before);
            assert_eq!(report.committed, SOURCE_COUNT as u32);
        }
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=32x8_body_local_reaction/source_cells={SOURCE_COUNT} candidates_per_cycle={} samples={SAMPLES} cycle_p50={:?} cycle_p95={:?} cycle_p99={:?} build_p50={:?} build_p95={:?} build_allocations_per_pass={:.1} build_bytes_per_pass={:.1} commit_p50={:?} commit_p95={:?} commit_allocations_per_pass={:.1} commit_bytes_per_pass={:.1} allocations_per_cycle={:.1} bytes_per_cycle={:.1} extra_peak={}B",
        SOURCE_COUNT * 2,
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        percentile(&build_elapsed, 50, 100),
        percentile(&build_elapsed, 95, 100),
        build_allocations as f64 / (SAMPLES * 2) as f64,
        build_bytes as f64 / (SAMPLES * 2) as f64,
        percentile(&commit_elapsed, 50, 100),
        percentile(&commit_elapsed, 95, 100),
        commit_allocations as f64 / (SAMPLES * 2) as f64,
        commit_bytes as f64 / (SAMPLES * 2) as f64,
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
    black_box(layer_id);
}

#[test]
#[ignore = "256 bodyのRapier候補複製・stepを手動計測"]
fn body_physics_step_profile() {
    const BODY_COUNT: usize = 256;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut world = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 9.81],
        body_capacity: BODY_COUNT as u32,
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    for index in 0..BODY_COUNT {
        let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.4 });
        body.pose.x = (index % 32) as f64 * 2.0;
        body.pose.y = (index / 32) as f64 * 2.0;
        world.insert(body).unwrap();
    }
    for _ in 0..WARMUP {
        world.step().unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.step().unwrap());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=256_body/no_contact samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

#[test]
#[ignore = "World境界検査と原子的BodyPhysics候補stepを手動計測"]
fn world_body_step_profile() {
    const BODY_COUNT: usize = 256;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer: LayerId = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 9.81],
            body_capacity: BODY_COUNT as u32,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            for index in 0..BODY_COUNT {
                let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.4 });
                body.pose.x = 2.0 + (index % 32) as f64 * 1.8;
                body.pose.y = 2.0 + (index / 32) as f64 * 2.0;
                physics.insert(body)?;
            }
            Ok(())
        })
        .unwrap();
    for _ in 0..WARMUP {
        world.step_bodies(layer, 1.0).unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.step_bodies(layer, 1.0).unwrap());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=256_body/no_contact/world_step samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

fn world_body_joint_step_profile_world(with_joints: bool) -> (World, LayerId) {
    const BODY_COUNT: usize = 256;
    const JOINT_COUNT: usize = 64;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: BODY_COUNT as u32,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            let mut body_ids = Vec::with_capacity(BODY_COUNT);
            for index in 0..BODY_COUNT {
                let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.4 });
                body.pose.x = 2.0 + (index % 32) as f64 * 1.8;
                body.pose.y = 2.0 + (index / 32) as f64 * 1.8;
                body.velocity.x = if index % 2 == 0 { 0.1 } else { -0.1 };
                body_ids.push(physics.insert(body)?);
            }
            if with_joints {
                for pair in 0..JOINT_COUNT {
                    physics.insert_joint(JointDef {
                        body1: body_ids[pair * 2],
                        body2: body_ids[pair * 2 + 1],
                        local_anchor1: [0.9, 0.0],
                        local_anchor2: [-0.9, 0.0],
                        kind: JointKind::Fixed,
                    })?;
                }
            }
            Ok(())
        })
        .unwrap();
    (world, layer)
}

fn measure_world_body_joint_step_profile(world: &mut World, layer: LayerId, with_joints: bool) {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    for _ in 0..WARMUP {
        world.step_bodies(layer, 1.0).unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.step_bodies(layer, 1.0).unwrap());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=256_body/{}64_fixed_joints samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        if with_joints { "with_" } else { "without_" },
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

#[test]
#[ignore = "256 bodyのfixed-stepに対する64 joint拘束費用を手動計測"]
fn world_body_joint_step_profile() {
    let (mut without_joints, layer) = world_body_joint_step_profile_world(false);
    let (mut with_joints, _) = world_body_joint_step_profile_world(true);
    measure_world_body_joint_step_profile(&mut without_joints, layer, false);
    measure_world_body_joint_step_profile(&mut with_joints, layer, true);
}

#[test]
#[ignore = "流体接触時のWorld候補複製と剛体stepを手動計測"]
fn world_body_fluid_overlap_step_profile() {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:water", MaterialBase::Liquid).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("bench:terrain").unwrap();
    let stone_key = StableKey::parse("bench:stone").unwrap();
    let water = content
        .material_id(MaterialHandle(StableKey::parse("bench:water").unwrap()))
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for x in 4..60 {
                edit.set_cell(
                    layer,
                    CellPoint { x, y: 4 },
                    charon_engine::world::CellCore {
                        material: water,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = BodyLocalGrid::new(
        1,
        1,
        1.0,
        [0.5, 0.5],
        0,
        BodyLocalGridLimits {
            cells: 128,
            geometry_cells: 1,
            sidecar_bytes: 1,
        },
    )
    .unwrap();
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
    )
    .unwrap();
    let mut body = BodyDef::dynamic(ColliderShape::Cuboid {
        half_extents: [0.5, 0.5],
    });
    body.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle: 0.0,
    };
    body.velocity.x = 30.0;
    world
        .edit_bodies(|physics| physics.insert_with_local_grid(body, grid).map(|_| ()))
        .unwrap();

    for _ in 0..WARMUP {
        world.step_bodies(layer, 1.0).unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.step_bodies(layer, 1.0).unwrap());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=one_body/active_fluid_overlap/world_step samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

#[test]
#[ignore = "複数body・多chunk Worldの流体連成候補複製を手動計測"]
fn world_multi_body_large_fluid_step_profile() {
    const BODY_COUNT: usize = 16;
    const CHUNKS_PER_AXIS: i32 = 8;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:water", MaterialBase::Liquid).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("bench:terrain").unwrap();
    let stone_key = StableKey::parse("bench:stone").unwrap();
    let water = content
        .material_id(MaterialHandle(StableKey::parse("bench:water").unwrap()))
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            for chunk_y in 0..CHUNKS_PER_AXIS {
                for chunk_x in 0..CHUNKS_PER_AXIS {
                    edit.insert_empty_chunk(ChunkCoord::new(chunk_x, chunk_y)?)?;
                    let filler = CellPoint {
                        x: chunk_x * 16 + 8,
                        y: chunk_y * 16 + 8,
                    };
                    edit.set_cell(
                        layer,
                        filler,
                        charon_engine::world::CellCore {
                            material: water,
                            flags: 0,
                            variation: 0,
                        },
                    )?;
                }
            }
            for x in 4..128 {
                edit.set_cell(
                    layer,
                    CellPoint { x, y: 4 },
                    charon_engine::world::CellCore {
                        material: water,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: BODY_COUNT as u32,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    for index in 0..BODY_COUNT {
        let mut grid = BodyLocalGrid::new(
            1,
            1,
            1.0,
            [0.5, 0.5],
            0,
            BodyLocalGridLimits {
                cells: 1,
                geometry_cells: 1,
                sidecar_bytes: 1,
            },
        )
        .unwrap();
        grid.set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
        let mut body = BodyDef::dynamic(ColliderShape::Cuboid {
            half_extents: [0.5, 0.5],
        });
        body.pose = BodyPose {
            x: 4.5 + index as f64 * 7.0,
            y: 4.5,
            angle: 0.0,
        };
        body.velocity.x = 12.0;
        world
            .edit_bodies(|physics| physics.insert_with_local_grid(body, grid).map(|_| ()))
            .unwrap();
    }

    for _ in 0..WARMUP {
        world.step_bodies(layer, 1.0).unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let mut sample_costs = Vec::with_capacity(SAMPLES);
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for sample_index in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.step_bodies(layer, 1.0).unwrap());
        let sample_elapsed = started.elapsed();
        elapsed.push(sample_elapsed);
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        let sample_allocations = count.saturating_sub(previous_count);
        let sample_bytes = bytes.saturating_sub(previous_bytes);
        allocations += sample_allocations;
        allocated_bytes += sample_bytes;
        sample_costs.push((
            sample_elapsed,
            sample_index,
            sample_allocations,
            sample_bytes,
        ));
        previous_count = count;
        previous_bytes = bytes;
    }
    sample_costs.sort_unstable_by_key(|sample| std::cmp::Reverse(sample.0));
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=16_bodies/64_chunks/active_fluid_row/world_step samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
    eprintln!(
        "slowest_samples={:?}",
        &sample_costs[..sample_costs.len().min(8)]
    );
}

fn body_step_provider_profile_world() -> (World, LayerId) {
    const BODY_COUNT: usize = 16;
    const CHUNKS_PER_AXIS: i32 = 8;
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("bench:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("bench:terrain").unwrap())
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            for chunk_y in 0..CHUNKS_PER_AXIS {
                for chunk_x in 0..CHUNKS_PER_AXIS {
                    edit.insert_empty_chunk(ChunkCoord::new(chunk_x, chunk_y)?)?;
                }
            }
            Ok(())
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: BODY_COUNT as u32,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    for index in 0..BODY_COUNT {
        let mut body = BodyDef::dynamic(ColliderShape::Cuboid {
            half_extents: [0.5, 0.5],
        });
        body.pose = BodyPose {
            x: 8.5 + index as f64 * 8.0,
            y: 8.5,
            angle: 0.0,
        };
        body.velocity = Default::default();
        world.edit_bodies(|physics| physics.insert(body)).unwrap();
    }
    (world, layer)
}

fn measure_body_step_provider_profile(world: &mut World, layer: LayerId, provider: bool) {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    for _ in 0..WARMUP {
        if provider {
            world
                .step_bodies_with_provider(layer, 1.0, &NoopBodyPhysicsStep)
                .unwrap();
        } else {
            world.step_bodies(layer, 1.0).unwrap();
        }
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        if provider {
            black_box(
                world
                    .step_bodies_with_provider(layer, 1.0, &NoopBodyPhysicsStep)
                    .unwrap(),
            );
        } else {
            black_box(world.step_bodies(layer, 1.0).unwrap());
        }
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=16_bodies/64_chunks/zero_gravity/{} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        if provider { "provider" } else { "rapier" },
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

fn measure_body_coupling_profile(
    world: &mut World,
    layer: LayerId,
    provider: &BenchCouplingProvider,
    label: &str,
) {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    for _ in 0..WARMUP {
        world
            .step_bodies_with_coupling_provider(layer, 1.0, provider)
            .unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(
            world
                .step_bodies_with_coupling_provider(layer, 1.0, provider)
                .unwrap(),
        );
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=16_bodies/64_chunks/no_fluid/coupling/{label} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

fn custom_occupancy_for_profile(world: &World) -> Vec<BodyFluidOccupancySample> {
    let physics = world.body_physics().unwrap();
    let mut occupancy = Vec::new();
    for body in physics.body_ids() {
        let state = physics.state(body).unwrap();
        let center_x = state.pose.x.floor() as i32;
        let center_y = state.pose.y.floor() as i32;
        for y_offset in -2..2 {
            for x_offset in -2..2 {
                occupancy.push(BodyFluidOccupancySample {
                    body,
                    world_cell: CellPoint {
                        x: center_x + x_offset,
                        y: center_y + y_offset,
                    },
                });
            }
        }
    }
    occupancy
}

#[test]
#[ignore = "custom CouplingProviderの占有preflight費用を手動profile"]
fn world_custom_coupling_occupancy_preflight_profile() {
    let (mut default_world, layer) = body_step_provider_profile_world();
    let (mut custom_world, _) = body_step_provider_profile_world();
    let default_provider = BenchCouplingProvider {
        contacts: None,
        occupancy: None,
    };
    let custom_provider = BenchCouplingProvider {
        contacts: Some(Vec::new()),
        occupancy: Some(custom_occupancy_for_profile(&custom_world)),
    };
    measure_body_coupling_profile(
        &mut default_world,
        layer,
        &default_provider,
        "standard_geometry",
    );
    measure_body_coupling_profile(
        &mut custom_world,
        layer,
        &custom_provider,
        "custom_256_occupancy_cells",
    );
}

#[test]
#[ignore = "作者fixed-step providerのWorld candidate費用を手動計測"]
fn world_body_physics_provider_candidate_profile() {
    let (mut rapier_world, layer) = body_step_provider_profile_world();
    let (mut provider_world, _) = body_step_provider_profile_world();
    measure_body_step_provider_profile(&mut rapier_world, layer, false);
    measure_body_step_provider_profile(&mut provider_world, layer, true);
}

fn measure_collision_query_validation_profile(
    world: &World,
    request: &CollisionQueryRequest,
    provider: &BenchCollisionQueries,
    label: &str,
) {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    for _ in 0..WARMUP {
        black_box(world.query_body_collisions(request, provider).unwrap());
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        black_box(world.query_body_collisions(request, provider).unwrap());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=16_bodies/64_chunks/query_hits={} mode={label} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations_per_query={:.1} bytes_per_query={:.1} extra_peak={}B",
        provider.hits.len(),
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}

#[test]
#[ignore = "CollisionQueriesのstable hit整列費用をno-sort経路と比較計測"]
fn world_collision_query_validation_profile() {
    const HIT_COUNT: usize = 128;
    let (world, layer) = body_step_provider_profile_world();
    let provider = BenchCollisionQueries {
        hits: (0..HIT_COUNT)
            .rev()
            .map(|index| CollisionQueryHit {
                target: CollisionQueryTarget::Terrain {
                    cell: CellPoint {
                        x: index as i32,
                        y: 3,
                    },
                },
                material: None,
                point: [index as f64, 3.0],
                normal: [1.0, 0.0],
                time_of_impact: index as f64 / HIT_COUNT as f64,
                geometry_revision: 1,
                coverage: 1.0,
            })
            .collect(),
    };
    let request = CollisionQueryRequest {
        geometry: CollisionQueryGeometry::Ray {
            origin: [0.0, 3.0],
            direction: [1.0, 0.0],
            max_distance: HIT_COUNT as f64,
        },
        layer,
        collision_groups: u32::MAX,
        owner: None,
        ignore_bodies: Vec::new(),
        one_way_direction: None,
        include_sensors: false,
        include_initial_overlap: false,
        include_boundary: true,
        epsilon: 0.0,
        tick: 1,
        substep: 0,
        read_latest_candidate: true,
        first_hit_only: false,
        stable_order: true,
        max_hits: HIT_COUNT as u32,
    };
    measure_collision_query_validation_profile(&world, &request, &provider, "stable_order");
    let mut request_without_sort = request;
    request_without_sort.stable_order = false;
    measure_collision_query_validation_profile(
        &world,
        &request_without_sort,
        &provider,
        "provider_order",
    );
}

#[test]
#[ignore = "16×16 BodyStencilの予約領域生成を手動計測"]
fn body_stencil_sweep_profile() {
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let stencil = BodyStencil::new(16, 16, 1.0, [8.0, 8.0], 1, vec![true; 256]).unwrap();
    let start = BodyPose {
        x: 20.5,
        y: 30.5,
        angle: 0.0,
    };
    let end = BodyPose {
        x: 21.5,
        y: 30.5,
        angle: 0.02,
    };
    let mut output = Vec::<CellPoint>::new();
    let mut sample = Vec::<CellPoint>::new();
    for _ in 0..WARMUP {
        stencil
            .sampled_sweep_world_cells_with_scratch(start, end, 8, &mut output, &mut sample)
            .unwrap();
    }
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let mut previous_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let mut previous_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut allocations = 0usize;
    let mut allocated_bytes = 0usize;
    let start_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(start_live, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        stencil
            .sampled_sweep_world_cells_with_scratch(start, end, 8, &mut output, &mut sample)
            .unwrap();
        black_box(output.as_slice());
        elapsed.push(started.elapsed());
        let count = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
        allocations += count.saturating_sub(previous_count);
        allocated_bytes += bytes.saturating_sub(previous_bytes);
        previous_count = count;
        previous_bytes = bytes;
    }
    let extra_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(start_live);
    eprintln!(
        "world=16x16_stencil/sweep samples={SAMPLES} cells={} p50={:?} p95={:?} p99={:?} allocations_per_step={:.1} bytes_per_step={:.1} extra_peak={}B",
        output.len(),
        percentile(&elapsed, 50, 100),
        percentile(&elapsed, 95, 100),
        percentile(&elapsed, 99, 100),
        allocations as f64 / SAMPLES as f64,
        allocated_bytes as f64 / SAMPLES as f64,
        extra_peak,
    );
}
