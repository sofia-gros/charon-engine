//! 仕様 28 §10: お気に入り・ゴミ箱・クイックスタック。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use charon_core::testkit::{box_art, world_from_ascii};
use charon_core::world::World;
use charon_game::blocks::BlockPos;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::items::ItemId;
use charon_game::save::GameSave;
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

/// 上の箱・下の箱・遠い箱。
const UPPER: BlockPos = BlockPos { bx: 8, by: 4 };
const LOWER: BlockPos = BlockPos { bx: 10, by: 6 };
const FAR: BlockPos = BlockPos { bx: 14, by: 5 };

struct Setup {
    world: World,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    id: EntityId,
}

fn setup(seed: u64) -> Setup {
    let mut world = world_from_ascii(&box_art(130, 64), seed);
    let defs = Definitions::engine_defaults(world.materials()).unwrap();
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(2 * 16, 2 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(id, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(id).unwrap();
    let chest = defs.item_id("storage_box", world.materials()).unwrap();
    game.inventories.get_mut(&id).unwrap().add_item(chest, 3);
    for pos in [UPPER, LOWER, FAR] {
        game.place_fixture(&mut world, &mut entities, id, pos, chest, &defs).unwrap();
    }
    Setup { world, entities, game, defs, id }
}

impl Setup {
    fn sand(&self) -> ItemId {
        ItemId::cells(self.world.materials().id("sand").unwrap())
    }

    fn dirt(&self) -> ItemId {
        ItemId::cells(self.world.materials().id("dirt").unwrap())
    }

    fn inventory(&mut self) -> &mut charon_game::inventory::Inventory {
        self.game.inventories.get_mut(&self.id).unwrap()
    }

    /// 箱 `pos` に、`item` を 1 個入れておく (同じアイテムがすでに入っている箱にする)。
    fn seed_chest(&mut self, pos: BlockPos, item: ItemId) {
        self.inventory().add_item(item, 1);
        self.game.transfer_to_chest(&self.world, self.id, pos, item, 1, &self.defs).unwrap();
    }

    fn chest_count(&self, pos: BlockPos, item: ItemId) -> u32 {
        let name = self.defs.item_name(item, self.world.materials()).unwrap();
        self.game.fixtures.get(pos).unwrap().contents.get(&name).copied().unwrap_or(0)
    }

    fn quick_stack(&mut self, only: Option<BlockPos>) -> u32 {
        self.game.quick_stack(&self.world, &self.entities, self.id, &self.defs, only)
    }
}

/// お気に入りは、ゴミ箱・クイックスタックの対象外。保存して復帰できる。投げる操作の保護はアプリ側のテスト。
#[test]
fn favorite_items_are_protected_and_saved() {
    for seed in 1..=8 {
        let mut s = setup(seed);
        let (sand, dirt) = (s.sand(), s.dirt());
        s.seed_chest(LOWER, sand);
        s.seed_chest(LOWER, dirt);
        let inventory = s.inventory();
        inventory.add_item(sand, 10);
        inventory.add_item(dirt, 4);
        assert!(inventory.toggle_favorite(sand), "お気に入りにした");
        assert!(inventory.is_favorite(sand) && !inventory.is_favorite(dirt));
        // ゴミ箱もクイックスタックも、お気に入りには手を出さない。
        let before = s.game.state_hash();
        assert!(!s.game.trash_item(s.id, sand), "seed={seed}: お気に入りは捨てられない");
        assert_eq!(s.game.state_hash(), before);
        assert_eq!(s.quick_stack(None), 1, "お気に入りでない泥だけ移る");
        assert_eq!(s.inventory().item_count(sand), 10);
        assert_eq!(s.inventory().item_count(dirt), 0);
        assert_eq!(s.chest_count(LOWER, dirt), 5);
        assert_eq!(s.chest_count(LOWER, sand), 1);
        // 外すと、対象になる。
        assert!(!s.inventory().toggle_favorite(sand));
        assert_eq!(s.quick_stack(None), 1);
        assert_eq!(s.chest_count(LOWER, sand), 11);
        // お気に入りの印は、持っていなくても付けられ、ハッシュに出て、外すと元に戻る。
        let plain = s.inventory().state_hash();
        s.inventory().set_favorite(ItemId(7), true);
        assert_ne!(s.inventory().state_hash(), plain);
        s.inventory().set_favorite(ItemId(7), false);
        assert_eq!(s.inventory().state_hash(), plain);

        // 保存して、新しいゲームへ戻す。お気に入りは名前で保存する。
        let helmet = s.defs.item_id("wood_helmet", s.world.materials()).unwrap();
        s.inventory().add_item(helmet, 1);
        s.inventory().add_item(sand, 3);
        s.inventory().set_favorite(sand, true);
        s.inventory().set_favorite(helmet, true);
        let save = GameSave::capture(&s.game, &s.entities, s.id, s.world.materials(), 5)
            .with_items(&s.game, s.id, &s.defs, s.world.materials());
        let text = save.to_ron().unwrap();
        let loaded = GameSave::from_ron(&text).unwrap();
        assert_eq!(loaded, save);
        let mut favorites = loaded.player.as_ref().unwrap().favorites.clone();
        favorites.sort();
        assert_eq!(favorites, ["cells:sand", "wood_helmet"], "seed={seed}");
        let mut t = setup(seed);
        loaded.apply(&mut t.game, &mut t.entities, &t.world, t.id);
        loaded.apply_items(&mut t.game, t.id, &t.defs, t.world.materials());
        assert!(t.inventory().is_favorite(sand) && t.inventory().is_favorite(helmet));
        assert!(!t.inventory().is_favorite(dirt));
        // 古い保存 (お気に入りの欄がない) は、空で読める。
        let start = text.find("favorites:").unwrap();
        let end = start + text[start..].find("],").unwrap() + 2;
        let without = format!("{}{}", &text[..start], &text[end..]);
        let parsed = GameSave::from_ron(&without).unwrap_or_else(|e| panic!("{e}\n{without}"));
        assert!(parsed.player.unwrap().favorites.is_empty());
    }
}

/// 同じアイテムのある箱へ移す。複数あれば低い箱へ。箱にないアイテムは移さない。
#[test]
fn quick_stack_moves_matching_items_to_the_lowest_chest() {
    for seed in 1..=8 {
        let mut s = setup(seed);
        let (sand, dirt) = (s.sand(), s.dirt());
        let stone = ItemId::block(s.world.materials().id("stone").unwrap());
        // 砂は上と下の両方の箱に、泥は上の箱だけに、石のブロックはどの箱にもない。
        s.seed_chest(UPPER, sand);
        s.seed_chest(LOWER, sand);
        s.seed_chest(UPPER, dirt);
        let inventory = s.inventory();
        inventory.add_item(sand, 20);
        inventory.add_item(dirt, 7);
        inventory.add_item(stone, 3);
        assert_eq!(s.quick_stack(None), 2, "seed={seed}");
        assert_eq!(s.chest_count(LOWER, sand), 21, "砂は低い箱へ");
        assert_eq!(s.chest_count(UPPER, sand), 1);
        assert_eq!(s.chest_count(UPPER, dirt), 8, "泥は入っている箱へ");
        assert_eq!(s.inventory().item_count(stone), 3, "箱にないものはそのまま");
        assert_eq!(s.inventory().item_count(sand) + s.inventory().item_count(dirt), 0);
        assert_eq!(s.quick_stack(None), 0, "もう移すものがない");
    }
}

/// 遠い箱・ホットバーに印したもの・装備中のものは移さない。
#[test]
fn quick_stack_skips_favorites_equipment_and_far_chests() {
    let mut s = setup(1);
    let (sand, dirt) = (s.sand(), s.dirt());
    let helmet = s.defs.item_id("wood_helmet", s.world.materials()).unwrap();
    s.seed_chest(FAR, sand);
    s.seed_chest(LOWER, dirt);
    s.seed_chest(LOWER, helmet);
    let inventory = s.inventory();
    inventory.add_item(sand, 6);
    inventory.add_item(dirt, 6);
    // 箱にも入っている防具を装備する: 装備中のものは持ち物にないので、移らない。
    inventory.add_item(helmet, 1);
    let defs = s.defs.clone();
    s.game.equip(s.id, helmet, &defs).unwrap();
    // 遠い箱 (プレイヤーから約 118 セル) は、半径 100 セルの外。下の箱 (約 92 セル) は内。
    let moved = s.game.quick_stack_within(&s.world, &s.entities, s.id, &s.defs, None, 100);
    assert_eq!(moved, 1, "泥だけ");
    assert_eq!(s.chest_count(FAR, sand), 1);
    assert_eq!(s.inventory().item_count(sand), 6);
    // 半径を広げれば (既定の 300 セルなら、この世界の箱は全部届く)、遠い箱へも移る。
    assert_eq!(s.quick_stack(None), 1);
    assert_eq!(s.chest_count(FAR, sand), 7);
    // ホットバーに印したアイテムは、動かさない。
    s.seed_chest(LOWER, dirt);
    let inventory = s.inventory();
    inventory.add_item(dirt, 2);
    assert!(inventory.pin(0, dirt));
    assert_eq!(s.quick_stack(None), 0, "印したものは移さない");
    assert_eq!(s.inventory().item_count(dirt), 2);
    // 装備中のものは、持ち物にない。
    assert_eq!(s.quick_stack(None), 0);
    assert_eq!(s.chest_count(LOWER, helmet), 1, "装備中の防具は箱へ移っていない");
    assert!(s.game.equipment[&s.id].get(charon_game::items::EquipSlot::Head).is_some());
    // 箱が入りきらない (種類が上限) ときは、そのまま。
    assert_eq!(s.quick_stack(Some(BlockPos { bx: 1, by: 1 })), 0, "箱でない場所");
}

/// 箱を開いているときは、その箱だけ。
#[test]
fn quick_stack_into_the_open_chest_only() {
    let mut s = setup(1);
    let (sand, dirt) = (s.sand(), s.dirt());
    s.seed_chest(UPPER, sand);
    s.seed_chest(LOWER, sand);
    s.seed_chest(UPPER, dirt);
    let inventory = s.inventory();
    inventory.add_item(sand, 5);
    inventory.add_item(dirt, 5);
    // 上の箱を開いている: 低い箱があっても、上の箱へ。上の箱にあるものだけ移る。
    assert_eq!(s.quick_stack(Some(UPPER)), 2);
    assert_eq!(s.chest_count(UPPER, sand), 6);
    assert_eq!(s.chest_count(UPPER, dirt), 6);
    assert_eq!(s.chest_count(LOWER, sand), 1, "もう一方の箱は変わらない");
    // 下の箱だけを開いている: 下の箱にないものは移らない。
    s.inventory().add_item(dirt, 3);
    assert_eq!(s.quick_stack(Some(LOWER)), 0);
    assert_eq!(s.inventory().item_count(dirt), 3);
}

/// ゴミ箱: スタックが全部消え、前のものは戻らず置き換わる。装備・持っていないもの・お気に入りは入れられない。
#[test]
fn trash_slot_destroys_the_stack_and_replaces_the_previous() {
    let mut s = setup(1);
    let (sand, dirt) = (s.sand(), s.dirt());
    let sword = s.defs.item_id("wood_sword", s.world.materials()).unwrap();
    let inventory = s.inventory();
    inventory.add_item(sand, 30);
    inventory.add_item(dirt, 4);
    inventory.add_item(sword, 1);
    assert!(s.game.trash_item(s.id, sand));
    assert_eq!(s.inventory().item_count(sand), 0, "全数が消える");
    assert_eq!(s.game.trash[&s.id], (sand, 30));
    // 別のものを入れると、前のものは戻らず置き換わる。
    assert!(s.game.trash_item(s.id, dirt));
    assert_eq!(s.game.trash[&s.id], (dirt, 4));
    assert_eq!(s.inventory().item_count(sand), 0, "前のものは戻らない");
    // 持っていない・装備中のものは入れられず、ゴミ箱も変わらない。
    let before = s.game.state_hash();
    assert!(!s.game.trash_item(s.id, sand), "もう持っていない");
    assert!(!s.game.trash_item(EntityId(999), dirt), "キャラクターがいない");
    let defs = s.defs.clone();
    s.game.equip(s.id, sword, &defs).unwrap();
    assert!(!s.game.trash_item(s.id, sword), "装備中は持ち物にない");
    assert_eq!(s.game.trash[&s.id], (dirt, 4));
    assert_ne!(before, 0);
}
