//! 仕様 18 §2: カーソル (使うボタンのスティックで動かす、届く範囲の円の中の場所)。整数だけ。
//!
//! 画面 (タッチ・マウス) の側は、カーソルの**ずれ** (プレイヤーの中心からのセル) か、スマートカーソルの**向き**を渡す。
//! ここでは、届く範囲の円に収め、ブロックの格子に揃え、そこに今のアイテムが使えるかを調べる。

use charon_core::cell::MaterialId;
use charon_core::coords::CellPos;
use charon_core::world::World;

use crate::blocks::{BLOCK_CELLS, BlockPos};
use crate::{Entity, EntityId, EntityWorld};

/// 届く距離 (セル)。6 ブロック。
pub const REACH_CELLS: i32 = 48;

/// カーソルが指す場所。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// 指しているセル。
    pub cell: CellPos,
    /// それを含むブロック。
    pub block: BlockPos,
}

impl Cursor {
    pub fn at(cell: CellPos) -> Self {
        Self { cell, block: BlockPos::containing(cell) }
    }
}

/// 何をするためのカーソルか (選んでいるアイテムで決まる)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorUse {
    /// ブロックを置く。ブロックの格子に揃う。
    Place,
    /// ブロックを壊す。ブロックの格子に揃う。
    Mine,
    /// 武器・道具など。格子に揃えない (向きだけ使う)。
    Free,
}

/// カーソルの状態 (枠の色)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorState {
    Ok,
    /// 届かない。
    OutOfReach,
    /// 置けない・壊せない。
    Invalid,
}

/// エンティティの中心のセル。
pub fn center_cell(entity: &Entity) -> CellPos {
    let cx = (entity.x16 + i64::from(entity.width) * 8).div_euclid(16);
    let cy = (entity.y16 + i64::from(entity.height) * 8).div_euclid(16);
    CellPos::new(cx.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32, {
        cy.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    })
}

fn isqrt(n: i64) -> i64 {
    // 差の 2 乗の和なので、負にならない。
    n.max(0).isqrt()
}

/// `(dx, dy)` を、長さ `REACH_CELLS` の円の中に収める。
pub fn clamp_to_reach(offset: (i32, i32)) -> (i32, i32) {
    let (dx, dy) = (i64::from(offset.0), i64::from(offset.1));
    let len = isqrt(dx * dx + dy * dy);
    let reach = i64::from(REACH_CELLS);
    if len <= reach {
        return offset;
    }
    // len > reach > 0 なので、割り算は安全。結果は reach 以下で i32 に収まる。
    ((dx * reach / len) as i32, (dy * reach / len) as i32)
}

/// 向き `dir` (長さは問わない) の、円の縁へのずれ。`dir` が (0, 0) なら `None`。
pub fn rim_offset(dir: (i32, i32)) -> Option<(i32, i32)> {
    let (dx, dy) = (i64::from(dir.0), i64::from(dir.1));
    let len = isqrt(dx * dx + dy * dy);
    if len == 0 {
        return None;
    }
    let reach = i64::from(REACH_CELLS);
    Some(((dx * reach / len) as i32, (dy * reach / len) as i32))
}

fn offset_cell(center: CellPos, offset: (i32, i32)) -> CellPos {
    CellPos::new(center.x.saturating_add(offset.0), center.y.saturating_add(offset.1))
}

/// カーソルを、中心からのずれで指す (通常のカーソル)。円の外は縁に収める。ブロックを使うもの (`Place`・`Mine`) は、
/// そのセルを含むブロックの中心に揃える (`cell` はブロックの中心のセル)。
pub fn cursor_from_offset(center: CellPos, offset: (i32, i32), usage: CursorUse) -> Cursor {
    let cell = offset_cell(center, clamp_to_reach(offset));
    snap(cell, usage)
}

fn snap(cell: CellPos, usage: CursorUse) -> Cursor {
    match usage {
        CursorUse::Free => Cursor::at(cell),
        CursorUse::Place | CursorUse::Mine => {
            let block = BlockPos::containing(cell);
            // ブロックの中心 (4 セル目) のセル。座標が大きすぎるときは、そのままのセル。
            let cell = block.origin().map_or(cell, |o| CellPos::new(o.x + 4, o.y + 4));
            Cursor { cell, block }
        }
    }
}

