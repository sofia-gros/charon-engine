//! 仕様 05 §7: セル配列から剛体の質量・重心と三角形コライダーを作る。

mod contour;
mod damage;
mod displace;
mod explosion;
mod forces;
mod heat;
mod particle;
pub mod rope;
mod save;
mod stamp;
mod terrain;

pub use explosion::{ExplosionError, explode};
pub use particle::{PARTICLE_LIMIT, Particle};
pub use save::BodySaveError;

use std::collections::BTreeMap;
use std::fmt;

use charon_core::cell::{Cell, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::MaterialTable;
use rapier2d::prelude::{
    ColliderBuilder, MassProperties, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle, Vector,
};

pub const CELLS_PER_METER: f32 = 16.0;
pub const CELL_MASS_SCALE: f32 = 1.0 / (1000.0 * CELLS_PER_METER * CELLS_PER_METER);

/// セル配列と物理エンジンの剛体を結び付ける。origin はセル単位の重心。
#[derive(Debug)]
pub struct PixelBody {
    pub handle: RigidBodyHandle,
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,
    pub stamped: Vec<CellPos>,
    pub origin: [f32; 2],
    damaged: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BodyError {
    InvalidDimensions,
    Empty,
    InvalidMaterial(MaterialId),
    InvalidContour,
    Triangulation,
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions => write!(f, "剛体の幅・高さとセル数が一致しません"),
            Self::Empty => write!(f, "剛体に空でないセルがありません"),
            Self::InvalidMaterial(id) => write!(f, "剛体の材質 ID {} が無効です", id.0),
            Self::InvalidContour => write!(f, "剛体の輪郭を閉じられません"),
            Self::Triangulation => write!(f, "剛体の輪郭を三角形に分割できません"),
        }
    }
}

impl std::error::Error for BodyError {}

/// rapier のシミュレーションと挿入順に並べたピクセル剛体。
pub struct PhysicsEngine {
    pub world: PhysicsWorld,
    pub bodies: Vec<PixelBody>,
    particles: Vec<Particle>,
    terrain: BTreeMap<charon_core::coords::ChunkPos, Option<RigidBodyHandle>>,
}

impl fmt::Debug for PhysicsEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicsEngine").field("bodies", &self.bodies).finish_non_exhaustive()
    }
}

