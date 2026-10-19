//! E05収支監査をRuntime Materials barrierへ接続する公開APIの統合試験。
use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        schema::Schema,
    },
    runtime::{
        Engine, Scene, SceneContext, StepResult,
        config::{EngineSettingsPatch, SimulationPatch, WorkerCount},
        input::TickInput,
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
    },
    world::*,
};
use std::sync::Arc;
use std::{
    alloc::{GlobalAlloc, Layout, System as SystemAllocator},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct RuntimeProfileAllocator;
static MEASURE_RUNTIME_PROFILE: AtomicBool = AtomicBool::new(false);
static PROFILE_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PROFILE_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_BYTES: AtomicUsize = AtomicUsize::new(0);
static PROFILE_LARGE_ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static PROFILE_MAX_ALLOCATION: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_COUNT_1M: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_BYTES_1M: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_COUNT_16M: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATION_BYTES_16M: AtomicUsize = AtomicUsize::new(0);

fn record_runtime_profile_allocation(size: usize) {
    PROFILE_ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    PROFILE_ALLOCATION_BYTES.fetch_add(size, Ordering::Relaxed);
    if size >= 64 * 1024 {
        PROFILE_LARGE_ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if size >= 1024 * 1024 {
        PROFILE_ALLOCATION_COUNT_1M.fetch_add(1, Ordering::Relaxed);
        PROFILE_ALLOCATION_BYTES_1M.fetch_add(size, Ordering::Relaxed);
    }
    if size >= 16 * 1024 * 1024 {
        PROFILE_ALLOCATION_COUNT_16M.fetch_add(1, Ordering::Relaxed);
        PROFILE_ALLOCATION_BYTES_16M.fetch_add(size, Ordering::Relaxed);
    }
    PROFILE_MAX_ALLOCATION.fetch_max(size, Ordering::Relaxed);
}

#[global_allocator]
static RUNTIME_PROFILE_ALLOCATOR: RuntimeProfileAllocator = RuntimeProfileAllocator;

// SAFETY: 実メモリ操作は有効な引数のまま標準allocatorへ委譲し、atomic計測値だけを更新する。
unsafe impl GlobalAlloc for RuntimeProfileAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: 有効な割当条件を標準allocatorへ委譲する。
        let pointer = unsafe { SystemAllocator.alloc(layout) };
        if !pointer.is_null() {
            let live =
                PROFILE_LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PROFILE_PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
            if MEASURE_RUNTIME_PROFILE.load(Ordering::Relaxed) {
                record_runtime_profile_allocation(layout.size());
            }
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if !pointer.is_null() {
            PROFILE_LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: pointer/layoutはこのallocatorから受け取った組である。
        unsafe { SystemAllocator.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: 有効なpointer/layoutを標準allocatorへ委譲する。
        let resized = unsafe { SystemAllocator.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            let increase = new_size.saturating_sub(layout.size());
            let decrease = layout.size().saturating_sub(new_size);
            let live = PROFILE_LIVE_BYTES.fetch_add(increase, Ordering::Relaxed) + increase;
            PROFILE_LIVE_BYTES.fetch_sub(decrease, Ordering::Relaxed);
            PROFILE_PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
            if MEASURE_RUNTIME_PROFILE.load(Ordering::Relaxed) {
                record_runtime_profile_allocation(new_size);
            }
        }
        resized
    }
}

struct RuntimeProfileSample {
    tick: u64,
    elapsed: Duration,
    allocations: usize,
    allocated_bytes: usize,
    large_allocations: usize,
    allocations_1m: usize,
    allocated_bytes_1m: usize,
    allocations_16m: usize,
    allocated_bytes_16m: usize,
    max_allocation: usize,
    peak_live_delta: usize,
}

fn begin_runtime_profile_sample() -> (Instant, usize) {
    let baseline_live = PROFILE_LIVE_BYTES.load(Ordering::Relaxed);
    PROFILE_PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    PROFILE_ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    PROFILE_ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    PROFILE_LARGE_ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    PROFILE_MAX_ALLOCATION.store(0, Ordering::Relaxed);
    PROFILE_ALLOCATION_COUNT_1M.store(0, Ordering::Relaxed);
    PROFILE_ALLOCATION_BYTES_1M.store(0, Ordering::Relaxed);
    PROFILE_ALLOCATION_COUNT_16M.store(0, Ordering::Relaxed);
    PROFILE_ALLOCATION_BYTES_16M.store(0, Ordering::Relaxed);
    MEASURE_RUNTIME_PROFILE.store(true, Ordering::Relaxed);
    (Instant::now(), baseline_live)
}

fn finish_runtime_profile_sample(
    started: Instant,
    baseline_live: usize,
    tick: u64,
) -> RuntimeProfileSample {
    let elapsed = started.elapsed();
    MEASURE_RUNTIME_PROFILE.store(false, Ordering::Relaxed);
    RuntimeProfileSample {
        tick,
        elapsed,
        allocations: PROFILE_ALLOCATION_COUNT.load(Ordering::Relaxed),
        allocated_bytes: PROFILE_ALLOCATION_BYTES.load(Ordering::Relaxed),
        large_allocations: PROFILE_LARGE_ALLOCATION_COUNT.load(Ordering::Relaxed),
        allocations_1m: PROFILE_ALLOCATION_COUNT_1M.load(Ordering::Relaxed),
        allocated_bytes_1m: PROFILE_ALLOCATION_BYTES_1M.load(Ordering::Relaxed),
        allocations_16m: PROFILE_ALLOCATION_COUNT_16M.load(Ordering::Relaxed),
        allocated_bytes_16m: PROFILE_ALLOCATION_BYTES_16M.load(Ordering::Relaxed),
        max_allocation: PROFILE_MAX_ALLOCATION.load(Ordering::Relaxed),
        peak_live_delta: PROFILE_PEAK_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(baseline_live),
    }
}

fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn profile() -> MaterialInventoryProfile {
    MaterialInventoryProfile::new(
        key("test:inventory"),
        1,
        Vec::new(),
        Vec::new(),
        MaterialInventoryLimits {
            atoms: 128,
            components: 8,
        },
    )
    .unwrap()
}
struct NoArchives;
impl ArchiveStorage for NoArchives {
    fn put(&mut self, _hash: [u8; 32], _bytes: &[u8]) -> EngineResult<SaveDurability> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "テストにarchiveなし",
        ))
    }
    fn read(&mut self, _hash: [u8; 32], _limit: usize) -> EngineResult<Vec<u8>> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "テストにarchiveなし",
        ))
    }
}

struct RuntimeInventoryProvider {
    provider_key: StableKey,
    record: StableKey,
    material: StableKey,
}
impl MaterialInventoryProvider for RuntimeInventoryProvider {
    fn key(&self) -> StableKey {
        self.provider_key
    }
    fn version(&self) -> u32 {
        1
    }
    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        match world.state::<u64>(self.record)? {
            0 => Ok(()),
            1 => output.add(
                world.content().layer_id(key("test:layer"))?,
                MaterialHandle(self.material),
                0,
                MixtureRef::NONE,
                MaterialQuantity::ONE,
            ),
            _ => Err(EngineError::new(
                ErrorCode::InvalidValue,
                "inventory Providerの試験fixture所有数",
            )),
        }
    }
}

