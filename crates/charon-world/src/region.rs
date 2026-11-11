//! 仕様 04 §3: 32×32 チャンクをまとめるリージョンファイル。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use charon_core::chunk::Chunk;
use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;

pub use crate::RegionError;
use crate::{decode_chunk, encode_chunk_with_names, material_names};

const MAGIC: &[u8; 4] = b"CHRN";
const VERSION: u16 = 1;
const ENTRIES: usize = 32 * 32;
const HEADER_LEN: usize = 8 + ENTRIES * 8;

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    offset: u32,
    length: u32,
}

/// `saves/<world_name>` を起点とするリージョンの読み書き。
#[derive(Debug)]
pub struct RegionStore {
    root: PathBuf,
}

impl RegionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, pos: ChunkPos) -> PathBuf {
        self.root.join("regions").join(format!("r.{}.{}.bin", pos.x >> 5, pos.y >> 5))
    }

    /// 保存していないチャンクなら `None`。未知のバージョンは明示的に拒否する。
    pub fn read_chunk(
        &self,
        pos: ChunkPos,
        saved_names: &[String],
        materials: Arc<MaterialTable>,
    ) -> Result<Option<Chunk>, RegionError> {
        let bytes = match fs::read(self.path(pos)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let entries = parse_header(&bytes)?;
        let entry = entries[index(pos)];
        if entry.length == 0 {
            return Ok(None);
        }
        let start = entry.offset as usize;
        let end = start + entry.length as usize;
        Ok(Some(decode_chunk(&bytes[start..end], saved_names, materials)?))
    }

    /// 新しいチャンクを末尾に追記した内容を別ファイルに作り、同期後に置換する。
    /// 無駄な領域が本体の半分を超えたら、生きたデータだけ詰め直してから置換する。
    pub fn write_chunk(
        &self,
        pos: ChunkPos,
        chunk: &Chunk,
        materials: &MaterialTable,
    ) -> Result<(), RegionError> {
        self.write_chunk_with_names(pos, chunk, materials, &material_names(materials))
    }

    /// 保存元の材質名の順番でパレットを記録する。
    pub fn write_chunk_with_names(
        &self,
        pos: ChunkPos,
        chunk: &Chunk,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<(), RegionError> {
        let compressed = encode_chunk_with_names(chunk, materials, saved_names)?;
        let path = self.path(pos);
        let mut bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => fresh_region(),
            Err(err) => return Err(err.into()),
        };
        let mut entries = parse_header(&bytes)?;
        let offset = u32::try_from(bytes.len()).map_err(|_| RegionError::TooLarge)?;
        let length = u32::try_from(compressed.len()).map_err(|_| RegionError::TooLarge)?;
        let _end = offset.checked_add(length).ok_or(RegionError::TooLarge)?;
        bytes.extend_from_slice(&compressed);
        entries[index(pos)] = Entry { offset, length };
        let live: usize = entries.iter().map(|entry| entry.length as usize).sum();
        let body = bytes.len() - HEADER_LEN;
        if body - live > body / 2 {
            bytes = compact(&bytes, &mut entries)?;
        }
        write_header(&mut bytes, &entries);
        atomic_replace(&path, &bytes)?;
        Ok(())
    }
}

fn index(pos: ChunkPos) -> usize {
    ((pos.y & 31) as usize) * 32 + (pos.x & 31) as usize
}

fn fresh_region() -> Vec<u8> {
    let mut bytes = vec![0; HEADER_LEN];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
    bytes
}

fn parse_header(bytes: &[u8]) -> Result<[Entry; ENTRIES], RegionError> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return Err(RegionError::InvalidHeader);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version > VERSION {
        return Err(RegionError::NewerVersion(version));
    }
    if version != VERSION || bytes[6..8] != [0, 0] {
        return Err(RegionError::InvalidHeader);
    }
    let mut entries = [Entry::default(); ENTRIES];
    for (i, entry) in entries.iter_mut().enumerate() {
        let at = 8 + i * 8;
        entry.offset = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        entry.length =
            u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]);
        if entry.length != 0 {
            let start = entry.offset as usize;
            let Some(end) = start.checked_add(entry.length as usize) else {
                return Err(RegionError::InvalidHeader);
            };
            if start < HEADER_LEN || end > bytes.len() {
                return Err(RegionError::InvalidHeader);
            }
        }
    }
    Ok(entries)
}

fn write_header(bytes: &mut [u8], entries: &[Entry; ENTRIES]) {
    for (i, entry) in entries.iter().enumerate() {
        let at = 8 + i * 8;
        bytes[at..at + 4].copy_from_slice(&entry.offset.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&entry.length.to_le_bytes());
    }
}

fn compact(bytes: &[u8], entries: &mut [Entry; ENTRIES]) -> Result<Vec<u8>, RegionError> {
    let mut result = fresh_region();
    for entry in entries.iter_mut().filter(|entry| entry.length != 0) {
        let offset = u32::try_from(result.len()).map_err(|_| RegionError::TooLarge)?;
        let end = (entry.offset as usize) + entry.length as usize;
        result.extend_from_slice(&bytes[entry.offset as usize..end]);
        entry.offset = offset;
    }
    Ok(result)
}

