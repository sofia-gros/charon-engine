//! 仕様 19: アイテム・持ち物・クラフト・装備・自動装備・保存。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions, DefsError};
use charon_game::game::GameState;
use charon_game::inventory::HOTBAR_SLOTS;
use charon_game::items::{EquipSlot, ItemClass, ItemId, ItemKind, Resist, WeaponClass};
use charon_game::loadout::{ItemError, auto_equip_best};
use charon_game::save::{GAME_SAVE_VERSION, GameSave, PlayerSave};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

struct Setup {
    materials: Arc<MaterialTable>,
    defs: Definitions,
    world: World,
    entities: EntityWorld,
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
    Setup { materials, defs, world, entities, game, player }
}

fn mod_items(s: &Setup, text: &str) -> Definitions {
    let mut defs = s.defs.clone();
    let files = DefinitionFiles { items: Some(text), ..Default::default() };
    defs.layer(files, &s.materials, Some("m")).unwrap();
    defs
}

fn item(s: &Setup, name: &str) -> ItemId {
    s.defs.item_id(name, &s.materials).unwrap()
}

#[test]
fn items_load_layer_and_validate() {
    let s = setup();
    let materials = &s.materials;
    let names: Vec<&str> = s.defs.item_names().collect();
    assert!(names.contains(&"wood_pickaxe") && names.contains(&"heal_potion"));
    assert!(s.defs.recipe("stone_sword").is_some());
    let sword = s.defs.item_def(item(&s, "stone_sword")).unwrap();
    assert!(matches!(sword.kind, ItemKind::Weapon { class: WeaponClass::Melee, damage: 10, .. }));

    // MOD: 追加は `m:` が付く。`replace: true` は同じ名前を上書きする。
    let mut defs = s.defs.clone();
    let add = "[(name: \"gem\", kind: Misc), \
               (name: \"wood_sword\", replace: true, kind: Weapon(class: Melee, damage: 99))]";
    let files = DefinitionFiles { items: Some(add), ..Default::default() };
    defs.layer(files, materials, Some("m")).unwrap();
    assert!(defs.item_id("m:gem", materials).is_some());
    let replaced = defs.item_def(defs.item_id("wood_sword", materials).unwrap()).unwrap();
    assert!(matches!(replaced.kind, ItemKind::Weapon { damage: 99, .. }));
    assert_eq!(defs.item_names().count(), s.defs.item_names().count() + 1);

    // 不正な値・知らない杖・重複は拒否し、定義を変えない。
    let before = defs.content_hash();
    let bad: [(&str, bool); 5] = [
        ("[(name: \"x\", kind: Weapon(class: Melee, damage: 20000))]", true),
        ("[(name: \"x\", kind: Armor(slot: Head, defense: 1, fire: 101))]", true),
        ("[(name: \"x\", kind: Weapon(class: Wand, wand: Some(\"nope\")))]", false),
        ("[(name: \"x\", kind: Misc), (name: \"x\", kind: Misc)]", false),
        ("[(name: \"bad name\", kind: Misc)]", true),
    ];
    for (text, _) in bad {
        let files = DefinitionFiles { items: Some(text), ..Default::default() };
        assert!(defs.layer(files, materials, Some("m")).is_err(), "{text}");
    }
    let unknown = "[(name: \"x\", kind: Weapon(class: Wand, wand: Some(\"nope\")))]";
    let files = DefinitionFiles { items: Some(unknown), ..Default::default() };
    assert_eq!(
        defs.layer(files, materials, Some("m")).unwrap_err(),
        DefsError::UnknownWand("nope".to_owned())
    );
    let unknown_recipe = "[(name: \"r\", inputs: [(\"cells:nope\", 1)], output: (\"m:gem\", 1))]";
    let files = DefinitionFiles { recipes: Some(unknown_recipe), ..Default::default() };
    assert!(matches!(
        defs.layer(files, materials, Some("m")).unwrap_err(),
        DefsError::UnknownItem { .. }
    ));
    assert_eq!(defs.content_hash(), before, "失敗したら変えない");
}

