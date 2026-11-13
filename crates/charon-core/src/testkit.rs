//! 場面テストのための補助 (`docs/TESTING.md` §2)。
//!
//! テスト専用の道具なので、誤った使い方 (知らない文字など) は `panic` で知らせる。

use std::sync::Arc;

use crate::cell::MaterialId;
use crate::coords::{CHUNK_SHIFT, CHUNK_SIZE, CellPos, ChunkPos};
use crate::material::MaterialTable;
use crate::world::World;

/// 文字と材質名の対応 (`docs/TESTING.md` §2 の表)。
const LEGEND: &[(char, &str)] = &[
    ('.', "empty"),
    ('#', "stone"),
    ('s', "sand"),
    ('d', "dirt"),
    ('w', "water"),
    ('m', "slime"),
    ('o', "oil"),
    ('l', "lava"),
    ('g', "methane"),
    ('t', "steam"),
    ('W', "wood"),
    ('i', "ice"),
    ('a', "acid"),
    ('f', "fire"),
    ('k', "smoke"),
    ('h', "ash"),
];

/// 文字の絵から世界を作る。絵の左上が `CellPos { x: 0, y: 0 }`。
///
/// 絵を覆うのに必要なチャンクを作り、絵の外側のセルはすべて石で埋める。
/// 各行の前後の空白と空行は無視する。全行の長さが同じでなければ panic。
pub fn world_from_ascii(art: &str, seed: u64) -> World {
    let rows: Vec<Vec<char>> =
        art.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| l.chars().collect()).collect();
    assert!(!rows.is_empty(), "絵が空です");
    let width = rows[0].len();
    assert!(rows.iter().all(|r| r.len() == width), "絵の行の長さが揃っていません");
    let height = rows.len();

    let materials = MaterialTable::builtin().expect("assets/materials.ron を読めません");
    let mut world =
        World::try_new(Arc::new(materials), seed).expect("assets/reactions.ron を読めません");
    let chunks_x = (width as i32 - 1) >> CHUNK_SHIFT;
    let chunks_y = (height as i32 - 1) >> CHUNK_SHIFT;
    for cy in 0..=chunks_y {
        for cx in 0..=chunks_x {
            world.insert_chunk(ChunkPos::new(cx, cy));
        }
    }

    for y in 0..(chunks_y + 1) * CHUNK_SIZE {
        for x in 0..(chunks_x + 1) * CHUNK_SIZE {
            let ch = rows.get(y as usize).and_then(|r| r.get(x as usize)).copied().unwrap_or('#');
            if ch != '.' {
                place(&mut world, x, y, ch);
            }
        }
    }
    world
}

/// 上が開いた箱の絵を作る。内側は幅 `width`、高さ `height` の空間で、左右と下が石の壁。
/// 内側の左上は `(1, 0)`、床の上の行は `y = height - 1`。
pub fn box_art(width: usize, height: usize) -> String {
    let mut art = String::new();
    for _ in 0..height {
        art.push('#');
        art.push_str(&".".repeat(width));
        art.push_str("#\n");
    }
    art.push_str(&"#".repeat(width + 2));
    art
}

/// M9 の性能場面: 砂 1,048,576 個を空行と交互に並べ、更新される tick 1 へ進める。
pub fn falling_million() -> World {
    let mut world = world_from_ascii(&box_art(1024, 2048), 1);
    for y in (0..2048).step_by(2) {
        fill(&mut world, 1, y, 1024, y, 's');
    }
    // tick 0 は初期 parity と一致し、砂の移動を測れない。
    world.step();
    world
}

/// 世界の矩形 (`x0..x1`、`y0..y1`。終端は含まない) を文字の絵に戻す。行は改行で区切る。
pub fn to_ascii(world: &World, x0: i32, y0: i32, x1: i32, y1: i32) -> String {
    let materials = world.materials();
    let mut out = String::new();
    for y in y0..y1 {
        if y > y0 {
            out.push('\n');
        }
        for x in x0..x1 {
            let name = &materials.get(world.get(CellPos::new(x, y)).material).name;
            let ch = LEGEND.iter().find(|(_, n)| n == name).map(|(c, _)| *c).unwrap_or('?');
            out.push(ch);
        }
    }
    out
}