fn runtime_multi_provider_profile() -> Arc<MaterialInventoryProfile> {
    let providers: Vec<Arc<dyn MaterialInventoryProvider>> = vec![
        Arc::new(RuntimeInventoryProvider {
            provider_key: key("test:body-inventory"),
            record: key("test:body-owned"),
            material: key("test:body-token"),
        }),
        Arc::new(RuntimeInventoryProvider {
            provider_key: key("test:reservoir-inventory"),
            record: key("test:reservoir-owned"),
            material: key("test:reservoir-token"),
        }),
    ];
    Arc::new(
        MaterialInventoryProfile::new(
            key("test:runtime-multi-provider-inventory"),
            1,
            Vec::new(),
            providers,
            MaterialInventoryLimits {
                atoms: 256,
                components: 8,
            },
        )
        .unwrap(),
    )
}
fn build_content() -> Arc<Content> {
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
        .register(MaterialBuilder::new("test:sand", MaterialBase::Powder).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:water", MaterialBase::Liquid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:wall", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:body-token", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:reservoir-token", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialBalanceLog>(key("test:balance-schema"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(
            Schema::of::<ScalarMaterialSleepTracker>(key("test:scalar-sleep-schema"), 1, 1_000_000)
                .unwrap(),
        )
        .unwrap();
    registry
        .schema(
            Schema::of::<MaterialParticleSleepTracker>(
                key("test:particle-sleep-schema"),
                1,
                1_000_000,
            )
            .unwrap(),
        )
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:debug-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(
            Schema::of::<MaterialMotionJob>(key("test:motion-job-schema"), 1, 1_000_000).unwrap(),
        )
        .unwrap();
    registry
        .schema(
            Schema::of::<MaterialParticleJob>(key("test:particle-job-schema"), 1, 1_000_000)
                .unwrap(),
        )
        .unwrap();
    registry
        .schema(Schema::of::<i64>(key("test:i64-schema"), 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<PressureJob>(key("test:pressure-job-schema"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(
            Schema::of::<PressurePushSearch>(key("test:push-job-schema"), 1, 1_000_000).unwrap(),
        )
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-schema"),
                layout: GridLayout {
                    origin: p(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MaterialMotion::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    for channel in ["test:pressure", "test:pump"] {
        registry
            .channel(ChannelDefinition {
                key: key(channel),
                version: 1,
                value_schema: key("test:i64-schema"),
                layout: GridLayout {
                    origin: p(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0i64,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            })
            .unwrap();
    }
    registry.freeze().unwrap()
}

struct AuditedMaterialSystem {
    profile: Arc<MaterialInventoryProfile>,
    worker_barrier: Option<Arc<std::sync::Barrier>>,
    inject_unrecorded_source: bool,
    swallow_nested_error: bool,
    unaccounted_raw: bool,
    unaccounted_raw_state: bool,
    recorded_source: bool,
    raw_then_audited: bool,
    apply_pipeline: bool,
    sleep_pipeline: bool,
    particle_sleep_pipeline: bool,
    particle_sleep_cache: Arc<std::sync::Mutex<Option<MaterialParticleSleepTracker>>>,
}
impl System for AuditedMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        if let Some(barrier) = &self.worker_barrier {
            barrier.wait();
        }
        let cached_particle_tracker = self
            .particle_sleep_cache
            .lock()
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "粒子sleep test mutex"))?
            .take();
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("world")?;
        let (archive, layer, material, tracker, particle_tracker) = {
            let view = worlds.view();
            let terrain = view.terrain(id)?;
            (
                terrain.load_material_archive_inventory(
                    &self.profile,
                    &mut NoArchives,
                    1_000_000,
                )?,
                terrain.content().layer_id(key("test:layer"))?,
                terrain
                    .content()
                    .material_id(MaterialHandle(key("test:sand")))?,
                terrain.state::<ScalarMaterialSleepTracker>(key("test:scalar-sleep"))?,
                if self.particle_sleep_pipeline {
                    Some(match cached_particle_tracker {
                        Some(tracker) => tracker,
                        None => terrain
                            .state::<MaterialParticleSleepTracker>(key("test:particle-sleep"))?,
                    })
                } else {
                    None
                },
            )
        };
        let profile = Arc::clone(&self.profile);
        let fail = self.inject_unrecorded_source;
        let swallow = self.swallow_nested_error;
        let recorded_source = self.recorded_source;
        let apply_pipeline = self.apply_pipeline;
        let sleep_pipeline = self.sleep_pipeline;
        let particle_sleep_pipeline = self.particle_sleep_pipeline;
        let particle_sleep_cache = Arc::clone(&self.particle_sleep_cache);
        if self.raw_then_audited {
            // 同tickの先行barrierが物質を未記録で発行する。後続の監査付き
            // 原子取引はその改竄を見逃さず、Runtime全体の候補を破棄する。
            worlds.edit_terrain(id, move |edit| {
                edit.set_cell(
                    layer,
                    p(3, 1),
                    CellCore {
                        material,
                        ..CellCore::AIR
                    },
                )
            })?;
        }
        if self.unaccounted_raw_state {
            worlds.set(
                id,
                key("test:debug-record"),
                key("test:debug-schema"),
                &7u64,
            )?;
            return Ok(WorkStatus::Complete);
        }
        if self.unaccounted_raw {
            worlds.edit_terrain(id, move |edit| {
                edit.set_cell(
                    layer,
                    p(3, 1),
                    CellCore {
                        material,
                        ..CellCore::AIR
                    },
                )
            })?;
            return Ok(WorkStatus::Complete);
        }
        worlds.edit_material_accounted(
            id,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile,
                archives: archive,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: recorded_source.then_some(MaterialChangeReason {
                    key: key("test:explicit-source"),
                    version: 1,
                    kind: MaterialReceiptKind::Source,
                }),
            },
            move |edit| {
                if particle_sleep_pipeline {
                    let particle_config = MaterialPhaseConfig {
                        layer,
                        pressure: None,
                        motion: None,
                        scalar_candidate_limit: None,
                        particle: Some((
                            MaterialParticleForceConfig {
                                gravity_q16: [0, 0],
                                candidate_limit: 16,
                                max_velocity_q16: 65536,
                            },
                            8,
                        )),
                    };
                    let (_, tracker, sleep) = edit.step_material_phase_with_particle_sleep(
                        particle_config,
                        &StandardMaterialForces,
                        particle_tracker.expect("粒子sleep trackerを準備する"),
                    )?;
                    edit.set_state(
                        key("test:particle-sleep"),
                        key("test:particle-sleep-schema"),
                        &tracker,
                    )?;
                    edit.set_state(
                        key("test:particle-sleep-result"),
                        key("test:debug-schema"),
                        &(u64::from(sleep.sleeping) | (u64::from(sleep.woke) << 1)),
                    )?;
                    *particle_sleep_cache.lock().map_err(|_| {
                        EngineError::new(ErrorCode::InvalidState, "粒子sleep test mutex")
                    })? = Some(tracker.clone());
                    return Ok(());
                }
                if apply_pipeline {
                    let config = MaterialPhaseConfig {
                        layer,
                        pressure: None,
                        motion: Some((
                            MaterialForceConfig {
                                channel: key("test:motion"),
                                gravity_q16: [0, 0],
                                candidate_limit: 16,
                                max_velocity_q16: 65536,
                            },
                            MaterialMotionConfig {
                                channel: key("test:motion"),
                                candidate_limit: 16,
                                max_distance: 8,
                                step_limit: 128,
                            },
                        )),
                        scalar_candidate_limit: Some(16),
                        particle: None,
                    };
                    if sleep_pipeline {
                        let (_, tracker) = edit.step_material_phase_with_scalar_sleep(
                            config,
                            &StandardMaterialForces,
                            tracker,
                        )?;
                        edit.set_state(
                            key("test:scalar-sleep"),
                            key("test:scalar-sleep-schema"),
                            &tracker,
                        )?;
                    } else {
                        edit.step_material_phase(config, &StandardMaterialForces)?;
                    }
                    return Ok(());
                }
                edit.transport_path(layer, &[p(2, 1), p(1, 1)])?;
                if swallow {
                    // 意図的にErrを無視しても、WorldEditは失敗を覚えており
                    // 先行の正常な物質移動をcommitしてはならない。
                    let _ = edit.set_cell(
                        layer,
                        p(1024, 1024),
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    );
                }
                if fail || recorded_source {
                    edit.set_cell(
                        layer,
                        p(3, 1),
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::Complete)
    }
}

#[derive(Clone, Copy, Default)]
struct MaterialRuntimeOptions {
    inject_unrecorded_source: bool,
    swallow_nested_error: bool,
    unaccounted_raw: bool,
    unaccounted_raw_state: bool,
    recorded_source: bool,
    raw_then_audited: bool,
    apply_pipeline: bool,
    sleep_pipeline: bool,
    particle_sleep_pipeline: bool,
}

fn engine_with_options(
    options: MaterialRuntimeOptions,
    workers: Option<u16>,
    worker_barrier: Option<Arc<std::sync::Barrier>>,
    observer_calls: Option<Arc<std::sync::atomic::AtomicUsize>>,
) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    let content = build_content();
    let profile = Arc::new(profile());
    let init_profile = Arc::clone(&profile);
    let particle_sleep_pipeline = options.particle_sleep_pipeline;
    let initial_material = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let builder = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        let layer = world.content().layer_id(key("test:layer"))?;
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: initial_material,
                    ..CellCore::AIR
                },
            )?;
            if particle_sleep_pipeline {
                edit.emit_material_particle(layer, p(1, 1), [0, 0])?;
                edit.set_cell(
                    layer,
                    p(1, 1),
                    CellCore {
                        material: initial_material,
                        ..CellCore::AIR
                    },
                )?;
            }
            Ok(())
        })?;
        let archive =
            world.load_material_archive_inventory(&init_profile, &mut NoArchives, 1_000_000)?;
        world.initialize_material_accounting(
            &init_profile,
            &archive,
            key("test:balance"),
            key("test:balance-schema"),
            8,
        )?;
        world.edit(|edit| {
            edit.set_state(
                key("test:scalar-sleep"),
                key("test:scalar-sleep-schema"),
                &ScalarMaterialSleepTracker::default(),
            )?;
            edit.set_state(
                key("test:particle-sleep"),
                key("test:particle-sleep-schema"),
                &MaterialParticleSleepTracker::default(),
            )
        })
    });
    let mut descriptor = SystemDescriptor::new(key("test:audited-materials"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:debug-record"))
        .in_world(key("game:world"));
    if observer_calls.is_some() {
        descriptor = descriptor.parallel();
    }
    let mut runtime_builder = Engine::builder().sandbox_world("world", builder);
    if let Some(workers) = workers {
        runtime_builder = runtime_builder.settings(EngineSettingsPatch {
            simulation: Some(SimulationPatch {
                system_workers: Some(WorkerCount::Count(workers)),
                ..SimulationPatch::default()
            }),
            ..EngineSettingsPatch::default()
        });
    }
    runtime_builder = runtime_builder.system(
        descriptor,
        AuditedMaterialSystem {
            profile: Arc::clone(&profile),
            worker_barrier: worker_barrier.as_ref().map(Arc::clone),
            inject_unrecorded_source: options.inject_unrecorded_source,
            swallow_nested_error: options.swallow_nested_error,
            unaccounted_raw: options.unaccounted_raw,
            unaccounted_raw_state: options.unaccounted_raw_state,
            recorded_source: options.recorded_source,
            raw_then_audited: options.raw_then_audited,
            apply_pipeline: options.apply_pipeline,
            sleep_pipeline: options.sleep_pipeline,
            particle_sleep_pipeline: options.particle_sleep_pipeline,
            particle_sleep_cache: Arc::new(std::sync::Mutex::new(None)),
        },
    );
    if let Some(calls) = observer_calls {
        runtime_builder = runtime_builder.system(
            SystemDescriptor::new(key("test:independent-observer"), Phase::Materials).parallel(),
            IndependentObserver {
                worker_barrier,
                calls,
            },
        );
    }
    let runtime = runtime_builder.build().unwrap();
    (runtime, content, profile)
}
fn engine_with_failure(
    options: MaterialRuntimeOptions,
) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    engine_with_options(options, None, None, None)
}

struct IndependentObserver {
    worker_barrier: Option<Arc<std::sync::Barrier>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl System for IndependentObserver {
    fn run(&mut self, _: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        if let Some(barrier) = &self.worker_barrier {
            barrier.wait();
        }
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(WorkStatus::Complete)
    }
}
fn engine(fail: bool) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    engine_with_failure(MaterialRuntimeOptions {
        inject_unrecorded_source: fail,
        ..MaterialRuntimeOptions::default()
    })
}
fn at(world: &World, layer: LayerId, point: CellPoint) -> MaterialId {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(value) => value.core.material,
        _ => panic!("この試験のresidentが失われた"),
    }
}

#[test]
fn runtime_material_barrier_audits_mass_and_publishes_on_same_tick() {
    let (mut runtime, content, profile) = engine(false);
    let id = runtime.worlds().id("world").unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let terrain = runtime.worlds().terrain(id).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(at(terrain, layer, p(1, 1)), MaterialId::AIR);
    assert_eq!(at(terrain, layer, p(2, 1)), sand);
    assert_eq!(terrain.tick(), 1);
    let archive = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    let saved = runtime.worlds().checkpoint(id).unwrap();
    let restored = World::restore(&saved, content, WorldLimits::default(), saved.len()).unwrap();
    restored
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_rejects_unaccounted_source_and_rolls_back_entire_tick() {
    let (mut runtime, content, profile) = engine(true);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let world = runtime.worlds().terrain(id).unwrap();
    let archive = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        at(world, layer, p(1, 1)),
        content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
}

#[test]
fn ignored_nested_edit_error_cannot_publish_a_partial_material_tick() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        swallow_nested_error: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        at(terrain, layer, p(1, 1)),
        content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(at(terrain, layer, p(2, 1)), MaterialId::AIR);
    let archives = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &archives, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_raw_material_edit_remains_available_but_is_detected_by_explicit_audit() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        unaccounted_raw: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(runtime.worlds().tick(id).unwrap(), 1);
    let world = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_ne!(at(world, layer, p(3, 1)), MaterialId::AIR);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    assert!(
        world
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .is_err()
    );
}

#[test]
fn runtime_explicit_material_source_writes_audited_receipt_and_restores() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        recorded_source: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let world = runtime.worlds().terrain(id).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(world, layer, p(2, 1)), sand);
    assert_eq!(at(world, layer, p(3, 1)), sand);
    let log: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    assert_eq!(log.receipts().len(), 1);
    assert_eq!(log.receipts()[0].reason().kind, MaterialReceiptKind::Source);
    // Materials barrierの確定時はtick0で、終了時にRuntimeがtick1へ進める。
    assert_eq!(log.receipts()[0].tick(), 0);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    let bytes = runtime.worlds().checkpoint(id).unwrap();
    let restored = World::restore(&bytes, content, WorldLimits::default(), bytes.len()).unwrap();
    restored
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_unrelated_author_state_put_remains_available_with_accounting() {
    let (mut runtime, _content, profile) = engine_with_failure(MaterialRuntimeOptions {
        unaccounted_raw_state: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(runtime.worlds().tick(id).unwrap(), 1);
    let terrain = runtime.worlds().terrain(id).unwrap();
    assert_eq!(terrain.state::<u64>(key("test:debug-record")).unwrap(), 7);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn raw_then_accounted_in_one_materials_barrier_rolls_back_entire_runtime_tick() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        raw_then_audited: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    // raw編集の直後にaccounted編集を同じMaterials barrierへ予約。
    // 台帳との差を後続のpreflightで検知し、raw編集を含む全候補をrollback。
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(terrain, layer, p(3, 1)), MaterialId::AIR);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_materials_phase_pipeline_moves_sand_and_keeps_accounting_over_ticks() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        apply_pipeline: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    for tick in 1..=3 {
        assert_eq!(
            runtime.step(TickInput::empty(tick)).unwrap(),
            StepResult::Complete(tick)
        );
        let terrain = runtime.worlds().terrain(id).unwrap();
        assert_eq!(terrain.tick(), tick);
        assert_eq!(at(terrain, layer, p(1, tick as i32 + 1)), sand);
        let cache = terrain
            .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
            .unwrap();
        terrain
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .unwrap();
        let snapshot = runtime.worlds().checkpoint(id).unwrap();
        let restored = World::restore(
            &snapshot,
            Arc::clone(&content),
            WorldLimits::default(),
            snapshot.len(),
        )
        .unwrap();
        assert_eq!(restored.checkpoint(snapshot.len()).unwrap(), snapshot);
    }
}

#[test]
fn runtime_material_phase_matches_across_worker_counts_with_parallel_sibling() {
    let mut expected: Option<Vec<Vec<u8>>> = None;
    for workers in [1_u16, 2, 4] {
        let barrier = (workers > 1).then(|| Arc::new(std::sync::Barrier::new(2)));
        let observer_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (mut runtime, content, profile) = engine_with_options(
            MaterialRuntimeOptions {
                apply_pipeline: true,
                ..MaterialRuntimeOptions::default()
            },
            Some(workers),
            barrier,
            Some(Arc::clone(&observer_calls)),
        );
        let id = runtime.worlds().id("world").unwrap();
        let layer = content.layer_id(key("test:layer")).unwrap();
        let sand = content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap();
        let mut checkpoints = Vec::new();
        for tick in 1..=3 {
            assert_eq!(
                runtime.step(TickInput::empty(tick)).unwrap(),
                StepResult::Complete(tick)
            );
            assert_eq!(
                observer_calls.load(std::sync::atomic::Ordering::Relaxed),
                tick as usize
            );
            let terrain = runtime.worlds().terrain(id).unwrap();
            assert_eq!(terrain.tick(), tick);
            assert_eq!(at(terrain, layer, p(1, tick as i32 + 1)), sand);
            let archive = terrain
                .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
                .unwrap();
            terrain
                .verify_material_accounting(&profile, &archive, key("test:balance"))
                .unwrap();
            let checkpoint = runtime.worlds().checkpoint(id).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(
                    &checkpoint,
                    &expected[tick as usize - 1],
                    "Materials tick {tick}と独立Systemの結果はworker数{workers}でも同じ"
                );
            } else {
                checkpoints.push(checkpoint);
            }
        }
        if workers == 1 {
            expected = Some(checkpoints);
        }
    }
}

#[test]
fn runtime_scalar_sleep_phase_stores_tracker_inside_the_audited_candidate() {
    let (mut runtime, content, profile) = engine_with_failure(MaterialRuntimeOptions {
        apply_pipeline: true,
        sleep_pipeline: true,
        ..MaterialRuntimeOptions::default()
    });
    let id = runtime.worlds().id("world").unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    for tick in 1..=3 {
        assert_eq!(
            runtime.step(TickInput::empty(tick)).unwrap(),
            StepResult::Complete(tick)
        );
        let terrain = runtime.worlds().terrain(id).unwrap();
        assert_eq!(terrain.tick(), tick);
        assert_eq!(at(terrain, layer, p(1, tick as i32 + 1)), sand);
        let tracker = terrain
            .state::<ScalarMaterialSleepTracker>(key("test:scalar-sleep"))
            .unwrap();
        assert!(!tracker.has_proof(), "移動があるscalarをsleep証明しない");
        let archive = terrain
            .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
            .unwrap();
        terrain
            .verify_material_accounting(&profile, &archive, key("test:balance"))
            .unwrap();
        let checkpoint = runtime.worlds().checkpoint(id).unwrap();
        let restored = World::restore(
            &checkpoint,
            Arc::clone(&content),
            WorldLimits::default(),
            checkpoint.len(),
        )
        .unwrap();
        assert_eq!(restored.checkpoint(checkpoint.len()).unwrap(), checkpoint);
    }
}

#[test]
fn runtime_particle_sleep_phase_persists_tracker_and_skips_later_force_steps() {
    let (mut runtime, content, profile) = engine_with_options(
        MaterialRuntimeOptions {
            particle_sleep_pipeline: true,
            ..MaterialRuntimeOptions::default()
        },
        None,
        None,
        None,
    );
    let id = runtime.worlds().id("world").unwrap();

    for tick in 1..=3 {
        assert_eq!(
            runtime.step(TickInput::empty(tick)).unwrap(),
            StepResult::Complete(tick)
        );
        let terrain = runtime.worlds().terrain(id).unwrap();
        assert_eq!(terrain.tick(), tick);
        assert_eq!(terrain.material_particles().count(), 1);
        assert_eq!(
            terrain
                .state::<u64>(key("test:particle-sleep-result"))
                .unwrap(),
            u64::from(tick > 1),
            "最初の参照tick後だけsleep経路へ入る (tick={tick}, revision={})",
            terrain.revision()
        );
        assert!(
            terrain
                .state::<MaterialParticleSleepTracker>(key("test:particle-sleep"))
                .unwrap()
                .has_proof()
        );

        let archive = terrain
            .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
            .unwrap();
        terrain
            .verify_material_accounting(&profile, &archive, key("test:balance"))
            .unwrap();
        let checkpoint = runtime.worlds().checkpoint(id).unwrap();
        let restored = World::restore(
            &checkpoint,
            Arc::clone(&content),
            WorldLimits::default(),
            checkpoint.len(),
        )
        .unwrap();
        assert_eq!(restored.checkpoint(checkpoint.len()).unwrap(), checkpoint);
    }
}

/// 同じMaterials phaseで別Systemが別の監査receiptを出す。barrier間で
/// 前の結果を確認し、後段が失敗すると両Systemの編集が全て巻き戻る。
struct ChainedMaterialSystem {
    profile: Arc<MaterialInventoryProfile>,
    issue_source: bool,
    inject_failure: bool,
}
impl System for ChainedMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut access = ctx.worlds();
        let id = access.view().id("world")?;
        let terrain = access.view().terrain(id)?;
        let layer = terrain.content().layer_id(key("test:layer"))?;
        let sand = terrain
            .content()
            .material_id(MaterialHandle(key("test:sand")))?;
        if !self.issue_source {
            // 後段Systemのrun開始時点で、前段barrierの変更を読む。
            // phase終端で2commandをまとめてflushしただけでは通らない。
            if at(terrain, layer, p(4, 4)) != sand {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "前段Materials barrierの地形変更が見えない",
                ));
            }
            let log: MaterialBalanceLog = terrain.state(key("test:balance"))?;
            if log.receipts().len() != 1
                || log.receipts()[0].reason().kind != MaterialReceiptKind::Source
            {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "前段Materials barrierのsource receiptが見えない",
                ));
            }
        }
        let archive =
            terrain.load_material_archive_inventory(&self.profile, &mut NoArchives, 1_000_000)?;
        let source = self.issue_source;
        let fail = self.inject_failure;
        let profile = Arc::clone(&self.profile);
        access.edit_material_accounted(
            id,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile,
                archives: archive,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: Some(MaterialChangeReason {
                    key: key(if source {
                        "test:phase-source"
                    } else {
                        "test:phase-sink"
                    }),
                    version: 1,
                    kind: if source {
                        MaterialReceiptKind::Source
                    } else {
                        MaterialReceiptKind::Sink
                    },
                }),
            },
            move |edit| {
                edit.set_cell(
                    layer,
                    p(4, 4),
                    CellCore {
                        material: if source { sand } else { MaterialId::AIR },
                        ..CellCore::AIR
                    },
                )?;
                if fail {
                    edit.set_cell(
                        layer,
                        p(9999, 9999),
                        CellCore {
                            material: sand,
                            ..CellCore::AIR
                        },
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::Complete)
    }
}

