//! 仕様 04 §2: カメラ範囲の読み込み・退避と固定 tick の挿入。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};

use charon_core::chunk::Chunk;
use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::world::World;

use crate::generator::ChunkGenerator;
use crate::material_names;
use crate::region::{RegionError, RegionStore};

const MAX_PER_TICK: usize = 4;
pub const LOAD_DELAY: u64 = 30;

/// ワーカーの停止、または保存・読み込みの失敗。
#[derive(Debug)]
pub enum StreamError {
    Io(io::Error),
    Region(RegionError),
    WorkerClosed,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "作業スレッドの開始に失敗: {err}"),
            Self::Region(err) => write!(f, "チャンクの読み書きに失敗: {err}"),
            Self::WorkerClosed => write!(f, "チャンクの作業スレッドが終了しました"),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<RegionError> for StreamError {
    fn from(err: RegionError) -> Self {
        Self::Region(err)
    }
}

enum Job {
    Load { id: u64, pos: ChunkPos },
    Save { pos: ChunkPos, chunk: Chunk },
}

enum Reply {
    Loaded { id: u64, result: Result<Chunk, RegionError> },
    Saved(Result<(), RegionError>),
}

#[derive(Clone, Copy)]
struct Pending {
    id: u64,
    due: u64,
}

struct WorkContext {
    store: RegionStore,
    generator: Arc<dyn ChunkGenerator>,
    materials: Arc<MaterialTable>,
    saved_names: Vec<String>,
    seed: u64,
    #[cfg(test)]
    delay: Option<Arc<dyn Fn(ChunkPos) + Send + Sync>>,
}

impl WorkContext {
    fn process(&self, job: Job) -> Reply {
        match job {
            Job::Load { id, pos } => {
                #[cfg(test)]
                if let Some(delay) = &self.delay {
                    delay(pos);
                }
                let result = self
                    .store
                    .read_chunk(pos, &self.saved_names, Arc::clone(&self.materials))
                    .map(|saved| {
                        saved.unwrap_or_else(|| {
                            self.generator.generate(self.seed, pos, &self.materials)
                        })
                    });
                Reply::Loaded { id, result }
            }
            Job::Save { pos, chunk } => Reply::Saved(self.store.write_chunk_with_names(
                pos,
                &chunk,
                &self.materials,
                &self.saved_names,
            )),
        }
    }
}

/// 複数のカメラのチェビシェフ範囲を維持する。`tick` は `World::step` と同じ頻度で呼ぶ。
pub struct Streamer {
    load_radius: i32,
    pending: BTreeMap<ChunkPos, Pending>,
    ready: BTreeMap<u64, Chunk>,
    next_id: u64,
    receiver: Receiver<Reply>,
    #[cfg(not(target_arch = "wasm32"))]
    sender: Option<Sender<Job>>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<JoinHandle<()>>,
    #[cfg(target_arch = "wasm32")]
    reply_sender: Sender<Reply>,
    #[cfg(target_arch = "wasm32")]
    context: WorkContext,
}

impl fmt::Debug for Streamer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Streamer")
            .field("load_radius", &self.load_radius)
            .field("pending", &self.pending.len())
            .field("ready", &self.ready.len())
            .finish_non_exhaustive()
    }
}

impl Streamer {
    /// 読み込み範囲の半径とワールド固有の生成器・保存場所を指定する。
    pub fn new(
        root: impl Into<PathBuf>,
        materials: Arc<MaterialTable>,
        seed: u64,
        generator: Arc<dyn ChunkGenerator>,
        load_radius: i32,
    ) -> Result<Self, StreamError> {
        let names = material_names(&materials);
        Self::with_saved_names(root, materials, seed, generator, load_radius, names)
    }

