//! 仕様 28 §10: お気に入りの保護・ゴミ箱・クイックスタック。整数だけ。

use charon_core::coords::CellPos;
use charon_core::world::World;

use crate::blocks::BlockPos;
use crate::defs::Definitions;
use crate::game::GameState;
use crate::items::ItemId;
use crate::{EntityId, EntityWorld};

/// クイックスタックが届く距離 (セル)。半径 37.5 タイル。
pub const QUICK_STACK_RADIUS: i64 = 300;

impl GameState {
    /// ゴミ箱に入れる: `item` のスタックを全部、持ち物から消す。お気に入り・持っていないものは何もせず `false`。
    /// 前に入っていたものは戻らず、これで置き換わる (最後に捨てたものだけ覚える)。
    pub fn trash_item(&mut self, id: EntityId, item: ItemId) -> bool {
        let Some(inventory) = self.inventories.get_mut(&id) else { return false };
        let count = inventory.item_count(item);
        if count == 0 || inventory.is_favorite(item) || !inventory.take_item(item, count) {
            return false;
        }
        self.trash.insert(id, (item, count));
        true
    }

    /// クイックスタック: 持ち物のアイテムのうち、同じアイテムがすでに入っている箱があるものを、その箱へ全部移す。
    ///
    /// - 対象の箱: プレイヤーから `QUICK_STACK_RADIUS` セル以内の有効な箱。`only` があれば、その箱だけ。
    /// - 複数の箱に同じアイテムがあれば、**世界で低い (`by` が大きい)** 箱へ。同じ高さなら `bx` が小さいほう。
    /// - お気に入り・ホットバーに印したアイテムは移さない (装備中のものは持ち物にない)。
    /// - 箱が入りきらないなど、移せなかったアイテムはそのまま。移したアイテムの種類の数を返す。
    pub fn quick_stack(
        &mut self,
        world: &World,
        entities: &EntityWorld,
        id: EntityId,
        defs: &Definitions,
        only: Option<BlockPos>,
    ) -> u32 {
        self.quick_stack_within(world, entities, id, defs, only, QUICK_STACK_RADIUS)
    }

    /// `quick_stack` の、届く距離 (セル) を指定する版。
    pub fn quick_stack_within(
        &mut self,
        world: &World,
        entities: &EntityWorld,
        id: EntityId,
        defs: &Definitions,
        only: Option<BlockPos>,
        radius: i64,
    ) -> u32 {
        let Some(center) = crate::cursor::center_of(entities, id) else { return 0 };
        // 届く範囲の有効な箱と、中身のアイテムの名前。
        let chests: Vec<(BlockPos, Vec<String>)> = self
            .fixtures
            .iter()
            .filter(|(pos, _)| only.is_none_or(|open| open == **pos))
            .filter(|(pos, _)| within(center, **pos, radius))
            .filter(|(pos, _)| crate::autoselect::is_active_chest(world, self, defs, **pos))
            .map(|(&pos, fixture)| {
                (
                    pos,
                    fixture
                        .contents
                        .iter()
                        .filter(|(_, n)| **n > 0)
                        .map(|(k, _)| k.clone())
                        .collect(),
                )
            })
            .collect();
        let Some(inventory) = self.inventories.get(&id) else { return 0 };
        let pinned: Vec<ItemId> =
            (0..crate::inventory::HOTBAR_SLOTS).filter_map(|slot| inventory.hotbar(slot)).collect();
        let candidates: Vec<(ItemId, u32)> = inventory
            .item_iter()
            .filter(|&(item, count)| {
                count > 0 && !inventory.is_favorite(item) && !pinned.contains(&item)
            })
            .collect();
        let mut moved = 0;
        for (item, count) in candidates {
            let Some(name) = defs.item_name(item, world.materials()) else { continue };
            // 低い箱を優先 (by が大きい、同じなら bx が小さい)。
            let target = chests
                .iter()
                .filter(|(_, names)| names.contains(&name))
                .max_by_key(|(pos, _)| (pos.by, std::cmp::Reverse(pos.bx)))
                .map(|(pos, _)| *pos);
            if let Some(pos) = target
                && self.transfer_to_chest(world, id, pos, item, count, defs).is_ok()
            {
                moved += 1;
            }
        }
        moved
    }
}

/// 箱のブロックの中心が、`center` から `radius` セル以内か。
fn within(center: CellPos, block: BlockPos, radius: i64) -> bool {
    let Some(origin) = block.origin() else { return false };
    let (dx, dy) = (
        i64::from(origin.x + 4) - i64::from(center.x),
        i64::from(origin.y + 4) - i64::from(center.y),
    );
    dx * dx + dy * dy <= radius * radius
}
