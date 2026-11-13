//! 仕様 21: 冒険の世界の地形生成。表面 (丘・池・バイオーム)・層・洞窟 (大広間・坑道・立て坑)・鉱石・地下の水と溶岩・
//! ブロックでできた構造物 (ダンジョン・家)・底。整数だけで、他のチャンクを参照せず、決定的。
//!
//! 座標は、y が大きいほど下。表面の高さは `SURFACE_BASE` のまわり。底 (`BOTTOM`) より下は壊せない境界。

use std::sync::Arc;

use charon_core::cell::{FLAG_BLOCK, MaterialId};
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_SIZE, CellPos, ChunkPos};
use charon_core::material::{MaterialTable, Phase};
use charon_core::rng::hash;
use charon_core::world::World;

use crate::generator::ChunkGenerator;

/// 表面の高さの基準。
pub const SURFACE_BASE: i32 = 200;
/// 底。これ以上の y は、壊せない境界。
pub const BOTTOM: i32 = SURFACE_BASE + 1200;
/// 出発点 (x = 0) のまわりの、平らな表面の半幅。
const SPAWN_FLAT: i32 = 24;
/// バイオームの幅。
pub const BIOME_WIDTH: i32 = 512;
/// 洞窟が始まる深さ (表面から)。これより浅いところは、立て坑以外は埋まっている。
const CAVE_START: i32 = 26;
/// ダンジョンの区画の大きさ (セル)。ブロックの 8 の倍数。
const DUNGEON_W: i32 = 256;
const DUNGEON_H: i32 = 192;
/// 家の区画の幅。
const HOUSE_W: i32 = 192;

const S_BIOME: u32 = 0x4244_0001;
const S_HILL: u32 = 0x4244_0002;
const S_HILL2: u32 = 0x4244_0003;
const S_WARP_X: u32 = 0x4244_0004;
const S_WARP_Y: u32 = 0x4244_0005;
const S_HALL_A: u32 = 0x4244_0006;
const S_HALL_B: u32 = 0x4244_0007;
const S_WORM_A: u32 = 0x4244_0008;
const S_WORM_B: u32 = 0x4244_0009;
const S_ROUGH: u32 = 0x4244_000A;
const S_SHAFT: u32 = 0x4244_000B;
const S_SHAFT_X: u32 = 0x4244_000C;
const S_POND: u32 = 0x4244_000D;
const S_POOL: u32 = 0x4244_000E;
const S_POOL_Y: u32 = 0x4244_000F;
const S_POOL_KIND: u32 = 0x4244_0010;
const S_COAL: u32 = 0x4244_0011;
const S_COAL2: u32 = 0x4244_0012;
const S_IRON: u32 = 0x4244_0013;
const S_IRON2: u32 = 0x4244_0014;
const S_GOLD: u32 = 0x4244_0015;
const S_GOLD2: u32 = 0x4244_0016;
const S_DUNGEON: u32 = 0x4244_0017;
const S_DUNGEON_KIND: u32 = 0x4244_0018;
const S_DUNGEON_X: u32 = 0x4244_0019;
const S_DUNGEON_Y: u32 = 0x4244_001A;
const S_HOUSE: u32 = 0x4244_001B;
const S_HOUSE_X: u32 = 0x4244_001C;
const S_DIRT: u32 = 0x4244_001D;

/// バイオーム (x の 512 セルごと)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    /// 草と土。
    Forest,
    /// 砂。池はない。
    Desert,
    /// 氷。
    Tundra,
}

/// `x` のバイオーム。出発点の帯 (x = 0 を含む区画) は、いつも森。
pub fn biome(seed: u64, x: i32) -> Biome {
    let region = x.div_euclid(BIOME_WIDTH);
    if region == 0 {
        return Biome::Forest;
    }
    match hash(seed, 0, region, 0, S_BIOME) % 5 {
        0 | 1 => Biome::Forest,
        2 | 3 => Biome::Desert,
        _ => Biome::Tundra,
    }
}

