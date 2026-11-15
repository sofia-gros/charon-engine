//! 仕様 28 §7: クイック回復・クイックマナ。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::items::ItemId;
use charon_game::quick::{pick_quick_heal, pick_quick_mana};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

struct Setup {
    defs: Definitions,
    materials: Arc<MaterialTable>,
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
    Setup { defs, materials, game, player }
}

/// 足りない体力を回復しきれるもののうち最小、なければ最大。同点は `ItemId` の小さいほう。
#[test]
fn quick_heal_chooses_the_least_wasteful_potion() {
    let (small, medium, large, twin) = (ItemId(1), ItemId(2), ItemId(3), ItemId(4));
    let options = [(small, 20), (medium, 50), (large, 100), (twin, 50)];
    assert_eq!(pick_quick_heal(10, &options), Some(small), "20 で足りる最小");
    assert_eq!(pick_quick_heal(20, &options), Some(small), "ちょうど足りる");
    assert_eq!(pick_quick_heal(21, &options), Some(medium), "50 で足りる最小。同点は ID が小さい");
    assert_eq!(pick_quick_heal(80, &options), Some(large), "100 でないと足りない");
    // 回復しきれるものがなければ、最大。同点は ID が小さい。
    assert_eq!(pick_quick_heal(500, &options), Some(large));
    assert_eq!(pick_quick_heal(500, &[(twin, 50), (medium, 50)]), Some(medium));
    // 満タン・回復量 0 だけ・何もないときは選ばない。
    assert_eq!(pick_quick_heal(0, &options), None);
    assert_eq!(pick_quick_heal(-5, &options), None);
    assert_eq!(pick_quick_heal(30, &[(small, 0)]), None);
    assert_eq!(pick_quick_heal(30, &[]), None);
}

/// マナは、`ItemId` が最小の回復アイテム (持ち物の左上から最初)。
#[test]
fn quick_mana_uses_the_first_mana_potion() {
    let (a, b, c) = (ItemId(7), ItemId(3), ItemId(5));
    assert_eq!(pick_quick_mana(&[(a, 50), (b, 20), (c, 0)]), Some(b));
    assert_eq!(pick_quick_mana(&[(c, 0)]), None, "回復量 0 は選ばない");
    assert_eq!(pick_quick_mana(&[]), None);
}

/// 満タン・持っていない・間違ったアイテムでは、何も変えない。使うと 1 個減って回復する。
#[test]
fn quick_heal_and_mana_do_nothing_when_full_or_empty() {
    let mut s = setup();
    let heal = s.defs.item_id("heal_potion", &s.materials).unwrap();
    let mana = s.defs.item_id("mana_potion", &s.materials).unwrap();
    let sword = s.defs.item_id("wood_sword", &s.materials).unwrap();
    // 持っていないときは選ばない。
    assert_eq!(s.game.quick_heal_item(s.player, &s.defs), None);
    assert_eq!(s.game.quick_mana_item(s.player, &s.defs), None);
    let inventory = s.game.inventories.get_mut(&s.player).unwrap();
    inventory.add_item(heal, 2);
    inventory.add_item(mana, 2);
    inventory.add_item(sword, 1);
    // 満タンなら選ばない。
    assert_eq!(s.game.quick_heal_item(s.player, &s.defs), None);
    assert_eq!(s.game.quick_mana_item(s.player, &s.defs), None);
    let before = s.game.state_hash();
    assert!(!s.game.use_consumable(s.player, sword, &s.defs), "武器は消耗品ではない");
    assert_eq!(s.game.state_hash(), before, "失敗したら何も変えない");
    // 傷ついて、マナを使うと、選ばれる。
    s.game.vitals.get_mut(s.player).unwrap().hp = 30;
    s.game.wands.get_mut(&s.player).unwrap().mana = 0;
    assert_eq!(s.game.quick_heal_item(s.player, &s.defs), Some(heal));
    assert_eq!(s.game.quick_mana_item(s.player, &s.defs), Some(mana));
    assert!(s.game.use_consumable(s.player, heal, &s.defs));
    assert_eq!(s.game.vitals.get(s.player).unwrap().hp, 80);
    assert_eq!(s.game.inventories[&s.player].item_count(heal), 1);
    assert!(s.game.use_consumable(s.player, mana, &s.defs));
    assert!(s.game.wands[&s.player].mana > 0);
    // 最後の 1 個を使うと、なくなって選ばれない。
    assert!(s.game.use_consumable(s.player, heal, &s.defs));
    assert_eq!(s.game.vitals.get(s.player).unwrap().hp, MAX_HP);
    assert_eq!(s.game.inventories[&s.player].item_count(heal), 0);
    assert!(!s.game.use_consumable(s.player, heal, &s.defs), "持っていない");
    s.game.vitals.get_mut(s.player).unwrap().hp = 10;
    assert_eq!(s.game.quick_heal_item(s.player, &s.defs), None, "なくなった");
    // 倒れているキャラクターには使わない。
    s.game.vitals.get_mut(s.player).unwrap().hp = 0;
    assert_eq!(s.game.quick_mana_item(s.player, &s.defs), None);
    assert!(!s.game.use_consumable(s.player, mana, &s.defs));
}
