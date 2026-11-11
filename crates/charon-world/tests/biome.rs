use std::collections::BTreeMap;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::rng::hash;
use charon_world::generator::{BiomeGenerator, ChunkGenerator};

const SALT_STRUCTURE: u32 = 0x5354_5255;

fn generator(materials: &MaterialTable) -> BiomeGenerator {
    BiomeGenerator {
        dirt: materials.id("dirt").expect("テスト用の土"),
        stone: materials.id("stone").expect("テスト用の石"),
        wood: materials.id("wood").expect("テスト用の木"),
    }
}

#[test]
fn biome_boundary_is_pure() {
    let materials = MaterialTable::builtin().unwrap();
    let terrain = generator(&materials);
    let positions =
        [ChunkPos::new(-5, -1), ChunkPos::new(-4, 0), ChunkPos::new(3, 0), ChunkPos::new(4, -1)];
    for seed in 1..=8 {
        let forward: Vec<_> =
            positions.iter().map(|&pos| terrain.generate(seed, pos, &materials)).collect();
        for (i, &pos) in positions.iter().enumerate().rev() {
            assert_eq!(
                forward[i].cells,
                terrain.generate(seed, pos, &materials).cells,
                "seed={seed}, {pos:?}"
            );
        }
    }
}

#[test]
fn structure_crosses_chunk_border() {
    let materials = MaterialTable::builtin().unwrap();
    let terrain = generator(&materials);
    for seed in 1..=8 {
        // 候補の出現は seed と band だけで決まり、生成順には依存しない。
        let band =
            (-64..=64).find(|&band| hash(seed, 0, band, 0, SALT_STRUCTURE) & 7 == 0).unwrap();
        let center = band * 128 + 64;
        let mut chunks = BTreeMap::new();
        for cx in [center.div_euclid(64) - 1, center.div_euclid(64)] {
            for cy in -2..=2 {
                let pos = ChunkPos::new(cx, cy);
                chunks.insert(pos, terrain.generate(seed, pos, &materials));
            }
        }
        let ground = (-128..=128)
            .find(|&y| {
                let pos = CellPos::new(center, y);
                chunks
                    .get(&pos.chunk())
                    .is_some_and(|chunk| chunk.cells[pos.local_index()].material == terrain.dirt)
            })
            .unwrap();
        for x in center - 4..=center + 4 {
            let pos = CellPos::new(x, ground - 6);
            let cell = chunks.get(&pos.chunk()).unwrap().cells[pos.local_index()];
            assert_eq!(cell.material, terrain.wood, "seed={seed}, beam {pos:?}");
        }
        for y in ground - 6..ground {
            for x in center - 1..=center + 1 {
                let pos = CellPos::new(x, y);
                assert_eq!(
                    chunks.get(&pos.chunk()).unwrap().cells[pos.local_index()].material,
                    terrain.wood,
                    "seed={seed}, pillar {pos:?}"
                );
            }
        }
        for (&pos, original) in chunks.iter().rev() {
            assert_eq!(
                terrain.generate(seed, pos, &materials).cells,
                original.cells,
                "seed={seed}, {pos:?}"
            );
        }
    }
}

#[test]
fn biome_materials_are_named() {
    let first = MaterialTable::from_ron(
        r#"[
        (name: "stone", phase: Solid, density: 2600, color: (1, 1, 1, 255)),
        (name: "dirt", phase: Powder, density: 1300, color: (2, 2, 2, 255)),
        (name: "wood", phase: Solid, density: 600, color: (3, 3, 3, 255)),
    ]"#,
    )
    .unwrap();
    let second = MaterialTable::from_ron(
        r#"[
        (name: "wood", phase: Solid, density: 600, color: (3, 3, 3, 255)),
        (name: "dirt", phase: Powder, density: 1300, color: (2, 2, 2, 255)),
        (name: "stone", phase: Solid, density: 2600, color: (1, 1, 1, 255)),
    ]"#,
    )
    .unwrap();
    assert_ne!(first.id("stone"), second.id("stone"));
    let a = generator(&first);
    let b = generator(&second);
    for seed in 1..=8 {
        for pos in [ChunkPos::new(-1, 0), ChunkPos::new(0, 0), ChunkPos::new(1, -1)] {
            let left = a.generate(seed, pos, &first);
            let right = b.generate(seed, pos, &second);
            for (l, r) in left.cells.iter().zip(right.cells.iter()) {
                assert_eq!(
                    first.get(l.material).name,
                    second.get(r.material).name,
                    "seed={seed}, {pos:?}"
                );
            }
        }
    }
}