fn chained_runtime(fail_second: bool) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    let content = build_content();
    let profile = Arc::new(profile());
    let init_profile = Arc::clone(&profile);
    let initial_material = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        let layer = world.content().layer_id(key("test:layer"))?;
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: initial_material,
                    ..CellCore::AIR
                },
            )
        })?;
        let cache =
            world.load_material_archive_inventory(&init_profile, &mut NoArchives, 1_000_000)?;
        world.initialize_material_accounting(
            &init_profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            8,
        )
    });
    let source = SystemDescriptor::new(key("test:material-source-system"), Phase::Materials)
        .write(key("engine:terrain"))
        .in_world(key("game:world"))
        .barrier();
    let sink = SystemDescriptor::new(key("test:material-sink-system"), Phase::Materials)
        .after(key("test:material-source-system"))
        .write(key("engine:terrain"))
        .in_world(key("game:world"))
        .barrier();
    let runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(
            source,
            ChainedMaterialSystem {
                profile: Arc::clone(&profile),
                issue_source: true,
                inject_failure: false,
            },
        )
        .system(
            sink,
            ChainedMaterialSystem {
                profile: Arc::clone(&profile),
                issue_source: false,
                inject_failure: fail_second,
            },
        )
        .build()
        .unwrap();
    (runtime, content, profile)
}

