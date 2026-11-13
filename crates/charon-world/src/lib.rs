//! 仕様 04 §3: チャンクとリージョンファイルの保存形式。

pub mod background;
pub mod building;
pub mod demo;
pub mod generator;
#[cfg(not(target_arch = "wasm32"))]
pub mod region;
mod region_error;
pub use region_error::RegionError;
pub mod save;
pub mod streaming;
pub mod web_save;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use charon_core::cell::{Cell, FLAG_BODY, FLAG_PARITY, MaterialId};
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_AREA, CHUNK_SIZE, CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::world::World;

// LZ4 のサイズ接頭辞と、ローカルパレット + 最大 4096 個の 8 バイトセル。
const MAX_RAW_SIZE: usize = 2 + CHUNK_AREA * (2 + 8);

/// 壊れた、または現在の材質表に収まらないチャンク。
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkCodecError {
    InvalidMaterial(MaterialId),
    InvalidData,
}

impl fmt::Display for ChunkCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMaterial(id) => write!(f, "保存できない材質 ID: {}", id.0),
            Self::InvalidData => write!(f, "チャンクの保存データが壊れています"),
        }
    }
}

impl std::error::Error for ChunkCodecError {}

/// `world.ron` に入れる名前。ID の昇順なのでリストの添字が保存時の材質 ID になる。
pub fn material_names(materials: &MaterialTable) -> Vec<String> {
    (0..materials.len()).map(|id| materials.get(MaterialId(id as u16)).name.clone()).collect()
}

/// 保存時に動的なフラグと剛体のスタンプを除いたセル。
fn stored_cell(cell: Cell) -> Cell {
    if cell.has(FLAG_BODY) { Cell::default() } else { cell.with(FLAG_PARITY, false) }
}

/// チャンクをローカルパレットで符号化し、サイズを付けた LZ4 で圧縮する。
/// `world.ron` に書く材質名の一覧は [`material_names`] で取得する。
pub fn encode_chunk(chunk: &Chunk, materials: &MaterialTable) -> Result<Vec<u8>, ChunkCodecError> {
    encode_chunk_with_names(chunk, materials, &material_names(materials))
}

/// 現在の材質 ID を `world.ron` に保存した名前の順へ引き直して符号化する。
pub fn encode_chunk_with_names(
    chunk: &Chunk,
    materials: &MaterialTable,
    saved_names: &[String],
) -> Result<Vec<u8>, ChunkCodecError> {
    if chunk.cells.iter().copied().all(|cell| stored_cell(cell) == Cell::default()) {
        return Ok(lz4_flex::compress_prepend_size(&0u16.to_le_bytes()));
    }
    let name_indexes: BTreeMap<&str, u16> = saved_names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| u16::try_from(index).ok().map(|index| (name.as_str(), index)))
        .collect();
    let mut indexes = BTreeMap::<u16, u16>::new();
    let mut palette = Vec::<u16>::new();
    for cell in chunk.cells.iter() {
        let id = stored_cell(*cell).material;
        if id.0 as usize >= materials.len() {
            return Err(ChunkCodecError::InvalidMaterial(id));
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = indexes.entry(id.0) {
            let saved_id = name_indexes
                .get(materials.get(id).name.as_str())
                .copied()
                .ok_or(ChunkCodecError::InvalidMaterial(id))?;
            entry.insert(palette.len() as u16);
            palette.push(saved_id);
        }
    }
    let wide = palette.len() > 256;
    let mut raw =
        Vec::with_capacity(2 + palette.len() * 2 + CHUNK_AREA * (if wide { 8 } else { 7 }));
    raw.extend_from_slice(&(palette.len() as u16).to_le_bytes());
    for id in palette {
        raw.extend_from_slice(&id.to_le_bytes());
    }
    for cell in chunk.cells.iter() {
        let cell = stored_cell(*cell);
        let index = indexes[&cell.material.0];
        if wide {
            raw.extend_from_slice(&index.to_le_bytes());
        } else {
            raw.push(index as u8);
        }
        raw.push(cell.flags);
        raw.push(cell.shade);
        raw.extend_from_slice(&cell.temp.to_le_bytes());
        raw.push(cell.vy as u8);
        raw.push(cell.life);
    }
    Ok(lz4_flex::compress_prepend_size(&raw))
}

