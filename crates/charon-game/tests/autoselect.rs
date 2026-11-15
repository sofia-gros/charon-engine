//! 仕様 28 §8: オートセレクトの道具選び。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::autoselect::best_tool_for;
use charon_game::blocks::BlockPos;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::items::ItemId;
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

struct Setup {
    defs: Definitions,
    world: World,
    game: GameState,
    player: EntityId,
}

fn setup() -> Setup {
    let materials = Arc::new(MaterialTable::builtin().unwrap());
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let mut world = World::new(Arc::clone(&materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 60, 63, 60, '#');
    let mut entities = EntityWorld::new();
    let player = entities.spawn_player(10 * 16, 26 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    Setup { defs, world, game, player }
}

impl Setup {
    fn give(&mut self, name: &str) -> ItemId {
        let item = self.defs.item_id(name, self.world.materials()).unwrap();
        self.game.inventories.get_mut(&self.player).unwrap().add_item(item, 1);
        item
    }

    fn best(&self, block: BlockPos) -> Option<ItemId> {
        best_tool_for(&self.world, &self.game, self.player, &self.defs, block)
    }
}

/// 石は力が最大のつるはし、木は斧。ホットバーの選択は変わらず、解除すると元に戻る。
#[test]
fn auto_select_picks_the_best_tool_for_the_target() {
    let mut s = setup();
    let wood_pick = s.give("wood_pickaxe");
    let iron_pick = s.give("iron_pickaxe");
    let stone_pick = s.give("stone_pickaxe");
    let axe = s.give("wood_axe");
    let sword = s.give("wood_sword");
    testkit::place_block(&mut s.world, 2, 2, '#');
    testkit::place_block(&mut s.world, 4, 2, 'W');
    let stone = BlockPos { bx: 2, by: 2 };
    let wood = BlockPos { bx: 4, by: 2 };
    assert_eq!(s.best(stone), Some(iron_pick), "石: つるはしの力が最大");
    assert_ne!(Some(wood_pick), s.best(stone));
    assert_ne!(Some(stone_pick), s.best(stone));
    assert_eq!(s.best(wood), Some(axe), "木: 斧");
    // ホットバーの選択は、そのまま。オートセレクトのアイテムが優先され、解除すると戻る。
    let inventory = s.game.inventories.get_mut(&s.player).unwrap();
    inventory.pin(0, sword);
    inventory.select(0);
    let hash_before = inventory.state_hash();
    inventory.set_auto_item(Some(iron_pick));
    assert_eq!(inventory.selected_item(), Some(iron_pick));
    assert_eq!(inventory.selected(), 0, "選んでいる枠は変わらない");
    assert_eq!(inventory.hotbar(0), Some(sword));
    assert_ne!(inventory.state_hash(), hash_before, "選んでいるものが変わるので、ハッシュに出る");
    inventory.set_auto_item(None);
    assert_eq!(inventory.selected_item(), Some(sword));
    assert_eq!(inventory.state_hash(), hash_before, "解除したら、元のハッシュ");
    // 持っていないアイテムは選べない。
    inventory.set_auto_item(Some(ItemId(12_345)));
    assert_eq!(inventory.auto_item(), None);
}

/// 空き・不完全なブロック・合う道具がないときは、何も選ばない。
#[test]
fn auto_select_ignores_empty_targets_and_missing_tools() {
    let mut s = setup();
    testkit::place_block(&mut s.world, 4, 2, 'W');
    testkit::place_block(&mut s.world, 2, 2, '#');
    let wood = BlockPos { bx: 4, by: 2 };
    let stone = BlockPos { bx: 2, by: 2 };
    assert_eq!(s.best(stone), None, "道具がない");
    s.give("wood_sword");
    assert_eq!(s.best(stone), None, "武器は道具ではない");
    s.give("wood_pickaxe");
    assert!(s.best(stone).is_some());
    assert_eq!(s.best(wood), None, "斧がないと、木には何も選ばない (つるはしの力では壊せない)");
    // 空き。
    assert_eq!(s.best(BlockPos { bx: 6, by: 2 }), None, "空き");
    // 不完全なブロック (1 セル欠けた) は壊せない。
    let origin = stone.origin().unwrap();
    s.world.set(origin, s.world.make_cell(charon_core::cell::MaterialId::EMPTY, origin));
    assert_eq!(s.best(stone), None, "欠けたブロック");
    // 読み込まれていない場所。
    assert_eq!(s.best(BlockPos { bx: 500, by: 500 }), None);
}
