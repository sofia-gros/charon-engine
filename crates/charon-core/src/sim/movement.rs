//! セルの移動 (仕様 02 §5〜§8)。

use crate::cell::{Cell, FLAG_BODY, FLAG_BOUNCED, FLAG_FALLING, FLAG_FLOW_RIGHT};
use crate::material::{Material, MaterialTable, Phase};
use crate::rng::{SALT_DIAG, SALT_DRAG, SALT_FALL, SALT_STICK, SALT_VISC, SALT_WAKE, chance};
use crate::sim::{Neighborhood, SCAN};

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

/// 気体 `mover` が `target` の場所へ昇れるか (仕様 02 §5)。
pub(super) fn can_rise(materials: &MaterialTable, mover: Cell, target: Cell) -> bool {
    if target.has(FLAG_BODY) {
        return false;
    }
    let tm = materials.get(target.material);
    match tm.phase {
        Phase::Empty => true,
        Phase::Gas => materials.get(mover.material).density < tm.density,
        _ => false,
    }
}

/// 液体と気体の移動 (仕様 02 §8)。気体は「上下が逆の液体」として同じ手順で動かす。
pub(super) fn move_fluid(nb: &mut Neighborhood<'_>, x: i32, y: i32, cell: Cell, mat: &Material) {
    let materials = nb.materials();
    let up = mat.phase == Phase::Gas;
    let dy = if up { -1 } else { 1 };
    let pass = |nb: &Neighborhood<'_>, tx: i32, ty: i32| {
        let target = nb.get(tx, ty);
        if up { can_rise(materials, cell, target) } else { can_displace(materials, cell, target) }
    };
    // 動ける場所があるのに今回は見送るか (粘性)。1 セルにつき 1 tick に 1 回だけ引く。
    let skip = chance(nb.hash(x, y, SALT_VISC), if up { 128 } else { mat.viscosity });
    // 1 回に横へ進める距離。読み書きを MAX_REACH に収めるため SCAN を上限にする。
    let reach = (mat.dispersion.max(1) as i32).min(SCAN);
    let d = if cell.has(FLAG_FLOW_RIGHT) { 1 } else { -1 };

    // 1. 縦
    if pass(nb, x, y + dy) {
        if skip {
            nb.keep_awake(x, y);
            return;
        }
        let (steps, vy) = if up { (1, 0) } else { fall_steps(nb, x, y, cell, mat) };
        let mut cy = y;
        let mut moved = 0;
        while moved < steps && pass(nb, x, cy + dy) {
            nb.swap(x, cy, x, cy + dy);
            cy += dy;
            moved += 1;
        }
        let mut c = nb.get(x, cy).with(FLAG_FALLING, true).with(FLAG_BOUNCED, false);
        c.vy = if moved == steps { vy } else { 0 };
        nb.set(x, cy, c);
        return;
    }

    // 2. 斜め
    for dx in [d, -d] {
        if pass(nb, x + dx, y + dy) && pass(nb, x + dx, y) {
            if skip {
                nb.keep_awake(x, y);
                return;
            }
            nb.swap(x, y, x + dx, y + dy);
            let mut c = nb
                .get(x + dx, y + dy)
                .with(FLAG_FALLING, true)
                .with(FLAG_BOUNCED, false)
                .with(FLAG_FLOW_RIGHT, dx > 0);
            c.vy = 0;
            nb.set(x + dx, y + dy, c);
            return;
        }
    }

    // ここから先は縦に進めなかった場合。
    let mut cell = cell;
    if cell.vy != 0 {
        cell.vy = 0;
        nb.set(x, y, cell);
    }

    // 3. 行き先を探す
    let dirs = [d, -d];
    let mut open = [0i32; 2];
    for (i, dir) in dirs.into_iter().enumerate() {
        let mut goal = 0;
        for k in 1..=SCAN {
            if !pass(nb, x + dir * k, y) {
                break;
            }
            open[i] = k;
            if pass(nb, x + dir * k, y + dy) {
                goal = k;
                break;
            }
        }
        if goal > 0 {
            if skip {
                nb.keep_awake(x, y);
                return;
            }
            let tx = x + dir * goal.min(reach);
            nb.swap(x, y, tx, y);
            set_flags(nb, tx, y, |c| c.with(FLAG_FLOW_RIGHT, dir > 0));
            return;
        }
    }

    // 4. 押されている
    let behind = nb.get(x, y - dy);
    let bm = materials.get(behind.material);
    let pressed = bm.phase == mat.phase
        && !behind.has(FLAG_BODY)
        && if up { bm.density <= mat.density } else { bm.density >= mat.density };
    if pressed {
        for (i, dir) in dirs.into_iter().enumerate() {
            if open[i] > 0 {
                if skip {
                    nb.keep_awake(x, y);
                    return;
                }
                let tx = x + dir * open[i].min(reach);
                nb.swap(x, y, tx, y);
                set_flags(nb, tx, y, |c| {
                    c.with(FLAG_FALLING, true)
                        .with(FLAG_BOUNCED, false)
                        .with(FLAG_FLOW_RIGHT, dir > 0)
                });
                return;
            }
        }
        return; // 両側とも塞がっている。何も書き込まない
    }

    // 5. 勢い
    if !cell.has(FLAG_FALLING) {
        return; // 何も書き込まない。ここで休眠する
    }
    if open[0] > 0 {
        if skip {
            nb.keep_awake(x, y);
            return;
        }
        nb.swap(x, y, x + d * open[0].min(reach), y);
        return;
    }
    if materials.get(nb.get(x + d, y).material).phase == mat.phase {
        return; // 同じ相に塞がれた。待つ (何も書き込まない)
    }
    if !cell.has(FLAG_BOUNCED) {
        // 壁に 1 回目のはね返り
        nb.set(x, y, cell.with(FLAG_BOUNCED, true).with(FLAG_FLOW_RIGHT, d < 0));
        return;
    }
    // 2 回目。勢いを失う (書き込みはこの 1 回だけ)
    nb.set(x, y, cell.with(FLAG_FALLING, false).with(FLAG_BOUNCED, false));
}

/// `(x, y)` のセルのフラグを変える。値が変わる場合だけ書き込む。
fn set_flags(nb: &mut Neighborhood<'_>, x: i32, y: i32, f: impl FnOnce(Cell) -> Cell) {
    let old = nb.get(x, y);
    let new = f(old);
    if new != old {
        nb.set(x, y, new);
    }
}