    /// 保存済み `world.ron` の材質名順を用いて古い ID を復元する。
    pub fn with_saved_names(
        root: impl Into<PathBuf>,
        materials: Arc<MaterialTable>,
        seed: u64,
        generator: Arc<dyn ChunkGenerator>,
        load_radius: i32,
        saved_names: Vec<String>,
    ) -> Result<Self, StreamError> {
        Self::start(
            root.into(),
            materials,
            seed,
            generator,
            load_radius,
            saved_names,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    fn with_delay(
        root: PathBuf,
        materials: Arc<MaterialTable>,
        seed: u64,
        generator: Arc<dyn ChunkGenerator>,
        load_radius: i32,
        delay: Arc<dyn Fn(ChunkPos) + Send + Sync>,
    ) -> Result<Self, StreamError> {
        let names = material_names(&materials);
        Self::start(root, materials, seed, generator, load_radius, names, Some(delay))
    }

    fn start(
        root: PathBuf,
        materials: Arc<MaterialTable>,
        seed: u64,
        generator: Arc<dyn ChunkGenerator>,
        load_radius: i32,
        saved_names: Vec<String>,
        #[cfg(test)] delay: Option<Arc<dyn Fn(ChunkPos) + Send + Sync>>,
    ) -> Result<Self, StreamError> {
        let context = WorkContext {
            store: RegionStore::new(root),
            generator,
            saved_names,
            materials,
            seed,
            #[cfg(test)]
            delay,
        };
        let (reply_sender, receiver) = mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (sender, jobs) = mpsc::channel();
            let worker = thread::Builder::new()
                .name("charon-world-stream".into())
                .spawn(move || {
                    for job in jobs {
                        let _ = reply_sender.send(context.process(job));
                    }
                })
                .map_err(StreamError::Io)?;
            Ok(Self {
                load_radius: load_radius.max(0),
                pending: BTreeMap::new(),
                ready: BTreeMap::new(),
                next_id: 0,
                receiver,
                sender: Some(sender),
                worker: Some(worker),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Self {
                load_radius: load_radius.max(0),
                pending: BTreeMap::new(),
                ready: BTreeMap::new(),
                next_id: 0,
                receiver,
                reply_sender,
                context,
            })
        }
    }

    /// 画面サイズや拡大率の変化に合わせて読み込み半径を更新する。
    pub fn set_load_radius(&mut self, radius: i32) {
        self.load_radius = radius.max(0);
    }

    /// 要求 → 完成分の受領（最大 4）→ 範囲外の保存・退避（最大 4）の順。
    pub fn tick(&mut self, world: &mut World, cameras: &[ChunkPos]) -> Result<(), StreamError> {
        let now = world.tick();
        let mut wanted = BTreeSet::new();
        for &camera in cameras {
            for dy in -self.load_radius..=self.load_radius {
                for dx in -self.load_radius..=self.load_radius {
                    wanted.insert(ChunkPos::new(camera.x + dx, camera.y + dy));
                }
            }
        }
        let mut missing: Vec<_> = wanted
            .iter()
            .copied()
            .filter(|&pos| world.chunk(pos).is_none() && !self.pending.contains_key(&pos))
            .collect();
        missing.sort_by_key(|&pos| (distance(pos, cameras), pos));
        for pos in missing {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            self.dispatch(Job::Load { id, pos })?;
            self.pending.insert(pos, Pending { id, due: now.saturating_add(LOAD_DELAY) });
        }
        self.drain()?;

        let mut due: Vec<_> = self
            .pending
            .iter()
            .filter(|(pos, pending)| pending.due <= now && wanted.contains(pos))
            .map(|(&pos, &pending)| (pending.due, distance(pos, cameras), pos, pending.id))
            .collect();
        due.sort_unstable();
        for (_, _, pos, id) in due.into_iter().take(MAX_PER_TICK) {
            while !self.ready.contains_key(&id) {
                self.receive()?;
            }
            if let Some(chunk) = self.ready.remove(&id) {
                world.insert_loaded_chunk(pos, chunk);
                self.pending.remove(&pos);
            }
        }
        // 範囲から外れた要求は捨てる。遅れて届く古い完了は id で識別して無視する。
        self.pending.retain(|pos, _| wanted.contains(pos));
        self.ready.retain(|id, _| self.pending.values().any(|entry| entry.id == *id));

        let mut evict: Vec<_> = world
            .chunk_positions()
            .into_iter()
            .filter(|&pos| distance(pos, cameras) > i64::from(self.load_radius) + 2)
            .collect();
        evict.sort_unstable();
        for pos in evict.into_iter().take(MAX_PER_TICK) {
            if let Some(chunk) = world.take_chunk(pos) {
                self.dispatch(Job::Save { pos, chunk })?;
            }
        }
        Ok(())
    }

    /// 終了時に読み込み済みの全チャンクも保存キューへ送る。
    pub fn save_loaded(&self, world: &World) -> Result<(), StreamError> {
        for pos in world.chunk_positions() {
            if let Some(chunk) = world.chunk(pos) {
                self.dispatch(Job::Save { pos, chunk: chunk.clone() })?;
            }
        }
        Ok(())
    }

    /// 終了時にキュー内の保存をすべて完了させる。保存エラーも返す。
    pub fn finish(mut self) -> Result<(), StreamError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sender.take();
            if let Some(worker) = self.worker.take() {
                worker.join().map_err(|_| StreamError::WorkerClosed)?;
            }
        }
        self.drain()
    }

