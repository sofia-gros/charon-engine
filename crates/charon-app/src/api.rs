//! ゲームがエンジンを使う入口 (仕様 14)。`Game` を実装して `AppOptions::logic` に渡すと、
//! エンジンが毎 tick (固定 60 回/秒) 呼ぶ。描画・入力・保存・音はエンジンが受け持つ。

use std::sync::{Arc, Mutex};

use charon_core::world::World;
use charon_game::game::{GameEvent, GameState};
use charon_game::{EntityId, EntityWorld};

use crate::audio::SoundEvent;

/// ゲームが 1 tick に触れるもの。世界・ゲーム状態・エンティティは借りたもの。
/// 演出 (揺れ・音) は `shake` / `play` で頼み、エンジンがそのフレームの描画のときに行う。
#[derive(Debug)]
pub struct Context<'a> {
    pub world: &'a mut World,
    pub game: &'a mut GameState,
    pub entities: &'a mut EntityWorld,
    /// プレイヤー (まだ生成されていなければ `None`)。
    pub player: Option<EntityId>,
    /// 世界の tick。
    pub tick: u64,
    pub(crate) effects: &'a mut Effects,
}

/// ゲームがエンジンに頼む演出。描画側で 1 フレームに 1 回まとめて受け取る。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects {
    /// 画面の揺れの強さ (足し合わせ、1 で頭打ち)。
    pub shake: f32,
    /// 鳴らす効果音。
    pub sounds: Vec<SoundEvent>,
}

/// 溜められる演出の音の数の上限 (ゲームが暴走しても増え続けない)。
pub const SOUNDS_MAX: usize = 64;

impl Context<'_> {
    /// 画面を揺らす (強さ 0..=1)。
    pub fn shake(&mut self, amount: f32) {
        if amount.is_finite() {
            self.effects.shake = (self.effects.shake + amount.max(0.0)).min(1.0);
        }
    }

    /// 効果音を鳴らす。
    pub fn play(&mut self, sound: SoundEvent) {
        if self.effects.sounds.len() < SOUNDS_MAX {
            self.effects.sounds.push(sound);
        }
    }
}

/// ゲームのロジック。必要なものだけ実装する。
pub trait Game: Send {
    /// プレイヤーが生まれた最初の tick に 1 回。
    fn on_start(&mut self, _ctx: &mut Context<'_>) {}

    /// 毎 tick、ゲーム状態を進めたあとに呼ぶ。`events` はこの tick に起きた出来事 (起きた順)。
    /// ここで世界やゲーム状態を変えるなら、同期 (Lockstep) のために、乱数は `rng::hash`、
    /// 入力は同期された値だけを使うこと。
    fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]);
}

/// `AppOptions` に持たせる、共有できる `Game`。
#[derive(Clone)]
pub struct GameLogic(Arc<Mutex<Box<dyn Game>>>);

impl GameLogic {
    pub fn new(game: impl Game + 'static) -> Self {
        Self(Arc::new(Mutex::new(Box::new(game))))
    }

    /// 1 tick 分を呼ぶ。`started` は、まだ `on_start` を呼んでいないとき真で、呼んだら偽にする。
    pub(crate) fn run(&self, started: &mut bool, ctx: &mut Context<'_>, events: &[GameEvent]) {
        // 毒になっても (ゲームの中で panic しても) 続ける。
        let mut game = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*started && ctx.player.is_some() {
            *started = true;
            game.on_start(ctx);
        }
        game.on_tick(ctx, events);
    }
}

impl std::fmt::Debug for GameLogic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GameLogic(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::material::MaterialTable;
    use charon_game::game::GameEvent;

    #[derive(Default)]
    struct Counter {
        started: u32,
        ticks: u32,
        defeated: u32,
    }

    impl Game for Counter {
        fn on_start(&mut self, ctx: &mut Context<'_>) {
            self.started += 1;
            ctx.play(SoundEvent::Pickup);
        }
        fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]) {
            self.ticks += 1;
            for event in events {
                if matches!(event, GameEvent::Defeated(_)) {
                    self.defeated += 1;
                    ctx.shake(0.8);
                }
            }
        }
    }

    /// 共有しても中身を覗けるように、テスト用に包む。
    struct Shared(Arc<Mutex<Counter>>);
    impl Game for Shared {
        fn on_start(&mut self, ctx: &mut Context<'_>) {
            self.0.lock().unwrap().on_start(ctx);
        }
        fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]) {
            self.0.lock().unwrap().on_tick(ctx, events);
        }
    }

    fn parts() -> (World, GameState, EntityWorld) {
        let materials = MaterialTable::builtin().unwrap();
        let world = World::new(Arc::new(materials), 1);
        (world, GameState::new(Default::default()), EntityWorld::new())
    }

    /// `on_start` はプレイヤーが生まれてから 1 回だけ、`on_tick` は毎 tick、出来事が渡る。演出は上限つきで足される。
    #[test]
    fn game_logic_is_started_once_and_ticked_with_events() {
        let counter = Arc::new(Mutex::new(Counter::default()));
        let logic = GameLogic::new(Shared(counter.clone()));
        let (mut world, mut game, mut entities) = parts();
        let mut effects = Effects::default();
        let mut started = false;

        // プレイヤーがいない間は on_start を呼ばない。
        let mut ctx = Context::for_test(&mut world, &mut game, &mut entities, None, &mut effects);
        logic.run(&mut started, &mut ctx, &[]);
        {
            // ガードを 2 つ同時に取ると行き詰まるので、1 つだけ取る。
            let c = counter.lock().unwrap();
            assert_eq!((c.started, c.ticks), (0, 1));
        }

        world.insert_chunk(charon_core::coords::ChunkPos::new(0, 0));
        let id = entities.spawn_player(10 * 16, 10 * 16, &world).unwrap();
        let mut ctx =
            Context::for_test(&mut world, &mut game, &mut entities, Some(id), &mut effects);
        logic.run(&mut started, &mut ctx, &[GameEvent::Defeated(id)]);
        logic.run(&mut started, &mut ctx, &[]);
        {
            let c = counter.lock().unwrap();
            assert_eq!((c.started, c.ticks, c.defeated), (1, 3, 1));
        }
        assert_eq!(effects.sounds, vec![SoundEvent::Pickup]);
        assert!((effects.shake - 0.8).abs() < 1e-6);
    }

    #[test]
    fn effects_are_clamped_and_bounded() {
        let (mut world, mut game, mut entities) = parts();
        let mut effects = Effects::default();
        let mut ctx = Context::for_test(&mut world, &mut game, &mut entities, None, &mut effects);
        ctx.shake(0.8);
        ctx.shake(0.8);
        ctx.shake(f32::NAN);
        ctx.shake(-1.0);
        for _ in 0..SOUNDS_MAX + 10 {
            ctx.play(SoundEvent::Jump);
        }
        assert_eq!(effects.shake, 1.0);
        assert_eq!(effects.sounds.len(), SOUNDS_MAX);
    }
}

impl Context<'_> {
    /// テストや画面なしの実行で、`Context` を作る (エンジンは内部で同じものを作る)。
    pub fn for_test<'a>(
        world: &'a mut World,
        game: &'a mut GameState,
        entities: &'a mut EntityWorld,
        player: Option<EntityId>,
        effects: &'a mut Effects,
    ) -> Context<'a> {
        let tick = world.tick();
        Context { world, game, entities, player, tick, effects }
    }
}
