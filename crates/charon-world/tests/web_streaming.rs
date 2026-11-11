#![allow(clippy::unwrap_used)] // テストの前提と保存失敗は即時に報告する。

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit::{box_art, world_from_ascii};
use charon_world::generator::FlatGenerator;
use charon_world::material_names;
use charon_world::streaming::{LOAD_DELAY, MemoryStore, Streamer};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn web_streaming_tick_is_deterministic() {
    let materials = Arc::new(MaterialTable::builtin().unwrap());
    for seed in 1..=8 {
        let root = std::env::temp_dir().join(format!(
            "charon-web-stream-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let generator = Arc::new(FlatGenerator { stone: materials.id("stone").unwrap() });
        let names = material_names(&materials);
        let store = Arc::new(MemoryStore::new());
        let mut native = Streamer::with_saved_names(
            &root,
            Arc::clone(&materials),
            seed,
            generator.clone(),
            0,
            names.clone(),
        )
        .unwrap();
        let mut synchronous = Streamer::with_memory_store(
            Arc::clone(&store),
            Arc::clone(&materials),
            seed,
            generator,
            0,
            names,
        )
        .unwrap();
        let art = box_art(62, 64);
        let mut a = world_from_ascii(&art, seed);
        let mut b = world_from_ascii(&art, seed);
        let camera = ChunkPos::new(9, -9);
        for tick in 0..=LOAD_DELAY + 2 {
            native.tick(&mut a, &[camera]).unwrap();
            synchronous.tick(&mut b, &[camera]).unwrap();
            let should_exist = tick >= LOAD_DELAY;
            assert_eq!(a.chunk(camera).is_some(), should_exist, "native seed={seed} tick={tick}");
            assert_eq!(b.chunk(camera).is_some(), should_exist, "web seed={seed} tick={tick}");
            assert_eq!(a.state_hash(), b.state_hash(), "seed={seed} tick={tick}");
            a.restore_tick(tick + 1);
            b.restore_tick(tick + 1);
        }
        // 退避されたチャンクも同期的に保存して再ロードする。
        for tick in LOAD_DELAY + 3..=LOAD_DELAY * 2 + 5 {
            native.tick(&mut a, &[ChunkPos::new(30, 30)]).unwrap();
            synchronous.tick(&mut b, &[ChunkPos::new(30, 30)]).unwrap();
            assert_eq!(a.state_hash(), b.state_hash(), "seed={seed} tick={tick}");
            a.restore_tick(tick + 1);
            b.restore_tick(tick + 1);
        }
        assert!(store.snapshot().unwrap().contains_key(&camera), "seed={seed}");
        native.finish().unwrap();
        synchronous.finish().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