#[test]
fn item_names_roundtrip() {
    let s = setup();
    let materials = &s.materials;
    let wood = materials.id("wood").unwrap();
    for name in ["cells:wood", "block:wood", "stone_helmet"] {
        let id = s.defs.item_id(name, materials).unwrap();
        assert_eq!(s.defs.item_name(id, materials).as_deref(), Some(name));
    }
    assert_eq!(s.defs.item_id("cells:wood", materials), Some(ItemId::cells(wood)));
    assert_eq!(s.defs.item_id("block:wood", materials), Some(ItemId::block(wood)));
    assert!(matches!(item(&s, "wood_helmet").class(), ItemClass::Def(_)));
    for bad in ["cells:nope", "block:nope", "nope", "cells:", ""] {
        assert_eq!(s.defs.item_id(bad, materials), None, "{bad}");
    }
    assert_eq!(s.defs.item_name(ItemId::def(9999), materials), None);
}

#[test]
fn recipes_craft_consume_and_fail_cleanly() {
    let mut s = setup();
    let wood = s.materials.id("wood").unwrap();
    let (p, defs) = (s.player, s.defs.clone());
    let pick = item(&s, "wood_pickaxe");

    assert_eq!(s.game.craft(p, "wood_pickaxe", &[], &defs), Err(ItemError::MissingIngredients));
    assert_eq!(s.game.craft(p, "no_such_recipe", &[], &defs), Err(ItemError::UnknownRecipe));
    s.game.inventories.get_mut(&p).unwrap().add(wood, 45);
    let before = s.game.inventories[&p].clone();
    s.game.craft(p, "wood_pickaxe", &[], &defs).unwrap();
    let inv = &s.game.inventories[&p];
    assert_eq!((inv.count(wood), inv.item_count(pick)), (5, 1), "40 個を使って 1 個");
    assert_ne!(*inv, before);

    // 足りなければ何も変えない。
    let before = s.game.inventories[&p].clone();
    assert_eq!(s.game.craft(p, "wood_pickaxe", &[], &defs), Err(ItemError::MissingIngredients));
    assert_eq!(s.game.inventories[&p], before);

    // 持ち切れない (出力の上限) ときも、何も変えない。
    let inv = s.game.inventories.get_mut(&p).unwrap();
    inv.add_item(pick, charon_game::inventory::STACK_MAX);
    inv.add(wood, 40);
    let before = inv.clone();
    assert_eq!(s.game.craft(p, "wood_pickaxe", &[], &defs), Err(ItemError::NoRoom));
    assert_eq!(s.game.inventories[&p], before);
    assert_eq!(
        s.game.craft(EntityId(999), "wood_pickaxe", &[], &defs),
        Err(ItemError::NoInventory)
    );

    // 作業台つきのレシピ: 登録済みの Station 設備が近いときだけ作れる。
    let mut defs = defs;
    let text = "[(name: \"bench_gem\", inputs: [(\"cells:wood\", 10)], output: (\"wood_sword\", 1), \
                station: Some(\"workbench\"))]";
    let files = DefinitionFiles { recipes: Some(text), ..Default::default() };
    defs.layer(files, &s.materials, Some("m")).unwrap();
    let station = defs.item_id("workbench", &s.materials).unwrap();
    let sword = item(&s, "wood_sword");
    let before = s.game.inventories[&p].clone();
    assert_eq!(s.game.craft(p, "m:bench_gem", &[], &defs), Err(ItemError::MissingStation));
    assert_eq!(
        s.game.craft(p, "m:bench_gem", &[ItemId::block(wood)], &defs),
        Err(ItemError::MissingStation)
    );
    assert_eq!(s.game.inventories[&p], before);
    s.game.craft(p, "m:bench_gem", &[station], &defs).unwrap();
    assert_eq!(s.game.inventories[&p].item_count(sword), 1);
    // 普通の石ブロックや Chest を station とする旧ルールは採用しない。
    let before = defs.content_hash();
    for name in ["block:stone", "storage_box"] {
        let invalid = format!(
            "[(name: \"wrong_station\", inputs: [(\"cells:wood\", 1)], \
            output: (\"wood_sword\", 1), station: Some(\"{name}\"))]"
        );
        assert!(matches!(
            defs.layer(
                DefinitionFiles { recipes: Some(&invalid), ..Default::default() },
                &s.materials,
                None
            ),
            Err(DefsError::Invalid { .. })
        ));
        assert_eq!(defs.content_hash(), before, "失敗した層は反映しない");
    }
}