#[test]
fn multiple_runtime_material_systems_commit_in_declared_order_with_two_receipts() {
    let (mut runtime, content, profile) = chained_runtime(false);
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(terrain, layer, p(4, 4)), MaterialId::AIR);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    let log: MaterialBalanceLog = terrain.state(key("test:balance")).unwrap();
    assert_eq!(log.receipts().len(), 2);
    assert_eq!(log.receipts()[0].reason().kind, MaterialReceiptKind::Source);
    assert_eq!(log.receipts()[1].reason().kind, MaterialReceiptKind::Sink);
    assert_eq!(terrain.tick(), 1);
}

#[test]
fn second_materials_system_error_rolls_back_first_system_and_clock() {
    let (mut runtime, content, profile) = chained_runtime(true);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    let world = runtime.worlds().terrain(id).unwrap();
    assert_eq!(world.tick(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(world, layer, p(4, 4)), MaterialId::AIR);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[derive(Clone)]
struct NoGravity;
impl MaterialForceProvider for NoGravity {
    fn key(&self) -> StableKey {
        key("test:runtime-no-gravity")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        Ok(input.motion)
    }
}

#[derive(Clone)]
struct SandDrift;
impl MaterialForceProvider for SandDrift {
    fn key(&self) -> StableKey {
        key("test:runtime-sand-drift")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        let mut motion = input.motion;
        if input.point == p(40, 40) {
            motion.velocity_q16[0] = 65536;
        }
        Ok(motion)
    }
}

#[derive(Clone)]
struct RuntimeForceProviders {
    water: NoGravity,
    sand: SandDrift,
    water_calls: Arc<std::sync::atomic::AtomicUsize>,
    sand_calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl MaterialForceProvider for RuntimeForceProviders {
    fn key(&self) -> StableKey {
        key("test:runtime-force-profile")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        match input.material.key() {
            material if material == key("test:water") => {
                self.water_calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.water.integrate(input)
            }
            material if material == key("test:sand") => {
                self.sand_calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.sand.integrate(input)
            }
            _ => Ok(input.motion),
        }
    }
}

struct SameTickMaterialSystem {
    calls: u64,
    config: MaterialPhaseConfig,
}
impl System for SameTickMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        self.calls += 1;
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        worlds.set(
            world,
            key("test:debug-record"),
            key("test:debug-schema"),
            &self.calls,
        )?;
        if self.calls == 1 {
            let config = self.config;
            worlds.edit_terrain(world, move |edit| {
                edit.step_material_phase(config, &NoGravity)?;
                Ok(())
            })?;
            Ok(WorkStatus::ContinueSameTick)
        } else {
            Ok(WorkStatus::Complete)
        }
    }
}

#[test]
fn runtime_same_tick_resume_runs_material_phase_once_before_clock_commit() {
    let content = build_content();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                p(1, 1),
                MaterialMotion {
                    velocity_q16: [65536, 0],
                    remainder_q16: [0, 0],
                },
            )
        })
    });
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((
            MaterialForceConfig {
                channel: key("test:motion"),
                gravity_q16: [0, 0],
                candidate_limit: 16,
                max_velocity_q16: 1_000_000,
            },
            MaterialMotionConfig {
                channel: key("test:motion"),
                candidate_limit: 16,
                max_distance: 4,
                step_limit: 32,
            },
        )),
        scalar_candidate_limit: None,
        particle: None,
    };
    let descriptor = SystemDescriptor::new(key("test:material-same-tick"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:debug-record"))
        .in_world(key("game:world"))
        .barrier();
    let mut runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(descriptor, SameTickMaterialSystem { calls: 0, config })
        .work_budget(1)
        .build()
        .unwrap();
    let world_id = runtime.worlds().id("world").unwrap();

    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Pending(1)
    );
    assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
    assert_eq!(runtime.resume().unwrap(), StepResult::Complete(1));
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(terrain.tick(), 1);
    assert_eq!(terrain.state::<u64>(key("test:debug-record")).unwrap(), 2);
    assert_eq!(at(terrain, layer, p(1, 1)), MaterialId::AIR);
    assert_eq!(at(terrain, layer, p(2, 1)), sand);
}

/// Job本体をSystemの一時fieldへ隠さず、各barrierでWorld StateStoreへ保存します。
struct PersistedMotionJobSystem {
    layer: LayerId,
    source: CellPoint,
}
impl System for PersistedMotionJobSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        let (job, status) = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            let mut job = match terrain.state::<MaterialMotionJob>(key("test:motion-job")) {
                Ok(job) => job,
                Err(error) if error.code == ErrorCode::MissingKey => MaterialMotionJob::new(
                    terrain,
                    self.layer,
                    self.source,
                    key("test:motion"),
                    8,
                    16,
                    8,
                )?,
                Err(error) => return Err(error),
            };
            let status = job.step(terrain, 1)?;
            (job, status)
        };
        worlds.set(
            world,
            key("test:motion-job"),
            key("test:motion-job-schema"),
            &job,
        )?;
        if status == MaterialMotionJobStatus::Ready {
            worlds.edit_terrain(world, move |edit| {
                job.publish(edit)?;
                Ok(())
            })?;
            Ok(WorkStatus::Complete)
        } else {
            Ok(WorkStatus::ContinueSameTick)
        }
    }
}

#[test]
fn runtime_resumes_material_job_from_world_state_each_budget_and_publishes_atomically() {
    let content = build_content();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                p(1, 1),
                MaterialMotion {
                    velocity_q16: [3 * 65536, 0],
                    remainder_q16: [0, 0],
                },
            )
        })
    });
    let descriptor = SystemDescriptor::new(key("test:persisted-motion-job"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:motion-job"))
        .in_world(key("game:world"))
        .barrier();
    let mut runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(
            descriptor,
            PersistedMotionJobSystem {
                layer,
                source: p(1, 1),
            },
        )
        .work_budget(1)
        .build()
        .unwrap();
    let world_id = runtime.worlds().id("world").unwrap();

    let mut result = runtime.step(TickInput::empty(1)).unwrap();
    let mut resumes = 0;
    while result == StepResult::Pending(1) {
        resumes += 1;
        assert!(resumes <= 8, "Jobのbudget進行が有限に完了する");
        assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
        result = runtime.resume().unwrap();
    }
    assert_eq!(result, StepResult::Complete(1));
    assert!(resumes >= 2, "budget=1で経路検査を複数pollへ分割する");
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(terrain.tick(), 1);
    assert_eq!(at(terrain, layer, p(1, 1)), MaterialId::AIR);
    assert_eq!(at(terrain, layer, p(4, 1)), sand);
    assert_eq!(
        terrain
            .state::<MaterialMotionJob>(key("test:motion-job"))
            .unwrap()
            .status(),
        MaterialMotionJobStatus::Ready
    );
    let checkpoint = terrain.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(
        restored
            .state::<MaterialMotionJob>(key("test:motion-job"))
            .unwrap()
            .status(),
        MaterialMotionJobStatus::Ready
    );
}

struct PersistedParticleJobSystem;
impl System for PersistedParticleJobSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        let (job, status) = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            let mut job = match terrain.state::<MaterialParticleJob>(key("test:particle-job")) {
                Ok(job) => job,
                Err(error) if error.code == ErrorCode::MissingKey => {
                    MaterialParticleJob::new(terrain, 8, 16)?
                }
                Err(error) => return Err(error),
            };
            let status = job.step(terrain, 1)?;
            (job, status)
        };
        worlds.set(
            world,
            key("test:particle-job"),
            key("test:particle-job-schema"),
            &job,
        )?;
        if status == MaterialParticleJobStatus::Ready {
            worlds.edit_terrain(world, move |edit| {
                job.publish(edit)?;
                edit.remove_state(key("test:particle-job"))?;
                Ok(())
            })?;
            Ok(WorkStatus::Complete)
        } else {
            Ok(WorkStatus::ContinueSameTick)
        }
    }
}

