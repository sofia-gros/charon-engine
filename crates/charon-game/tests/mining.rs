//! 仕様 24 §3: つるはし・斧による完全なブロックの固定 tick 採掘。
#![allow(clippy::unwrap_used)] // 前提が壊れたら失敗させる。

use charon_core::coords::CellPos;
use charon_core::material::HarvestTool;
use charon_core::testkit::{box_art, count_block_cells, place, place_block, world_from_ascii};
use charon_core::world::World;
use charon_game::blocks::BlockPos;
use charon_game::cursor::Cursor;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::GameState;
use charon_game::items::{ItemId, ItemKind};
use charon_game::loadout::{UseError, UseOutcome};
use charon_game::{EntityId, EntityWorld};

const WOOD: BlockPos = BlockPos { bx: 2, by: 5 };
const STONE: BlockPos = BlockPos { bx: 4, by: 5 };

fn setup(seed: u64) -> (World, EntityWorld, GameState, Definitions, EntityId, ItemId, ItemId) {
    let mut world = world_from_ascii(&box_art(64, 64), seed);
    place_block(&mut world, WOOD.bx, WOOD.by, 'W');
    place_block(&mut world, STONE.bx, STONE.by, '#');
    let defs = Definitions::engine_defaults(world.materials()).unwrap();
    let pick = defs.item_id("wood_pickaxe", world.materials()).unwrap();
    let axe = defs.item_id("wood_axe", world.materials()).unwrap();
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(2 * 16, 2 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(
        id,
        charon_game::vitals::Vitals::new(charon_game::vitals::MAX_HP).unwrap(),
        defs.wand("basic").unwrap(),
    );
    game.give_inventory(id).unwrap();
    let inventory = game.inventories.get_mut(&id).unwrap();
    inventory.add_item(pick, 1);
    inventory.add_item(axe, 1);
    inventory.pin(0, pick);
    inventory.pin(1, axe);
    (world, entities, game, defs, id, pick, axe)
}

fn cursor(at: BlockPos) -> Cursor {
    let origin = at.origin().unwrap();
    Cursor::at(CellPos::new(origin.x + 4, origin.y + 4))
}

fn tick(
    world: &mut World,
    entities: &EntityWorld,
    game: &mut GameState,
    defs: &Definitions,
    id: EntityId,
    at: BlockPos,
    held: bool,
) -> Result<UseOutcome, UseError> {
    game.mine_selected_tick(world, entities, id, defs, cursor(at), held)
}

/// 硬さ / 力を切り上げた tick にだけ、一度ブロックを回収する。
#[test]
fn mining_progress_uses_hardness_and_tool_power() {
    for seed in 1..=8 {
        let (mut world, entities, mut game, defs, id, pick, _) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let stone = world.materials().id("stone").unwrap();
        assert_eq!(world.materials().get(wood).harvest_tool, HarvestTool::Axe);
        assert_eq!(world.materials().get(stone).harvest_tool, HarvestTool::Pick);
        assert_eq!(defs.item_def(pick).unwrap().kind, ItemKind::Tool { pick: 4, axe: 0 });
        for (at, material, needed) in [(WOOD, wood, 38), (STONE, stone, 150)] {
            let initial = game.state_hash();
            for n in 1..needed {
                assert_eq!(
                    tick(&mut world, &entities, &mut game, &defs, id, at, true),
                    Ok(UseOutcome::Nothing),
                    "seed={seed} tick={n}"
                );
                assert_eq!(game.inventories[&id].block_count(material), 0);
                assert_eq!(
                    count_block_cells(&world, at.bx * 8, at.by * 8, at.bx * 8 + 7, at.by * 8 + 7),
                    64
                );
            }
            assert_ne!(game.state_hash(), initial, "途中経過は同期ハッシュに入る");
            assert_eq!(
                tick(&mut world, &entities, &mut game, &defs, id, at, true),
                Ok(UseOutcome::Broken)
            );
            assert_eq!(game.inventories[&id].block_count(material), 1);
            assert_eq!(
                count_block_cells(&world, at.bx * 8, at.by * 8, at.bx * 8 + 7, at.by * 8 + 7),
                0
            );
            assert_eq!(
                tick(&mut world, &entities, &mut game, &defs, id, at, true),
                Err(UseError::Invalid),
                "二重採掘はしない"
            );
            assert_eq!(game.inventories[&id].block_count(material), 1);
        }
    }
}

/// 手を離す・持ち替える・別のブロックを狙う・失敗は途中経過を残さず、セルも個数も変えない。
#[test]
fn mining_interrupt_or_failure_changes_nothing() {
    for seed in 1..=8 {
        let (mut world, entities, mut game, defs, id, pick, _) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let initial_world = world.state_hash();
        let initial_inventory = game.inventories[&id].clone();
        for _ in 0..10 {
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true).unwrap();
        }
        let pending_hash = game.state_hash();
        let save = charon_game::save::GameSave::capture(&game, &entities, id, world.materials(), 0)
            .with_items(&game, id, &defs, world.materials())
            .to_ron()
            .unwrap();
        assert!(!save.contains("mining"), "途中経過は game.ron に書かない");
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, false),
            Ok(UseOutcome::Nothing)
        );
        assert_ne!(game.state_hash(), pending_hash);
        for _ in 0..37 {
            assert_eq!(
                tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
                Ok(UseOutcome::Nothing)
            );
        }
        // 別のブロックを狙うと、元の 37 tick は持ち越されない。
        tick(&mut world, &entities, &mut game, &defs, id, STONE, true).unwrap();
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Ok(UseOutcome::Nothing)
        );
        game.inventories.get_mut(&id).unwrap().select(1);
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Ok(UseOutcome::Nothing)
        );
        game.inventories.get_mut(&id).unwrap().select(0);
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Ok(UseOutcome::Nothing)
        );
        // 壊れたブロックや届かない場所は、進捗だけを捨てる。
        place(&mut world, 16, 40, '.');
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Err(UseError::Invalid)
        );
        place_block(&mut world, WOOD.bx, WOOD.by, 'W');
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Ok(UseOutcome::Nothing)
        );
        let too_far = Cursor::at(CellPos::new(60, 60));
        assert_eq!(
            game.mine_selected_tick(&mut world, &entities, id, &defs, too_far, true),
            Err(UseError::OutOfReach)
        );
        let before = world.state_hash();
        let inv_before = game.inventories[&id].clone();
        game.inventories.get_mut(&id).unwrap().add_block(wood, charon_game::inventory::STACK_MAX);
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Err(UseError::Build(charon_game::game::BuildError::InventoryFull))
        );
        assert_eq!(world.state_hash(), before);
        assert_eq!(game.inventories[&id].block_count(wood), charon_game::inventory::STACK_MAX);
        assert_ne!(game.inventories[&id], inv_before);
        game.inventories.get_mut(&id).unwrap().take_block(wood, charon_game::inventory::STACK_MAX);
        assert_eq!(world.state_hash(), initial_world);
        assert_eq!(game.inventories[&id], initial_inventory);
        assert_eq!(game.inventories[&id].item_count(pick), 1);
    }
}

