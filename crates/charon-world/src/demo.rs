//! 仕様 09: 参照デモ場面の生成。虹色の選び方、字形、洞窟の生成器。

use std::collections::BTreeSet;

use charon_core::cell::MaterialId;
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_AREA, CHUNK_SIZE, CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::rng::hash;

use crate::background::BackgroundWorld;
use crate::generator::{ChunkGenerator, generate_cells};

/// 虹色の材質の数 (色相 30° 刻み)。
pub const RAINBOW_COUNT: i32 = 12;

/// 仕様 09 §2: セルの x 座標から虹色の添字 (0..=11) を求める。4 セルごとに色が変わる。
pub fn rainbow_index(x: i32) -> u16 {
    // rem_euclid は 0..=11 を返すので u16 に収まる。
    x.div_euclid(4).rem_euclid(RAINBOW_COUNT) as u16
}

/// 虹色の材質群の先頭 ID `first` から、`x` の位置の色の ID を返す。ID は連続している。
pub fn rainbow_material(first: MaterialId, x: i32) -> MaterialId {
    MaterialId(first.0.saturating_add(rainbow_index(x)))
}

/// 洞窟のある縦の範囲 `0..WORLD_HEIGHT`。外側はすべて石。
pub const WORLD_HEIGHT: i32 = 640;
/// 部屋の外周を含む矩形 (`x0, y0, x1, y1`、終端は含まない)。
pub const ROOM: [i32; 4] = [4, 196, 188, 268];
/// 部屋の空洞 (`x0, y0, x1, y1`、終端は含まない)。床は `y1` の行から。
pub const ROOM_INTERIOR: [i32; 4] = [8, 200, 184, 264];
/// プレイヤーの初期位置 (左上のセル)。8×16 で、床の上に立つ。
pub const PLAYER_SPAWN: [i32; 2] = [40, 248];
/// `Tile Entity` の文字の左上と、それを囲む枠 (`x0, y0, x1, y1`、終端は含まない)。
pub const SIGN_TEXT: [i32; 2] = [100, 215];
pub const SIGN_FRAME: [i32; 4] = [98, 213, 168, 224];
/// 部屋の右の壁に開けた出入口 (`x0, y0, x1, y1`、終端は含まない)。通路 (`tunnel_center`) につながる。
pub const DOOR: [i32; 4] = [184, 245, 188, 264];
/// 通路の中心線が部屋の高さに揃う x と、そこから離れて自由に曲がるまでの距離。
const TUNNEL_ANCHOR_X: i32 = 188;
const TUNNEL_HALF_HEIGHT: i32 = 9;

const BIOME_WIDTH: i32 = 384;
const TEXT_BAND: i32 = 160;
const BG_BAND: i32 = 200;

const S_BIOME: u32 = 0x4445_4d30;
const S_CAVE_A: u32 = 0x4445_4d31;
const S_CAVE_B: u32 = 0x4445_4d32;
const S_BLOB: u32 = 0x4445_4d33;
const S_BLOB2: u32 = 0x4445_4d34;
const S_GREEN: u32 = 0x4445_4d35;
const S_GREEN2: u32 = 0x4445_4d36;
const S_ORANGE: u32 = 0x4445_4d37;
const S_ORANGE2: u32 = 0x4445_4d38;
const S_TEXT: u32 = 0x4445_4d39;
const S_TEXT_Y: u32 = 0x4445_4d3a;
const S_BG: u32 = 0x4445_4d3b;
const S_BG_Y: u32 = 0x4445_4d3c;
const S_TUNNEL: u32 = 0x4445_4d3d;

pub const YELLOW_TEXT: &str = "yellow thing";
pub const GREEN_LABEL: &str = "green biome";
pub const SIGN_LABEL: &str = "Tile Entity";

