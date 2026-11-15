//! 仕様 28 §8: ロックオン。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::cursor::center_cell;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::GameState;
use charon_game::targeting::{LOCK_LOST_TICKS, LOCK_RANGE, LockOn, LockPriority, ranked_targets};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

const ENEMIES: &str = r#"[
    (name: "dummy", width: 8, height: 16, max_hp: 100, walk_speed: 0, jump_speed: 0,
     sight: 1, attack: None),
]"#;

struct Arena {
    world: World,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    player: EntityId,
}

impl Arena {
    fn new() -> Self {
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        defs.layer(
            DefinitionFiles { enemies: Some(ENEMIES), ..Default::default() },
            &materials,
            Some("t"),
        )
        .unwrap();
        let mut world = World::new(Arc::clone(&materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        world.insert_chunk(ChunkPos::new(1, 0));
        testkit::fill(&mut world, 0, 60, 127, 60, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 26 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        Self { world, entities, game, defs, player }
    }

    fn dummy(&mut self, x: i32, y: i32) -> EntityId {
        self.game
            .spawn_enemy(&self.defs, &mut self.entities, &self.world, "t:dummy", (x, y))
            .unwrap()
    }

    /// 中心がプレイヤーの中心から `(dx, dy)` の位置になるように、敵を置く (敵の大きさは 8×16)。
    fn dummy_at(&mut self, dx: i32, dy: i32) -> EntityId {
        let c = self.center();
        self.dummy(c.x + dx - 4, c.y + dy - 8)
    }

    fn center(&self) -> CellPos {
        center_cell(self.entities.get(self.player).unwrap())
    }

    fn ranked(&self, cursor: Option<CellPos>, priority: LockPriority) -> Vec<EntityId> {
        ranked_targets(&self.world, &self.entities, &self.game, self.center(), cursor, priority)
    }

    /// プレイヤーの中心から右へ `dx` の位置に、目の高さの小さな壁 (3 セル) を立てる。水平の視線だけを遮る。
    fn wall(&mut self, dx: i32) {
        let c = self.center();
        testkit::fill(&mut self.world, c.x + dx, c.y - 1, c.x + dx + 1, c.y + 1, '#');
    }

    /// `wall` で立てた壁を取り除く。
    fn clear_wall(&mut self, dx: i32) {
        let c = self.center();
        testkit::fill(&mut self.world, c.x + dx, c.y - 1, c.x + dx + 1, c.y + 1, '.');
    }
}

/// 2 つの優先: 見通せる敵のうちカーソルに最も近いもの / 壁を無視してプレイヤーに最も近いもの。
#[test]
fn lock_on_picks_by_priority() {
    let mut a = Arena::new();
    let near = a.dummy_at(22, 0); // 近い。壁の向こう。
    let far = a.dummy_at(50, -30); // 遠い。上のほうで見通せる。
    let low = a.dummy_at(40, 10); // 遠い。下のほうで見通せる。
    a.wall(8);
    let c = a.center();
    assert!(a.world.line_of_sight(
        c,
        center_cell(a.entities.get(far).unwrap()),
        charon_core::query::Stop::Solid
    ));
    assert!(!a.world.line_of_sight(
        c,
        center_cell(a.entities.get(near).unwrap()),
        charon_core::query::Stop::Solid
    ));
    // 壁を無視して、近い順。
    assert_eq!(a.ranked(None, LockPriority::TargetClosest), vec![near, low, far]);
    // 見通せるものだけ。カーソルがなければプレイヤーからの近さ、あればカーソルからの近さ。
    let ranked = a.ranked(None, LockPriority::ClearestLine);
    assert!(!ranked.contains(&near), "壁の向こうは候補に入らない");
    assert_eq!(ranked.len(), 2);
    let near_far = center_cell(a.entities.get(far).unwrap());
    let near_low = center_cell(a.entities.get(low).unwrap());
    assert_eq!(a.ranked(Some(near_far), LockPriority::ClearestLine)[0], far);
    assert_eq!(a.ranked(Some(near_low), LockPriority::ClearestLine)[0], low);
    // 範囲の外 (LOCK_RANGE) は、どちらでも候補に入らない。
    let outside = a.dummy_at(LOCK_RANGE + 10, 0);
    assert!(!a.ranked(None, LockPriority::TargetClosest).contains(&outside));
    // 倒れた敵は候補に入らない。
    a.game.vitals.get_mut(near).unwrap().hp = 0;
    assert!(!a.ranked(None, LockPriority::TargetClosest).contains(&near));
}

/// ロックは対象を追い、死ぬ・範囲を出る・見通しが 60 tick 切れると解除する。
#[test]
fn lock_on_follows_and_releases() {
    let mut a = Arena::new();
    let target = a.dummy_at(30, 0);
    let mut lock = LockOn::default();
    lock.press(&a.world, &a.entities, &a.game, a.center(), None, LockPriority::ClearestLine);
    assert_eq!(lock.target, Some(target));
    assert!(lock.target_cell(&a.entities).is_some());
    // 見通しが切れても、59 tick は続く。戻れば数え直し。
    a.wall(10);
    for _ in 0..LOCK_LOST_TICKS - 1 {
        lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::ClearestLine);
    }
    assert_eq!(lock.target, Some(target), "59 tick: まだロック中");
    a.clear_wall(10);
    lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::ClearestLine);
    a.wall(10);
    for _ in 0..LOCK_LOST_TICKS - 1 {
        lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::ClearestLine);
    }
    assert_eq!(lock.target, Some(target), "戻ったので、数え直し");
    lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::ClearestLine);
    assert_eq!(lock.target, None, "60 tick 見通せない: 解除");
    // `TargetClosest` は壁を気にしない。
    lock.press(&a.world, &a.entities, &a.game, a.center(), None, LockPriority::TargetClosest);
    assert_eq!(lock.target, Some(target));
    for _ in 0..200 {
        lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::TargetClosest);
    }
    assert_eq!(lock.target, Some(target));
    // プレイヤーが離れて範囲の外になると、解除する。
    let far = CellPos::new(a.center().x - LOCK_RANGE - 20, a.center().y);
    lock.update(&a.world, &a.entities, &a.game, far, LockPriority::TargetClosest);
    assert_eq!(lock.target, None, "範囲外");
    // 死んだら解除する。
    lock.press(&a.world, &a.entities, &a.game, a.center(), None, LockPriority::TargetClosest);
    assert_eq!(lock.target, Some(target));
    a.game.vitals.get_mut(target).unwrap().hp = 0;
    lock.update(&a.world, &a.entities, &a.game, a.center(), LockPriority::TargetClosest);
    assert_eq!(lock, LockOn::default(), "死んだ: 解除");
}

