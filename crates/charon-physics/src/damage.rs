//! 仕様 05 §7: 損傷したセル配列を 4 近傍で分割する。

use std::collections::VecDeque;

use charon_core::cell::{Cell, FLAG_BODY, FLAG_SMOOTH, MaterialId};
use charon_core::world::World;
use rapier2d::prelude::{Rotation, Vector};

use crate::particle::{PARTICLE_LIMIT, Particle};
use crate::{BodyError, CELLS_PER_METER, PhysicsEngine, PixelBody, stamp};

fn components(body: &PixelBody) -> Vec<Vec<usize>> {
    let (width, height) = (usize::from(body.width), usize::from(body.height));
    let mut visited = vec![false; body.cells.len()];
    let mut result = Vec::new();
    for start in 0..body.cells.len() {
        if visited[start] || body.cells[start].material == MaterialId::EMPTY {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        let mut connected = Vec::new();
        visited[start] = true;
        while let Some(index) = queue.pop_front() {
            connected.push(index);
            let (x, y) = (index % width, index / width);
            for next in [
                (y > 0).then(|| index - width),
                (x + 1 < width).then(|| index + 1),
                (y + 1 < height).then(|| index + width),
                (x > 0).then(|| index - 1),
            ]
            .into_iter()
            .flatten()
            {
                if !visited[next] && body.cells[next].material != MaterialId::EMPTY {
                    visited[next] = true;
                    queue.push_back(next);
                }
            }
        }
        result.push(connected);
    }
    result
}

impl PhysicsEngine {
    /// 剛体のローカルセルを削る。次の tick の冒頭で `split_damaged` を呼ぶ。
    pub fn damage(&mut self, body: usize, local_x: u16, local_y: u16) -> bool {
        let Some(body) = self.bodies.get_mut(body) else { return false };
        if local_x >= body.width || local_y >= body.height {
            return false;
        }
        let index = usize::from(local_y) * usize::from(body.width) + usize::from(local_x);
        if body.cells[index].material == MaterialId::EMPTY {
            return false;
        }
        body.cells[index] = Cell::default();
        body.damaged = true;
        true
    }

    /// tick の冒頭で、変更された形を作り直し、複数成分と小片を分離する。
    pub fn split_damaged(&mut self, world: &mut World) -> Result<(), BodyError> {
        let mut index = 0;
        while index < self.bodies.len() {
            if !self.bodies[index].damaged {
                index += 1;
                continue;
            }
            let body = &self.bodies[index];
            let groups = components(body);
            let small_count: usize =
                groups.iter().filter(|group| group.len() < 8).map(Vec::len).sum();
            // 上限のため小片を失わない。粒子が着地した次 tick に再試行する。
            if self.particles.len().saturating_add(small_count) > PARTICLE_LIMIT {
                index += 1;
                continue;
            }
            let Some(rigid) = self.world.bodies.get(body.handle) else {
                // 剛体のハンドルが外部から無効化された場合、セルを消さない。
                debug_assert!(false, "損傷した剛体の rapier ハンドルがない");
                index += 1;
                continue;
            };
            let (width, height, origin) = (body.width, body.height, body.origin);
            // 壊れた破片は、元の剛体と同じ描き方を引き継ぐ (仕様 05 §11)。
            let render = body.render;
            let cells = body.cells.clone();
            let translation = rigid.translation();
            let angle = rigid.rotation().angle();
            let (sin, cos) = angle.sin_cos();
            let linear = rigid.linvel();
            let angular = rigid.angvel();
            let original_len = self.bodies.len();
            let mut particles = Vec::with_capacity(small_count);
            for group in groups {
                if group.len() < 8 {
                    for offset in group {
                        let dx = (offset % usize::from(width)) as f32 + 0.5 - origin[0];
                        let dy = (offset / usize::from(width)) as f32 + 0.5 - origin[1];
                        let rx = cos * dx - sin * dy;
                        let ry = sin * dx + cos * dy;
                        particles.push(Particle {
                            pos: [
                                translation.x * CELLS_PER_METER + rx,
                                translation.y * CELLS_PER_METER + ry,
                            ],
                            vel: [
                                (linear.x * CELLS_PER_METER - angular * ry) / 60.0,
                                (linear.y * CELLS_PER_METER + angular * rx) / 60.0,
                            ],
                            cell: cells[offset].with(FLAG_BODY, false).with(FLAG_SMOOTH, false),
                        });
                    }
                    continue;
                }
                let mut part = vec![Cell::default(); cells.len()];
                for offset in group {
                    part[offset] = cells[offset];
                }
                let new_index = match self.create_body(
                    width,
                    height,
                    part,
                    world.materials(),
                    [translation.x * CELLS_PER_METER, translation.y * CELLS_PER_METER],
                ) {
                    Ok(created) => created,
                    Err(error) => {
                        let inserted: Vec<_> =
                            self.bodies.drain(original_len..).map(|body| body.handle).collect();
                        for handle in inserted {
                            self.world.remove_body(handle);
                        }
                        return Err(error);
                    }
                };
                self.bodies[new_index].render = render;
                let new_origin = self.bodies[new_index].origin;
                let dx = new_origin[0] - origin[0];
                let dy = new_origin[1] - origin[1];
                if let Some(created) = self.world.bodies.get_mut(self.bodies[new_index].handle) {
                    created.set_rotation(Rotation::new(angle), true);
                    created.set_translation(
                        Vector::new(
                            translation.x + (cos * dx - sin * dy) / CELLS_PER_METER,
                            translation.y + (sin * dx + cos * dy) / CELLS_PER_METER,
                        ),
                        true,
                    );
                    created.set_linvel(linear, true);
                    created.set_angvel(angular, true);
                }
            }
            stamp::unstamp(&mut self.bodies[index], world);
            let old = self.bodies.remove(index);
            self.world.remove_body(old.handle);
            self.particles.extend(particles);
        }
        Ok(())
    }
}