/// 保存時の `world.ron` の材質名から現在の ID へ変換してチャンクを読む。
/// セルは `World::set` を通して復元する（描画・衝突・休眠の dirty も付く）。
pub fn decode_chunk(
    compressed: &[u8],
    saved_names: &[String],
    materials: Arc<MaterialTable>,
) -> Result<Chunk, ChunkCodecError> {
    let size = compressed
        .get(..4)
        .and_then(|header| header.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ChunkCodecError::InvalidData)? as usize;
    if size > MAX_RAW_SIZE {
        return Err(ChunkCodecError::InvalidData);
    }
    let raw = lz4_flex::decompress_size_prepended(compressed)
        .map_err(|_| ChunkCodecError::InvalidData)?;
    let mut input = raw.as_slice();
    let palette_len = take_u16(&mut input)? as usize;
    if palette_len == 0 {
        return if input.is_empty() { Ok(Chunk::new()) } else { Err(ChunkCodecError::InvalidData) };
    }
    if palette_len > CHUNK_AREA {
        return Err(ChunkCodecError::InvalidData);
    }
    let wide = palette_len > 256;
    let expected = palette_len * 2 + CHUNK_AREA * (if wide { 8 } else { 7 });
    if input.len() != expected {
        return Err(ChunkCodecError::InvalidData);
    }
    let mut palette = Vec::with_capacity(palette_len);
    for _ in 0..palette_len {
        let name =
            saved_names.get(take_u16(&mut input)? as usize).ok_or(ChunkCodecError::InvalidData)?;
        let material = materials.id(name).unwrap_or_else(|| {
            eprintln!("不明な材質 {name:?} は EMPTY として読み込みます");
            MaterialId::EMPTY
        });
        palette.push((material, materials.id(name).is_none()));
    }
    let mut world = World::new(materials, 0);
    let pos = ChunkPos::new(0, 0);
    world.insert_chunk(pos);
    for y in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let index = if wide {
                take_u16(&mut input)? as usize
            } else {
                take(&mut input, 1)?[0] as usize
            };
            let (material, unknown) = *palette.get(index).ok_or(ChunkCodecError::InvalidData)?;
            let flags = take(&mut input, 1)?[0] & !(FLAG_PARITY | FLAG_BODY);
            let shade = take(&mut input, 1)?[0];
            let temp = i16::from_le_bytes(
                take(&mut input, 2)?.try_into().map_err(|_| ChunkCodecError::InvalidData)?,
            );
            let vy = take(&mut input, 1)?[0] as i8;
            let life = take(&mut input, 1)?[0];
            let cell = if unknown {
                Cell::default()
            } else {
                Cell { material, flags, shade, temp, vy, life }
            };
            if cell != Cell::default() {
                world.set(CellPos::new(x, y), cell);
            }
        }
    }
    world.chunk(pos).cloned().ok_or(ChunkCodecError::InvalidData)
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], ChunkCodecError> {
    let (value, rest) = input.split_at_checked(count).ok_or(ChunkCodecError::InvalidData)?;
    *input = rest;
    Ok(value)
}

