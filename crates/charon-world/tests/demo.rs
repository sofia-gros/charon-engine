#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use std::collections::BTreeSet;

use charon_core::cell::MaterialId;
use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_world::demo::{
    Biome, DOOR, DemoGenerator, DemoMaterials, GREEN_LABEL, PLAYER_SPAWN, ROOM_INTERIOR,
    SIGN_FRAME, SIGN_LABEL, SIGN_TEXT, WORLD_HEIGHT, YELLOW_TEXT, biome, text_cells, tunnel_center,
    yellow_text_placement,
};
use charon_world::generator::ChunkGenerator;

fn table() -> MaterialTable {
    MaterialTable::from_mods(&[(
        "fse_demo",
        include_str!("../../../assets/mods/fse_demo/materials.ron"),
    )])
    .unwrap()
}

fn generator(table: &MaterialTable) -> DemoGenerator {
    DemoGenerator { materials: DemoMaterials::from_table(table).unwrap() }
}

#[test]
fn demo_generator_is_pure() {
    let table = table();
    let generator = generator(&table);
    let positions = [
        ChunkPos::new(0, 3),
        ChunkPos::new(-3, 5),
        ChunkPos::new(7, 2),
        ChunkPos::new(2, -1),
        ChunkPos::new(-1, 10),
        ChunkPos::new(5, 6),
    ];
    for seed in 1..=8 {
        let forward: Vec<_> =
            positions.iter().map(|&pos| generator.generate(seed, pos, &table)).collect();
        for (i, &pos) in positions.iter().enumerate().rev() {
            let again = generator.generate(seed, pos, &table);
            assert_eq!(forward[i].cells, again.cells, "seed {seed}, {pos:?}");
        }
    }
    // キャッシュを使うチャンク生成と、1 セルずつの評価が同じ結果になる。
    for seed in [1, 2, 5] {
        for &pos in &positions {
            let chunk = generator.generate(seed, pos, &table);
            let origin = pos.origin();
            for ly in 0..64 {
                for lx in 0..64 {
                    let expected = generator
                        .material_at(seed, origin.x + lx, origin.y + ly)
                        .unwrap_or(MaterialId::EMPTY);
                    assert_eq!(
                        chunk.cells[(ly * 64 + lx) as usize].material,
                        expected,
                        "seed {seed}, {pos:?}, ({lx}, {ly})"
                    );
                }
            }
        }
    }
}

#[test]
fn demo_room_is_open() {
    let table = table();
    let generator = generator(&table);
    let materials = generator.materials;
    let [x0, y0, x1, y1] = ROOM_INTERIOR;
    let sign: BTreeSet<_> = text_cells(SIGN_LABEL, 1, false)
        .into_iter()
        .map(|(dx, dy)| (SIGN_TEXT[0] + dx, SIGN_TEXT[1] + dy))
        .collect();
    for seed in 1..=8 {
        for y in y0..y1 {
            for x in x0..x1 {
                let cell = generator.material_at(seed, x, y);
                let on_frame = x >= SIGN_FRAME[0]
                    && x < SIGN_FRAME[2]
                    && y >= SIGN_FRAME[1]
                    && y < SIGN_FRAME[3]
                    && (x == SIGN_FRAME[0]
                        || x == SIGN_FRAME[2] - 1
                        || y == SIGN_FRAME[1]
                        || y == SIGN_FRAME[3] - 1);
                if on_frame || sign.contains(&(x, y)) {
                    assert_eq!(cell, Some(materials.sign_blue), "seed {seed}, ({x}, {y})");
                } else {
                    assert_eq!(cell, None, "部屋の内側は空: seed {seed}, ({x}, {y})");
                }
            }
        }
        for x in x0 - 4..x1 + 4 {
            assert_eq!(generator.material_at(seed, x, y1), Some(materials.stone), "床");
            assert_eq!(generator.material_at(seed, x, y0 - 4), Some(materials.stone), "天井");
        }
        for y in y0 - 4..y1 + 4 {
            assert_eq!(generator.material_at(seed, x0 - 1, y), Some(materials.stone), "左壁");
            let door = (DOOR[1]..DOOR[3]).contains(&y);
            let right = generator.material_at(seed, x1, y);
            if door {
                assert_eq!(right, None, "右壁の出入口は空: seed {seed}, y {y}");
            } else {
                assert_eq!(right, Some(materials.stone), "右壁: seed {seed}, y {y}");
            }
        }
        // 出入口から、出発点の帯 (x < 384) の終わりまで、通路の中心線の上下 9 セルずつが空いている。
        for x in DOOR[0]..384 {
            let center = tunnel_center(seed, x);
            for y in center - 9..=center + 9 {
                assert_eq!(generator.material_at(seed, x, y), None, "seed {seed}, ({x}, {y})");
            }
        }
        // プレイヤー (8×16) と剛体 3 つを置く床の上は空。
        for y in PLAYER_SPAWN[1]..PLAYER_SPAWN[1] + 16 {
            for x in PLAYER_SPAWN[0]..PLAYER_SPAWN[0] + 8 {
                assert_eq!(generator.material_at(seed, x, y), None);
            }
        }
        for y in 232..264 {
            for x in 64..170 {
                assert_eq!(generator.material_at(seed, x, y), None, "剛体の置き場所");
            }
        }
    }
    // 縦の外側はすべて石。
    for y in [-50, -1, WORLD_HEIGHT, WORLD_HEIGHT + 90] {
        assert_eq!(generator.material_at(1, 10, y), Some(materials.stone));
    }
}

