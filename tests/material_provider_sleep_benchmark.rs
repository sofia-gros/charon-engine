//! 作者MaterialForceProviderの静止領域で参照実行とsleep経路を比較する手動計測。

use charon_engine::{
    foundation::{EngineResult, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::{
    alloc::{GlobalAlloc, Layout, System as SystemAllocator},
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct CountingAllocator;
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_BYTES: AtomicUsize = AtomicUsize::new(0);

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

// SAFETY: 実際の割当・解放は標準allocatorへ委譲し、計測値のみをatomicで集計します。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: layoutは呼出元が指定した有効な割当条件です。
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
        // SAFETY: pointer/layoutはこのallocatorから受け取った組です。
        unsafe { SystemAllocator.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointer/layoutはこのallocatorから受け取った組です。
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

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}

fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

struct StaticProvider {
    calls: Arc<AtomicUsize>,
}
impl MaterialForceProvider for StaticProvider {
    fn key(&self) -> StableKey {
        key("bench:static-provider")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(black_box(input.motion))
    }
    fn sleep_stencil(&self) -> Option<MaterialForceDependencyStencil> {
        Some(MaterialForceDependencyStencil::new(0, 0).unwrap())
    }
    fn sleep_invariant(
        &self,
        _world: &World,
        _layer: LayerId,
        _force: MaterialForceConfig,
        _motion: MaterialMotionConfig,
    ) -> EngineResult<bool> {
        Ok(true)
    }
}

fn fixture() -> (World, LayerId, Arc<Content>) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("bench:layer"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:solid", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("bench:liquid", MaterialBase::Liquid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotion>(key("bench:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<i64>(key("bench:pressure-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("bench:layer"),
            ChannelDefinition {
                key: key("bench:motion"),
                version: 1,
                value_schema: key("bench:motion-schema"),
                layout: GridLayout {
                    origin: p(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MaterialMotion::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 4_000_000,
                },
            },
        )
        .unwrap();
    registry
        .channel(ChannelDefinition {
            key: key("bench:pressure"),
            version: 1,
            value_schema: key("bench:pressure-schema"),
            layout: GridLayout {
                origin: p(0, 0),
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0i64,
            limits: GridLimits {
                chunks: 8,
                checkpoint_bytes: 4_000_000,
            },
        })
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("bench:layer")).unwrap();
    let material = content
        .material_id(MaterialHandle(key("bench:solid")))
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        1,
        WorldBounds::expandable(),
        Arc::clone(&content),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("bench:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            for y in 0..64 {
                for x in 0..64 {
                    edit.set_cell(
                        layer,
                        p(x, y),
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
    (world, layer, content)
}

fn particle_fixture() -> (World, LayerId, Arc<Content>) {
    let (mut world, layer, content) = fixture();
    let material = content
        .material_id(MaterialHandle(key("bench:solid")))
        .unwrap();
    world
        .edit(|edit| {
            for y in 0..64 {
                for x in 0..64 {
                    let point = p(x, y);
                    edit.emit_material_particle(layer, point, [0, 0])?;
                    // 粒子の静止着地先を閉じ、同じcellに別tokenを置いて双方の所有権を保つ。
                    edit.set_cell(
                        layer,
                        point,
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
    (world, layer, content)
}

fn percentile(samples: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    ordered[(ordered.len() - 1) * numerator / denominator]
}

fn pressure_fixture() -> (World, LayerId, Arc<Content>) {
    let (_, layer, content) = fixture();
    let liquid = content
        .material_id(MaterialHandle(key("bench:liquid")))
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        1,
        WorldBounds::expandable(),
        Arc::clone(&content),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("bench:pressure-active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            for y in 0..64 {
                for x in 0..64 {
                    edit.set_cell(
                        layer,
                        p(x, y),
                        CellCore {
                            material: liquid,
                            ..CellCore::AIR
                        },
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
    (world, layer, content)
}

fn measure_pressure(
    world: &mut World,
    layer: LayerId,
    tracker: Option<&mut PressureSleepTracker>,
    ticks: usize,
) -> (Vec<Duration>, usize, usize, usize) {
    let config = PressureRelaxation {
        channel: key("bench:pressure"),
        iterations: 4,
        gravity_q16: 0,
        cell_limit: 4096,
    };
    let mut tracker = tracker;
    let mut samples = Vec::with_capacity(ticks);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..ticks {
        let started = Instant::now();
        if let Some(tracker) = tracker.as_mut() {
            world
                .relax_pressure_sleeping(tracker, layer, config)
                .unwrap();
        } else {
            world
                .edit(|edit| edit.relax_pressure(layer, config))
                .unwrap();
        }
        world.edit(|edit| edit.advance_tick()).unwrap();
        samples.push(started.elapsed());
    }
    (
        samples,
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        PEAK_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(baseline_live),
    )
}

fn measure_material_phase(
    world: &mut World,
    layer: LayerId,
    combined: bool,
    ticks: usize,
) -> (Vec<Duration>, usize, usize, usize) {
    let pressure = PressureRelaxation {
        channel: key("bench:pressure"),
        iterations: 4,
        gravity_q16: 0,
        cell_limit: 4096,
    };
    let config = MaterialPhaseConfig {
        layer,
        pressure: Some(pressure),
        motion: None,
        scalar_candidate_limit: Some(4096),
        particle: None,
    };
    let mut samples = Vec::with_capacity(ticks);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..ticks {
        let started = Instant::now();
        world
            .edit(|edit| {
                if combined {
                    edit.step_material_phase(config, &StandardMaterialForces)?;
                } else {
                    edit.relax_pressure(layer, pressure)?;
                    edit.transport_materials(layer, 4096)?;
                }
                edit.advance_tick()
            })
            .unwrap();
        samples.push(started.elapsed());
    }
    (
        samples,
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        PEAK_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(baseline_live),
    )
}

fn measure(
    world: &mut World,
    layer: LayerId,
    provider: &StaticProvider,
    tracker: Option<&mut MaterialProviderSleepTracker>,
    ticks: u64,
) -> (Vec<Duration>, usize, usize, usize) {
    let force = MaterialForceConfig {
        channel: key("bench:motion"),
        gravity_q16: [0, 0],
        candidate_limit: 4096,
        max_velocity_q16: 65536,
    };
    let motion = MaterialMotionConfig {
        channel: key("bench:motion"),
        candidate_limit: 4096,
        max_distance: 1,
        step_limit: 4096,
    };
    let mut tracker = tracker;
    let mut samples = Vec::with_capacity(ticks as usize);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..ticks {
        let started = Instant::now();
        if let Some(tracker) = tracker.as_mut() {
            world
                .step_material_force_motion_sleeping_with_provider(
                    tracker, layer, force, motion, provider,
                )
                .unwrap();
        } else {
            world
                .edit(|edit| edit.step_material_force_motion(layer, force, motion, provider))
                .unwrap();
        }
        world.edit(|edit| edit.advance_tick()).unwrap();
        samples.push(started.elapsed());
    }
    (
        samples,
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        PEAK_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(baseline_live),
    )
}

fn measure_particles(
    world: &mut World,
    layer: LayerId,
    tracker: Option<&mut MaterialParticleSleepTracker>,
    ticks: usize,
) -> (Vec<Duration>, usize, usize, usize) {
    let force = MaterialParticleForceConfig {
        gravity_q16: [0, 0],
        candidate_limit: 4096,
        max_velocity_q16: 65536,
    };
    let mut tracker = tracker;
    let mut samples = Vec::with_capacity(ticks);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..ticks {
        let started = Instant::now();
        if let Some(tracker) = tracker.as_mut() {
            world
                .step_material_particle_force_motion_sleeping(
                    tracker,
                    layer,
                    force,
                    1,
                    &StandardMaterialForces,
                )
                .unwrap();
        } else {
            world
                .edit(|edit| {
                    edit.step_material_particle_forces_in_layer(
                        layer,
                        force,
                        &StandardMaterialForces,
                        1,
                    )
                })
                .unwrap();
        }
        world.edit(|edit| edit.advance_tick()).unwrap();
        samples.push(started.elapsed());
    }
    (
        samples,
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        PEAK_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(baseline_live),
    )
}

#[test]
#[ignore = "静止4,096候補で作者Provider sleepの費用を手動比較する"]
fn material_force_provider_sleep_comparison() {
    const WARMUP: u64 = 20;
    const SAMPLES: u64 = 200;
    let (base, layer, content) = fixture();
    let checkpoint = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut sleeping = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = StaticProvider {
        calls: Arc::clone(&calls),
    };
    let mut tracker = MaterialProviderSleepTracker::default();

    let mut first_reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut first_sleep = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    calls.store(0, Ordering::Relaxed);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    let first_sleep_started = Instant::now();
    let first_report = first_sleep
        .step_material_force_motion_sleeping_with_provider(
            &mut tracker,
            layer,
            MaterialForceConfig {
                channel: key("bench:motion"),
                gravity_q16: [0, 0],
                candidate_limit: 4096,
                max_velocity_q16: 65536,
            },
            MaterialMotionConfig {
                channel: key("bench:motion"),
                candidate_limit: 4096,
                max_distance: 1,
                step_limit: 4096,
            },
            &provider,
        )
        .unwrap();
    let first_sleep_time = first_sleep_started.elapsed();
    let first_sleep_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let first_sleep_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let first_sleep_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    assert!(!first_report.sleeping);
    assert!(tracker.has_proof());

    calls.store(0, Ordering::Relaxed);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    let first_reference_started = Instant::now();
    let first_reference_report = first_reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                MaterialForceConfig {
                    channel: key("bench:motion"),
                    gravity_q16: [0, 0],
                    candidate_limit: 4096,
                    max_velocity_q16: 65536,
                },
                MaterialMotionConfig {
                    channel: key("bench:motion"),
                    candidate_limit: 4096,
                    max_distance: 1,
                    step_limit: 4096,
                },
                &provider,
            )
        })
        .unwrap();
    let first_reference_time = first_reference_started.elapsed();
    let first_reference_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let first_reference_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let first_reference_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    assert_eq!(first_report.reference, first_reference_report);
    assert_eq!(
        first_reference.checkpoint(4_000_000).unwrap(),
        first_sleep.checkpoint(4_000_000).unwrap()
    );
    println!(
        "first_step static_candidates=4096 reference={first_reference_time:?} allocations={first_reference_allocations} bytes={first_reference_bytes} peak_live_delta={first_reference_peak}B provider_sleep_proof={first_sleep_time:?} allocations={first_sleep_allocations} bytes={first_sleep_bytes} peak_live_delta={first_sleep_peak}B"
    );

    measure(&mut reference, layer, &provider, None, WARMUP);
    measure(&mut sleeping, layer, &provider, Some(&mut tracker), WARMUP);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );

    calls.store(0, Ordering::Relaxed);
    let (reference_samples, reference_allocations, reference_bytes, reference_peak) =
        measure(&mut reference, layer, &provider, None, SAMPLES);
    let reference_calls = calls.swap(0, Ordering::Relaxed);
    let (sleep_samples, sleep_allocations, sleep_bytes, sleep_peak) =
        measure(&mut sleeping, layer, &provider, Some(&mut tracker), SAMPLES);
    let sleep_calls = calls.load(Ordering::Relaxed);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );
    assert_eq!(reference_calls, 4096 * SAMPLES as usize);
    assert_eq!(sleep_calls, 0);
    println!(
        "static_candidates=4096 ticks={SAMPLES} reference_total={:?} p50={:?} p95={:?} p99={:?} allocations={reference_allocations} bytes={reference_bytes} peak_live_delta={reference_peak}B",
        reference_samples.iter().copied().sum::<Duration>(),
        percentile(&reference_samples, 50, 100),
        percentile(&reference_samples, 95, 100),
        percentile(&reference_samples, 99, 100),
    );
    println!(
        "static_candidates=4096 ticks={SAMPLES} provider_sleep_total={:?} p50={:?} p95={:?} p99={:?} allocations={sleep_allocations} bytes={sleep_bytes} peak_live_delta={sleep_peak}B",
        sleep_samples.iter().copied().sum::<Duration>(),
        percentile(&sleep_samples, 50, 100),
        percentile(&sleep_samples, 95, 100),
        percentile(&sleep_samples, 99, 100),
    );
}

#[test]
#[ignore = "静止4,096流体cellで圧力Jacobi sleepの費用を手動比較する"]
fn pressure_sleep_comparison() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 200;
    let (base, layer, content) = pressure_fixture();
    let checkpoint = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut sleeping = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = PressureSleepTracker::default();

    let mut first_reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut first_sleep = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let config = PressureRelaxation {
        channel: key("bench:pressure"),
        iterations: 4,
        gravity_q16: 0,
        cell_limit: 4096,
    };
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let first_report = first_sleep
        .relax_pressure_sleeping(&mut tracker, layer, config)
        .unwrap();
    let sleep_first = started.elapsed();
    let sleep_first_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let sleep_first_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let sleep_first_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    assert!(!first_report.sleeping);

    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let reference_report = first_reference
        .edit(|edit| edit.relax_pressure(layer, config))
        .unwrap();
    let reference_first = started.elapsed();
    let reference_first_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let reference_first_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let reference_first_peak = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    assert_eq!(first_report.reference, reference_report);
    assert_eq!(
        first_reference.checkpoint(4_000_000).unwrap(),
        first_sleep.checkpoint(4_000_000).unwrap()
    );
    println!(
        "pressure_first_step cells=4096 reference={reference_first:?} allocations={reference_first_allocations} bytes={reference_first_bytes} peak_live_delta={reference_first_peak}B sleep_proof={sleep_first:?} allocations={sleep_first_allocations} bytes={sleep_first_bytes} peak_live_delta={sleep_first_peak}B"
    );

    measure_pressure(&mut reference, layer, None, WARMUP);
    measure_pressure(&mut sleeping, layer, Some(&mut tracker), WARMUP);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );
    let (reference_samples, reference_allocations, reference_bytes, reference_peak) =
        measure_pressure(&mut reference, layer, None, SAMPLES);
    let (sleep_samples, sleep_allocations, sleep_bytes, sleep_peak) =
        measure_pressure(&mut sleeping, layer, Some(&mut tracker), SAMPLES);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        sleeping.checkpoint(4_000_000).unwrap()
    );
    println!(
        "pressure_cells=4096 ticks={SAMPLES} reference_total={:?} p50={:?} p95={:?} p99={:?} allocations={reference_allocations} bytes={reference_bytes} peak_live_delta={reference_peak}B",
        reference_samples.iter().copied().sum::<Duration>(),
        percentile(&reference_samples, 50, 100),
        percentile(&reference_samples, 95, 100),
        percentile(&reference_samples, 99, 100),
    );
    println!(
        "pressure_cells=4096 ticks={SAMPLES} sleep_total={:?} p50={:?} p95={:?} p99={:?} allocations={sleep_allocations} bytes={sleep_bytes} peak_live_delta={sleep_peak}B",
        sleep_samples.iter().copied().sum::<Duration>(),
        percentile(&sleep_samples, 50, 100),
        percentile(&sleep_samples, 95, 100),
        percentile(&sleep_samples, 99, 100),
    );
}

#[test]
#[ignore = "4,096液体cellでpressure+scalar参照経路と統合Materials phaseを手動比較する"]
fn integrated_material_phase_comparison() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 200;
    let (base, layer, content) = pressure_fixture();
    let checkpoint = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();
    let mut integrated = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();

    measure_material_phase(&mut reference, layer, false, WARMUP);
    measure_material_phase(&mut integrated, layer, true, WARMUP);
    assert_eq!(
        reference.checkpoint(checkpoint.len()).unwrap(),
        integrated.checkpoint(checkpoint.len()).unwrap()
    );
    let (reference_samples, reference_allocations, reference_bytes, reference_peak) =
        measure_material_phase(&mut reference, layer, false, SAMPLES);
    let (integrated_samples, integrated_allocations, integrated_bytes, integrated_peak) =
        measure_material_phase(&mut integrated, layer, true, SAMPLES);
    assert_eq!(
        reference.checkpoint(checkpoint.len()).unwrap(),
        integrated.checkpoint(checkpoint.len()).unwrap()
    );
    println!(
        "material_phase_cells=4096 ticks={SAMPLES} reference_total={:?} p50={:?} p95={:?} p99={:?} allocations={reference_allocations} bytes={reference_bytes} peak_live_delta={reference_peak}B",
        reference_samples.iter().copied().sum::<Duration>(),
        percentile(&reference_samples, 50, 100),
        percentile(&reference_samples, 95, 100),
        percentile(&reference_samples, 99, 100),
    );
    println!(
        "material_phase_cells=4096 ticks={SAMPLES} integrated_total={:?} p50={:?} p95={:?} p99={:?} allocations={integrated_allocations} bytes={integrated_bytes} peak_live_delta={integrated_peak}B",
        integrated_samples.iter().copied().sum::<Duration>(),
        percentile(&integrated_samples, 50, 100),
        percentile(&integrated_samples, 95, 100),
        percentile(&integrated_samples, 99, 100),
    );
}

#[test]
#[ignore = "静止正規粒子4,096個でforce/DDA sleepの費用を手動比較する"]
fn material_particle_sleep_comparison() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 200;
    let (base, layer, content) = particle_fixture();
    let checkpoint = base.checkpoint(4_000_000).unwrap();
    let mut reference = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();
    let mut sleeping = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();
    let force = MaterialParticleForceConfig {
        gravity_q16: [0, 0],
        candidate_limit: 4096,
        max_velocity_q16: 65536,
    };
    let mut tracker = MaterialParticleSleepTracker::default();

    let started = Instant::now();
    let reference_report = reference
        .edit(|edit| {
            edit.step_material_particle_forces_in_layer(layer, force, &StandardMaterialForces, 1)
        })
        .unwrap();
    let reference_first = started.elapsed();
    let started = Instant::now();
    let sleep_report = sleeping
        .step_material_particle_force_motion_sleeping(
            &mut tracker,
            layer,
            force,
            1,
            &StandardMaterialForces,
        )
        .unwrap();
    let proof_first = started.elapsed();
    assert!(!sleep_report.sleeping);
    assert!(tracker.has_proof());
    assert_eq!(sleep_report.reference, reference_report);
    reference.edit(|edit| edit.advance_tick()).unwrap();
    sleeping.edit(|edit| edit.advance_tick()).unwrap();
    assert_eq!(
        reference.checkpoint(checkpoint.len()).unwrap(),
        sleeping.checkpoint(checkpoint.len()).unwrap()
    );
    println!("particle_count=4096 first_reference={reference_first:?} first_proof={proof_first:?}");

    measure_particles(&mut reference, layer, None, WARMUP);
    measure_particles(&mut sleeping, layer, Some(&mut tracker), WARMUP);
    assert_eq!(
        reference.checkpoint(checkpoint.len()).unwrap(),
        sleeping.checkpoint(checkpoint.len()).unwrap()
    );
    let (reference_samples, reference_allocations, reference_bytes, reference_peak) =
        measure_particles(&mut reference, layer, None, SAMPLES);
    let (sleep_samples, sleep_allocations, sleep_bytes, sleep_peak) =
        measure_particles(&mut sleeping, layer, Some(&mut tracker), SAMPLES);
    assert_eq!(
        reference.checkpoint(checkpoint.len()).unwrap(),
        sleeping.checkpoint(checkpoint.len()).unwrap()
    );
    println!(
        "particle_count=4096 ticks={SAMPLES} reference_total={:?} p50={:?} p95={:?} p99={:?} allocations={reference_allocations} bytes={reference_bytes} peak_live_delta={reference_peak}B",
        reference_samples.iter().copied().sum::<Duration>(),
        percentile(&reference_samples, 50, 100),
        percentile(&reference_samples, 95, 100),
        percentile(&reference_samples, 99, 100),
    );
    println!(
        "particle_count=4096 ticks={SAMPLES} sleep_total={:?} p50={:?} p95={:?} p99={:?} allocations={sleep_allocations} bytes={sleep_bytes} peak_live_delta={sleep_peak}B",
        sleep_samples.iter().copied().sum::<Duration>(),
        percentile(&sleep_samples, 50, 100),
        percentile(&sleep_samples, 95, 100),
        percentile(&sleep_samples, 99, 100),
    );
}