#[test]
fn block_recipe_is_built_in() {
    let mut s = setup();
    let stone = s.materials.id("stone").unwrap();
    let p = s.player;
    s.game.inventories.get_mut(&p).unwrap().add(stone, 64);
    assert!(s.game.craft_block(&s.world, p, stone));
    let inv = &s.game.inventories[&p];
    assert_eq!((inv.count(stone), inv.block_count(stone)), (0, 1));
    assert_eq!(inv.item_count(ItemId::block(stone)), 1, "ItemId でも数えられる");
    assert!(!s.game.craft_block(&s.world, p, stone), "セルが足りない");
    assert!(
        s.defs.recipes().iter().all(|r| r.output.0 != ItemId::block(stone)),
        "ファイルには書かない"
    );
}

#[test]
fn equip_swaps_and_unequip_returns_items() {
    let mut s = setup();
    let (p, defs) = (s.player, s.defs.clone());
    let (wood_h, stone_h) = (item(&s, "wood_helmet"), item(&s, "stone_helmet"));
    let (pick, potion) = (item(&s, "wood_pickaxe"), item(&s, "heal_potion"));
    let inv = s.game.inventories.get_mut(&p).unwrap();
    for it in [wood_h, stone_h, pick, potion] {
        inv.add_item(it, 1);
    }
    assert_eq!(s.game.equip(p, item(&s, "wood_sword"), &defs), Err(ItemError::NotOwned));
    assert_eq!(s.game.equip(p, pick, &defs), Err(ItemError::NotEquippable), "道具は装備できない");
    assert_eq!(s.game.equip(p, potion, &defs), Err(ItemError::NotEquippable));
    assert_eq!(s.game.equip(EntityId(999), wood_h, &defs), Err(ItemError::NoInventory));

    s.game.equip(p, wood_h, &defs).unwrap();
    assert_eq!(s.game.equipment[&p].get(EquipSlot::Head), Some(wood_h));
    assert_eq!(s.game.inventories[&p].item_count(wood_h), 0, "持ち物から装備へ移る");
    // 入れ替え: 前の装備は持ち物へもどる。
    s.game.equip(p, stone_h, &defs).unwrap();
    assert_eq!(s.game.equipment[&p].get(EquipSlot::Head), Some(stone_h));
    assert_eq!(s.game.inventories[&p].item_count(wood_h), 1);
    assert_eq!(s.game.inventories[&p].item_count(stone_h), 0);
    // 外す。
    s.game.unequip(p, EquipSlot::Head).unwrap();
    assert_eq!(s.game.equipment[&p].get(EquipSlot::Head), None);
    assert_eq!(s.game.inventories[&p].item_count(stone_h), 1);
    s.game.unequip(p, EquipSlot::Head).unwrap(); // 空の枠は何もしない。
    // 持ち切れないときは、外せない・入れ替えられない。
    s.game.equip(p, stone_h, &defs).unwrap();
    let inv = s.game.inventories.get_mut(&p).unwrap();
    inv.add_item(stone_h, charon_game::inventory::STACK_MAX);
    let inv_before = inv.clone();
    assert_eq!(s.game.unequip(p, EquipSlot::Head), Err(ItemError::NoRoom));
    assert_eq!(s.game.equip(p, wood_h, &defs), Err(ItemError::NoRoom));
    assert_eq!(s.game.inventories[&p], inv_before);
    assert_eq!(s.game.equipment[&p].get(EquipSlot::Head), Some(stone_h));
}

