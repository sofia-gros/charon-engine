//! 仕様 06 §8: seed とブラシ操作の列を保存し、同じ結果を再生する。

use charon_core::cell::MaterialId;
use charon_core::coords::CellPos;
use charon_core::world::World;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BrushOp {
    pub tick: u64,
    pub x: i32,
    pub y: i32,
    pub material: u16,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Replay {
    pub seed: u64,
    pub tick: u64,
    pub state_hash: u64,
    pub operations: Vec<BrushOp>,
}

impl Replay {
    pub fn play(&self, make_world: fn(u64) -> World) -> Result<World, String> {
        let mut world = make_world(self.seed);
        for operation in &self.operations {
            if operation.tick < world.tick() || operation.tick > self.tick {
                return Err("操作の tick の順番が正しくありません".into());
            }
            if usize::from(operation.material) >= world.materials().len() {
                return Err("存在しない材質 ID です".into());
            }
            while world.tick() < operation.tick {
                world.step();
            }
            let pos = CellPos::new(operation.x, operation.y);
            if world.chunk(pos.chunk()).is_none() {
                return Err("操作の対象チャンクがありません".into());
            }
            world.set(pos, world.make_cell(MaterialId(operation.material), pos));
        }
        while world.tick() < self.tick {
            world.step();
        }
        if world.state_hash() != self.state_hash {
            return Err("再生結果のハッシュが一致しません".into());
        }
        Ok(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::testkit::{box_art, material_of, world_from_ascii};

    fn make_world(seed: u64) -> World {
        world_from_ascii(&box_art(32, 32), seed)
    }

    #[test]
    fn recorded_operations_replay() {
        let mut world = make_world(7);
        let material = material_of(&world, 's');
        let pos = CellPos::new(12, 4);
        world.set(pos, world.make_cell(material, pos));
        for _ in 0..10 {
            world.step();
        }
        let replay = Replay {
            seed: 7,
            tick: world.tick(),
            state_hash: world.state_hash(),
            operations: vec![BrushOp { tick: 0, x: 12, y: 4, material: material.0 }],
        };
        let encoded = ron::ser::to_string(&replay).unwrap();
        let decoded: Replay = ron::from_str(&encoded).unwrap();
        assert_eq!(decoded.play(make_world).unwrap().state_hash(), world.state_hash());
    }
}
