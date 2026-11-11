//! 仕様 05 §5: 隣接する液体と静止した粉体が剛体の面に加える力。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use charon_core::cell::{FLAG_BODY, FLAG_FALLING};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;
use rapier2d::prelude::Vector;

use crate::{CELL_MASS_SCALE, CELLS_PER_METER, PhysicsEngine};

pub(crate) const PRESSURE_SCALE: f32 = (9.81 * 4.0) * CELL_MASS_SCALE;

const NEIGHBORS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

fn column_height(world: &World, start: CellPos, phase: Phase, resting: bool) -> u32 {
    let mut height = 0;
    for offset in 0..48i32 {
        let Some(y) = start.y.checked_sub(offset) else { break };
        let cell = world.get(CellPos::new(start.x, y));
        if cell.has(FLAG_BODY)
            || world.materials().get(cell.material).phase != phase
            || (resting && cell.has(FLAG_FALLING))
        {
            break;
        }
        height += 1;
    }
    height
}

/// 4 近傍でつながる液体領域の最上段。結果は力の計算中だけ共有する。
fn liquid_surface(world: &World, start: CellPos, cache: &mut BTreeMap<CellPos, i32>) -> i32 {
    if let Some(&surface) = cache.get(&start) {
        return surface;
    }
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let mut surface = start.y;
    while let Some(pos) = queue.pop_front() {
        surface = surface.min(pos.y);
        for (dx, dy) in NEIGHBORS {
            let (Some(x), Some(y)) = (pos.x.checked_add(dx), pos.y.checked_add(dy)) else {
                continue;
            };
            let next = CellPos::new(x, y);
            if seen.contains(&next) {
                continue;
            }
            let cell = world.get(next);
            if !cell.has(FLAG_BODY) && world.materials().get(cell.material).phase == Phase::Liquid {
                seen.insert(next);
                queue.push_back(next);
            }
        }
    }
    for pos in seen {
        cache.insert(pos, surface);
    }
    surface
}

impl PhysicsEngine {
    /// 現在スタンプされた面を調べ、rapier の剛体へ各セルの圧力を加える。
    pub fn apply_cell_forces(&mut self, world: &World) {
        let mut surfaces = BTreeMap::new();
        for body in &self.bodies {
            let mut forces = Vec::new();
            for &pos in &body.stamped {
                for (dx, dy) in NEIGHBORS {
                    let (Some(x), Some(y)) = (pos.x.checked_add(dx), pos.y.checked_add(dy)) else {
                        continue;
                    };
                    let neighbor = CellPos::new(x, y);
                    let cell = world.get(neighbor);
                    if cell.has(FLAG_BODY) {
                        continue;
                    }
                    let material = world.materials().get(cell.material);
                    let force = if material.phase == Phase::Liquid {
                        let surface = liquid_surface(world, neighbor, &mut surfaces);
                        let face_y = pos.y as f32 + 0.5 + dy as f32 * 0.5;
                        let depth = (face_y - surface as f32).clamp(0.0, 48.0);
                        let pressure = f32::from(material.density) * depth * PRESSURE_SCALE;
                        Vector::new(-(dx as f32) * pressure, -(dy as f32) * pressure)
                    } else if material.phase == Phase::Powder && !cell.has(FLAG_FALLING) && dy == -1
                    {
                        let height = column_height(world, neighbor, Phase::Powder, true);
                        Vector::new(
                            0.0,
                            f32::from(material.density) * height as f32 * PRESSURE_SCALE,
                        )
                    } else {
                        continue;
                    };
                    let point = Vector::new(
                        (pos.x as f32 + 0.5) / CELLS_PER_METER,
                        (pos.y as f32 + 0.5) / CELLS_PER_METER,
                    );
                    forces.push((force, point));
                }
            }
            if let Some(rigid) = self.world.bodies.get_mut(body.handle) {
                // rapier の add_force は次の tick にも残るため、固定 dt 分の力積として一度だけ加える。
                for (force, point) in forces {
                    rigid.apply_impulse_at_point(
                        force * self.world.integration_parameters.dt,
                        point,
                        true,
                    );
                }
            }
        }
    }
}
