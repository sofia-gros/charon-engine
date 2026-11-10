//! セルの移動 (仕様 02 §5〜§8)。

use crate::cell::{Cell, FLAG_BODY, FLAG_FALLING};
use crate::material::{Material, MaterialTable, Phase};
use crate::rng::{SALT_DIAG, SALT_DRAG, SALT_FALL, SALT_STICK, SALT_WAKE, chance};
use crate::sim::Neighborhood;

/// `mover` が `target` の場所へ下向き・横向きに入れるか (仕様 02 §5)。
pub(super) fn can_displace(materials: &MaterialTable, mover: Cell, target: Cell) -> bool {
    if target.has(FLAG_BODY) {
        return false;
    }
    let tm = materials.get(target.material);
    let mm = materials.get(mover.material);
    match tm.phase {
        Phase::Empty => true,
        Phase::Solid | Phase::Powder => false,
        Phase::Liquid => {
            matches!(mm.phase, Phase::Powder | Phase::Liquid) && mm.density > tm.density
        }
        Phase::Gas => mm.phase != Phase::Gas || mm.density > tm.density,
    }
}

/// この tick に落ちる歩数と、新しい `vy` (仕様 02 §6)。
pub(super) fn fall_steps(
    nb: &Neighborhood<'_>,
    x: i32,
    y: i32,
    cell: Cell,
    mat: &Material,
) -> (i32, i8) {
    let vy = (cell.vy as i32 + mat.gravity as i32).clamp(0, mat.max_speed.min(127) as i32);
    let mut steps = vy / 16;
    if ((nb.hash(x, y, SALT_FALL) & 15) as i32) < vy % 16 {
        steps += 1;
    }
    (steps.max(1), vy as i8)
}

/// 粉体の移動 (仕様 02 §7)。
pub(super) fn move_powder(nb: &mut Neighborhood<'_>, x: i32, y: i32, cell: Cell, mat: &Material) {
    let materials = nb.materials();

    // 真下
    let below = nb.get(x, y + 1);
    if can_displace(materials, cell, below) {
        // 液体の中を沈むときの抵抗
        let bm = materials.get(below.material);
        if bm.phase == Phase::Liquid && chance(nb.hash(x, y, SALT_DRAG), bm.viscosity / 2 + 64) {
            nb.keep_awake(x, y);
            return;
        }
        let (steps, vy) = fall_steps(nb, x, y, cell, mat);
        let mut cy = y;
        let mut moved = 0;
        while moved < steps && can_displace(materials, cell, nb.get(x, cy + 1)) {
            nb.swap(x, cy, x, cy + 1);
            cy += 1;
            moved += 1;
        }
        let mut c = nb.get(x, cy).with(FLAG_FALLING, true);
        c.vy = if moved == steps { vy } else { 0 };
        nb.set(x, cy, c);
        wake_neighbors(nb, x, y);
        return;
    }

    // 斜め下 (動いている途中のセルだけ)
    // 摩擦: 着地したセルは確率 friction / 256 でその場に止まる。
    if cell.has(FLAG_FALLING) && !chance(nb.hash(x, y, SALT_STICK), mat.friction) {
        let d = if nb.hash(x, y, SALT_DIAG) & 1 == 1 { 1 } else { -1 };
        for dx in [d, -d] {
            if can_displace(materials, cell, nb.get(x + dx, y + 1))
                && can_displace(materials, cell, nb.get(x + dx, y))
            {
                nb.swap(x, y, x + dx, y + 1);
                if cell.vy != 0 {
                    let mut c = nb.get(x + dx, y + 1);
                    c.vy = 0;
                    nb.set(x + dx, y + 1, c);
                }
                wake_neighbors(nb, x, y);
                return;
            }
        }
    }

    // どこにも動けなかった。値が変わる場合だけ書き込む (仕様 02 §9)。
    if cell.has(FLAG_FALLING) || cell.vy != 0 {
        let mut c = nb.get(x, y).with(FLAG_FALLING, false);
        c.vy = 0;
        nb.set(x, y, c);
    }
}

/// セルが出ていった場所 `(x, y)` の左右と上の、止まっている粉体を起こす (摩擦。仕様 02 §7)。
fn wake_neighbors(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    let materials = nb.materials();
    for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1)] {
        let n = nb.get(nx, ny);
        if n.has(FLAG_FALLING) || n.has(FLAG_BODY) {
            continue;
        }
        let nm = materials.get(n.material);
        if nm.phase == Phase::Powder && chance(nb.hash(nx, ny, SALT_WAKE), 255 - nm.friction) {
            nb.set(nx, ny, n.with(FLAG_FALLING, true));
        }
    }
}

/// 液体と気体の移動 (仕様 02 §8)。T1.10 で実装する。
pub(super) fn move_fluid(
    _nb: &mut Neighborhood<'_>,
    _x: i32,
    _y: i32,
    _cell: Cell,
    _mat: &Material,
) {
}