#[test]
fn equipment_stats_add_up_and_cap_resists() {
    let mut s = setup();
    let text = "[(name: \"h\", kind: Armor(slot: Head, defense: 3, fire: 60)), \
                 (name: \"b\", kind: Armor(slot: Body, defense: 5, fire: 60, cold: 10)), \
                 (name: \"l\", kind: Armor(slot: Legs, defense: 2, poison: 30)), \
                 (name: \"w\", kind: Weapon(class: Ranged, damage: 12))]";
    let defs = mod_items(&s, text);
    let mt = Arc::clone(&s.materials);
    let p = s.player;
    let ids: Vec<ItemId> =
        ["m:h", "m:b", "m:l", "m:w"].iter().map(|n| defs.item_id(n, &mt).unwrap()).collect();
    let inv = s.game.inventories.get_mut(&p).unwrap();
    for &id in &ids {
        inv.add_item(id, 1);
    }
    let none = s.game.equipment.get(&p).map(|e| e.stats(|i| defs.item_def(i))).unwrap_or_default();
    assert_eq!((none.defense, none.damage, none.weapon_class), (0, 0, None));
    for &id in &ids {
        s.game.equip(p, id, &defs).unwrap();
    }
    let stats = s.game.equipment[&p].stats(|i| defs.item_def(i));
    assert_eq!(stats.defense, 10);
    assert_eq!(stats.resist, Resist { fire: 100, poison: 30, cold: 10 }, "耐性は 100 が上限");
    assert_eq!((stats.damage, stats.weapon_class), (12, Some(WeaponClass::Ranged)));
}

#[test]
fn auto_equip_picks_the_best_and_is_deterministic() {
    let s = setup();
    let text = "[(name: \"t1\", kind: Armor(slot: Legs, defense: 4)), \
                 (name: \"t2\", kind: Armor(slot: Legs, defense: 4))]";
    let defs = mod_items(&s, text);
    let mt = Arc::clone(&s.materials);
    let id = |n: &str| defs.item_id(n, &mt).unwrap();
    let build = |order: &[&str]| {
        let mut inv = charon_game::inventory::Inventory::new();
        for n in order {
            inv.add_item(id(n), 1);
        }
        inv
    };
    let all = [
        "wood_helmet",
        "stone_helmet",
        "wood_sword",
        "stone_sword",
        "m:t2",
        "m:t1",
        "wood_pickaxe",
    ];
    let mut inv = build(&all);
    let mut eq = charon_game::items::Equipment::new();
    assert_eq!(auto_equip_best(&mut inv, &mut eq, &defs), 3, "頭・武器・脚");
    assert_eq!(eq.get(EquipSlot::Head), Some(id("stone_helmet")));
    assert_eq!(eq.get(EquipSlot::Weapon), Some(id("stone_sword")));
    assert_eq!(eq.get(EquipSlot::Legs), Some(id("m:t1")), "同点は ID の小さい方 (先に定義した方)");
    assert_eq!(eq.get(EquipSlot::Body), None, "胴の防具がなければ空");
    assert_eq!(inv.item_count(id("wood_helmet")), 1, "弱いものは持ち物に残る");
    assert_eq!(inv.item_count(id("m:t2")), 1);
    assert_eq!(inv.item_count(id("wood_pickaxe")), 1, "道具は装備しない");
    // 2 回目は何も変えない。順番を変えて入れても同じ結果。
    let snapshot = (inv.clone(), eq.clone());
    assert_eq!(auto_equip_best(&mut inv, &mut eq, &defs), 0);
    assert_eq!((inv, eq.clone()), snapshot);
    let mut reversed = build(&all.iter().rev().copied().collect::<Vec<_>>());
    let mut eq2 = charon_game::items::Equipment::new();
    auto_equip_best(&mut reversed, &mut eq2, &defs);
    assert_eq!(eq2, eq);

    // 今より強いものが来たら替える (前のものは持ち物へ)。弱いものには替えない。
    let mut inv = build(&["wood_helmet"]);
    let mut eq = charon_game::items::Equipment::new();
    auto_equip_best(&mut inv, &mut eq, &defs);
    inv.add_item(id("stone_helmet"), 1);
    assert_eq!(auto_equip_best(&mut inv, &mut eq, &defs), 1);
    assert_eq!(eq.get(EquipSlot::Head), Some(id("stone_helmet")));
    assert_eq!(inv.item_count(id("wood_helmet")), 1);
    let mut inv = build(&["wood_helmet"]);
    let mut eq = charon_game::items::Equipment::new();
    eq.set(EquipSlot::Head, Some(id("stone_helmet")));
    assert_eq!(auto_equip_best(&mut inv, &mut eq, &defs), 0, "弱いものには替えない");
    assert_eq!(eq.get(EquipSlot::Head), Some(id("stone_helmet")));
}

