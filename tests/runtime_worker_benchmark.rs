//! 同じ作者System負荷をworker数1/2/4で比較する手動計測。

use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        schema::{Schema, SchemaRegistry},
        state::{StateStore, StoreLimits},
    },
    runtime::{
        Engine, StepResult,
        config::{EngineSettingsPatch, SimulationPatch, WorkerCount},
        input::TickInput,
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
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

// SAFETY: 実メモリ操作は標準System allocatorへ委譲し、計測値のみatomicで更新する。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: layoutは呼出元が指定した有効な割当条件である。
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
        // SAFETY: pointer/layoutはこのallocatorから受け取った組である。
        unsafe { SystemAllocator.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointer/layoutはこのallocatorから受け取った組である。
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

fn state() -> StateStore {
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<u64>(key("bench:number"), 1, 8).unwrap())
        .unwrap();
    StateStore::new(
        key("bench:world"),
        schemas,
        StoreLimits {
            records: 16,
            payload_bytes: 128,
            checkpoint_bytes: 4096,
        },
    )
    .unwrap()
}

struct Workload {
    record: StableKey,
    seed: u64,
    operations: u64,
}
impl System for Workload {
    fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        // 仕事量をworker数間で揃え、結果をWorldへ書いて計算が除去されないようにする。
        let mut value = self.seed;
        for index in 0..self.operations {
            value = black_box(
                value
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(index + 1),
            );
        }
        let mut worlds = context.worlds();
        let world = worlds.view().id("main")?;
        worlds.set(world, self.record, key("bench:number"), &value)?;
        Ok(WorkStatus::Complete)
    }
}

fn percentile(samples: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    ordered[(ordered.len() - 1) * numerator / denominator]
}

#[test]
#[ignore = "手動でRuntime worker数の同一負荷比較を行う"]
fn runtime_worker_count_comparison() {
    const WARMUP: u64 = 20;
    const SAMPLES: u64 = 200;
    let records = ["bench:a", "bench:b", "bench:c", "bench:d"];
    for operations in [8192_u64, 65536] {
        for workers in [1_u16, 2, 4] {
            let mut builder =
                Engine::builder()
                    .world("main", state())
                    .settings(EngineSettingsPatch {
                        simulation: Some(SimulationPatch {
                            system_workers: Some(WorkerCount::Count(workers)),
                            ..SimulationPatch::default()
                        }),
                        ..EngineSettingsPatch::default()
                    });
            for (index, record) in records.iter().enumerate() {
                let record = key(record);
                builder = builder.system(
                    SystemDescriptor::new(
                        key(["bench:sys-a", "bench:sys-b", "bench:sys-c", "bench:sys-d"][index]),
                        Phase::Intent,
                    )
                    .write(record)
                    .parallel(),
                    Workload {
                        record,
                        seed: index as u64 + 1,
                        operations,
                    },
                );
            }
            let mut engine = builder.build().unwrap();
            for tick in 1..=WARMUP {
                assert_eq!(
                    engine.step(TickInput::empty(tick)).unwrap(),
                    StepResult::Complete(tick)
                );
            }
            let mut samples = Vec::with_capacity(SAMPLES as usize);
            let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
            PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
            ALLOCATION_COUNT.store(0, Ordering::Relaxed);
            ALLOCATION_BYTES.store(0, Ordering::Relaxed);
            for tick in WARMUP + 1..=WARMUP + SAMPLES {
                let started = Instant::now();
                assert_eq!(
                    engine.step(TickInput::empty(tick)).unwrap(),
                    StepResult::Complete(tick)
                );
                samples.push(started.elapsed());
            }
            let total: Duration = samples.iter().copied().sum();
            let peak_extra = PEAK_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(baseline_live);
            println!(
                "workers={workers} systems=4 operations_per_system={operations} ticks={SAMPLES} total={total:?} p50={:?} p95={:?} p99={:?} heap_allocations={} heap_bytes={} peak_live_delta={peak_extra}B",
                percentile(&samples, 50, 100),
                percentile(&samples, 95, 100),
                percentile(&samples, 99, 100),
                ALLOCATION_COUNT.load(Ordering::Relaxed),
                ALLOCATION_BYTES.load(Ordering::Relaxed),
            );
        }
    }
}
