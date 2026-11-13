//! 仕様 05 §8: チャンクごとの静的な地形コライダー。

use std::collections::BTreeMap;

use charon_core::cell::{Cell, FLAG_BODY, FLAG_FALLING, MaterialId};
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_SIZE, ChunkPos};
use charon_core::material::{MaterialTable, Phase};
use charon_core::world::World;
use rapier2d::prelude::{ColliderBuilder, RigidBodyBuilder, RigidBodyHandle, Vector};

use crate::{CELLS_PER_METER, PhysicsEngine, contour};

/// ボディの AABB と、チャンクをセル単位から換算した範囲。
#[derive(Clone, Copy)]
struct BodyBounds {
    handle: RigidBodyHandle,
    min: ChunkPos,
    max: ChunkPos,
    sleeping: bool,
}

fn distance(pos: ChunkPos, bounds: BodyBounds) -> i64 {
    let x = i64::from(pos.x);
    let y = i64::from(pos.y);
    (i64::from(bounds.min.x) - x)
        .max(x - i64::from(bounds.max.x))
        .max(0)
        .max((i64::from(bounds.min.y) - y).max(y - i64::from(bounds.max.y)).max(0))
}

/// 地形だけを選び、最も多い材質の摩擦係数と閉じた輪郭を返す。
fn shape(chunk: &Chunk, materials: &MaterialTable) -> Option<(ColliderBuilder, f32)> {
    let mut cells = Vec::with_capacity(chunk.cells.len());
    let mut counts = BTreeMap::<MaterialId, usize>::new();
    for &cell in chunk.cells.iter() {
        let collidable = !cell.has(FLAG_BODY)
            && match materials.get(cell.material).phase {
                Phase::Solid => true,
                Phase::Powder => !cell.has(FLAG_FALLING) || cell.has(charon_core::cell::FLAG_BLOCK),
                _ => false,
            };
        if collidable {
            *counts.entry(cell.material).or_default() += 1;
            cells.push(cell);
        } else {
            cells.push(Cell::default());
        }
    }
    let (&majority, _) = counts.iter().max_by_key(|(_, count)| *count)?;
    let contours = contour::trace(&cells, CHUNK_SIZE, CHUNK_SIZE).ok()?;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for contour in contours {
        let start = vertices.len() as u32;
        for (x, y) in contour {
            vertices.push(Vector::new(x as f32 / CELLS_PER_METER, y as f32 / CELLS_PER_METER));
        }
        let end = vertices.len() as u32;
        for index in start..end {
            indices.push([index, if index + 1 == end { start } else { index + 1 }]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    Some((
        ColliderBuilder::polyline(vertices, Some(indices)),
        materials.get(majority).rigid_friction,
    ))
}

impl PhysicsEngine {
    /// 起きている剛体に近い dirty チャンクを距離順に最大 4 個更新する。
    /// 地面が掘られた場合、そこに乗って眠っている剛体も起こす。
    pub fn update_terrain(&mut self, world: &mut World) {
        // ワールドから退避されたチャンクの静的剛体を残さない。
        self.terrain.retain(|pos, handle| {
            if world.chunk(*pos).is_some() {
                true
            } else {
                if let Some(handle) = handle {
                    self.world.remove_body(*handle);
                }
                false
            }
        });
        let mut bounds = Vec::new();
        for body in &self.bodies {
            let Some(rigid) = self.world.bodies.get(body.handle) else { continue };
            let Some(collider_handle) = rigid.colliders().first() else { continue };
            let Some(collider) = self.world.colliders.get(*collider_handle) else { continue };
            let aabb = collider.compute_aabb();
            let chunk = |value: f32| (value * CELLS_PER_METER).floor() as i32 >> 6;
            bounds.push(BodyBounds {
                handle: body.handle,
                min: ChunkPos::new(chunk(aabb.mins.x), chunk(aabb.mins.y)),
                max: ChunkPos::new(chunk(aabb.maxs.x), chunk(aabb.maxs.y)),
                sleeping: rigid.is_sleeping(),
            });
        }
        let mut candidates = Vec::new();
        for pos in world.chunk_positions() {
            let Some(chunk) = world.chunk(pos) else { continue };
            let needs_build = chunk.collider_dirty || !self.terrain.contains_key(&pos);
            if !needs_build {
                continue;
            }
            let mut nearest = None;
            for body in &bounds {
                let d = distance(pos, *body);
                if d > 1 {
                    continue;
                }
                if body.sleeping {
                    self.world.wake_up(body.handle, true);
                }
                nearest = Some(nearest.map_or(d, |current: i64| current.min(d)));
            }
            if let Some(d) = nearest {
                candidates.push((d, pos));
            }
        }
        candidates.sort_unstable();
        for (_, pos) in candidates.into_iter().take(4) {
            let Some(chunk) = world.chunk(pos) else { continue };
            let geometry = shape(chunk, world.materials());
            if let Some(Some(old)) = self.terrain.remove(&pos) {
                self.world.remove_body(old);
            }
            let handle = geometry.map(|(collider, friction)| {
                let origin = pos.origin();
                let rigid = RigidBodyBuilder::fixed().translation(Vector::new(
                    origin.x as f32 / CELLS_PER_METER,
                    origin.y as f32 / CELLS_PER_METER,
                ));
                self.world.insert(rigid, collider.friction(friction)).0
            });
            self.terrain.insert(pos, handle);
            world.clear_collider_dirty(pos);
        }
    }
}
