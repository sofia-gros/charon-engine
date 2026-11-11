//! 仕様 05 §4: 剛体が進む位置のセルを、質量を失わずに押しのける。

use charon_core::cell::{Cell, FLAG_FALLING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;
use rapier2d::prelude::Vector;

use crate::particle::{PARTICLE_LIMIT, Particle};
use crate::stamp::StampTransform;
use crate::{CELL_MASS_SCALE, CELLS_PER_METER, PixelBody};

const DIRECTIONS: [[i32; 2]; 8] =
    [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]];

fn direction(velocity: [f32; 2]) -> [i32; 2] {
    if velocity[0].hypot(velocity[1]) < 0.05 {
        return [0, -1];
    }
    let octant = (velocity[1].atan2(velocity[0]) * 4.0 / std::f32::consts::PI).round() as i32;
    DIRECTIONS[octant.rem_euclid(8) as usize]
}

fn vacant(world: &World, body: &PixelBody, transform: StampTransform, pos: CellPos) -> bool {
    world.get(pos).material == MaterialId::EMPTY && !transform.occupies(body, pos)
}

fn place(
    world: &mut World,
    body: &PixelBody,
    transform: StampTransform,
    pos: CellPos,
    cell: Cell,
) -> bool {
    if !vacant(world, body, transform, pos) {
        return false;
    }
    let mut moved = cell.with(FLAG_FALLING, true);
    moved.vy = 0;
    world.set(pos, moved);
    true
}

/// 密な内容物が前を塞ぐとき、空洞内の最初の空きへ手前から順に詰める。
/// 移動元とセルを返し、剛体へ各セルの反力を加えるために使う。
fn shift_chain(
    world: &mut World,
    body: &PixelBody,
    transform: StampTransform,
    from: CellPos,
    direction: [i32; 2],
) -> Option<Vec<(CellPos, Cell)>> {
    let mut path = vec![from];
    let limit = i32::from(body.width.max(body.height)) + 1;
    for i in 1..=limit {
        let x = from.x.checked_add(direction[0] * i)?;
        let y = from.y.checked_add(direction[1] * i)?;
        let next = CellPos::new(x, y);
        if transform.occupies(body, next) {
            return None;
        }
        let cell = world.get(next);
        if cell.has(charon_core::cell::FLAG_BODY) || cell.material == MaterialId::BOUNDARY {
            return None;
        }
        path.push(next);
        if cell.material == MaterialId::EMPTY {
            let mut shifted = Vec::with_capacity(path.len() - 1);
            for index in (1..path.len()).rev() {
                let source = path[index - 1];
                let destination = path[index];
                let moved = world.get(source);
                shifted.push((source, moved));
                world.swap(source, destination);
                let mut active = moved.with(FLAG_FALLING, true);
                active.vy = 0;
                if active != moved {
                    world.set(destination, active);
                }
            }
            return Some(shifted);
        }
        if !matches!(world.materials().get(cell.material).phase, Phase::Powder | Phase::Liquid) {
            return None;
        }
    }
    None
}

fn nearby(
    world: &mut World,
    body: &PixelBody,
    transform: StampTransform,
    from: CellPos,
    cell: Cell,
    first: i32,
    last: i32,
) -> bool {
    for radius in first..=last {
        let mut ring = Vec::new();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) == radius {
                    ring.push((dx, dy));
                }
            }
        }
        // 距離が同じなら真上から時計回りに調べる（上 → 右 → 下 → 左）。
        ring.sort_by(|&(ax, ay), &(bx, by)| {
            (ax * ax + ay * ay)
                .cmp(&(bx * bx + by * by))
                .then_with(|| {
                    let angle = |dx: i32, dy: i32| {
                        (dx as f32).atan2(-(dy as f32)).rem_euclid(std::f32::consts::TAU)
                    };
                    angle(ax, ay).total_cmp(&angle(bx, by))
                })
                .then_with(|| (ay, ax).cmp(&(by, bx)))
        });
        for (dx, dy) in ring {
            let Some(x) = from.x.checked_add(dx) else { continue };
            let Some(y) = from.y.checked_add(dy) else { continue };
            if place(world, body, transform, CellPos::new(x, y), cell) {
                return true;
            }
        }
    }
    false
}

fn record_impulse(
    impulses: &mut Vec<(Vector, Vector)>,
    world: &World,
    pos: CellPos,
    cell: Cell,
    velocity: [f32; 2],
) {
    let density = f32::from(world.materials().get(cell.material).density);
    let impulse = Vector::new(
        -velocity[0] * density * CELL_MASS_SCALE,
        -velocity[1] * density * CELL_MASS_SCALE,
    );
    let point =
        Vector::new((pos.x as f32 + 0.5) / CELLS_PER_METER, (pos.y as f32 + 0.5) / CELLS_PER_METER);
    impulses.push((impulse, point));
}

/// 1 回のスタンプで共通する速度・力積・パーティクル。
pub(crate) struct DisplaceContext<'a> {
    pub transform: StampTransform,
    pub linear: [f32; 2],
    pub angular: f32,
    pub impulses: &'a mut Vec<(Vector, Vector)>,
    pub particles: &'a mut Vec<Particle>,
}

/// セルを押し出せた場合だけ true。完全に詰まったときは元のセルを残す。
pub(crate) fn displace(
    cell: Cell,
    pos: CellPos,
    body: &PixelBody,
    context: &mut DisplaceContext<'_>,
    world: &mut World,
) -> bool {
    let transform = context.transform;
    let dx = pos.x as f32 + 0.5 - transform.translation[0];
    let dy = pos.y as f32 + 0.5 - transform.translation[1];
    let velocity =
        [context.linear[0] - context.angular * dy, context.linear[1] + context.angular * dx];
    let [dir_x, dir_y] = direction(velocity);
    let mut relocated = false;
    for i in 1..=4 {
        let Some(x) = pos.x.checked_add(dir_x * i) else { continue };
        let Some(y) = pos.y.checked_add(dir_y * i) else { continue };
        if place(world, body, transform, CellPos::new(x, y), cell) {
            relocated = true;
            break;
        }
    }
    let shifted = if !relocated
        && matches!(world.materials().get(cell.material).phase, Phase::Powder | Phase::Liquid)
    {
        shift_chain(world, body, transform, pos, [dir_x, dir_y])
    } else {
        None
    };
    if shifted.is_some() {
        relocated = true;
    }
    if !relocated {
        relocated = nearby(world, body, transform, pos, cell, 1, 3);
    }
    if !relocated && context.particles.len() < PARTICLE_LIMIT {
        context.particles.push(Particle {
            pos: [pos.x as f32 + 0.5, pos.y as f32 + 0.5],
            vel: [velocity[0], velocity[1] - 1.0],
            cell,
        });
        relocated = true;
    }
    if !relocated {
        relocated = nearby(world, body, transform, pos, cell, 4, 16);
    }
    if let Some(shifted) = shifted {
        for (source, moved) in shifted {
            record_impulse(context.impulses, world, source, moved, velocity);
        }
    } else if relocated {
        record_impulse(context.impulses, world, pos, cell, velocity);
    }
    relocated
}