/// 0..=255 の値ノイズ。`spacing` セルごとの格子の値を線形補間する。
fn octave(seed: u64, x: i32, y: i32, spacing: i32, salt: u32) -> i32 {
    let (gx, gy) = (x.div_euclid(spacing), y.div_euclid(spacing));
    let (fx, fy) = (x.rem_euclid(spacing), y.rem_euclid(spacing));
    let sample = |dx, dy| (hash(seed, 0, gx + dx, gy + dy, salt) & 255) as i32;
    let top = sample(0, 0) * (spacing - fx) + sample(1, 0) * fx;
    let bottom = sample(0, 1) * (spacing - fx) + sample(1, 1) * fx;
    (top * (spacing - fy) + bottom * fy) / (spacing * spacing)
}

/// 家 (表面のブロックの建物)。区画 (`HOUSE_W` 列) に 1 軒まで。
fn house_at(seed: u64, region: i32) -> Option<House> {
    // 出発点の区画と、森でない区画には建てない。
    if region == 0 || region == -1 || biome(seed, region * HOUSE_W) != Biome::Forest {
        return None;
    }
    if !hash(seed, 0, region, 0, S_HOUSE).is_multiple_of(3) {
        return None;
    }
    let template = &HOUSE;
    let w = template[0].len() as i32;
    let bx = (region * HOUSE_W) / 8 + 2 + (hash(seed, 0, region, 0, S_HOUSE_X) % 12) as i32;
    // 床の高さは、家の中心の列の元の表面を、ブロックの格子に切り上げた高さ。
    let center = (bx + w / 2) * 8;
    let floor_y = (raw_surface(seed, center) + 7).div_euclid(8) * 8;
    Some(House { bx, w, floor_y })
}

struct House {
    bx: i32,
    w: i32,
    floor_y: i32,
}

/// 家を考えない表面の高さ。丘 (2 つのノイズ) で、出発点のまわりは平ら。
fn raw_surface(seed: u64, x: i32) -> i32 {
    let hills =
        (octave(seed, x, 0, 160, S_HILL) - 128) / 5 + (octave(seed, x, 0, 40, S_HILL2) - 128) / 16;
    let free = (x.abs() - SPAWN_FLAT).clamp(0, 64);
    SURFACE_BASE + hills * free / 64
}

/// 家をならした表面の高さ (最初の固体のセルの y)。
pub fn surface_y(seed: u64, x: i32) -> i32 {
    let region = x.div_euclid(HOUSE_W);
    if let Some(house) = house_at(seed, region) {
        let (x0, x1) = (house.bx * 8 - 8, (house.bx + house.w) * 8 + 8);
        if x >= x0 && x < x1 {
            return house.floor_y;
        }
    }
    raw_surface(seed, x)
}

/// 池: 区画 (96 列) の 1/4 に、幅 44 の水たまり。`(元の表面, 深さ)` を返す。
fn pond_depth(seed: u64, x: i32) -> i32 {
    const W: i32 = 22;
    let region = x.div_euclid(96);
    if region == 0
        || !hash(seed, 0, region, 0, S_POND).is_multiple_of(4)
        || biome(seed, x) == Biome::Desert
    {
        return 0;
    }
    let center = region * 96 + 48;
    let d = (x - center).abs();
    if d >= W { 0 } else { (W - d) * (W - d) * 12 / (W * W) }
}

/// 立て坑: 表面から洞窟へ降りる縦の穴。区画 (160 列) の 1/3。
fn shaft_open(seed: u64, x: i32, depth: i32) -> bool {
    if depth > 100 {
        return false;
    }
    let region = x.div_euclid(160);
    if region == 0 || (region != 1 && !hash(seed, 0, region, 0, S_SHAFT).is_multiple_of(3)) {
        return false;
    }
    let center = region * 160 + 30 + (hash(seed, 0, region, 0, S_SHAFT_X) % 100) as i32;
    let half = 4 + (hash(seed, 1, region, 0, S_SHAFT_X) % 4) as i32;
    // 少し曲げる。
    let sway = (octave(seed, 0, depth, 24, S_SHAFT_X) - 128) / 16;
    (x - (center + sway)).abs() <= half
}

