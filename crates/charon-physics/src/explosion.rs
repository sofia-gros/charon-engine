//! 仕様 07 §5: 爆風のセルを粒子へ移し、剛体には既存の損傷経路を使う。

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::world::World;

use crate::{CELLS_PER_METER, PARTICLE_LIMIT, Particle, PhysicsEngine, stamp::StampTransform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplosionError {
    InvalidRadius,
}

impl std::fmt::Display for ExplosionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "爆発半径は 1..=16 セルです")
    }
}

impl std::error::Error for ExplosionError {}

/// 円内を y, x の昇順で処理し、収容できないセルは元の場所に残す。
pub fn explode(
    world: &mut World,
    physics: &mut PhysicsEngine,
    center: CellPos,
    radius: u8,
) -> Result<(), ExplosionError> {
    if !(1..=16).contains(&radius) {
        return Err(ExplosionError::InvalidRadius);
    }
    let r = i32::from(radius);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let (Some(x), Some(y)) = (center.x.checked_add(dx), center.y.checked_add(dy)) else {
                continue;
            };
            let pos = CellPos::new(x, y);
            let cell = world.get(pos);
            if cell.material == MaterialId::BOUNDARY || cell.material == MaterialId::EMPTY {
                continue;
            }
            let velocity = if dx == 0 && dy == 0 {
                [0.0, -1.0]
            } else {
                [dx.clamp(-8, 8) as f32, dy.clamp(-8, 8) as f32]
            };
            let particle = Particle {
                pos: [x as f32 + 0.5, y as f32 + 0.5],
                vel: velocity,
                cell: cell.with(FLAG_BODY, false),
            };
            if cell.has(FLAG_BODY) {
                let owner = physics.bodies.iter().enumerate().find_map(|(index, body)| {
                    if !body.stamped.contains(&pos) {
                        return None;
                    }
                    let rigid = physics.world.bodies.get(body.handle)?;
                    let translation = rigid.translation() * CELLS_PER_METER;
                    let transform = StampTransform::new(
                        [translation.x, translation.y],
                        rigid.rotation().angle(),
                    );
                    let local = transform.local_index(body, pos)?;
                    (body.cells[local].material != MaterialId::EMPTY).then_some((
                        index,
                        local % usize::from(body.width),
                        local / usize::from(body.width),
                    ))
                });
                if let Some((index, local_x, local_y)) = owner {
                    if physics.particles().len() >= PARTICLE_LIMIT
                        || !physics.spawn_particle(particle)
                    {
                        continue;
                    }
                    if !physics.damage(index, local_x as u16, local_y as u16) {
                        physics.particles.pop();
                    }
                }
            } else if physics.spawn_particle(particle) {
                world.set(pos, Cell::default());
            }
        }
    }
    Ok(())
}