#[test]
fn runtime_resumes_saved_particle_job_from_world_state_by_candidate_budget() {
    let content = build_content();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:particle-active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            for x in [1, 2] {
                edit.set_cell(
                    layer,
                    p(x, 1),
                    CellCore {
                        material: sand,
                        ..CellCore::AIR
                    },
                )?;
            }
            edit.emit_material_particle(layer, p(1, 1), [65536, 0])?;
            edit.emit_material_particle(layer, p(2, 1), [65536, 0])?;
            Ok(())
        })
    });
    let descriptor = SystemDescriptor::new(key("test:persisted-particle-job"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:particle-job"))
        .in_world(key("game:world"))
        .barrier();
    let mut runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(descriptor, PersistedParticleJobSystem)
        .work_budget(1)
        .build()
        .unwrap();
    let world_id = runtime.worlds().id("world").unwrap();

    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Pending(1)
    );
    assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
    let pending = runtime.worlds().terrain(world_id).unwrap();
    let mut pending_positions: Vec<_> = pending
        .material_particles()
        .map(|(_, particle)| particle.cell())
        .collect();
    pending_positions.sort_unstable_by_key(|point| (point.y, point.x));
    assert_eq!(pending_positions, [p(1, 1), p(2, 1)]);

    assert_eq!(runtime.resume().unwrap(), StepResult::Complete(1));
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(terrain.tick(), 1);
    assert!(
        terrain
            .state::<MaterialParticleJob>(key("test:particle-job"))
            .is_err()
    );
    let mut positions: Vec<_> = terrain
        .material_particles()
        .map(|(_, particle)| particle.cell())
        .collect();
    positions.sort_unstable_by_key(|point| (point.y, point.x));
    assert_eq!(positions, [p(1, 1), p(3, 1)]);
}

struct PersistedPressureJobSystem {
    layer: LayerId,
    config: PressureJobConfig,
}
impl System for PersistedPressureJobSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        let (job, status) = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            let mut job = match terrain.state::<PressureJob>(key("test:pressure-job")) {
                Ok(job) => job,
                Err(error) if error.code == ErrorCode::MissingKey => {
                    PressureJob::new(terrain, self.layer, self.config)?
                }
                Err(error) => return Err(error),
            };
            let status = job.step(terrain, 1)?;
            (job, status)
        };
        worlds.set(
            world,
            key("test:pressure-job"),
            key("test:pressure-job-schema"),
            &job,
        )?;
        if status == PressureJobStatus::Ready {
            worlds.edit_terrain(world, move |edit| {
                edit.publish_pressure_job(&job)?;
                Ok(())
            })?;
            Ok(WorkStatus::Complete)
        } else {
            Ok(WorkStatus::ContinueSameTick)
        }
    }
}

#[test]
fn runtime_resumes_pressure_job_from_world_state_each_budget_and_publishes_atomically() {
    let content = build_content();
    let water = content
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(2, 1),
                CellCore {
                    material: water,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(key("test:pump"), p(2, 1), 900i64)
        })
    });
    let config = PressureJobConfig {
        relaxation: PressureRelaxation {
            channel: key("test:pressure"),
            iterations: 1,
            gravity_q16: 0,
            cell_limit: 16,
        },
        gas: None,
        pump: Some(key("test:pump")),
    };
    let descriptor = SystemDescriptor::new(key("test:persisted-pressure-job"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:pressure-job"))
        .in_world(key("game:world"))
        .barrier();
    let mut runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(descriptor, PersistedPressureJobSystem { layer, config })
        .work_budget(1)
        .build()
        .unwrap();
    let world_id = runtime.worlds().id("world").unwrap();

    let mut result = runtime.step(TickInput::empty(1)).unwrap();
    let mut resumes = 0;
    while result == StepResult::Pending(1) {
        resumes += 1;
        assert!(resumes <= 32, "PressureJobが有限にbudget進行する");
        assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
        result = runtime.resume().unwrap();
    }
    assert_eq!(result, StepResult::Complete(1));
    assert!(resumes >= 3, "budget=1で圧力計算を複数pollへ分割する");
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(terrain.tick(), 1);
    match terrain
        .channel_value::<i64>(key("test:pressure"), p(2, 1))
        .unwrap()
    {
        ChannelQuery::Loaded(value) => assert_eq!(value, 900),
        _ => panic!("圧力channelがresidentである"),
    }
    let job = terrain
        .state::<PressureJob>(key("test:pressure-job"))
        .unwrap();
    assert_eq!(job.status(), PressureJobStatus::Ready);
    let checkpoint = terrain.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(
        restored
            .state::<PressureJob>(key("test:pressure-job"))
            .unwrap()
            .status(),
        PressureJobStatus::Ready
    );
}

struct ToggleWorldPause {
    updates: u8,
}
impl Scene for ToggleWorldPause {
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.updates += 1;
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("world")?;
        worlds.pause(id, self.updates == 1)
    }
}

struct PausedMaterialSystem {
    config: MaterialPhaseConfig,
}
impl System for PausedMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("world")?;
        worlds.set(
            id,
            key("test:debug-record"),
            key("test:debug-schema"),
            &1u64,
        )?;
        let config = self.config;
        worlds.edit_terrain(id, move |edit| {
            edit.step_material_phase(config, &NoGravity)?;
            Ok(())
        })?;
        Ok(WorkStatus::Complete)
    }
}

#[test]
fn runtime_pause_defers_material_phase_without_advancing_world_clock() {
    let content = build_content();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: sand,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                p(1, 1),
                MaterialMotion {
                    velocity_q16: [65536, 0],
                    remainder_q16: [0, 0],
                },
            )
        })
    });
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((
            MaterialForceConfig {
                channel: key("test:motion"),
                gravity_q16: [0, 0],
                candidate_limit: 16,
                max_velocity_q16: 1_000_000,
            },
            MaterialMotionConfig {
                channel: key("test:motion"),
                candidate_limit: 16,
                max_distance: 4,
                step_limit: 32,
            },
        )),
        scalar_candidate_limit: None,
        particle: None,
    };
    let descriptor = SystemDescriptor::new(key("test:material-paused-world"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:debug-record"))
        .in_world(key("game:world"))
        .barrier();
    let mut runtime = Engine::builder()
        .sandbox_world("world", world)
        .scene("controller", ToggleWorldPause { updates: 0 })
        .start_scene("controller")
        .system(descriptor, PausedMaterialSystem { config })
        .build()
        .unwrap();
    let world_id = runtime.worlds().id("world").unwrap();

    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
    assert!(terrain.state::<u64>(key("test:debug-record")).is_err());
    assert_eq!(at(terrain, layer, p(1, 1)), sand);

    assert_eq!(
        runtime.step(TickInput::empty(2)).unwrap(),
        StepResult::Complete(2)
    );
    let terrain = runtime.worlds().terrain(world_id).unwrap();
    assert_eq!(terrain.tick(), 1);
    assert_eq!(terrain.state::<u64>(key("test:debug-record")).unwrap(), 1);
    assert_eq!(at(terrain, layer, p(1, 1)), MaterialId::AIR);
    assert_eq!(at(terrain, layer, p(2, 1)), sand);
}

struct PersistedPressureSearchPhaseSystem {
    layer: LayerId,
    pressure_config: PressureJobConfig,
    phase_config: MaterialPhaseConfig,
    force_providers: RuntimeForceProviders,
    inventory_profile: Arc<MaterialInventoryProfile>,
    push_config: PressurePushConfig,
    source: CellPoint,
    worker_barrier: Option<Arc<std::sync::Barrier>>,
}
impl System for PersistedPressureSearchPhaseSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let current_tick = ctx.tick();
        if let Some(barrier) = self.worker_barrier.take() {
            barrier.wait();
        }
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        match worlds
            .view()
            .terrain(world)?
            .state::<u64>(key("test:pressure-phase-done"))
        {
            Ok(done_tick) if done_tick == current_tick => return Ok(WorkStatus::Complete),
            Ok(done_tick) if done_tick < current_tick => {}
            Ok(_) => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "phase marker value",
                ));
            }
            Err(error) if error.code == ErrorCode::MissingKey => {}
            Err(error) => return Err(error),
        }
        let (pressure, status) = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            let mut pressure = match terrain.state::<PressureJob>(key("test:pressure-job")) {
                Ok(pressure) => pressure,
                Err(error) if error.code == ErrorCode::MissingKey => {
                    PressureJob::new(terrain, self.layer, self.pressure_config)?
                }
                Err(error) => return Err(error),
            };
            let status = pressure.step(terrain, 1)?;
            (pressure, status)
        };
        worlds.set(
            world,
            key("test:pressure-job"),
            key("test:pressure-job-schema"),
            &pressure,
        )?;
        if status != PressureJobStatus::Ready {
            return Ok(WorkStatus::ContinueSameTick);
        }

        let phase_config = self.phase_config;
        let push_config = self.push_config;
        let layer = self.layer;
        let source = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            if at(terrain, layer, self.source) == MaterialId::AIR {
                p(self.source.x + 1, self.source.y)
            } else {
                self.source
            }
        };
        let done_tick = current_tick;
        let force_providers = self.force_providers.clone();
        let inventory_profile = Arc::clone(&self.inventory_profile);
        let archives = {
            let view = worlds.view();
            view.terrain(world)?.load_material_archive_inventory(
                &inventory_profile,
                &mut NoArchives,
                4_000_000,
            )?
        };
        worlds.edit_material_accounted(
            world,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile: inventory_profile,
                archives,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: None,
            },
            move |edit| {
                let report = edit.step_material_phase_with_pressure_job_search(
                    phase_config,
                    &force_providers,
                    &[pressure],
                    MaterialPhasePressureSearchInput {
                        layer,
                        source,
                        config: push_config,
                        budget: 1,
                    },
                )?;
                // pressure出力と後続処理の開始が確定する取引で、完了Jobを保存領域から外す。
                edit.remove_state(key("test:pressure-job"))?;
                if let Some(pending) = report.pending_search.as_ref() {
                    edit.set_state(key("test:push-job"), key("test:push-job-schema"), pending)?;
                } else {
                    edit.set_state(
                        key("test:pressure-phase-done"),
                        key("test:debug-schema"),
                        &done_tick,
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::Complete)
    }
}