/// 洞窟の空洞か (表面の下)。大広間・坑道・立て坑。深いほど広い。
fn cave_open(seed: u64, x: i32, y: i32, depth: i32) -> bool {
    if depth < CAVE_START {
        return false;
    }
    let wx = x + (octave(seed, x, y, 72, S_WARP_X) - 128) / 3;
    let wy = y + (octave(seed, x, y, 72, S_WARP_Y) - 128) / 3;
    let hall =
        (3 * octave(seed, wx, wy, 64, S_HALL_A) + 2 * octave(seed, wx, wy, 32, S_HALL_B)) / 5;
    let worm_a = (octave(seed, wx, wy, 90, S_WORM_A) - 128).abs();
    let worm_b = (octave(seed, wx, wy, 56, S_WORM_B) - 128).abs();
    let rough = octave(seed, x, y, 7, S_ROUGH) - 128;
    // 浅いところは狭く、深いほど広い。
    let grow = (depth / 12).min(60);
    let shallow = (depth - CAVE_START).clamp(0, 40);
    hall < 70 + grow * shallow / 40 + rough / 12
        || worm_a < 9 + depth / 90 + rough / 24
        || worm_b < 7 + depth / 120 + rough / 24
}

/// その材質を `FLAG_BLOCK` のブロックにしてよいか (固体)。
fn blockable(materials: &MaterialTable, id: MaterialId) -> bool {
    materials.get(id).phase == Phase::Solid
}

/// ダンジョンなどの形。文字は、`#` = 石のブロック、`W` = 木のブロック、`.` = 空洞 (地形を消す)。
type Template = [&'static str];

const HALL: &Template = &[
    "########################",
    "#......................#",
    "#......................#",
    "#..WWWW........WWWW....#",
    "#......................#",
    ".......................#",
    "#.....WWWWWWWWWW.......#",
    "#......................#",
    "#......................#",
    "########################",
];
const TOWER: &Template = &[
    "##########",
    "#........#",
    "#........#",
    "#.WWWWWW.#",
    "#........#",
    "#........#",
    "#WWWWWW..#",
    "#........#",
    "#........#",
    "#..WWWWWW#",
    "#........#",
    "#........#",
    "#.WWWWWW.#",
    "........ #",
    ".........#",
    "##########",
];
const CRYPT: &Template = &[
    "####################",
    "#..................#",
    "#.####..####..####.#",
    "#.#..........#...#.#",
    "#.#..WWWWWW..#...#.#",
    "...................#",
    "#.#..........#...#.#",
    "#.####..####..####.#",
    "#..................#",
    "####################",
];
const HOUSE: &Template = &[
    "..WWWWWWWW..",
    ".WWWWWWWWWW.",
    "WWW......WWW",
    "W..........W",
    "W..........W",
    "W..........W",
    "WWWWW..WWWWW",
];

fn template_cell(template: &Template, tx: i32, ty: i32) -> Option<char> {
    let row = usize::try_from(ty).ok().and_then(|y| template.get(y))?;
    row.chars().nth(usize::try_from(tx).ok()?)
}

/// 材質の ID (名前から引いたもの)。
#[derive(Clone, Copy, Debug)]
pub struct AdventureMaterials {
    pub stone: MaterialId,
    pub dirt: MaterialId,
    pub grass: MaterialId,
    pub sand: MaterialId,
    pub ice: MaterialId,
    pub water: MaterialId,
    pub lava: MaterialId,
    pub wood: MaterialId,
    pub coal: MaterialId,
    pub iron: MaterialId,
    pub gold: MaterialId,
}

impl AdventureMaterials {
    pub fn from_table(table: &MaterialTable) -> Option<Self> {
        Some(Self {
            stone: table.id("stone")?,
            dirt: table.id("dirt")?,
            grass: table.id("grass")?,
            sand: table.id("sand")?,
            ice: table.id("ice")?,
            water: table.id("water")?,
            lava: table.id("lava")?,
            wood: table.id("wood")?,
            coal: table.id("coal")?,
            iron: table.id("iron_ore")?,
            gold: table.id("gold_ore")?,
        })
    }
}

/// 冒険の世界の生成器 (仕様 21)。
#[derive(Clone, Copy, Debug)]
pub struct AdventureGenerator {
    pub materials: AdventureMaterials,
}

/// 1 つのセルの内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generated {
    pub material: MaterialId,
    /// ブロック (`FLAG_BLOCK`) か。
    pub block: bool,
}

