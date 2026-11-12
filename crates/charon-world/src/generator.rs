//! 仕様 04 §4: 他のチャンクを参照しない決定的な地形生成。

use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_SIZE, CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::rng::{SALT_CAVE, SALT_TERRAIN, hash};
use charon_core::world::World;

/// 同じ seed と座標なら、生成順やスレッドの数に関係なく同じチャンクを作る。
pub trait ChunkGenerator: Send + Sync {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk;
}

/// y >= 0 を石で埋めるテスト用の生成器。材質は呼び出し元が指定する。
#[derive(Clone, Copy, Debug)]
pub struct FlatGenerator {
    pub stone: MaterialId,
}

impl ChunkGenerator for FlatGenerator {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk {
        generate_cells(seed, pos, materials, |_, y| (y >= 0).then_some(self.stone))
    }
}

/// 1 次元の地表と 2 次元の洞窟を整数の値ノイズで生成する。
#[derive(Clone, Copy, Debug)]
pub struct NoiseGenerator {
    pub dirt: MaterialId,
    pub stone: MaterialId,
}

impl ChunkGenerator for NoiseGenerator {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk {
        let origin = pos.origin();
        let heights = std::array::from_fn::<_, { CHUNK_SIZE as usize }, _>(|x| {
            surface_height(seed, origin.x + x as i32)
        });
        generate_cells(seed, pos, materials, |x, y| {
            let height = heights[(x - origin.x) as usize];
            if y < height || (y >= height + 8 && cave_value(seed, x, y) < 48) {
                None
            } else if y < height + 8 {
                Some(self.dirt)
            } else {
                Some(self.stone)
            }
        })
    }
}

const SALT_BIOME: u32 = 0x4249_4f4d;
const SALT_STRUCTURE: u32 = 0x5354_5255;

/// 仕様 07 §3: 256 セル単位のバイオームと、128 セル単位の独立した構造物。
#[derive(Clone, Copy, Debug)]
pub struct BiomeGenerator {
    pub dirt: MaterialId,
    pub stone: MaterialId,
    pub wood: MaterialId,
}

impl ChunkGenerator for BiomeGenerator {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk {
        let origin = pos.origin();
        let heights = std::array::from_fn::<_, { CHUNK_SIZE as usize }, _>(|x| {
            biome_height(seed, origin.x + x as i32)
        });
        generate_cells(seed, pos, materials, |x, y| {
            let center = x.div_euclid(128) * 128 + 64;
            let ground = biome_height(seed, center);
            if hash(seed, 0, x.div_euclid(128), 0, SALT_STRUCTURE) & 7 == 0
                && ((x >= center - 4 && x <= center + 4 && y == ground - 6)
                    || (x >= center - 1 && x <= center + 1 && y >= ground - 6 && y < ground))
            {
                return Some(self.wood);
            }
            let height = heights[(x - origin.x) as usize];
            if y < height || (y >= height + 8 && cave_value(seed, x, y) < 48) {
                None
            } else if y < height + 8 {
                Some(self.dirt)
            } else {
                Some(self.stone)
            }
        })
    }
}

fn biome_height(seed: u64, x: i32) -> i32 {
    let base = surface_height(seed, x);
    if hash(seed, 0, x.div_euclid(256), 0, SALT_BIOME) & 1 == 0 { base } else { base - 12 }
}

/// 必ず World::set を使って、dirty・衝突・描画の更新も含めてチャンクを作る。
pub(crate) fn generate_cells(
    seed: u64,
    pos: ChunkPos,
    materials: &MaterialTable,
    mut material_at: impl FnMut(i32, i32) -> Option<MaterialId>,
) -> Chunk {
    let mut world = World::new(Arc::new(materials.clone()), seed);
    world.insert_chunk(pos);
    let origin = pos.origin();
    for ly in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            let cell_pos = CellPos::new(origin.x + lx, origin.y + ly);
            if let Some(material) = material_at(cell_pos.x, cell_pos.y) {
                let cell = world.make_cell(material, cell_pos);
                world.set(cell_pos, cell);
            }
        }
    }
    // insert_chunk で必ず存在する。起こりえない場合も空チャンクにフォールバックする。
    debug_assert!(world.chunk(pos).is_some());
    world.chunk(pos).cloned().unwrap_or_default()
}

fn surface_height(seed: u64, x: i32) -> i32 {
    const SPACING: i32 = 64;
    let left = x.div_euclid(SPACING);
    let fraction = x.rem_euclid(SPACING);
    let a = (hash(seed, 0, left, 0, SALT_TERRAIN) & 31) as i32 - 16;
    let b = (hash(seed, 0, left + 1, 0, SALT_TERRAIN) & 31) as i32 - 16;
    (a * (SPACING - fraction) + b * fraction) / SPACING
}

fn cave_value(seed: u64, x: i32, y: i32) -> i32 {
    const SPACING: i32 = 16;
    let (gx, gy) = (x.div_euclid(SPACING), y.div_euclid(SPACING));
    let (fx, fy) = (x.rem_euclid(SPACING), y.rem_euclid(SPACING));
    let sample = |dx, dy| (hash(seed, 0, gx + dx, gy + dy, SALT_CAVE) & 255) as i32;
    let top = sample(0, 0) * (SPACING - fx) + sample(1, 0) * fx;
    let bottom = sample(0, 1) * (SPACING - fx) + sample(1, 1) * fx;
    (top * (SPACING - fy) + bottom * fy) / (SPACING * SPACING)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_pure() {
        let materials = MaterialTable::builtin().unwrap();
        let stone = materials.id("stone").unwrap();
        let flat = FlatGenerator { stone };
        let noise = NoiseGenerator { dirt: materials.id("dirt").unwrap(), stone };
        let positions = [ChunkPos::new(0, 0), ChunkPos::new(-2, 1), ChunkPos::new(3, -1)];
        for seed in 1..=8 {
            for generator in [&flat as &dyn ChunkGenerator, &noise as &dyn ChunkGenerator] {
                let forward: Vec<_> = positions
                    .iter()
                    .map(|&pos| generator.generate(seed, pos, &materials))
                    .collect();
                for (i, &pos) in positions.iter().enumerate().rev() {
                    let again = generator.generate(seed, pos, &materials);
                    assert_eq!(forward[i].cells, again.cells, "seed {seed}, {pos:?}");
                }
            }
        }
        let upper = flat.generate(1, ChunkPos::new(0, -1), &materials);
        assert!(upper.cells.iter().all(|cell| cell.material == MaterialId::EMPTY));
        let lower = flat.generate(1, ChunkPos::new(0, 0), &materials);
        assert!(lower.cells.iter().all(|cell| cell.material == stone));
    }
}