fn take_u16(input: &mut &[u8]) -> Result<u16, ChunkCodecError> {
    let bytes: [u8; 2] = take(input, 2)?.try_into().map_err(|_| ChunkCodecError::InvalidData)?;
    Ok(u16::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::cell::FLAG_BLOCK;
    use charon_core::rng::{SALT_SHADE, hash};
    use charon_core::testkit::{box_art, world_from_ascii};

    #[test]
    fn roundtrip() {
        for seed in 1..=8 {
            let mut world = world_from_ascii(&box_art(62, 64), seed);
            let materials = Arc::clone(world.materials());
            let ids = [
                MaterialId::EMPTY,
                materials.id("sand").unwrap(),
                materials.id("water").unwrap(),
                materials.id("stone").unwrap(),
            ];
            for i in 0..300 {
                let x = 1 + (hash(seed, 0, i, 0, SALT_SHADE) % 62) as i32;
                let y = (hash(seed, 0, i, 1, SALT_SHADE) % 63) as i32;
                let pos = CellPos::new(x, y);
                let id = ids[(hash(seed, 0, i, 2, SALT_SHADE) % 4) as usize];
                let mut cell = world.make_cell(id, pos);
                cell.temp = i as i16 - 120;
                cell.vy = (i as i8).wrapping_sub(60);
                cell.life = i as u8;
                cell = cell.with(FLAG_PARITY, i % 2 == 0).with(FLAG_BODY, i % 19 == 0);
                world.set(pos, cell);
            }
            let original = world.chunk(ChunkPos::new(0, 0)).unwrap();
            let encoded = encode_chunk(original, &materials).unwrap();
            let decoded =
                decode_chunk(&encoded, &material_names(&materials), Arc::clone(&materials))
                    .unwrap();
            for (i, (&before, &after)) in
                original.cells.iter().zip(decoded.cells.iter()).enumerate()
            {
                let expected = if before.has(FLAG_BODY) {
                    Cell::default()
                } else {
                    before.with(FLAG_PARITY, false)
                };
                assert_eq!(after, expected, "seed {seed}, cell {i}");
            }
        }
    }

    /// ブロックの旗 (仕様 17 §2) は、保存・読み込みで残る (動的な旗とは違い、世界の一部)。
    #[test]
    fn block_flag_survives_a_chunk_roundtrip() {
        let mut world = world_from_ascii(&box_art(62, 64), 1);
        let materials = Arc::clone(world.materials());
        charon_core::testkit::place_block(&mut world, 2, 2, 's');
        charon_core::testkit::place_block(&mut world, 3, 2, '#');
        let original = world.chunk(ChunkPos::new(0, 0)).unwrap();
        let encoded = encode_chunk(original, &materials).unwrap();
        let decoded =
            decode_chunk(&encoded, &material_names(&materials), Arc::clone(&materials)).unwrap();
        let blocks = |chunk: &Chunk| chunk.cells.iter().filter(|c| c.has(FLAG_BLOCK)).count();
        assert_eq!(blocks(original), 128);
        assert_eq!(blocks(&decoded), 128, "旗が残る");
        let sand = materials.id("sand").unwrap();
        let (x, y) = (17usize, 17usize);
        let cell = decoded.cells[y * 64 + x];
        assert!(cell.has(FLAG_BLOCK) && cell.material == sand);
    }

    #[test]
    fn empty_chunk_is_small() {
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        let encoded = encode_chunk(&Chunk::new(), &materials).unwrap();
        assert!(encoded.len() <= 64, "空チャンクは {} バイト", encoded.len());
        let decoded =
            decode_chunk(&encoded, &material_names(&materials), Arc::clone(&materials)).unwrap();
        assert!(decoded.cells.iter().all(|cell| *cell == Cell::default()));

        // parity と剛体のスタンプだけなら、保存内容はやはり空チャンク。
        let mut world = World::new(Arc::clone(&materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        world.set(CellPos::new(1, 1), Cell::default().with(FLAG_PARITY, true));
        let pos = CellPos::new(2, 2);
        let body = world.make_cell(materials.id("stone").unwrap(), pos).with(FLAG_BODY, true);
        world.set(pos, body);
        let saved = encode_chunk(world.chunk(ChunkPos::new(0, 0)).unwrap(), &materials).unwrap();
        assert_eq!(saved, encoded);
        let invalid = lz4_flex::compress_prepend_size(&[0, 0, 1]);
        assert_eq!(
            decode_chunk(&invalid, &[], materials).err(),
            Some(ChunkCodecError::InvalidData)
        );
    }

    #[test]
    fn material_reorder() {
        let original = Arc::new(
            MaterialTable::from_ron(
                r#"[(name: "a", phase: Solid, color: (1, 2, 3, 255)),
                (name: "b", phase: Solid, color: (4, 5, 6, 255))]"#,
            )
            .unwrap(),
        );
        let reordered = Arc::new(
            MaterialTable::from_ron(
                r#"[(name: "b", phase: Solid, color: (4, 5, 6, 255)),
                (name: "a", phase: Solid, color: (1, 2, 3, 255))]"#,
            )
            .unwrap(),
        );
        let mut world = World::new(Arc::clone(&original), 4);
        world.insert_chunk(ChunkPos::new(0, 0));
        for (x, name) in [(2, "a"), (3, "b")] {
            let pos = CellPos::new(x, 4);
            world.set(pos, world.make_cell(original.id(name).unwrap(), pos));
        }
        let encoded = encode_chunk(world.chunk(ChunkPos::new(0, 0)).unwrap(), &original).unwrap();
        let decoded =
            decode_chunk(&encoded, &material_names(&original), Arc::clone(&reordered)).unwrap();
        for (x, name) in [(2, "a"), (3, "b")] {
            assert_eq!(reordered.get(decoded.get(x as u8, 4).material).name, name);
        }
        // 並び替え後の書き込みも、元の world.ron の ID 順を維持する。
        let reencoded =
            encode_chunk_with_names(&decoded, &reordered, &material_names(&original)).unwrap();
        let reloaded =
            decode_chunk(&reencoded, &material_names(&original), Arc::clone(&reordered)).unwrap();
        for (x, name) in [(2, "a"), (3, "b")] {
            assert_eq!(reordered.get(reloaded.get(x as u8, 4).material).name, name);
        }
    }

    #[test]
    fn unknown_material_becomes_empty() {
        let original = Arc::new(
            MaterialTable::from_ron(r#"[(name: "gone", phase: Solid, color: (1, 2, 3, 255))]"#)
                .unwrap(),
        );
        let mut world = World::new(Arc::clone(&original), 8);
        world.insert_chunk(ChunkPos::new(0, 0));
        let pos = CellPos::new(4, 5);
        world.set(pos, world.make_cell(original.id("gone").unwrap(), pos));
        let encoded = encode_chunk(world.chunk(ChunkPos::new(0, 0)).unwrap(), &original).unwrap();
        let current = Arc::new(MaterialTable::from_ron("[]").unwrap());
        let decoded = decode_chunk(&encoded, &material_names(&original), current).unwrap();
        assert_eq!(decoded.get(4, 5), Cell::default());
    }
}
