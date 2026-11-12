//! 1 tick の更新。描画ループと画面なしのテストが同じ手順を使う (仕様 06 §6、仕様 07 §2、仕様 05 §2、仕様 10 §6)。

use std::time::Instant;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::world::World;
use charon_game::game::{GameInput, GameState};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::wand::{SpellId, SpellTable, Wand};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use charon_render::EntitySprite;

use crate::StreamingState;
use crate::player::try_spawn_player;

/// プレイヤーの杖のマナの最大値。
const WAND_MANA: u32 = 100;

/// 呪文の RON から `GameState` を作る。`None` や読めないときは呪文なし (撃てない) で、後者は報告する。
pub(crate) fn game_for(spells: Option<&str>, materials: &MaterialTable) -> GameState {
    let table = match spells.map(|text| SpellTable::parse(text, materials, None)) {
        Some(Ok(table)) => table,
        Some(Err(err)) => {
            eprintln!("呪文の表を読めません: {err}");
            SpellTable::default()
        }
        None => SpellTable::default(),
    };
    GameState::new(table)
}

/// エンティティと飛んでいる弾の描画用の矩形。弾は 2×2 セルの点。
pub(crate) fn sprites(entities: &EntityWorld, game: &GameState) -> Vec<EntitySprite> {
    entities
        .iter()
        .map(|entity| EntitySprite {
            x16: entity.x16,
            y16: entity.y16,
            width: entity.width,
            height: entity.height,
        })
        .chain(game.projectiles.iter().map(|p| EntitySprite {
            x16: p.x16 - 16,
            y16: p.y16 - 16,
            width: 2,
            height: 2,
        }))
        .collect()
}

/// 1 tick が触れるものを借りたもの。
pub(crate) struct Sim<'a> {
    pub(crate) world: &'a mut World,
    pub(crate) streaming: &'a mut Option<StreamingState>,
    pub(crate) physics: &'a mut PhysicsEngine,
    /// 保存した剛体のスタンプを、そのチャンクが届いてから作るための印。
    pub(crate) initialized: &'a mut bool,
    pub(crate) entities: &'a mut EntityWorld,
    pub(crate) player: &'a mut Option<EntityId>,
    /// まだ生成していないプレイヤーの初期位置。生成に失敗したら `None` にする。
    pub(crate) spawn: &'a mut Option<[i32; 2]>,
    /// 体力・杖・弾 (仕様 10 §6.1)。
    pub(crate) game: &'a mut GameState,
}

impl Sim<'_> {
    /// 順序は「チャンクの読み込み → 剛体のスタンプ (初回) → プレイヤー → ゲーム状態 → 剛体とセル」。
    /// `Err` は続行できない失敗 (保存領域の読み書きなど)。物理にかかった時間 (ミリ秒) を `physics_ms` に足す。
    pub(crate) fn tick(
        &mut self,
        camera: ChunkPos,
        input: PlayerInput,
        action: GameInput,
        physics_ms: &mut f32,
    ) -> Result<(), String> {
        if let Some(state) = self.streaming.as_mut() {
            state.streamer.tick(self.world, &[camera]).map_err(|err| err.to_string())?;
        }
        if !*self.initialized {
            let world = &*self.world;
            let ready = self.physics.bodies.iter().all(|body| {
                self.physics.world.bodies.get(body.handle).is_some_and(|rigid| {
                    let position = rigid.translation() * CELLS_PER_METER;
                    world
                        .chunk(CellPos::new(position.x as i32, position.y as i32).chunk())
                        .is_some()
                })
            });
            if !ready {
                // 保存状態の初回読み込み中も tick を進め、30 tick 後の挿入を待つ。
                // 剛体スタンプは対応チャンクが来るまで作らない。
                self.world.step();
                return Ok(());
            }
            self.physics.stamp_all(self.world);
            *self.initialized = true;
        }
        if self.player.is_none()
            && let Some(spawn) = *self.spawn
        {
            match try_spawn_player(self.entities, self.world, spawn) {
                Ok(Some(id)) => {
                    *self.player = Some(id);
                    self.register_character(id);
                }
                Ok(None) => {}
                Err(err) => {
                    // 固体と重なる位置では、地形を消さずにプレイヤーなしで続ける。
                    eprintln!("プレイヤーを生成できません: {err}");
                    *self.spawn = None;
                }
            }
        }
        // 体力 0 のプレイヤーは入力を受けない (仕様 10 §6.3)。
        let alive = self.player.and_then(|id| self.game.vitals.get(id)).is_none_or(Vitals::alive);
        let input = if alive { input } else { PlayerInput::default() };
        let inputs: Vec<_> = self.player.iter().map(|&id| (id, input)).collect();
        self.entities.step_players(&inputs, self.world).map_err(|err| err.to_string())?;
        let actions: Vec<_> = self
            .player
            .iter()
            .copied()
            .filter(|id| self.game.wands.contains_key(id))
            .map(|id| (id, action))
            .collect();
        let (tick, seed) = (self.world.tick(), self.world.seed());
        self.game
            .step(self.world, self.physics, self.entities, &actions, tick, seed)
            .map_err(|err| err.to_string())?;
        let started = Instant::now();
        let result = self.physics.step(self.world).map_err(|err| err.to_string());
        *physics_ms += started.elapsed().as_secs_f32() * 1000.0;
        result
    }

    /// 呪文の表があれば、プレイヤーに体力と杖 (表の呪文を記載順に) を持たせる。
    fn register_character(&mut self, id: EntityId) {
        let len = self.game.spells.len().min(16);
        if len == 0 || self.game.wands.contains_key(&id) {
            return;
        }
        let spells: Vec<SpellId> = (0..len).map(|i| SpellId(i as u16)).collect();
        if let (Ok(vitals), Ok(wand)) =
            (Vitals::new(MAX_HP), Wand::new(spells, WAND_MANA, 1, 0, &self.game.spells))
        {
            self.game.add_character(id, vitals, wand);
        }
    }
}
