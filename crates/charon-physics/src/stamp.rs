//! 仕様 05 §3: 剛体の逆写像スタンプとアンスタンプ。

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::world::World;
use rapier2d::prelude::PhysicsWorld;

use crate::displace;
use crate::particle::Particle;
use crate::{CELLS_PER_METER, PixelBody};

#[derive(Clone, Copy)]
pub(crate) struct StampTransform {
    pub translation: [f32; 2],
    sin: f32,
    cos: f32,
}

impl StampTransform {
    pub(crate) fn new(translation: [f32; 2], angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self { translation, sin, cos }
    }

    pub(crate) fn local_index(self, body: &PixelBody, pos: CellPos) -> Option<usize> {
        let dx = pos.x as f32 + 0.5 - self.translation[0];
        let dy = pos.y as f32 + 0.5 - self.translation[1];
        let bx = (self.cos * dx + self.sin * dy + body.origin[0]).floor() as i32;
        let by = (-self.sin * dx + self.cos * dy + body.origin[1]).floor() as i32;
        if bx < 0 || bx >= i32::from(body.width) || by < 0 || by >= i32::from(body.height) {
            return None;
        }
        Some(by as usize * usize::from(body.width) + bx as usize)
    }

    /// 押しのけ先が今からスタンプされる位置か判定する。
    pub(crate) fn occupies(self, body: &PixelBody, pos: CellPos) -> bool {
        self.local_index(body, pos)
            .is_some_and(|index| body.cells[index].material != MaterialId::EMPTY)
    }

    fn aabb(self, body: &PixelBody) -> ([i32; 2], [i32; 2]) {
        let corners = [
            [0.0, 0.0],
            [f32::from(body.width), 0.0],
            [0.0, f32::from(body.height)],
            [f32::from(body.width), f32::from(body.height)],
        ];
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for [x, y] in corners {
            let x = x - body.origin[0];
            let y = y - body.origin[1];
            let point = [
                self.translation[0] + self.cos * x - self.sin * y,
                self.translation[1] + self.sin * x + self.cos * y,
            ];
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        (
            [min[0].floor() as i32, min[1].floor() as i32],
            [max[0].ceil() as i32, max[1].ceil() as i32],
        )
    }
}

/// 変換した四隅の AABB を走査し、各ワールドセルの中心を剛体のローカル座標へ戻す。
pub(crate) fn stamp(
    body: &mut PixelBody,
    physics: &mut PhysicsWorld,
    particles: &mut Vec<Particle>,
    world: &mut World,
) {
    let Some(rigid) = physics.bodies.get(body.handle) else { return };
    let translation = rigid.translation() * CELLS_PER_METER;
    let transform = StampTransform::new([translation.x, translation.y], rigid.rotation().angle());
    let linear = rigid.linvel() * (CELLS_PER_METER / 60.0);
    let linear = [linear.x, linear.y];
    let angular = rigid.angvel() / 60.0;
    let mut impulses = Vec::new();
    let mut context = displace::DisplaceContext {
        transform,
        linear,
        angular,
        impulses: &mut impulses,
        particles,
    };
    let (min, max) = transform.aabb(body);
    for wy in min[1]..max[1] {
        for wx in min[0]..max[0] {
            let pos = CellPos::new(wx, wy);
            let Some(index) = transform.local_index(body, pos) else { continue };
            let cell = body.cells[index];
            if cell.material == MaterialId::EMPTY {
                continue;
            }
            let current = world.get(pos);
            if current.material == MaterialId::BOUNDARY || current.has(FLAG_BODY) {
                continue;
            }
            if current.material != MaterialId::EMPTY
                && !displace::displace(current, pos, body, &mut context, world)
            {
                continue;
            }
            world.set(pos, cell.with(FLAG_BODY, true));
            body.stamped.push(pos);
        }
    }
    // スタンプ中の進行方向はスナップショットで固定し、反力は次の物理 tick に効かせる。
    if let Some(rigid) = physics.bodies.get_mut(body.handle) {
        for (impulse, point) in impulses {
            rigid.apply_impulse_at_point(impulse, point, true);
        }
    }
}

/// 記録した座標だけを消す。セルシミュレーションが変えた場所には触れない。
pub(crate) fn unstamp(body: &mut PixelBody, world: &mut World) {
    for pos in body.stamped.drain(..) {
        if world.get(pos).has(FLAG_BODY) {
            world.set(pos, Cell::default());
        }
    }
}
