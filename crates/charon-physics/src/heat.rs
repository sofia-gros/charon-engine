//! 仕様 07 §5: 現在のスタンプと隣の前景セルで、仕様 03 §2 の式を使う。

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::world::World;

use crate::{CELLS_PER_METER, PhysicsEngine, stamp::StampTransform};

const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

impl PhysicsEngine {
    /// ボディ順・ローカルセルの行優先順で現在のスタンプに接する熱を交換する。
    pub fn exchange_body_heat(&mut self, world: &mut World) {
        for body in &mut self.bodies {
            let Some(rigid) = self.world.bodies.get(body.handle) else { continue };
            let translation = rigid.translation() * CELLS_PER_METER;
            let transform =
                StampTransform::new([translation.x, translation.y], rigid.rotation().angle());
            let mut stamped: Vec<_> = body
                .stamped
                .iter()
                .filter_map(|&pos| transform.local_index(body, pos).map(|index| (index, pos)))
                .collect();
            stamped.sort_by_key(|&(index, pos)| (index, pos.y, pos.x));
            let mut changed = false;
            for (index, pos) in stamped {
                if !world.get(pos).has(FLAG_BODY) {
                    continue;
                }
                for (dx, dy) in DIRS {
                    let (Some(x), Some(y)) = (pos.x.checked_add(dx), pos.y.checked_add(dy)) else {
                        continue;
                    };
                    let neighbor = CellPos::new(x, y);
                    let mut other = world.get(neighbor);
                    if other.material == MaterialId::BOUNDARY || other.has(FLAG_BODY) {
                        continue;
                    }
                    let cell = &mut body.cells[index];
                    let k = i32::from(
                        world
                            .materials()
                            .get(cell.material)
                            .conductivity
                            .min(world.materials().get(other.material).conductivity),
                    );
                    let transfer = (i32::from(other.temp) - i32::from(cell.temp)) * k / 1024;
                    if transfer == 0 {
                        continue;
                    }
                    let low = (i32::from(i16::MIN) - i32::from(cell.temp))
                        .max(i32::from(other.temp) - i32::from(i16::MAX));
                    let high = (i32::from(i16::MAX) - i32::from(cell.temp))
                        .min(i32::from(other.temp) - i32::from(i16::MIN));
                    let transfer = transfer.clamp(low, high);
                    if transfer == 0 {
                        continue;
                    }
                    cell.temp = (i32::from(cell.temp) + transfer) as i16;
                    other.temp = (i32::from(other.temp) - transfer) as i16;
                    world.set(neighbor, other);
                    changed = true;
                }
            }
            // 起きた剛体はこの tick のアンスタンプ・スタンプで新しい温度を反映する。
            if changed && let Some(rigid) = self.world.bodies.get_mut(body.handle) {
                rigid.wake_up(true);
            }
        }
    }
}