impl Default for PhysicsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicsEngine {
    pub fn new() -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, 9.81 * 4.0);
        world.integration_parameters.dt = 1.0 / 60.0;
        Self { world, bodies: Vec::new(), particles: Vec::new(), terrain: BTreeMap::new() }
    }

    /// セルごとの質量を積算し、輪郭を簡略化・三角形分割して剛体を追加する。
    pub fn create_body(
        &mut self,
        width: u16,
        height: u16,
        cells: Vec<Cell>,
        materials: &MaterialTable,
        position: [f32; 2],
    ) -> Result<usize, BodyError> {
        if width == 0 || height == 0 || cells.len() != usize::from(width) * usize::from(height) {
            return Err(BodyError::InvalidDimensions);
        }
        let mut total_mass = 0.0;
        let mut weighted = [0.0, 0.0];
        let mut friction = 0.0;
        let mut restitution = 0.0;
        let mut count = 0u32;
        for (i, cell) in cells.iter().enumerate() {
            if cell.material == MaterialId::EMPTY {
                continue;
            }
            if usize::from(cell.material.0) >= materials.len() {
                return Err(BodyError::InvalidMaterial(cell.material));
            }
            let mat = materials.get(cell.material);
            let mass = f32::from(mat.density) * CELL_MASS_SCALE;
            let x = (i % usize::from(width)) as f32 + 0.5;
            let y = (i / usize::from(width)) as f32 + 0.5;
            total_mass += mass;
            weighted[0] += mass * x;
            weighted[1] += mass * y;
            friction += mat.rigid_friction;
            restitution += mat.restitution;
            count += 1;
        }
        if count == 0 || total_mass <= 0.0 {
            return Err(BodyError::Empty);
        }
        let origin = [weighted[0] / total_mass, weighted[1] / total_mass];
        let mut inertia = 0.0;
        for (i, cell) in cells.iter().enumerate() {
            if cell.material == MaterialId::EMPTY {
                continue;
            }
            let mass = f32::from(materials.get(cell.material).density) * CELL_MASS_SCALE;
            let dx = ((i % usize::from(width)) as f32 + 0.5 - origin[0]) / CELLS_PER_METER;
            let dy = ((i / usize::from(width)) as f32 + 0.5 - origin[1]) / CELLS_PER_METER;
            inertia += mass * (dx * dx + dy * dy + 1.0 / (6.0 * CELLS_PER_METER.powi(2)));
        }
        let triangles = contour::triangulate(&cells, width, height, origin)?;
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0] / CELLS_PER_METER, position[1] / CELLS_PER_METER))
            .additional_mass_properties(MassProperties::new(Vector::ZERO, total_mass, inertia));
        let collider = ColliderBuilder::compound(triangles)
            .density(0.0)
            .friction(friction / count as f32)
            .restitution(restitution / count as f32);
        let (handle, _) = self.world.insert(body, collider);
        // rapier は挿入直後の質量を次の physics step まで遅延更新する。
        if let Some(inserted) = self.world.bodies.get_mut(handle) {
            inserted.recompute_mass_properties_from_colliders(&self.world.colliders);
        }
        let index = self.bodies.len();
        self.bodies.push(PixelBody {
            handle,
            width,
            height,
            cells,
            stamped: Vec::new(),
            origin,
            damaged: false,
        });
        Ok(index)
    }

    /// 手でつかんだ剛体を固定し、重力と慣性を一時的に止める。
    pub fn begin_grab(&mut self, index: usize) {
        if let Some(body) = self.bodies.get(index)
            && let Some(rigid) = self.world.bodies.get_mut(body.handle)
        {
            rigid.set_gravity_scale(0.0, true);
            rigid.set_linvel(Vector::ZERO, true);
            rigid.set_angvel(0.0, true);
        }
    }

    /// つかんだ剛体の重心をセル座標に移す。
    pub fn drag_body(&mut self, index: usize, center: [f32; 2]) {
        if let Some(body) = self.bodies.get(index)
            && let Some(rigid) = self.world.bodies.get_mut(body.handle)
        {
            rigid.set_translation(
                Vector::new(center[0] / CELLS_PER_METER, center[1] / CELLS_PER_METER),
                true,
            );
            rigid.set_linvel(Vector::ZERO, true);
        }
    }

    /// つかんだ剛体を指定角度だけ回す。
    pub fn rotate_body(&mut self, index: usize, delta: f32) {
        if let Some(body) = self.bodies.get(index)
            && let Some(rigid) = self.world.bodies.get_mut(body.handle)
        {
            rigid.set_rotation(
                rapier2d::prelude::Rotation::new(rigid.rotation().angle() + delta),
                true,
            );
            rigid.set_angvel(0.0, true);
        }
    }

    /// つかんだ剛体を離し、重力を戻す。
    pub fn end_grab(&mut self, index: usize) {
        if let Some(body) = self.bodies.get(index)
            && let Some(rigid) = self.world.bodies.get_mut(body.handle)
        {
            rigid.set_gravity_scale(1.0, true);
        }
    }

    /// 格子にないパーティクルを作る。上限に達していれば追加しない。
    pub fn spawn_particle(&mut self, particle: Particle) -> bool {
        if self.particles.len() >= PARTICLE_LIMIT {
            return false;
        }
        self.particles.push(particle);
        true
    }

    /// 現在飛んでいるパーティクル。挿入順を保つ。
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// パーティクルを順に DDA で動かし、衝突したらセルに戻す。
    pub fn update_particles(&mut self, world: &mut charon_core::world::World) {
        particle::update(&mut self.particles, world);
    }

    /// 仕様 05 §2 の順序で 1 tick 進める。入力は呼び出す直前に適用する。
    pub fn step(&mut self, world: &mut charon_core::world::World) -> Result<(), BodyError> {
        self.split_damaged(world)?;
        self.apply_cell_forces(world);
        self.exchange_body_heat(world);
        let active: Vec<_> = self
            .bodies
            .iter()
            .map(|body| {
                self.world.bodies.get(body.handle).is_some_and(|rigid| !rigid.is_sleeping())
            })
            .collect();
        for (body, &awake) in self.bodies.iter_mut().zip(&active) {
            if awake {
                stamp::unstamp(body, world);
            }
        }
        self.world.step();
        for (body, &was_awake) in self.bodies.iter_mut().zip(&active) {
            let now_awake =
                self.world.bodies.get(body.handle).is_some_and(|rigid| !rigid.is_sleeping());
            if was_awake || now_awake {
                if !was_awake {
                    stamp::unstamp(body, world);
                }
                stamp::stamp(body, &mut self.world, &mut self.particles, world);
            }
        }
        world.step();
        self.update_particles(world);
        self.update_terrain(world);
        Ok(())
    }

    /// 全剛体を添字順にスタンプする。移動前に `unstamp_all` を呼ぶ。
    pub fn stamp_all(&mut self, world: &mut charon_core::world::World) {
        for body in &mut self.bodies {
            stamp::stamp(body, &mut self.world, &mut self.particles, world);
        }
    }

    /// 全剛体のスタンプを添字順に取り除く。
    pub fn unstamp_all(&mut self, world: &mut charon_core::world::World) {
        for body in &mut self.bodies {
            stamp::unstamp(body, world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_and_l_shape_mass_and_centroid() {
        for _seed in 1..=8 {
            let materials = MaterialTable::builtin().unwrap();
            let stone = materials.id("stone").unwrap();
            let cell = Cell { material: stone, ..Cell::default() };
            let mut engine = PhysicsEngine::new();
            let square = engine.create_body(4, 4, vec![cell; 16], &materials, [0.0, 0.0]).unwrap();
            let square = &engine.bodies[square];
            assert!((square.origin[0] - 2.0).abs() < 0.0001);
            assert!((square.origin[1] - 2.0).abs() < 0.0001);
            assert!(
                (engine.world.bodies[square.handle].mass() - 16.0 * 2600.0 * CELL_MASS_SCALE).abs()
                    < 0.0001
            );

            let mut l = vec![Cell::default(); 12];
            for &index in &[0, 1, 2, 3, 4, 8] {
                l[index] = cell;
            }
            let shape = engine.create_body(4, 3, l, &materials, [0.0, 0.0]).unwrap();
            let shape = &engine.bodies[shape];
            assert!((shape.origin[0] - 1.5).abs() < 0.0001);
            assert!((shape.origin[1] - 1.0).abs() < 0.0001);
            assert!(
                (engine.world.bodies[shape.handle].mass() - 6.0 * 2600.0 * CELL_MASS_SCALE).abs()
                    < 0.0001
            );
            assert_eq!(engine.world.bodies[shape.handle].colliders().len(), 1);
        }
    }
}