#[test]
fn demo_moss_and_blobs() {
    let table = table();
    let generator = generator(&table);
    let m = generator.materials;
    let (mut yellow, mut green, mut red) = (0u32, 0u32, 0u32);
    let (mut rock, mut green_ore, mut orange_ore) = (0u32, 0u32, 0u32);
    let (mut open, mut total) = (0u32, 0u32);
    let mut hues = BTreeSet::new();
    for seed in 1..=4 {
        for y in (0..WORLD_HEIGHT).step_by(3) {
            for x in (-2000..2000).step_by(3) {
                total += 1;
                let Some(cell) = generator.material_at(seed, x, y) else {
                    open += 1;
                    continue;
                };
                // 苔は対応するバイオームの帯にだけある。
                for (moss, kind) in [
                    (m.moss_yellow, Biome::Yellow),
                    (m.moss_green, Biome::Green),
                    (m.moss_red, Biome::Red),
                ] {
                    if cell == moss {
                        assert_eq!(biome(seed, x), kind, "seed {seed}, ({x}, {y})");
                    }
                }
                match cell {
                    id if id == m.moss_yellow => yellow += 1,
                    id if id == m.moss_green => green += 1,
                    id if id == m.moss_red => red += 1,
                    id if id == m.green_ore => green_ore += 1,
                    id if id == m.orange_ore => orange_ore += 1,
                    id if id.0 >= m.rainbow_rock.0 && id.0 < m.rainbow_rock.0 + 12 => {
                        rock += 1;
                        // 色は x だけで決まる。
                        assert_eq!(id.0 - m.rainbow_rock.0, charon_world::demo::rainbow_index(x));
                        hues.insert(id.0);
                    }
                    _ => {}
                }
            }
        }
    }
    for (name, count) in [
        ("yellow", yellow),
        ("green", green),
        ("red", red),
        ("rainbow rock", rock),
        ("green ore", green_ore),
        ("orange ore", orange_ore),
    ] {
        assert!(count >= 50, "{name}: {count}");
    }
    assert!(hues.len() >= 6, "虹色の塊は横方向に色が変わる: {hues:?}");
    let ratio = f64::from(open) / f64::from(total);
    assert!((0.15..0.6).contains(&ratio), "空洞の割合 {ratio}");
}