/// 文字から材質へ。知らない文字は panic。
pub fn material_of(world: &World, ch: char) -> MaterialId {
    let name = LEGEND
        .iter()
        .find(|(c, _)| *c == ch)
        .map(|(_, n)| *n)
        .unwrap_or_else(|| panic!("testkit が知らない文字です: {ch:?}"));
    world.materials().id(name).unwrap_or_else(|| panic!("材質 {name:?} が定義されていません"))
}

/// `(x, y)` に材質を 1 個置く。
pub fn place(world: &mut World, x: i32, y: i32, ch: char) {
    let pos = CellPos::new(x, y);
    let material = material_of(world, ch);
    // 空を置くときは、フラグも温度もない本当の「空」にする。
    let cell = if material == MaterialId::EMPTY {
        crate::cell::Cell::default()
    } else {
        world.make_cell(material, pos)
    };
    world.set(pos, cell);
}

/// ブロック `(bx, by)` (8×8 セル) を、材質 `ch` の `FLAG_BLOCK` つきのセルで埋める (仕様 17)。
pub fn place_block(world: &mut World, bx: i32, by: i32, ch: char) {
    let material = material_of(world, ch);
    for y in by * 8..by * 8 + 8 {
        for x in bx * 8..bx * 8 + 8 {
            let pos = CellPos::new(x, y);
            let cell = world
                .make_cell(material, pos)
                .with(crate::cell::FLAG_BLOCK, true)
                .with(crate::cell::FLAG_FALLING, false);
            world.set(pos, cell);
        }
    }
}

/// 旗 `FLAG_BLOCK` のセルの数。
pub fn count_block_cells(world: &World, x0: i32, y0: i32, x1: i32, y1: i32) -> usize {
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if world.get(CellPos::new(x, y)).has(crate::cell::FLAG_BLOCK) {
                n += 1;
            }
        }
    }
    n
}

/// 矩形 (両端を含む) を材質で埋める。
pub fn fill(world: &mut World, x0: i32, y0: i32, x1: i32, y1: i32, ch: char) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            place(world, x, y, ch);
        }
    }
}

/// `(x, 0)` から材質を `n` 個、1 tick に 1 個ずつ落とす (置くたびに 1 tick 進める)。
/// 注ぎ口が塞がって置けない状態が続いたら panic。
pub fn pour(world: &mut World, x: i32, ch: char, n: usize) {
    let mut placed = 0;
    let mut blocked = 0;
    while placed < n {
        if world.get(CellPos::new(x, 0)).material == MaterialId::EMPTY {
            place(world, x, 0, ch);
            placed += 1;
            blocked = 0;
        } else {
            blocked += 1;
            assert!(
                blocked < 1000,
                "注ぎ口 ({x}, 0) が塞がっています ({placed}/{n} 個を置いた時点)"
            );
        }
        world.step();
    }
}

/// 読み込まれている範囲 (セル単位。終端は含まない)。チャンクがなければ `None`。
fn bounds(world: &World) -> Option<(i32, i32, i32, i32)> {
    let chunks = world.chunk_positions();
    let x0 = chunks.iter().map(|c| c.x).min()?;
    let x1 = chunks.iter().map(|c| c.x).max()?;
    let y0 = chunks.iter().map(|c| c.y).min()?;
    let y1 = chunks.iter().map(|c| c.y).max()?;
    Some((x0 * CHUNK_SIZE, y0 * CHUNK_SIZE, (x1 + 1) * CHUNK_SIZE, (y1 + 1) * CHUNK_SIZE))
}

/// 世界全体で、その材質のセルを数える。
pub fn count(world: &World, ch: char) -> usize {
    let id = material_of(world, ch);
    world
        .chunk_positions()
        .iter()
        .filter_map(|&pos| world.chunk(pos))
        .map(|chunk| chunk.cells.iter().filter(|c| c.material == id).count())
        .sum()
}