struct PressureWorkerProbe {
    barrier: Option<Arc<std::sync::Barrier>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl System for PressureWorkerProbe {
    fn run(&mut self, _: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        if let Some(barrier) = self.barrier.take() {
            barrier.wait();
        }
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(WorkStatus::Complete)
    }
}

struct ResumePressureSearchPhaseSystem {
    layer: LayerId,
    phase_config: MaterialPhaseConfig,
    motion_source: CellPoint,
    force_providers: RuntimeForceProviders,
    inventory_profile: Arc<MaterialInventoryProfile>,
    inject_inventory_failure: bool,
    inject_particle_capacity_failure: bool,
}
impl System for ResumePressureSearchPhaseSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let current_tick = ctx.tick();
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("world")?;
        match worlds
            .view()
            .terrain(world)?
            .state::<u64>(key("test:pressure-phase-done"))
        {
            Ok(done_tick) if done_tick == current_tick => return Ok(WorkStatus::Complete),
            Ok(done_tick) if done_tick < current_tick => {}
            Ok(_) => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "phase marker value",
                ));
            }
            Err(error) if error.code == ErrorCode::MissingKey => {}
            Err(error) => return Err(error),
        }
        let search = {
            let view = worlds.view();
            view.terrain(world)?
                .state::<PressurePushSearch>(key("test:push-job"))?
        };
        let motion = {
            let view = worlds.view();
            let terrain = view.terrain(world)?;
            if at(terrain, self.layer, self.motion_source) == MaterialId::AIR {
                None
            } else {
                let mut motion = MaterialMotionJob::new(
                    terrain,
                    self.layer,
                    self.motion_source,
                    key("test:motion"),
                    4,
                    16,
                    8,
                )?;
                while motion.status() == MaterialMotionJobStatus::Computing {
                    motion.step(terrain, 8)?;
                }
                Some(motion)
            }
        };
        let config = self.phase_config;
        let layer = self.layer;
        let force_providers = self.force_providers.clone();
        let inventory_profile = Arc::clone(&self.inventory_profile);
        let inject_inventory_failure = self.inject_inventory_failure;
        let inject_particle_capacity_failure = self.inject_particle_capacity_failure;
        let done_tick = current_tick;
        let archives = {
            let view = worlds.view();
            view.terrain(world)?.load_material_archive_inventory(
                &inventory_profile,
                &mut NoArchives,
                4_000_000,
            )?
        };
        worlds.edit_material_accounted(
            world,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile: inventory_profile,
                archives,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: None,
            },
            move |edit| {
                let report = edit.resume_material_phase_with_pressure_search(
                    config,
                    &force_providers,
                    motion.as_slice(),
                    search,
                    1,
                )?;
                if let Some(pending) = report.pending_search.as_ref() {
                    edit.set_state(key("test:push-job"), key("test:push-job-schema"), pending)?;
                } else {
                    edit.remove_state(key("test:push-job"))?;
                    edit.set_state(
                        key("test:pressure-phase-done"),
                        key("test:debug-schema"),
                        &done_tick,
                    )?;
                }
                if inject_particle_capacity_failure && report.phase.is_some() {
                    // 既存の粒子poolを満杯にしておき、Pressure/Job/phase適用後に容量失敗させる。
                    edit.emit_material_particle(layer, p(50, 50), [0, 0])?;
                }
                if inject_inventory_failure && report.phase.is_some() {
                    edit.set_state(key("test:body-owned"), key("test:debug-schema"), &2u64)?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::ContinueSameTick)
    }
}

