//! 仕様 10 §12: 掘る道具 (Starbound のレーザーのように、狙った場所の円を削る)。
//!
//! すべての固体・粉体には**硬さ** (`Material::hardness`) があり、道具には**力** (`DigDef::power`) がある。
//! 当てている間、円の中の各セルへ 1 tick ごとに `power` のダメージが溜まり、材質の硬さに達したセルが消える。
//! だから掘る時間は `硬さ / 力` tick で、硬い石は長く、砂は一瞬。液体・気体・剛体・`boundary` は掘れない。
//! 整数だけで決まる (光線は 1/16 セルの整数で進める)。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

use crate::defs::DigDef;

/// 掘ったセル (消えたセルと、その材質)。拾う・集めるのはゲームの仕事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dug {
    pub pos: CellPos,
    pub material: MaterialId,
}

/// 描画用の光線: 手から、当たった点 (または届いた先) まで (セル座標)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    pub from: [f32; 2],
    pub to: [f32; 2],
    /// 掘れるセルに当たっているか。
    pub hit: bool,
}

/// 右手に持つ掘る道具 1 つ。
#[derive(Clone, Debug)]
pub struct Dig {
    pub def: DigDef,
    /// セルごとに溜まったダメージ。当てていないと `recover` ずつ回復して消える。
    damage: BTreeMap<(i32, i32), u16>,
    beam: Option<Beam>,
}

/// 光線の進み方: `aim` の長い方の軸が 1 セル (16) ずつ進む整数の歩幅。`aim` が 0 なら `None`。
fn step_vector(aim: (i32, i32)) -> Option<(i64, i64)> {
    let (ax, ay) = (i64::from(aim.0), i64::from(aim.1));
    let major = ax.abs().max(ay.abs());
    if major == 0 {
        return None;
    }
    Some((ax * 16 / major, ay * 16 / major))
}

/// 光線を通すセルか: 空・気体・液体 (掘れないので素通り)。固体・粉体・剛体・未読み込みは通さない。
fn passes(world: &World, pos: CellPos) -> bool {
    let cell = world.get(pos);
    cell.material != MaterialId::BOUNDARY
        && !cell.has(FLAG_BODY)
        && !matches!(world.materials().get(cell.material).phase, Phase::Solid | Phase::Powder)
}

impl Dig {
    pub fn new(def: DigDef) -> Self {
        Self { def, damage: BTreeMap::new(), beam: None }
    }

    pub fn beam(&self) -> Option<Beam> {
        self.beam
    }

    /// セル `pos` に溜まったダメージ。
    pub fn damage_at(&self, pos: CellPos) -> u16 {
        self.damage.get(&(pos.x, pos.y)).copied().unwrap_or(0)
    }

    /// 溜まっているダメージのあるセルの数。
    pub fn damaged_cells(&self) -> usize {
        self.damage.len()
    }

    /// 1 tick 進める。`origin16` は手の位置 (1/16 セル)、`aim` は狙いの向き、`active` は押しているか。
    /// 消えたセルを返す (消すのは `World::set` だけ)。
    pub(crate) fn step(
        &mut self,
        world: &mut World,
        origin16: (i64, i64),
        aim: (i32, i32),
        active: bool,
    ) -> Vec<Dug> {
        let mut dug = Vec::new();
        let mut touched = BTreeSet::new();
        self.beam = None;
        if active && let Some((sx, sy)) = step_vector(aim) {
            let range = i64::from(self.def.range);
            let (mut x, mut y) = origin16;
            let mut hit = None;
            for _ in 0..range {
                let (nx, ny) = (x + sx, y + sy);
                let cell = cell_of(nx, ny);
                let Some(pos) = cell else { break };
                if !passes(world, pos) {
                    // 掘れる固体・粉体なら、ここが当たった点。剛体・境界は、光線が止まるだけ。
                    let target = world.get(pos);
                    let diggable = target.material != MaterialId::BOUNDARY
                        && !target.has(FLAG_BODY)
                        && world.materials().get(target.material).hardness != u16::MAX;
                    if diggable {
                        hit = Some(pos);
                    }
                    x = nx;
                    y = ny;
                    break;
                }
                x = nx;
                y = ny;
            }
            let end = [x as f32 / 16.0, y as f32 / 16.0];
            self.beam = Some(Beam {
                from: [origin16.0 as f32 / 16.0, origin16.1 as f32 / 16.0],
                to: end,
                hit: hit.is_some(),
            });
            if let Some(center) = hit {
                self.carve(world, center, &mut touched, &mut dug);
            }
        }
        // 当てていないセルのダメージは回復する。
        let recover = self.def.recover;
        self.damage.retain(|key, value| {
            if touched.contains(key) {
                return true;
            }
            *value = value.saturating_sub(recover);
            *value > 0
        });
        dug
    }

    /// 円 (半径 `radius`) の中の掘れるセルにダメージを与え、硬さに達したセルを消す。
    fn carve(
        &mut self,
        world: &mut World,
        center: CellPos,
        touched: &mut BTreeSet<(i32, i32)>,
        dug: &mut Vec<Dug>,
    ) {
        let r = i32::from(self.def.radius);
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let pos = CellPos::new(center.x.saturating_add(dx), center.y.saturating_add(dy));
                let cell = world.get(pos);
                if cell.material == MaterialId::BOUNDARY || cell.has(FLAG_BODY) {
                    continue;
                }
                let hardness = world.materials().get(cell.material).hardness;
                let diggable = matches!(
                    world.materials().get(cell.material).phase,
                    Phase::Solid | Phase::Powder
                );
                if !diggable || hardness == 0 || hardness == u16::MAX {
                    continue;
                }
                let key = (pos.x, pos.y);
                let total =
                    self.damage.get(&key).copied().unwrap_or(0).saturating_add(self.def.power);
                if total >= hardness {
                    self.damage.remove(&key);
                    let empty = world.make_cell(MaterialId::EMPTY, pos);
                    world.set(pos, empty);
                    dug.push(Dug { pos, material: cell.material });
                } else {
                    self.damage.insert(key, total);
                    touched.insert(key);
                }
            }
        }
    }

    /// 状態を FNV-1a 64-bit で流す (同期の照合用)。
    pub(crate) fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        };
        for (&(x, y), &value) in &self.damage {
            feed(&x.to_le_bytes());
            feed(&y.to_le_bytes());
            feed(&value.to_le_bytes());
        }
        hash
    }
}

/// 1/16 セルの座標のセル (範囲外は `None`)。
fn cell_of(x16: i64, y16: i64) -> Option<CellPos> {
    let x = i32::try_from(x16.div_euclid(16)).ok()?;
    let y = i32::try_from(y16.div_euclid(16)).ok()?;
    Some(CellPos::new(x, y))
}