/// 生成に使う材質。`DemoMaterials::from_table` が名前から解決する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoMaterials {
    pub stone: MaterialId,
    /// `rainbow_sand_0` の ID。12 個が連続する。
    pub rainbow_sand: MaterialId,
    /// `rainbow_rock_0` の ID。12 個が連続する。
    pub rainbow_rock: MaterialId,
    pub moss_yellow: MaterialId,
    pub moss_green: MaterialId,
    pub moss_red: MaterialId,
    pub glow_orange: MaterialId,
    pub sign_blue: MaterialId,
    pub green_ore: MaterialId,
    pub orange_ore: MaterialId,
    pub bg_text_green: MaterialId,
}

impl DemoMaterials {
    /// `fse_demo` MOD を含む表から名前で解決する。足りなければ `None`。
    pub fn from_table(materials: &MaterialTable) -> Option<Self> {
        let demo = |name: &str| materials.id(&format!("fse_demo:{name}"));
        Some(Self {
            stone: materials.id("stone")?,
            rainbow_sand: demo("rainbow_sand_0")?,
            rainbow_rock: demo("rainbow_rock_0")?,
            moss_yellow: demo("glow_moss_yellow")?,
            moss_green: demo("glow_moss_green")?,
            moss_red: demo("glow_moss_red")?,
            glow_orange: demo("glow_orange")?,
            sign_blue: demo("sign_blue")?,
            green_ore: demo("green_ore")?,
            orange_ore: demo("orange_ore")?,
            bg_text_green: demo("bg_text_green")?,
        })
    }
}

/// 帯ごとのバイオーム。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    Plain,
    Yellow,
    Green,
    Red,
}

/// `x` を含む 384 セル幅の帯のバイオーム。`band == 0` は平坦。
pub fn biome(seed: u64, x: i32) -> Biome {
    let band = x.div_euclid(BIOME_WIDTH);
    if band == 0 {
        return Biome::Plain;
    }
    match hash(seed, 0, band, 0, S_BIOME) % 4 {
        0 => Biome::Yellow,
        1 => Biome::Green,
        2 => Biome::Red,
        _ => Biome::Plain,
    }
}

// ---- 字形 ---------------------------------------------------------------