/// 列ごとに 1 度だけ求める値。
#[derive(Clone, Copy, Debug)]
struct Column {
    surface: i32,
    pond: i32,
    biome: Biome,
}

impl AdventureGenerator {
    fn column(seed: u64, x: i32) -> Column {
        Column { surface: surface_y(seed, x), pond: pond_depth(seed, x), biome: biome(seed, x) }
    }

    /// ダンジョンの形のセル。`Some(None)` は空洞、`Some(Some(..))` はブロック、`None` は構造物ではない。
    fn dungeon_cell(&self, seed: u64, x: i32, y: i32) -> Option<Option<Generated>> {
        let (rx, ry) = (x.div_euclid(DUNGEON_W), y.div_euclid(DUNGEON_H));
        // 表面の近くには置かない。
        if ry * DUNGEON_H < SURFACE_BASE + 120 || (ry + 1) * DUNGEON_H > BOTTOM - 60 {
            return None;
        }
        if !hash(seed, 0, rx, ry, S_DUNGEON).is_multiple_of(3) {
            return None;
        }
        let template: &Template = match hash(seed, 0, rx, ry, S_DUNGEON_KIND) % 3 {
            0 => HALL,
            1 => TOWER,
            _ => CRYPT,
        };
        let (w, h) = (template[0].len() as i32, template.len() as i32);
        let (region_bw, region_bh) = (DUNGEON_W / 8, DUNGEON_H / 8);
        let bx0 = rx * region_bw
            + 2
            + (hash(seed, 0, rx, ry, S_DUNGEON_X) % (region_bw - w - 3) as u32) as i32;
        let by0 = ry * region_bh
            + 2
            + (hash(seed, 0, rx, ry, S_DUNGEON_Y) % (region_bh - h - 3) as u32) as i32;
        let (bx, by) = (x >> 3, y >> 3);
        let ch = template_cell(template, bx - bx0, by - by0)?;
        Some(match ch {
            '#' => Some(Generated { material: self.materials.stone, block: true }),
            'W' => Some(Generated { material: self.materials.wood, block: true }),
            _ => None,
        })
    }

    /// 家のセル。
    fn house_cell(&self, seed: u64, x: i32, y: i32) -> Option<Option<Generated>> {
        let region = x.div_euclid(HOUSE_W);
        let house = house_at(seed, region)?;
        let (bx, by) = (x >> 3, y >> 3);
        let h = HOUSE.len() as i32;
        // 家の下の行が、床 (floor_y) の真上のブロックの行。
        let by0 = house.floor_y / 8 - h;
        let ch = template_cell(HOUSE, bx - house.bx, by - by0)?;
        Some(match ch {
            'W' => Some(Generated { material: self.materials.wood, block: true }),
            _ => None,
        })
    }

    fn solid(&self, material: MaterialId) -> Option<Generated> {
        Some(Generated { material, block: false })
    }

    /// セル `(x, y)` の内容。空なら `None`。
    pub fn cell_at(&self, seed: u64, x: i32, y: i32) -> Option<Generated> {
        self.classify(seed, x, y, &Self::column(seed, x))
    }

    fn classify(&self, seed: u64, x: i32, y: i32, col: &Column) -> Option<Generated> {
        let m = &self.materials;
        if y >= BOTTOM {
            return Some(Generated { material: MaterialId::BOUNDARY, block: false });
        }
        if let Some(cell) = self.house_cell(seed, x, y) {
            return cell;
        }
        if let Some(cell) = self.dungeon_cell(seed, x, y) {
            return cell;
        }
        let depth = y - col.surface;
        if depth < 0 {
            // 空。
            return None;
        }
        // 池: 表面をへこませて、水をためる。
        if col.pond > 0 && depth < col.pond {
            return self.solid(m.water);
        }
        let depth = depth - col.pond;
        // 立て坑。
        if shaft_open(seed, x, y - col.surface) {
            return None;
        }
        // 表面の層。
        let dirt_depth = 22 + (hash(seed, 0, x.div_euclid(6), 0, S_DIRT) % 8) as i32;
        let layer = match col.biome {
            Biome::Forest if depth < 2 => Some(m.grass),
            Biome::Forest if depth < dirt_depth => Some(m.dirt),
            Biome::Desert if depth < 40 => Some(m.sand),
            Biome::Tundra if depth < 4 => Some(m.ice),
            Biome::Tundra if depth < dirt_depth => Some(m.dirt),
            _ => None,
        };
        if let Some(material) = layer {
            // 表面の層にも、洞窟が顔を出すことがある (坑道が表面近くを通るとき)。
            if depth >= CAVE_START - 6 && cave_open(seed, x, y, y - col.surface) {
                return self.open_cell(seed, x, y, y - col.surface);
            }
            return self.solid(material);
        }
        if cave_open(seed, x, y, y - col.surface) {
            return self.open_cell(seed, x, y, y - col.surface);
        }
        // 鉱石。
        if let Some(ore) = self.ore(seed, x, y, y - col.surface) {
            return self.solid(ore);
        }
        self.solid(m.stone)
    }