    fn dispatch(&self, job: Job) -> Result<(), StreamError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sender
                .as_ref()
                .ok_or(StreamError::WorkerClosed)?
                .send(job)
                .map_err(|_| StreamError::WorkerClosed)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.reply_sender.send(self.context.process(job)).map_err(|_| StreamError::WorkerClosed)
        }
    }

    fn drain(&mut self) -> Result<(), StreamError> {
        loop {
            match self.receiver.try_recv() {
                Ok(reply) => self.consume(reply)?,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn receive(&mut self) -> Result<(), StreamError> {
        let reply = self.receiver.recv().map_err(|_| StreamError::WorkerClosed)?;
        self.consume(reply)
    }

    fn consume(&mut self, reply: Reply) -> Result<(), StreamError> {
        match reply {
            Reply::Loaded { id, result } => {
                let chunk = result?;
                if self.pending.values().any(|pending| pending.id == id) {
                    self.ready.insert(id, chunk);
                }
            }
            Reply::Saved(result) => result?,
        }
        Ok(())
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sender.take();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

fn distance(pos: ChunkPos, cameras: &[ChunkPos]) -> i64 {
    cameras
        .iter()
        .map(|camera| {
            (i64::from(pos.x) - i64::from(camera.x))
                .abs()
                .max((i64::from(pos.y) - i64::from(camera.y)).abs())
        })
        .min()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use charon_core::coords::CellPos;
    use charon_core::rng::{SALT_CAVE, hash};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            loop {
                let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
                let path =
                    std::env::temp_dir().join(format!("charon-stream-{}-{id}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("テスト用ディレクトリを作成できない: {err}"),
                }
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("テスト用ディレクトリを削除できない");
        }
    }

    fn setup(dir: &TestDir, materials: &Arc<MaterialTable>, seed: u64) -> (World, Streamer) {
        let generator =
            Arc::new(crate::generator::FlatGenerator { stone: materials.id("stone").unwrap() });
        let world = World::new(Arc::clone(materials), seed);
        let streamer = Streamer::new(&dir.0, Arc::clone(materials), seed, generator, 1).unwrap();
        (world, streamer)
    }

    fn advance(world: &mut World, streamer: &mut Streamer, camera: ChunkPos, count: u64) {
        for _ in 0..count {
            streamer.tick(world, &[camera]).unwrap();
            world.step();
        }
    }

    #[test]
    fn walk_away_and_back() {
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        let dirt = materials.id("dirt").unwrap();
        for seed in 1..=8 {
            let dir = TestDir::new();
            let (mut world, mut streamer) = setup(&dir, &materials, seed);
            advance(&mut world, &mut streamer, ChunkPos::new(0, 0), 40);
            let changed = CellPos::new(4, 4);
            let cell = world.make_cell(dirt, changed);
            world.set(changed, cell);
            advance(&mut world, &mut streamer, ChunkPos::new(0, 0), 5);
            let before = world.state_hash();
            advance(&mut world, &mut streamer, ChunkPos::new(100, 0), 45);
            assert!(world.chunk(ChunkPos::new(0, 0)).is_none(), "seed {seed}");
            advance(&mut world, &mut streamer, ChunkPos::new(0, 0), 45);
            assert_eq!(world.state_hash(), before, "seed {seed}");
            assert_eq!(world.get(changed).material, dirt);
            streamer.finish().unwrap();
        }
    }

    #[test]
    fn streaming_is_deterministic() {
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        for seed in 1..=8 {
            let first_dir = TestDir::new();
            let second_dir = TestDir::new();
            let generator =
                Arc::new(crate::generator::FlatGenerator { stone: materials.id("stone").unwrap() });
            let mut first = World::new(Arc::clone(&materials), seed);
            let mut second = World::new(Arc::clone(&materials), seed);
            let mut a = Streamer::with_delay(
                first_dir.0.clone(),
                Arc::clone(&materials),
                seed,
                generator.clone(),
                1,
                Arc::new(|pos| {
                    std::thread::sleep(Duration::from_millis(
                        (hash(1, 0, pos.x, pos.y, SALT_CAVE) % 4) as u64,
                    ));
                }),
            )
            .unwrap();
            let mut b = Streamer::with_delay(
                second_dir.0.clone(),
                Arc::clone(&materials),
                seed,
                generator,
                1,
                Arc::new(|pos| {
                    std::thread::sleep(Duration::from_millis(
                        (hash(2, 0, pos.x, pos.y, SALT_CAVE) % 4) as u64,
                    ));
                }),
            )
            .unwrap();
            for tick in 0..120 {
                let camera = if tick < 40 {
                    ChunkPos::new(0, 0)
                } else if tick < 80 {
                    ChunkPos::new(4, 0)
                } else {
                    ChunkPos::new(0, 0)
                };
                a.tick(&mut first, &[camera]).unwrap();
                b.tick(&mut second, &[camera]).unwrap();
                assert_eq!(first.state_hash(), second.state_hash(), "seed {seed}, tick {tick}");
                first.step();
                second.step();
            }
            a.finish().unwrap();
            b.finish().unwrap();
        }
    }
}