/// 5×7 の字形。各行の下位 5 ビットのうち、最上位 (ビット 4) が左端。
fn glyph(ch: char) -> [u8; 7] {
    match ch {
        'y' => [0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b10001, 0b01110],
        'e' => [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110],
        'l' => [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'o' => [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        'w' => [0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010],
        't' => [0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00101, 0b00010],
        'h' => [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
        'i' => [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
        'n' => [0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
        'g' => [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
        'r' => [0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000],
        'b' => [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110],
        'm' => [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        _ => [0; 7],
    }
}

/// 文字列の幅と高さ (セル)。字の送り幅は 6、字の幅は 5。
pub fn text_size(text: &str, scale: i32) -> (i32, i32) {
    let count = text.chars().count() as i32;
    ((count * 6 - 1).max(0) * scale, 7 * scale)
}

/// 文字列の左上からの相対座標 `(dx, dy)` のセルが、文字のドットか。`rotate` は 180° 回転。
pub fn text_pixel(text: &str, scale: i32, rotate: bool, dx: i32, dy: i32) -> bool {
    let (width, height) = text_size(text, scale);
    if dx < 0 || dy < 0 || dx >= width || dy >= height {
        return false;
    }
    let (dx, dy) = if rotate { (width - 1 - dx, height - 1 - dy) } else { (dx, dy) };
    let (gx, gy) = (dx / scale, dy / scale);
    if gx % 6 == 5 {
        return false;
    }
    let Some(ch) = text.chars().nth((gx / 6) as usize) else { return false };
    (glyph(ch)[gy as usize] >> (4 - gx % 6)) & 1 == 1
}

/// 文字のドットになるセルの相対座標を、行優先で返す。
pub fn text_cells(text: &str, scale: i32, rotate: bool) -> Vec<(i32, i32)> {
    let (width, height) = text_size(text, scale);
    let mut cells = Vec::new();
    for dy in 0..height {
        for dx in 0..width {
            if text_pixel(text, scale, rotate, dx, dy) {
                cells.push((dx, dy));
            }
        }
    }
    cells
}

// ---- 地形 ---------------------------------------------------------------

/// 0..=255 の値ノイズ。`spacing` セルごとの格子の値を線形補間する。
fn octave(seed: u64, x: i32, y: i32, spacing: i32, salt: u32) -> i32 {
    let (gx, gy) = (x.div_euclid(spacing), y.div_euclid(spacing));
    let (fx, fy) = (x.rem_euclid(spacing), y.rem_euclid(spacing));
    let sample = |dx, dy| (hash(seed, 0, gx + dx, gy + dy, salt) & 255) as i32;
    let top = sample(0, 0) * (spacing - fx) + sample(1, 0) * fx;
    let bottom = sample(0, 1) * (spacing - fx) + sample(1, 1) * fx;
    (top * (spacing - fy) + bottom * fy) / (spacing * spacing)
}

fn cave_value(seed: u64, x: i32, y: i32) -> i32 {
    (2 * octave(seed, x, y, 48, S_CAVE_A) + octave(seed, x, y, 24, S_CAVE_B)) / 3
}

/// 横に長く続く通路の中心の y。出入口の付近では部屋の床の高さに揃い、離れるほど自由に曲がる。
pub fn tunnel_center(seed: u64, x: i32) -> i32 {
    let free = ((x - TUNNEL_ANCHOR_X).abs() - 20).clamp(0, 240);
    let wander = octave(seed, x, 0, 240, S_TUNNEL) - 128;
    254 + wander * free / 240
}

fn in_rect(rect: [i32; 4], x: i32, y: i32) -> bool {
    x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3]
}

/// 部屋の外周を含む範囲か。
fn in_room(x: i32, y: i32) -> bool {
    in_rect(ROOM, x, y)
}

/// そのセルが空洞か (部屋を含む)。
pub fn open_at(seed: u64, x: i32, y: i32) -> bool {
    if in_room(x, y) {
        return in_rect(ROOM_INTERIOR, x, y) || in_rect(DOOR, x, y);
    }
    (0..WORLD_HEIGHT).contains(&y)
        && (cave_value(seed, x, y) < 112
            || (y - tunnel_center(seed, x)).abs() <= TUNNEL_HALF_HEIGHT)
}

/// `Tile Entity` の枠か文字のセルか。
fn sign_cell(x: i32, y: i32) -> bool {
    let on_frame = in_rect(SIGN_FRAME, x, y)
        && (x == SIGN_FRAME[0]
            || x == SIGN_FRAME[2] - 1
            || y == SIGN_FRAME[1]
            || y == SIGN_FRAME[3] - 1);
    on_frame || text_pixel(SIGN_LABEL, 1, false, x - SIGN_TEXT[0], y - SIGN_TEXT[1])
}

/// 幅 160 の帯 `band` の `yellow thing` の位置 `(x0, y0, 回転)`。置かないなら `None`。
pub fn yellow_text_placement(seed: u64, band: i32) -> Option<(i32, i32, bool)> {
    if biome(seed, band * TEXT_BAND + TEXT_BAND / 2) != Biome::Yellow
        || hash(seed, 0, band, 0, S_TEXT) & 1 != 0
    {
        return None;
    }
    let x0 = band * TEXT_BAND + 40 + (hash(seed, 0, band, 1, S_TEXT) % 40) as i32;
    let (width, _) = text_size(YELLOW_TEXT, 1);
    for k in 0..24 {
        let y0 = 16 + (hash(seed, 0, band, k + 2, S_TEXT_Y) % 600) as i32;
        if open_at(seed, x0, y0 + 3) && open_at(seed, x0 + width - 1, y0 + 3) {
            return Some((x0, y0, hash(seed, 0, band, 99, S_TEXT) & 2 != 0));
        }
    }
    None
}

/// `(x, y)` が `yellow thing` の文字のドットか。
fn yellow_text_at(seed: u64, x: i32, y: i32) -> bool {
    let band = x.div_euclid(TEXT_BAND);
    yellow_text_placement(seed, band)
        .is_some_and(|(x0, y0, rotate)| text_pixel(YELLOW_TEXT, 1, rotate, x - x0, y - y0))
}

/// 参照デモの地形生成器。仕様 09 §3。
#[derive(Clone, Copy, Debug)]
pub struct DemoGenerator {
    pub materials: DemoMaterials,
}

impl DemoGenerator {
    /// 1 セルの材質 (`None` は空)。チャンク生成と同じ規則を、キャッシュなしで評価する。
    pub fn material_at(&self, seed: u64, x: i32, y: i32) -> Option<MaterialId> {
        self.classify(seed, x, y, &|ox, oy| open_at(seed, ox, oy), yellow_text_at(seed, x, y))
    }

    /// 背景レイヤーのセル。緑バイオームの帯に `green biome` だけがある。
    pub fn background_cell(&self, seed: u64, x: i32, y: i32) -> MaterialId {
        let band = x.div_euclid(BG_BAND);
        if biome(seed, band * BG_BAND + BG_BAND / 2) != Biome::Green
            || hash(seed, 0, band, 0, S_BG) & 1 != 0
        {
            return MaterialId::EMPTY;
        }
        let y0 = 40 + (hash(seed, 0, band, 1, S_BG_Y) % 500) as i32;
        let x0 = band * BG_BAND + 20;
        if text_pixel(GREEN_LABEL, 2, false, x - x0, y - y0) {
            self.materials.bg_text_green
        } else {
            MaterialId::EMPTY
        }
    }

    /// 読み込まれたチャンクのうち、まだ処理していないものの背景 (`green biome` の文字) を作る。
    /// すでに背景のチャンクがある位置 (保存済みや利用者が建築した背景) は書き換えない。
    /// `populated` に処理したチャンクを記録し、書いたセルの数を返す。
    pub fn populate_background(
        &self,
        seed: u64,
        chunks: &[ChunkPos],
        background: &mut BackgroundWorld,
        populated: &mut BTreeSet<ChunkPos>,
    ) -> usize {
        let mut written = 0;
        for &pos in chunks {
            if !populated.insert(pos) || background.chunk(pos).is_some() {
                continue;
            }
            let origin = pos.origin();
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let (x, y) = (origin.x + lx, origin.y + ly);
                    let material = self.background_cell(seed, x, y);
                    if material != MaterialId::EMPTY {
                        background.set(CellPos::new(x, y), material);
                        written += 1;
                    }
                }
            }
        }
        written
    }

    fn classify(
        &self,
        seed: u64,
        x: i32,
        y: i32,
        open: &dyn Fn(i32, i32) -> bool,
        glow: bool,
    ) -> Option<MaterialId> {
        let m = &self.materials;
        if !(0..WORLD_HEIGHT).contains(&y) {
            return Some(m.stone);
        }
        if in_room(x, y) {
            if in_rect(DOOR, x, y) {
                return None;
            }
            if !in_rect(ROOM_INTERIOR, x, y) {
                return Some(m.stone);
            }
            return sign_cell(x, y).then_some(m.sign_blue);
        }
        if glow {
            return Some(m.glow_orange);
        }
        if open(x, y) {
            return self.open_cell(seed, x, y);
        }
        let moss = match biome(seed, x) {
            Biome::Plain => None,
            Biome::Yellow => Some(m.moss_yellow),
            Biome::Green => Some(m.moss_green),
            Biome::Red => Some(m.moss_red),
        };
        let near_open = [(-1, 0), (1, 0), (0, -1), (0, 1), (-2, 0), (2, 0), (0, -2), (0, 2)]
            .iter()
            .any(|&(dx, dy)| open(x + dx, y + dy));
        Some(if near_open { moss.unwrap_or(m.stone) } else { m.stone })
    }

    /// 空洞に浮かぶ虹色の岩と鉱床。ない場所は空。
    fn open_cell(&self, seed: u64, x: i32, y: i32) -> Option<MaterialId> {
        let m = &self.materials;
        if x.div_euclid(BIOME_WIDTH) == 0 {
            return None; // 出発点の帯は塊を置かず、通路を空けておく。
        }
        let (bx, by) = (x.div_euclid(48), y.div_euclid(48));
        if hash(seed, 0, bx, by, S_BLOB) & 3 == 0 && octave(seed, x, y, 12, S_BLOB2) < 64 {
            return Some(rainbow_material(m.rainbow_rock, x));
        }
        if hash(seed, 0, bx, by, S_GREEN) & 7 == 0 && octave(seed, x, y, 10, S_GREEN2) < 44 {
            return Some(m.green_ore);
        }
        if hash(seed, 0, bx, by, S_ORANGE) & 7 == 0 && octave(seed, x, y, 10, S_ORANGE2) < 44 {
            return Some(m.orange_ore);
        }
        None
    }
}

impl ChunkGenerator for DemoGenerator {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk {
        const MARGIN: i32 = 2;
        const SIDE: usize = (CHUNK_SIZE + 2 * MARGIN) as usize;
        let origin = pos.origin();
        // 壁の苔の判定に使う隣接 2 セルまでの空洞を 1 度だけ求める。
        let mut open_map = vec![false; SIDE * SIDE];
        for my in 0..SIDE {
            for mx in 0..SIDE {
                open_map[my * SIDE + mx] =
                    open_at(seed, origin.x + mx as i32 - MARGIN, origin.y + my as i32 - MARGIN);
            }
        }
        let open = |x: i32, y: i32| {
            let (mx, my) = (x - origin.x + MARGIN, y - origin.y + MARGIN);
            if (0..SIDE as i32).contains(&mx) && (0..SIDE as i32).contains(&my) {
                open_map[my as usize * SIDE + mx as usize]
            } else {
                open_at(seed, x, y)
            }
        };
        // この範囲にかかる `yellow thing` を帯ごとに 1 度だけ配置する。
        let mut glow = vec![false; CHUNK_AREA];
        let first = origin.x.div_euclid(TEXT_BAND);
        let last = (origin.x + CHUNK_SIZE - 1).div_euclid(TEXT_BAND);
        for band in first..=last {
            let Some((x0, y0, rotate)) = yellow_text_placement(seed, band) else { continue };
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    if text_pixel(YELLOW_TEXT, 1, rotate, origin.x + lx - x0, origin.y + ly - y0) {
                        glow[(ly * CHUNK_SIZE + lx) as usize] = true;
                    }
                }
            }
        }
        generate_cells(seed, pos, materials, |x, y| {
            let index = ((y - origin.y) * CHUNK_SIZE + (x - origin.x)) as usize;
            self.classify(seed, x, y, &open, glow[index])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rainbow_index_is_periodic() {
        for x in -200..200 {
            let index = rainbow_index(x);
            assert!(index < 12, "x={x}");
            assert_eq!(index, rainbow_index(x + 48), "x={x}");
            assert_eq!(index, rainbow_index(x - 48), "x={x}");
        }
        // 4 セルごとに同じ色、境界で次の色。
        assert_eq!(rainbow_index(0), rainbow_index(3));
        assert_eq!(rainbow_index(4), rainbow_index(0) + 1);
        assert_eq!(rainbow_index(-1), 11);
        assert_eq!(rainbow_material(MaterialId(20), 8), MaterialId(22));
    }

    #[test]
    fn text_pixels_match_glyphs() {
        // 'T' は 1 行目が 5 つ並び、残りは中央の 1 列。
        let cells = text_cells("T", 1, false);
        assert_eq!(cells.len(), 5 + 6);
        assert!(cells.contains(&(0, 0)) && cells.contains(&(4, 0)) && cells.contains(&(2, 6)));
        // 180° 回転は点対称。拡大率 2 は 2×2 の塊。
        let rotated = text_cells("T", 1, true);
        assert_eq!(rotated.len(), cells.len());
        for &(x, y) in &cells {
            assert!(rotated.contains(&(4 - x, 6 - y)));
        }
        assert_eq!(text_cells("T", 2, false).len(), cells.len() * 4);
        assert_eq!(text_size("yellow thing", 1), (71, 7));
        assert_eq!(text_size("green biome", 2), (130, 14));
        assert!(!text_pixel("T", 1, false, 5, 0), "字間は空");
    }
}