#[test]
fn game_save_roundtrips_items_hotbar_and_equipment() {
    let mut s = setup();
    let (p, defs) = (s.player, s.defs.clone());
    let stone = s.materials.id("stone").unwrap();
    let (pick, potion, helmet) =
        (item(&s, "stone_pickaxe"), item(&s, "heal_potion"), item(&s, "stone_helmet"));
    let inv = s.game.inventories.get_mut(&p).unwrap();
    inv.add(stone, 7);
    inv.add_block(stone, 2);
    inv.add_item(pick, 1);
    inv.add_item(potion, 3);
    inv.add_item(helmet, 1);
    assert!(inv.pin(0, pick) && inv.pin(5, potion) && inv.pin(9, ItemId::cells(stone)));
    assert!(inv.select(5));
    s.game.equip(p, helmet, &defs).unwrap();

    let save = GameSave::capture(&s.game, &s.entities, p, &s.materials, 12).with_items(
        &s.game,
        p,
        &defs,
        &s.materials,
    );
    let text = save.to_ron().unwrap();
    let loaded = GameSave::from_ron(&text).unwrap();
    assert_eq!(loaded, save);
    let player = loaded.player.as_ref().unwrap();
    assert_eq!(player.hotbar.len(), HOTBAR_SLOTS);
    assert_eq!(player.hotbar[0].as_deref(), Some("stone_pickaxe"));
    assert_eq!(player.hotbar[9].as_deref(), Some("cells:stone"));
    assert_eq!(player.equipment[EquipSlot::Head.index()].as_deref(), Some("stone_helmet"));

    // 新しいゲームへ戻す。
    let mut t = setup();
    let applied = loaded.apply(&mut t.game, &mut t.entities, &t.world, t.player);
    assert_eq!(applied.kinds, 1);
    assert_eq!(loaded.apply_items(&mut t.game, t.player, &defs, &t.materials), 1);
    let (a, b) = (&s.game.inventories[&p], &t.game.inventories[&t.player]);
    assert_eq!(a, b, "持ち物・ホットバー・選択が同じ");
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(s.game.equipment[&p], t.game.equipment[&t.player]);
    assert_eq!(t.game.state_hash(), {
        // 同じ状態なら、ゲーム全体のハッシュも同じ (EntityId は同じ番号で作っている)。
        s.game.state_hash()
    });

    // 古い保存 (新しい項目なし) は読めて、何も足さない。
    let old = format!(
        "(version: {GAME_SAVE_VERSION}, tick: 0, player: Some((hp: 10, breath: 100, mana: 0, \
         x16: 0, y16: 0, inventory: [(\"stone\", 3)])))"
    );
    let old = GameSave::from_ron(&old).unwrap();
    let mut u = setup();
    old.apply(&mut u.game, &mut u.entities, &u.world, u.player);
    assert_eq!(old.apply_items(&mut u.game, u.player, &defs, &u.materials), 0);
    assert_eq!(u.game.inventories[&u.player].count(stone), 3);
    assert_eq!(u.game.inventories[&u.player].item_iter().count(), 1);

    // 知らない名前・枠に合わない装備は読み飛ばす。
    let mut weird = loaded.clone();
    let pl: &mut PlayerSave = weird.player.as_mut().unwrap();
    pl.items = vec![("nope".to_owned(), 5), ("potion_of_nothing".to_owned(), 1)];
    pl.hotbar = vec![Some("nope".to_owned()); 3];
    pl.equipment = vec![Some("wood_helmet".to_owned()), Some("nope".to_owned())];
    let mut v = setup();
    assert_eq!(weird.apply_items(&mut v.game, v.player, &defs, &v.materials), 0);
    let inv = &v.game.inventories[&v.player];
    assert_eq!((inv.item_iter().count(), inv.selected_item()), (0, None));
    assert!(v.game.equipment.get(&v.player).is_none_or(|e| e.iter().next().is_none()));

    // 項目が多すぎる保存は拒否する。
    let mut flood = save.clone();
    flood.player.as_mut().unwrap().hotbar = vec![None; HOTBAR_SLOTS + 1];
    assert!(GameSave::from_ron(&flood.to_ron().unwrap()).is_err());
}