/// 木は斧 8、斧を持たないつるはし 4。石は pick のみで、斧だけでは掘れない。
#[test]
fn axe_prefers_wood_and_pick_mines_stone() {
    for seed in 1..=8 {
        let (mut world, entities, mut game, defs, id, _, axe) = setup(seed);
        let stone = world.materials().id("stone").unwrap();
        game.inventories.get_mut(&id).unwrap().select(1);
        assert_eq!(defs.item_def(axe).unwrap().kind, ItemKind::Tool { pick: 0, axe: 8 });
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, STONE, true),
            Err(UseError::Invalid)
        );
        for _ in 1..19 {
            assert_eq!(
                tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
                Ok(UseOutcome::Nothing)
            );
        }
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, WOOD, true),
            Ok(UseOutcome::Broken)
        );
        game.inventories.get_mut(&id).unwrap().select(0);
        for _ in 1..150 {
            assert_eq!(
                tick(&mut world, &entities, &mut game, &defs, id, STONE, true),
                Ok(UseOutcome::Nothing)
            );
        }
        assert_eq!(
            tick(&mut world, &entities, &mut game, &defs, id, STONE, true),
            Ok(UseOutcome::Broken)
        );
        assert_eq!(game.inventories[&id].block_count(stone), 1);
        // 古い定義の Tool(pick) は axe=0 として読み取る。
        let legacy = Definitions::engine_defaults(world.materials()).unwrap();
        assert_eq!(
            legacy
                .item_def(legacy.item_id("wood_pickaxe", world.materials()).unwrap())
                .unwrap()
                .kind,
            ItemKind::Tool { pick: 4, axe: 0 }
        );
        let mut modified = legacy.clone();
        modified
            .layer(
                DefinitionFiles {
                    items: Some(r#"[(name: "wood_axe", replace: true, kind: Tool(axe: 9))]"#),
                    ..Default::default()
                },
                world.materials(),
                None,
            )
            .unwrap();
        assert_ne!(legacy.content_hash(), modified.content_hash(), "axe も定義ハッシュに含む");
        modified
            .layer(
                DefinitionFiles {
                    items: Some(r#"[(name: "no_power", kind: Tool())]"#),
                    ..Default::default()
                },
                world.materials(),
                Some("test"),
            )
            .unwrap();
        let zero = modified.item_id("test:no_power", world.materials()).unwrap();
        game.inventories.get_mut(&id).unwrap().add_item(zero, 1);
        game.inventories.get_mut(&id).unwrap().pin(2, zero);
        game.inventories.get_mut(&id).unwrap().select(2);
        place_block(&mut world, WOOD.bx, WOOD.by, 'W');
        assert_eq!(
            tick(&mut world, &entities, &mut game, &modified, id, WOOD, true),
            Err(UseError::Invalid)
        );
    }
}