pub(crate) fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), RegionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    for suffix in 0..32 {
        let temporary = path.with_extension(format!("bin.tmp.{suffix}"));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temporary) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        };
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, path)?;
        return Ok(());
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "空いている一時ファイル名がありません").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use charon_core::cell::Cell;
    use charon_core::coords::CellPos;
    use charon_core::testkit::{box_art, world_from_ascii};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            loop {
                let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
                let path = base.join(format!("charon-region-{}-{id}", std::process::id()));
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
            fs::remove_dir_all(&self.0).expect("テストの一時ファイルを削除できない");
        }
    }

    #[test]
    fn crash_safety() {
        let dir = TestDir::new();
        let store = RegionStore::new(&dir.0);
        let pos = ChunkPos::new(-1, 0);
        let mut world = world_from_ascii(&box_art(62, 64), 1);
        let materials = Arc::clone(world.materials());
        let saved_names = crate::material_names(&materials);
        let p = CellPos::new(10, 10);
        let sand = world.make_cell(materials.id("sand").unwrap(), p);
        world.set(p, sand);
        store.write_chunk(pos, world.chunk(ChunkPos::new(0, 0)).unwrap(), &materials).unwrap();
        let original =
            store.read_chunk(pos, &saved_names, Arc::clone(&materials)).unwrap().unwrap();
        assert_eq!(original.get(10, 10).material, sand.material);

        // 書き込み途中で落ちた一時ファイルは公開済みのリージョンに影響しない。
        let path = store.path(pos);
        let stale = path.with_extension("bin.tmp.0");
        let previous = fs::read(&path).unwrap();
        fs::write(&stale, &previous[..HEADER_LEN + 3]).unwrap();
        let loaded = store.read_chunk(pos, &saved_names, Arc::clone(&materials)).unwrap().unwrap();
        assert_eq!(loaded.get(10, 10), original.get(10, 10));
        assert_eq!(fs::read(&path).unwrap(), previous);

        let water = world.make_cell(materials.id("water").unwrap(), p);
        world.set(p, water);
        store.write_chunk(pos, world.chunk(ChunkPos::new(0, 0)).unwrap(), &materials).unwrap();
        let new = store.read_chunk(pos, &saved_names, Arc::clone(&materials)).unwrap().unwrap();
        assert_eq!(new.get(10, 10).material, water.material);
        assert!(store.read_chunk(ChunkPos::new(-2, 0), &saved_names, materials).unwrap().is_none());
    }

    #[test]
    fn newer_version_is_rejected() {
        let dir = TestDir::new();
        let store = RegionStore::new(&dir.0);
        let pos = ChunkPos::new(0, 0);
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        store.write_chunk(pos, &Chunk::new(), &materials).unwrap();
        let path = store.path(pos);
        let mut bytes = fs::read(&path).unwrap();
        bytes[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(matches!(store.read_chunk(pos, &[], materials), Err(RegionError::NewerVersion(2))));
    }

    #[test]
    fn compacts_when_stale_over_half() {
        let dir = TestDir::new();
        let store = RegionStore::new(&dir.0);
        let pos = ChunkPos::new(0, 0);
        let world = world_from_ascii(&box_art(62, 64), 1);
        let materials = Arc::clone(world.materials());
        store.write_chunk(pos, world.chunk(pos).unwrap(), &materials).unwrap();
        let full_size = fs::metadata(store.path(pos)).unwrap().len();
        store.write_chunk(pos, &Chunk::new(), &materials).unwrap();
        let compact_size = fs::metadata(store.path(pos)).unwrap().len();
        assert!(compact_size < full_size);
        assert_eq!(compact_size as usize, HEADER_LEN + 7);
        let loaded =
            store.read_chunk(pos, &crate::material_names(&materials), materials).unwrap().unwrap();
        assert_eq!(loaded.get(10, 10), Cell::default());
    }

    #[test]
    fn region_header_layout() {
        let dir = TestDir::new();
        let store = RegionStore::new(&dir.0);
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        let pos = ChunkPos::new(-1, 33);
        store.write_chunk(pos, &Chunk::new(), &materials).unwrap();
        let bytes = fs::read(store.path(pos)).unwrap();
        assert_eq!(&bytes[..4], MAGIC);
        assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
        assert_eq!(bytes.len(), HEADER_LEN + 7); // 空チャンクは 2 バイトの LZ4 保存値
        let entries = parse_header(&bytes).unwrap();
        assert_eq!(entries[index(pos)].offset as usize, HEADER_LEN);
        assert_eq!(
            store
                .read_chunk(pos, &crate::material_names(&materials), materials)
                .unwrap()
                .unwrap()
                .get(5, 5),
            Cell::default()
        );
    }
}