/// ロック中にもう一度押すと次の候補へ。ほかになければ解除。
#[test]
fn lock_on_cycles_to_the_next_target() {
    let mut a = Arena::new();
    let (first, second, third) = (a.dummy_at(20, 0), a.dummy_at(35, 0), a.dummy_at(50, 0));
    let mut lock = LockOn::default();
    let press = |lock: &mut LockOn, a: &Arena| {
        lock.press(&a.world, &a.entities, &a.game, a.center(), None, LockPriority::TargetClosest);
    };
    press(&mut lock, &a);
    assert_eq!(lock.target, Some(first));
    press(&mut lock, &a);
    assert_eq!(lock.target, Some(second));
    press(&mut lock, &a);
    assert_eq!(lock.target, Some(third));
    press(&mut lock, &a);
    assert_eq!(lock.target, Some(first), "一巡する");
    // 1 体だけなら、もう一度押すと解除。
    a.game.vitals.get_mut(second).unwrap().hp = 0;
    a.game.vitals.get_mut(third).unwrap().hp = 0;
    press(&mut lock, &a);
    assert_eq!(lock.target, None);
    // 候補がいなければ、押しても何もロックしない。
    a.game.vitals.get_mut(first).unwrap().hp = 0;
    press(&mut lock, &a);
    assert_eq!(lock.target, None);
}