/// 列 `x0..x1` のそれぞれについて、その材質のセル数を返す。水面の高さの比較に使う。
pub fn column_heights(world: &World, ch: char, x0: i32, x1: i32) -> Vec<u32> {
    let id = material_of(world, ch);
    let Some((_, y0, _, y1)) = bounds(world) else {
        return vec![0; (x1 - x0).max(0) as usize];
    };
    (x0..x1)
        .map(|x| (y0..y1).filter(|&y| world.get(CellPos::new(x, y)).material == id).count() as u32)
        .collect()
}

/// n tick 進める。
pub fn run(world: &mut World, ticks: u64) {
    for _ in 0..ticks {
        world.step();
    }
}

/// 世界全体が休眠する (`awake_chunks() == 0`) まで進める。かかった tick 数を返す。
/// `max_ticks` を超えたら `None`。
pub fn run_until_sleep(world: &mut World, max_ticks: u64) -> Option<u64> {
    for i in 0..=max_ticks {
        if world.awake_chunks() == 0 {
            return Some(i);
        }
        world.step();
    }
    None
}

/// `run_until_sleep` が `None` なら、世界の絵と起きているチャンクの位置を表示して panic。
pub fn assert_sleeps(world: &mut World, max_ticks: u64) {
    if run_until_sleep(world, max_ticks).is_some() {
        return;
    }
    let awake: Vec<ChunkPos> = world
        .chunk_positions()
        .into_iter()
        .filter(|&p| world.chunk(p).is_some_and(|c| c.dirty.is_some() || c.next_dirty.is_some()))
        .collect();
    let art = match bounds(world) {
        Some((x0, y0, x1, y1)) => to_ascii(world, x0, y0, x1, y1),
        None => String::new(),
    };
    panic!(
        "{max_ticks} tick 進めても休眠しません (seed {})。起きているチャンク: {awake:?}\n{art}",
        world.seed()
    );
}

/// n tick 進め、毎 tick の `state_hash()` を集めて返す。
pub fn hash_trace(world: &mut World, ticks: u64) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            world.step();
            world.state_hash()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ART: &str = "
        #......#
        #..ss..#
        #wwwwww#
        ########
    ";

    #[test]
    fn ascii_roundtrip() {
        let world = world_from_ascii(ART, 1);
        assert_eq!(to_ascii(&world, 0, 0, 8, 4), "#......#\n#..ss..#\n#wwwwww#\n########");
    }

    #[test]
    fn outside_of_art_is_stone() {
        let world = world_from_ascii(ART, 1);
        assert_eq!(to_ascii(&world, 8, 0, 10, 1), "##");
        assert_eq!(to_ascii(&world, 0, 4, 2, 5), "##");
        assert_eq!(world.chunk_positions(), vec![ChunkPos::new(0, 0)]);
    }

    #[test]
    fn large_art_spans_chunks() {
        let row = ".".repeat(65);
        let world = world_from_ascii(&row, 1);
        assert_eq!(world.chunk_positions().len(), 2);
    }

    #[test]
    fn count_and_column_heights() {
        let mut world = world_from_ascii(ART, 1);
        assert_eq!(count(&world, 's'), 2);
        assert_eq!(count(&world, 'w'), 6);
        assert_eq!(column_heights(&world, 'w', 0, 3), vec![0, 1, 1]);
        fill(&mut world, 1, 0, 2, 1, 'w');
        assert_eq!(column_heights(&world, 'w', 0, 3), vec![0, 3, 3]);
    }

    #[test]
    fn every_legend_entry_exists() {
        let world = world_from_ascii(".", 1);
        for (ch, _) in LEGEND {
            material_of(&world, *ch);
        }
    }

    #[test]
    #[should_panic(expected = "知らない文字")]
    fn unknown_char_panics() {
        world_from_ascii("Z", 1);
    }

    #[test]
    #[should_panic(expected = "揃っていません")]
    fn ragged_art_panics() {
        world_from_ascii("..\n...", 1);
    }

    #[test]
    fn hash_trace_has_one_entry_per_tick() {
        let mut world = world_from_ascii(ART, 1);
        assert_eq!(hash_trace(&mut world, 5).len(), 5);
        assert_eq!(world.tick(), 5);
    }
}