    /// 洞窟の空洞の中身: 水・溶岩の溜まり、底の溶岩の湖、そのほかは空。
    fn open_cell(&self, seed: u64, x: i32, y: i32, depth: i32) -> Option<Generated> {
        let m = &self.materials;
        if y >= BOTTOM - 24 {
            return self.solid(m.lava);
        }
        let (bx, by) = (x.div_euclid(48), y.div_euclid(48));
        if hash(seed, 0, bx, by, S_POOL).is_multiple_of(6) && depth > 60 {
            let level = by * 48 + 14 + (hash(seed, 0, bx, by, S_POOL_Y) % 24) as i32;
            if y >= level {
                // 深いほど溶岩が多い (深さ 0 → 0%、底 → 100%)。
                let lava_chance = (depth * 100 / (BOTTOM - SURFACE_BASE)).clamp(0, 100) as u32;
                let lava = hash(seed, 0, bx, by, S_POOL_KIND) % 100 < lava_chance;
                return self.solid(if lava { m.lava } else { m.water });
            }
        }
        None
    }

    /// 鉱石: 石の中の塊。石炭は浅く、鉄は中ほど、金は深い。
    fn ore(&self, seed: u64, x: i32, y: i32, depth: i32) -> Option<MaterialId> {
        let m = &self.materials;
        let (bx, by) = (x.div_euclid(40), y.div_euclid(40));
        let cluster =
            |gate: u32, salt_region: u32, salt_noise: u32, spacing: i32, threshold: i32| {
                hash(seed, 0, bx, by, salt_region).is_multiple_of(gate)
                    && octave(seed, x, y, spacing, salt_noise) < threshold
            };
        if (40..520).contains(&depth) && cluster(4, S_COAL, S_COAL2, 10, 52) {
            return Some(m.coal);
        }
        if (130..950).contains(&depth) && cluster(6, S_IRON, S_IRON2, 9, 46) {
            return Some(m.iron);
        }
        if depth >= 360 && cluster(9, S_GOLD, S_GOLD2, 8, 44) {
            return Some(m.gold);
        }
        None
    }

    /// 出発点: 表面に立つプレイヤー (16×24) の左上のセル。
    pub fn spawn_point(&self, seed: u64) -> [i32; 2] {
        [-8, surface_y(seed, 0) - 24 - 2]
    }
}

impl ChunkGenerator for AdventureGenerator {
    fn generate(&self, seed: u64, pos: ChunkPos, materials: &MaterialTable) -> Chunk {
        let mut world = World::new(Arc::new(materials.clone()), seed);
        world.insert_chunk(pos);
        let origin = pos.origin();
        let columns: Vec<Column> =
            (0..CHUNK_SIZE).map(|lx| Self::column(seed, origin.x + lx)).collect();
        for ly in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let cell_pos = CellPos::new(origin.x + lx, origin.y + ly);
                let col = &columns[lx as usize];
                let Some(g) = self.classify(seed, cell_pos.x, cell_pos.y, col) else { continue };
                let mut cell = world.make_cell(g.material, cell_pos);
                if g.block && blockable(materials, g.material) {
                    cell.flags |= FLAG_BLOCK;
                }
                world.set(cell_pos, cell);
            }
        }
        // insert_chunk で必ず存在する。起こりえない場合も空チャンクにフォールバックする。
        debug_assert!(world.chunk(pos).is_some());
        world.chunk(pos).cloned().unwrap_or_default()
    }
}