#[test]
fn demo_text_is_drawn() {
    let table = table();
    let generator = generator(&table);
    let m = generator.materials;
    let (mut normal, mut rotated) = (0, 0);
    for seed in 1..=8 {
        for band in -30..30 {
            let Some((x0, y0, rotate)) = yellow_text_placement(seed, band) else { continue };
            assert_eq!(biome(seed, band * 160 + 80), Biome::Yellow);
            if rotate {
                rotated += 1;
            } else {
                normal += 1;
            }
            let cells = text_cells(YELLOW_TEXT, 1, rotate);
            assert!(!cells.is_empty());
            for (dx, dy) in cells {
                assert_eq!(
                    generator.material_at(seed, x0 + dx, y0 + dy),
                    Some(m.glow_orange),
                    "seed {seed}, band {band}"
                );
            }
        }
    }
    assert!(normal > 0 && rotated > 0, "通常 {normal}、回転 {rotated}");
    // `Tile Entity` は部屋にある。
    let sign = text_cells(SIGN_LABEL, 1, false);
    assert!(!sign.is_empty());
    for (dx, dy) in sign {
        assert_eq!(
            generator.material_at(1, SIGN_TEXT[0] + dx, SIGN_TEXT[1] + dy),
            Some(m.sign_blue)
        );
    }
}

#[test]
fn demo_background_label() {
    let table = table();
    let generator = generator(&table);
    let m = generator.materials;
    let expected = text_cells(GREEN_LABEL, 2, false).len();
    let mut labelled = 0;
    for seed in 1..=8 {
        for band in -12..12 {
            let green = biome(seed, band * 200 + 100) == Biome::Green;
            let mut count = 0;
            for y in 0..700 {
                for x in band * 200..band * 200 + 200 {
                    let cell = generator.background_cell(seed, x, y);
                    if cell != MaterialId::EMPTY {
                        assert_eq!(cell, m.bg_text_green);
                        count += 1;
                    }
                }
            }
            if green {
                assert!(count == 0 || count == expected, "seed {seed}, band {band}: {count}");
                if count == expected {
                    labelled += 1;
                }
            } else {
                assert_eq!(count, 0, "緑バイオーム以外の背景は空: seed {seed}, band {band}");
            }
        }
        // 前景に背景用の材質は出ない。
        for y in (0..WORLD_HEIGHT).step_by(7) {
            for x in (-1500..1500).step_by(7) {
                assert_ne!(generator.material_at(seed, x, y), Some(m.bg_text_green));
            }
        }
    }
    assert!(labelled > 0, "どこかの緑バイオームに `green biome` がある");
}

#[test]
fn demo_background_is_populated_once() {
    use charon_world::background::BackgroundWorld;
    let table = table();
    let generator = generator(&table);
    let m = generator.materials;
    // ラベルのある緑バイオームの帯を探し、その帯の 64×64 チャンクを順に処理する。
    let (seed, band) = (1..=8u64)
        .flat_map(|seed| (-12..12).map(move |band| (seed, band)))
        .find(|&(seed, band)| {
            biome(seed, band * 200 + 100) == Biome::Green
                && (0..700).any(|y| {
                    (band * 200..band * 200 + 200)
                        .any(|x| generator.background_cell(seed, x, y) != MaterialId::EMPTY)
                })
        })
        .unwrap();
    let chunks: Vec<_> = (band * 200 / 64 - 1..=(band * 200 + 200) / 64)
        .flat_map(|cx| (0..11).map(move |cy| ChunkPos::new(cx, cy)))
        .collect();
    let mut background = BackgroundWorld::new();
    let mut populated = BTreeSet::new();
    let written = generator.populate_background(seed, &chunks, &mut background, &mut populated);
    assert_eq!(written, text_cells(GREEN_LABEL, 2, false).len(), "ラベル 1 つ分");
    assert_eq!(populated.len(), chunks.len());
    let count = |background: &BackgroundWorld| {
        background
            .iter()
            .map(|(_, chunk)| chunk.cells.iter().filter(|&&c| c == m.bg_text_green).count())
            .sum::<usize>()
    };
    assert_eq!(count(&background), written);
    // 処理済みのチャンクは何度呼んでも書かない。既存の背景 (建築) も上書きしない。
    assert_eq!(generator.populate_background(seed, &chunks, &mut background, &mut populated), 0);
    let mut built = BackgroundWorld::new();
    built.set(charon_core::coords::CellPos::new(band * 200 + 3, 100), m.sign_blue);
    let mut fresh = BTreeSet::new();
    let again = generator.populate_background(seed, &chunks, &mut built, &mut fresh);
    assert!(again <= written);
    assert_eq!(built.get(charon_core::coords::CellPos::new(band * 200 + 3, 100)), m.sign_blue);
}