fn run_pressure_runtime_reference_tick(
    world: &mut World,
    layer: LayerId,
    source: CellPoint,
    profile: &MaterialInventoryProfile,
    pressure_config: PressureJobConfig,
    tick: u64,
) -> EngineResult<()> {
    let push_source = if at(world, layer, source) == MaterialId::AIR {
        p(source.x + 1, source.y)
    } else {
        source
    };
    let mut pressure = PressureJob::new(world, layer, pressure_config)?;
    while pressure.status() != PressureJobStatus::Ready {
        pressure.step(world, 1)?;
    }
    let archives = world.load_material_archive_inventory(profile, &mut NoArchives, 4_000_000)?;
    world.edit_material_accounted(
        profile,
        &archives,
        key("test:balance"),
        key("test:balance-schema"),
        None,
        |edit| {
            edit.publish_pressure_job_then_search_push(
                &pressure,
                layer,
                push_source,
                PressurePushConfig {
                    channel: key("test:pressure"),
                    gravity_q16: 0,
                    threshold_q16: 0,
                    nodes: 16,
                },
                16,
            )?;
            Ok(())
        },
    )?;

    let motion = if at(world, layer, p(20, 20)) == MaterialId::AIR {
        None
    } else {
        let mut motion =
            MaterialMotionJob::new(world, layer, p(20, 20), key("test:motion"), 4, 16, 8)?;
        while motion.status() == MaterialMotionJobStatus::Computing {
            motion.step(world, 8)?;
        }
        Some(motion)
    };
    let providers = RuntimeForceProviders {
        water: NoGravity,
        sand: SandDrift,
        water_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        sand_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let config = MaterialPhaseConfig {
        layer,
        pressure: None,
        motion: Some((
            MaterialForceConfig {
                channel: key("test:motion"),
                gravity_q16: [0, 0],
                candidate_limit: 256,
                max_velocity_q16: 8 * 65536,
            },
            MaterialMotionConfig {
                channel: key("test:motion"),
                candidate_limit: 256,
                max_distance: 8,
                step_limit: 256,
            },
        )),
        scalar_candidate_limit: None,
        particle: Some((
            MaterialParticleForceConfig {
                gravity_q16: [0, 0],
                candidate_limit: 16,
                max_velocity_q16: 65536,
            },
            32,
        )),
    };
    let archives = world.load_material_archive_inventory(profile, &mut NoArchives, 4_000_000)?;
    world.edit_material_accounted(
        profile,
        &archives,
        key("test:balance"),
        key("test:balance-schema"),
        None,
        |edit| {
            edit.step_material_phase_with_jobs(config, &providers, motion.as_slice(), &[])?;
            edit.set_state(
                key("test:pressure-phase-done"),
                key("test:debug-schema"),
                &tick,
            )
        },
    )?;
    world.edit(|edit| edit.advance_tick())
}

fn assert_pressure_runtime_profile_matches_reference(
    actual: &World,
    reference: &World,
    layer: LayerId,
    profile: &MaterialInventoryProfile,
) {
    assert_eq!(actual.tick(), reference.tick());
    for y in 0..64 {
        for x in 0..64 {
            let point = p(x, y);
            assert_eq!(
                at(actual, layer, point),
                at(reference, layer, point),
                "material at {point:?}, tick {}",
                actual.tick()
            );
            match (
                actual
                    .channel_value::<i64>(key("test:pressure"), point)
                    .unwrap(),
                reference
                    .channel_value::<i64>(key("test:pressure"), point)
                    .unwrap(),
            ) {
                (ChannelQuery::Loaded(left), ChannelQuery::Loaded(right)) => {
                    assert_eq!(left, right, "pressure at {point:?}, tick {}", actual.tick())
                }
                _ => panic!("参照Worldのpressure coverageが不一致: {point:?}"),
            }
            match (
                actual
                    .channel_value::<MaterialMotion>(key("test:motion"), point)
                    .unwrap(),
                reference
                    .channel_value::<MaterialMotion>(key("test:motion"), point)
                    .unwrap(),
            ) {
                (ChannelQuery::Loaded(left), ChannelQuery::Loaded(right)) => {
                    assert_eq!(left, right, "motion at {point:?}, tick {}", actual.tick())
                }
                _ => panic!("参照Worldのmotion coverageが不一致: {point:?}"),
            }
        }
    }
    let actual_particles: Vec<_> = actual
        .material_particles()
        .map(|(_, particle)| (particle.layer(), particle.cell(), particle.velocity_q16()))
        .collect();
    let reference_particles: Vec<_> = reference
        .material_particles()
        .map(|(_, particle)| (particle.layer(), particle.cell(), particle.velocity_q16()))
        .collect();
    assert_eq!(actual_particles, reference_particles);
    for record in [key("test:body-owned"), key("test:reservoir-owned")] {
        assert_eq!(
            actual.state::<u64>(record).unwrap(),
            reference.state::<u64>(record).unwrap()
        );
    }
    let mut no_archives = NoArchives;
    let actual_archives = actual
        .load_material_archive_inventory(profile, &mut no_archives, 4_000_000)
        .unwrap();
    let mut no_archives = NoArchives;
    let reference_archives = reference
        .load_material_archive_inventory(profile, &mut no_archives, 4_000_000)
        .unwrap();
    actual
        .verify_material_accounting(profile, &actual_archives, key("test:balance"))
        .unwrap();
    reference
        .verify_material_accounting(profile, &reference_archives, key("test:balance"))
        .unwrap();
    assert_eq!(
        actual
            .material_inventory(profile, &actual_archives)
            .unwrap(),
        reference
            .material_inventory(profile, &reference_archives)
            .unwrap()
    );
}

#[test]
fn runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers() {
    let run_profile = |workers: u16,
                       inject_inventory_failure: bool,
                       inject_particle_capacity_failure: bool| {
        let barrier = (workers > 1).then(|| Arc::new(std::sync::Barrier::new(2)));
        let probe_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let content = build_content();
        let water = content
            .material_id(MaterialHandle(key("test:water")))
            .unwrap();
        let wall = content
            .material_id(MaterialHandle(key("test:wall")))
            .unwrap();
        let sand = content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap();
        let layer = content.layer_id(key("test:layer")).unwrap();
        let source = p(10, 10);
        let inventory_profile = runtime_multi_provider_profile();
        let init_inventory_profile = Arc::clone(&inventory_profile);
        let mut world_limits = WorldLimits::default();
        if inject_particle_capacity_failure {
            world_limits.particles = 1;
        }
        let world = WorldBuilder::new(Arc::clone(&content))
            .limits(world_limits)
            .initialize(move |world| {
                world.edit(|edit| {
                    edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                    edit.acquire_lease(RegionLease {
                        source: key("test:active"),
                        region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                        mode: LeaseMode::Simulate,
                        expires: None,
                    })?;
                    for position in [p(10, 9), p(9, 10), p(10, 11), p(11, 9), p(11, 11)] {
                        edit.set_cell(
                            layer,
                            position,
                            CellCore {
                                material: wall,
                                ..CellCore::AIR
                            },
                        )?;
                    }
                    for position in [source, p(11, 10)] {
                        edit.set_cell(
                            layer,
                            position,
                            CellCore {
                                material: water,
                                ..CellCore::AIR
                            },
                        )?;
                    }
                    edit.set_cell(
                        layer,
                        p(20, 20),
                        CellCore {
                            material: sand,
                            ..CellCore::AIR
                        },
                    )?;
                    edit.set_channel(
                        key("test:motion"),
                        p(20, 20),
                        MaterialMotion {
                            velocity_q16: [2 * 65536, 0],
                            remainder_q16: [0, 0],
                        },
                    )?;
                    edit.set_cell(
                        layer,
                        p(30, 30),
                        CellCore {
                            material: sand,
                            ..CellCore::AIR
                        },
                    )?;
                    edit.emit_material_particle(layer, p(30, 30), [65536, 0])?;
                    edit.set_cell(
                        layer,
                        p(40, 40),
                        CellCore {
                            material: sand,
                            ..CellCore::AIR
                        },
                    )?;
                    edit.set_cell(
                        layer,
                        p(50, 50),
                        CellCore {
                            material: water,
                            ..CellCore::AIR
                        },
                    )?;
                    edit.set_state(key("test:body-owned"), key("test:debug-schema"), &1u64)?;
                    edit.set_state(key("test:reservoir-owned"), key("test:debug-schema"), &1u64)?;
                    edit.set_channel(key("test:pump"), source, 900i64)
                })?;
                let archives = world.load_material_archive_inventory(
                    &init_inventory_profile,
                    &mut NoArchives,
                    4_000_000,
                )?;
                world.initialize_material_accounting(
                    &init_inventory_profile,
                    &archives,
                    key("test:balance"),
                    key("test:balance-schema"),
                    8,
                )
            });
        let pressure_config = PressureJobConfig {
            relaxation: PressureRelaxation {
                channel: key("test:pressure"),
                iterations: 1,
                gravity_q16: 0,
                cell_limit: 16,
            },
            gas: None,
            pump: Some(key("test:pump")),
        };
        let descriptor = SystemDescriptor::new(
            key("test:persisted-pressure-search-phase"),
            Phase::Materials,
        )
        .parallel()
        .write(key("engine:terrain"))
        .write(key("test:pressure-job"))
        .write(key("test:push-job"))
        .write(key("test:pressure-phase-done"))
        .in_world(key("game:world"))
        .barrier();
        let resume_descriptor = SystemDescriptor::new(
            key("test:resume-persisted-pressure-search-phase"),
            Phase::Materials,
        )
        .after(key("test:persisted-pressure-search-phase"))
        .write(key("engine:terrain"))
        .write(key("test:push-job"))
        .write(key("test:pressure-phase-done"))
        .in_world(key("game:world"))
        .barrier();
        let force_providers = RuntimeForceProviders {
            water: NoGravity,
            sand: SandDrift,
            water_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sand_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let mut builder = Engine::builder().sandbox_world("world", world);
        if workers > 1 {
            builder = builder.settings(EngineSettingsPatch {
                simulation: Some(SimulationPatch {
                    system_workers: Some(WorkerCount::Count(workers)),
                    ..SimulationPatch::default()
                }),
                ..EngineSettingsPatch::default()
            });
        }
        let mut runtime = builder
            .system(
                // scheduleはStableKey順なので、probeをbarrier Systemより先に置く。
                SystemDescriptor::new(key("test:aaa-pressure-worker-probe"), Phase::Materials)
                    .parallel(),
                PressureWorkerProbe {
                    barrier: barrier.as_ref().map(Arc::clone),
                    calls: Arc::clone(&probe_calls),
                },
            )
            .system(
                descriptor,
                PersistedPressureSearchPhaseSystem {
                    layer,
                    pressure_config,
                    inventory_profile: Arc::clone(&inventory_profile),
                    force_providers: force_providers.clone(),
                    phase_config: MaterialPhaseConfig {
                        layer,
                        pressure: None,
                        motion: Some((
                            MaterialForceConfig {
                                channel: key("test:motion"),
                                gravity_q16: [0, 0],
                                candidate_limit: 256,
                                max_velocity_q16: 8 * 65536,
                            },
                            MaterialMotionConfig {
                                channel: key("test:motion"),
                                candidate_limit: 256,
                                max_distance: 8,
                                step_limit: 256,
                            },
                        )),
                        scalar_candidate_limit: None,
                        particle: Some((
                            MaterialParticleForceConfig {
                                gravity_q16: [0, 0],
                                candidate_limit: 16,
                                max_velocity_q16: 65536,
                            },
                            32,
                        )),
                    },
                    push_config: PressurePushConfig {
                        channel: key("test:pressure"),
                        gravity_q16: 0,
                        threshold_q16: 0,
                        nodes: 16,
                    },
                    source,
                    worker_barrier: barrier.as_ref().map(Arc::clone),
                },
            )
            .system(
                resume_descriptor,
                ResumePressureSearchPhaseSystem {
                    layer,
                    phase_config: MaterialPhaseConfig {
                        layer,
                        pressure: None,
                        motion: Some((
                            MaterialForceConfig {
                                channel: key("test:motion"),
                                gravity_q16: [0, 0],
                                candidate_limit: 256,
                                max_velocity_q16: 8 * 65536,
                            },
                            MaterialMotionConfig {
                                channel: key("test:motion"),
                                candidate_limit: 256,
                                max_distance: 8,
                                step_limit: 256,
                            },
                        )),
                        scalar_candidate_limit: None,
                        particle: Some((
                            MaterialParticleForceConfig {
                                gravity_q16: [0, 0],
                                candidate_limit: 16,
                                max_velocity_q16: 65536,
                            },
                            32,
                        )),
                    },
                    motion_source: p(20, 20),
                    force_providers: force_providers.clone(),
                    inventory_profile: Arc::clone(&inventory_profile),
                    inject_inventory_failure,
                    inject_particle_capacity_failure,
                },
            )
            .work_budget(2)
            .build()
            .unwrap();
        let world_id = runtime.worlds().id("world").unwrap();

        let checkpoint_before = runtime.worlds().checkpoint(world_id).unwrap();
        let mut profile_samples = Vec::with_capacity(3);
        let first_measurement = (!inject_inventory_failure && !inject_particle_capacity_failure)
            .then(begin_runtime_profile_sample);
        let mut result = runtime.step(TickInput::empty(1));
        let mut resumes = 0;
        let mut failure = None;
        let mut completed = false;
        loop {
            match result {
                Err(error) => {
                    failure = Some(error);
                    break;
                }
                Ok(StepResult::Pending(1)) => {
                    resumes += 1;
                    assert!(resumes <= 32, "圧力探索phaseが有限にbudget進行する");
                    assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
                    result = runtime.resume();
                }
                Ok(StepResult::Complete(1)) => {
                    completed = true;
                    break;
                }
                Ok(other) => panic!("予期しないRuntime結果: {other:?}"),
            }
        }
        if let Some((started, baseline_live)) = first_measurement {
            profile_samples.push(finish_runtime_profile_sample(started, baseline_live, 1));
        }
        if inject_inventory_failure || inject_particle_capacity_failure {
            let error = failure.expect("pressure・Job・phase後の失敗でRuntime tickが失敗する");
            assert_eq!(
                error.code,
                if inject_inventory_failure {
                    ErrorCode::InvalidValue
                } else {
                    ErrorCode::CapacityExceeded
                }
            );
            assert!(resumes >= 4, "圧力とJobを完了した後に後段の失敗へ到達する");
            assert!(
                force_providers
                    .water_calls
                    .load(std::sync::atomic::Ordering::Relaxed)
                    > 0
            );
            assert!(
                force_providers
                    .sand_calls
                    .load(std::sync::atomic::Ordering::Relaxed)
                    > 0
            );
            assert_eq!(runtime.worlds().tick(world_id).unwrap(), 0);
            assert_eq!(
                runtime.worlds().checkpoint(world_id).unwrap(),
                checkpoint_before
            );
            return checkpoint_before;
        }
        assert!(failure.is_none(), "通常profileでは失敗しない: {failure:?}");
        assert!(completed);
        assert!(resumes >= 4, "PressureJobと押出し探索を各barrierで継続する");
        let terrain = runtime.worlds().terrain(world_id).unwrap();
        assert_eq!(terrain.tick(), 1);
        assert!(matches!(
            terrain
                .channel_value::<i64>(key("test:pressure"), source)
                .unwrap(),
            ChannelQuery::Loaded(900)
        ));
        assert_eq!(at(terrain, layer, source), MaterialId::AIR);
        assert_eq!(at(terrain, layer, p(11, 10)), water);
        assert_eq!(at(terrain, layer, p(12, 10)), water);
        assert_eq!(at(terrain, layer, p(22, 20)), sand);
        assert_eq!(at(terrain, layer, p(41, 40)), sand);
        assert_eq!(at(terrain, layer, p(50, 50)), water);
        let particles: Vec<_> = terrain.material_particles().collect();
        assert_eq!(particles.len(), 1, "正規粒子が物質所有を維持する");
        assert_eq!(particles[0].1.cell(), p(31, 30));
        assert!(
            terrain
                .state::<PressurePushSearch>(key("test:push-job"))
                .is_err()
        );
        assert!(
            terrain
                .state::<PressureJob>(key("test:pressure-job"))
                .is_err()
        );
        assert_eq!(probe_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(
            force_providers
                .water_calls
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "水向けProviderがMaterials phaseで実行される"
        );
        assert!(
            force_providers
                .sand_calls
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "砂向けProviderがMaterials phaseで実行される"
        );
        let archive = terrain
            .load_material_archive_inventory(&inventory_profile, &mut NoArchives, 4_000_000)
            .unwrap();
        terrain
            .verify_material_accounting(&inventory_profile, &archive, key("test:balance"))
            .unwrap();
        let inventory = terrain
            .material_inventory(&inventory_profile, &archive)
            .unwrap();
        for material in [key("test:body-token"), key("test:reservoir-token")] {
            assert!(inventory.quantities().iter().any(|quantity| {
                quantity.material == material && quantity.tokens == MaterialQuantity::ONE
            }));
        }

        // Runtimeのbudget=1/barrier経路を、各参照APIを個別に完了させる経路と照合する。
        // まず圧力Jobを完成し、新圧力fieldで押出しを公開してから、移動Jobと通常phaseを実行する。
        let mut reference = World::restore(
            &checkpoint_before,
            Arc::clone(&content),
            WorldLimits::default(),
            checkpoint_before.len(),
        )
        .unwrap();
        let mut pressure = PressureJob::new(&reference, layer, pressure_config).unwrap();
        while pressure.status() != PressureJobStatus::Ready {
            pressure.step(&reference, 1).unwrap();
        }
        let reference_archives = reference
            .load_material_archive_inventory(&inventory_profile, &mut NoArchives, 4_000_000)
            .unwrap();
        let pressure_push = reference
            .edit_material_accounted(
                &inventory_profile,
                &reference_archives,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    edit.publish_pressure_job_then_search_push(
                        &pressure,
                        layer,
                        source,
                        PressurePushConfig {
                            channel: key("test:pressure"),
                            gravity_q16: 0,
                            threshold_q16: 0,
                            nodes: 16,
                        },
                        16,
                    )
                },
            )
            .unwrap();
        assert_eq!(pressure_push.search_status, PressurePushStatus::Ready);
        assert!(pressure_push.published_push);

        let mut motion =
            MaterialMotionJob::new(&reference, layer, p(20, 20), key("test:motion"), 4, 16, 8)
                .unwrap();
        while motion.status() == MaterialMotionJobStatus::Computing {
            motion.step(&reference, 8).unwrap();
        }
        let reference_force_providers = RuntimeForceProviders {
            water: NoGravity,
            sand: SandDrift,
            water_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sand_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let reference_phase = MaterialPhaseConfig {
            layer,
            pressure: None,
            motion: Some((
                MaterialForceConfig {
                    channel: key("test:motion"),
                    gravity_q16: [0, 0],
                    candidate_limit: 256,
                    max_velocity_q16: 8 * 65536,
                },
                MaterialMotionConfig {
                    channel: key("test:motion"),
                    candidate_limit: 256,
                    max_distance: 8,
                    step_limit: 256,
                },
            )),
            scalar_candidate_limit: None,
            particle: Some((
                MaterialParticleForceConfig {
                    gravity_q16: [0, 0],
                    candidate_limit: 16,
                    max_velocity_q16: 65536,
                },
                32,
            )),
        };
        reference
            .edit_material_accounted(
                &inventory_profile,
                &reference_archives,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    edit.step_material_phase_with_jobs(
                        reference_phase,
                        &reference_force_providers,
                        std::slice::from_ref(&motion),
                        &[],
                    )?;
                    edit.set_state(
                        key("test:pressure-phase-done"),
                        key("test:debug-schema"),
                        &1u64,
                    )
                },
            )
            .unwrap();
        reference.edit(|edit| edit.advance_tick()).unwrap();

        assert_eq!(terrain.tick(), reference.tick());
        assert_eq!(
            terrain.material_particles().count(),
            reference.material_particles().count()
        );
        assert_eq!(
            terrain.material_particles().next().unwrap().1.cell(),
            reference.material_particles().next().unwrap().1.cell()
        );
        for y in 0..64 {
            for x in 0..64 {
                let point = p(x, y);
                assert_eq!(
                    at(terrain, layer, point),
                    at(&reference, layer, point),
                    "material at {point:?}"
                );
                match (
                    terrain
                        .channel_value::<i64>(key("test:pressure"), point)
                        .unwrap(),
                    reference
                        .channel_value::<i64>(key("test:pressure"), point)
                        .unwrap(),
                ) {
                    (ChannelQuery::Loaded(actual), ChannelQuery::Loaded(expected)) => {
                        assert_eq!(actual, expected, "pressure at {point:?}");
                    }
                    _ => panic!("参照Worldのpressure coverageが不一致: {point:?}"),
                }
                match (
                    terrain
                        .channel_value::<MaterialMotion>(key("test:motion"), point)
                        .unwrap(),
                    reference
                        .channel_value::<MaterialMotion>(key("test:motion"), point)
                        .unwrap(),
                ) {
                    (ChannelQuery::Loaded(actual), ChannelQuery::Loaded(expected)) => {
                        assert_eq!(actual, expected, "motion at {point:?}");
                    }
                    _ => panic!("参照Worldのmotion coverageが不一致: {point:?}"),
                }
            }
        }
        assert_eq!(
            terrain.state::<u64>(key("test:body-owned")).unwrap(),
            reference.state::<u64>(key("test:body-owned")).unwrap()
        );
        assert_eq!(
            terrain.state::<u64>(key("test:reservoir-owned")).unwrap(),
            reference.state::<u64>(key("test:reservoir-owned")).unwrap()
        );
        let reference_inventory = reference
            .material_inventory(&inventory_profile, &reference_archives)
            .unwrap();
        assert_eq!(inventory, reference_inventory);

        for tick in 2..=3 {
            let measurement = begin_runtime_profile_sample();
            let mut result = runtime.step(TickInput::empty(tick));
            let mut resumes = 0;
            loop {
                match result {
                    Err(error) => panic!("複合Runtime tick {tick}が失敗: {error}"),
                    Ok(StepResult::Pending(pending_tick)) if pending_tick == tick => {
                        resumes += 1;
                        assert!(resumes <= 32, "複合pressure profileが有限に進行する");
                        assert_eq!(runtime.worlds().tick(world_id).unwrap(), tick - 1);
                        result = runtime.resume();
                    }
                    Ok(StepResult::Complete(completed_tick)) if completed_tick == tick => break,
                    Ok(other) => panic!("複合Runtime tick {tick}の予期しない結果: {other:?}"),
                }
            }
            profile_samples.push(finish_runtime_profile_sample(
                measurement.0,
                measurement.1,
                tick,
            ));
            run_pressure_runtime_reference_tick(
                &mut reference,
                layer,
                source,
                &inventory_profile,
                pressure_config,
                tick,
            )
            .unwrap();
            let actual = runtime.worlds().terrain(world_id).unwrap();
            assert_pressure_runtime_profile_matches_reference(
                actual,
                &reference,
                layer,
                &inventory_profile,
            );
        }
        assert_eq!(probe_calls.load(std::sync::atomic::Ordering::Relaxed), 3);
        let total: Duration = profile_samples.iter().map(|sample| sample.elapsed).sum();
        println!(
            "複合Runtime profile: workers={workers} ticks={} total={total:?} heap_allocations={} heap_bytes={} large_allocations={} ge1MiB={}/{}B ge16MiB={}/{}B max_allocation={}B peak_live_delta={}B",
            profile_samples.len(),
            profile_samples
                .iter()
                .map(|sample| sample.allocations)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.allocated_bytes)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.large_allocations)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.allocations_1m)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.allocated_bytes_1m)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.allocations_16m)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.allocated_bytes_16m)
                .sum::<usize>(),
            profile_samples
                .iter()
                .map(|sample| sample.max_allocation)
                .max()
                .unwrap_or(0),
            profile_samples
                .iter()
                .map(|sample| sample.peak_live_delta)
                .max()
                .unwrap_or(0),
        );
        for sample in &profile_samples {
            println!(
                "複合Runtime sample: workers={workers} tick={} elapsed={:?} heap_allocations={} heap_bytes={} large_allocations={} ge1MiB={}/{}B ge16MiB={}/{}B max_allocation={}B peak_live_delta={}B",
                sample.tick,
                sample.elapsed,
                sample.allocations,
                sample.allocated_bytes,
                sample.large_allocations,
                sample.allocations_1m,
                sample.allocated_bytes_1m,
                sample.allocations_16m,
                sample.allocated_bytes_16m,
                sample.max_allocation,
                sample.peak_live_delta,
            );
        }
        runtime.worlds().checkpoint(world_id).unwrap()
    };

    let mut expected = None;
    for workers in [1_u16, 2, 4] {
        let checkpoint = run_profile(workers, false, false);
        if let Some(expected) = &expected {
            assert_eq!(
                &checkpoint, expected,
                "PressureJob・押出し探索・移動Job・Materials phaseはworker数{workers}でも同じ"
            );
        } else {
            expected = Some(checkpoint);
        }
    }
    run_profile(1, true, false);
    run_profile(1, false, true);
}
