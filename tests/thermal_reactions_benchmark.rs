//! 熱solverの固定条件を手動計測し、時間とRust heap割当を記録します。

use charon_engine::{
    foundation::StableKey,
    world::{
        EnthalpyBand, EnthalpyFamily, ReactionCell, ReactionChainPolicy, ReactionJob, ReactionRule,
        ReactionRuleSet, ReactionSidecar, ReactionStateTransfer, ThermalCellState,
        ThermalEdgeState, ThermalStepConfig, diffuse_enthalpy_into,
    },
};
use std::{
    alloc::{GlobalAlloc, Layout, System as SystemAllocator},
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

// SAFETY: メモリ操作をSystem allocatorへ委譲し、計測値だけをatomicで更新します。
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
        // SAFETY: pointerとlayoutはこのallocatorから受け取った組です。
        unsafe { SystemAllocator.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointerとlayoutはこのallocatorから受け取った有効な組です。
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

fn percentile(samples: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    ordered[(ordered.len() - 1) * numerator / denominator]
}

#[test]
#[ignore = "10,000セル熱solverの手動性能計測"]
fn thermal_solver_profile() {
    const WIDTH: usize = 500;
    const HEIGHT: usize = 200;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let family = EnthalpyFamily::new(
        key("bench:thermal-family"),
        vec![EnthalpyBand::Sensible {
            energy_start: 0,
            energy_end: 50_000,
            temperature_start_mk: 0,
            temperature_end_mk: 50_000,
            material: key("bench:material"),
        }],
    )
    .unwrap();
    let len = WIDTH * HEIGHT;
    let input = (0..len)
        .map(|index| ThermalCellState {
            energy: if (index + index / WIDTH).is_multiple_of(2) {
                40_000
            } else {
                10_000
            },
        })
        .collect::<Vec<_>>();
    let input_edges = vec![ThermalEdgeState::default(); len];
    let mut delta = vec![0_i128; len];
    let mut lookup = vec![Default::default(); len];
    let mut output = vec![ThermalCellState::default(); len];
    let mut output_edges = vec![ThermalEdgeState::default(); len];
    let config = ThermalStepConfig {
        conductance_numerator: 1,
        conductance_denominator: 4,
    };
    for _ in 0..WARMUP {
        diffuse_enthalpy_into(
            WIDTH,
            HEIGHT,
            &family,
            &input,
            &input_edges,
            |_| true,
            config,
            &mut delta,
            &mut lookup,
            &mut output,
            &mut output_edges,
        )
        .unwrap();
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let report = diffuse_enthalpy_into(
            WIDTH,
            HEIGHT,
            &family,
            &input,
            &input_edges,
            |_| true,
            config,
            &mut delta,
            &mut lookup,
            &mut output,
            &mut output_edges,
        )
        .unwrap();
        samples.push(started.elapsed());
        assert_eq!(
            report.edges,
            2 * WIDTH as u64 * HEIGHT as u64 - WIDTH as u64 - HEIGHT as u64
        );
        assert_eq!(
            output
                .iter()
                .map(|cell| i128::from(cell.energy))
                .sum::<i128>(),
            report.total_energy
        );
    }
    let allocation_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let peak_additional = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    eprintln!(
        "熱solver width={WIDTH} height={HEIGHT} cells={len} edges={} warmup={WARMUP} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations={allocation_count} allocated_bytes={allocated_bytes} peak_additional_bytes={peak_additional}",
        2 * WIDTH * HEIGHT - WIDTH - HEIGHT,
        percentile(&samples, 50, 100),
        percentile(&samples, 95, 100),
        percentile(&samples, 99, 100),
    );
}

#[test]
#[ignore = "10,000候補の反応job構築を手動性能計測"]
fn reaction_candidate_profile() {
    const CANDIDATES: usize = 10_000;
    const WARMUP: usize = 5;
    const SAMPLES: usize = 50;
    let material = key("bench:material");
    let family = EnthalpyFamily::new(
        key("bench:reaction-family"),
        vec![EnthalpyBand::Sensible {
            energy_start: 0,
            energy_end: 50_000,
            temperature_start_mk: 0,
            temperature_end_mk: 50_000,
            material,
        }],
    )
    .unwrap();
    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: key("bench:reaction"),
            input: material,
            contact: None,
            priority: 0,
            consume: 1,
            output: material,
            produce: 1,
            output_offset: [0, 1],
            particle_velocity_q16: None,
            minimum_temperature_mk: 0,
            fuel_cost: 0,
            timer_cost: 0,
            energy_delta: 0,
            maximum_wetness: u16::MAX,
            corrosion_delta: 0,
            state_transfer: ReactionStateTransfer::Consume,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    let cells = (0..CANDIDATES)
        .map(|x| ReactionCell {
            x: x as i32,
            y: 0,
            material,
            quantity: 1,
            energy: 25_000,
            state: ReactionSidecar::default(),
            can_react: true,
        })
        .collect::<Vec<_>>();
    for _ in 0..WARMUP {
        assert_eq!(
            ReactionJob::build(&cells, &rules, CANDIDATES)
                .unwrap()
                .remaining(),
            CANDIDATES
        );
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_live, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let job = ReactionJob::build(&cells, &rules, CANDIDATES).unwrap();
        samples.push(started.elapsed());
        assert_eq!(job.remaining(), CANDIDATES);
    }
    let allocation_count = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let peak_additional = PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_live);
    eprintln!(
        "反応候補build candidates={CANDIDATES} warmup={WARMUP} samples={SAMPLES} p50={:?} p95={:?} p99={:?} allocations={allocation_count} allocated_bytes={allocated_bytes} peak_additional_bytes={peak_additional}",
        percentile(&samples, 50, 100),
        percentile(&samples, 95, 100),
        percentile(&samples, 99, 100),
    );
}
