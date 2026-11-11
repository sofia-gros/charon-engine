//! 仕様 05 §6: 飛んでいるセルを DDA で動かして格子に戻す。

use charon_core::cell::{Cell, FLAG_FALLING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::world::World;

pub const PARTICLE_LIMIT: usize = 50_000;
const MAX_STEP: f32 = 32.0;
const GRAVITY: f32 = 0.25;

/// 格子から一時的に離れているセル。座標と速度はセル単位 (tick ごと)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub cell: Cell,
}

/// 衝突前に通った最後の空きセルへ戻る。空きがまだなければ粒子を保持する。
fn advance(particle: &mut Particle, world: &mut World) -> bool {
    particle.vel[1] += GRAVITY;
    let length = particle.vel[0].hypot(particle.vel[1]);
    if length > MAX_STEP {
        particle.vel[0] *= MAX_STEP / length;
        particle.vel[1] *= MAX_STEP / length;
    }
    let [x, y] = particle.pos;
    let [vx, vy] = particle.vel;
    let mut cx = x.floor() as i32;
    let mut cy = y.floor() as i32;
    let start = CellPos::new(cx, cy);
    let mut last_empty = (world.get(start).material == MaterialId::EMPTY).then_some(start);
    let step_x = vx.signum() as i32;
    let step_y = vy.signum() as i32;
    let delta_x = if vx == 0.0 { f32::INFINITY } else { 1.0 / vx.abs() };
    let delta_y = if vy == 0.0 { f32::INFINITY } else { 1.0 / vy.abs() };
    let mut next_x = if vx == 0.0 {
        f32::INFINITY
    } else if vx > 0.0 {
        ((cx + 1) as f32 - x) * delta_x
    } else {
        (x - cx as f32) * delta_x
    };
    let mut next_y = if vy == 0.0 {
        f32::INFINITY
    } else if vy > 0.0 {
        ((cy + 1) as f32 - y) * delta_y
    } else {
        (y - cy as f32) * delta_y
    };
    loop {
        let time = next_x.min(next_y);
        if time > 1.0 {
            particle.pos = [x + vx, y + vy];
            return false;
        }
        if next_x <= next_y {
            cx += step_x;
            next_x += delta_x;
        } else {
            cy += step_y;
            next_y += delta_y;
        }
        let pos = CellPos::new(cx, cy);
        if world.get(pos).material != MaterialId::EMPTY {
            if let Some(empty) = last_empty {
                world.set(empty, particle.cell.with(FLAG_FALLING, true));
                return true;
            }
            // 押しのけ後に詰まった場所から飛び出す場合も、セルを失わない。
            return false;
        }
        last_empty = Some(pos);
    }
}

pub(crate) fn update(particles: &mut Vec<Particle>, world: &mut World) {
    let mut remaining = Vec::with_capacity(particles.len());
    for mut particle in particles.drain(..) {
        if !advance(&mut particle, world) {
            remaining.push(particle);
        }
    }
    *particles = remaining;
}