/// 向き `dir` の直線上で、届く範囲の中を 1 セルずつ調べる。最初の固体のセルと、その手前の最後の空きのセル。
fn march(
    world: &World,
    center: CellPos,
    dir: (i32, i32),
) -> (Option<CellPos>, Option<CellPos>, CellPos) {
    let (dx, dy) = (i64::from(dir.0), i64::from(dir.1));
    let len = isqrt(dx * dx + dy * dy).max(1);
    let mut last_free = None;
    let mut cell = center;
    for t in 1..=i64::from(REACH_CELLS) {
        // 向きの単位ベクトル × t。丸めるので、斜めでも 1 セルずつ進む。
        let x = i64::from(center.x) + (dx * t + len / 2 * dx.signum()) / len;
        let y = i64::from(center.y) + (dy * t + len / 2 * dy.signum()) / len;
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else { break };
        cell = CellPos::new(x, y);
        if world.get(cell).is_solid(world.materials()) {
            return (Some(cell), last_free, cell);
        }
        last_free = Some(cell);
    }
    (None, last_free, cell)
}

/// スマートカーソル (仕様 18 §2 の 5): 倒した向きに、届く範囲の円の縁へすぐに動く。ブロックを使うときは、向きの直線上で最初に当たる
/// 掘れるブロック (`Mine`)、またはその手前の置ける場所 (`Place`) に合わせる。届かなければ円の縁。`dir` が (0, 0) なら `None`。
pub fn smart_cursor(
    world: &World,
    center: CellPos,
    dir: (i32, i32),
    usage: CursorUse,
) -> Option<Cursor> {
    let rim = rim_offset(dir)?;
    let rim_cursor = snap(offset_cell(center, rim), usage);
    Some(match usage {
        CursorUse::Free => rim_cursor,
        CursorUse::Mine => match march(world, center, dir) {
            (Some(solid), _, _) => snap(solid, usage),
            _ => rim_cursor,
        },
        CursorUse::Place => match march(world, center, dir) {
            (Some(_), Some(free), _) => snap(free, usage),
            _ => rim_cursor,
        },
    })
}

/// ブロックの 64 セルのうち、旗つきのセルの数。
fn block_cells_flagged(world: &World, block: BlockPos) -> Option<usize> {
    let cells = block.cells();
    if cells.len() != BLOCK_CELLS {
        return None;
    }
    cells
        .iter()
        .map(|&pos| world.get(pos))
        .try_fold(0usize, |n, cell| Some(n + usize::from(cell.has(charon_core::cell::FLAG_BLOCK))))
}

/// カーソルの状態。中心 `center` から届くか、そこに置ける (`Place`)・壊せる (`Mine`) か。
pub fn cursor_state(
    world: &World,
    center: CellPos,
    cursor: Cursor,
    usage: CursorUse,
) -> CursorState {
    let (dx, dy) = (
        i64::from(cursor.cell.x) - i64::from(center.x),
        i64::from(cursor.cell.y) - i64::from(center.y),
    );
    let reach = i64::from(REACH_CELLS);
    if dx * dx + dy * dy > reach * reach {
        return CursorState::OutOfReach;
    }
    match usage {
        CursorUse::Free => CursorState::Ok,
        CursorUse::Place => match block_cells_flagged(world, cursor.block) {
            Some(0) => CursorState::Ok,
            _ => CursorState::Invalid,
        },
        CursorUse::Mine => match block_cells_flagged(world, cursor.block) {
            Some(n) if n == BLOCK_CELLS => CursorState::Ok,
            _ => CursorState::Invalid,
        },
    }
}

/// プレイヤー `id` の中心 (エンティティがなければ `None`)。
pub fn center_of(entities: &EntityWorld, id: EntityId) -> Option<CellPos> {
    entities.get(id).map(center_cell)
}

/// カーソルの用途を、選んでいるアイテムから決める。ブロックは `Place`、つるはしは `Mine`、ほかは `Free`。
pub fn usage_of(item: Option<crate::items::ItemId>, defs: &crate::defs::Definitions) -> CursorUse {
    use crate::items::{ItemClass, ItemKind};
    let Some(item) = item else { return CursorUse::Free };
    match item.class() {
        ItemClass::Block(m) if m != MaterialId::EMPTY => CursorUse::Place,
        ItemClass::Def(_) => match defs.item_def(item).map(|d| &d.kind) {
            Some(ItemKind::Tool { .. }) => CursorUse::Mine,
            _ => CursorUse::Free,
        },
        _ => CursorUse::Free,
    }
}
