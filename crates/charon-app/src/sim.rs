//! 1 tick の更新。描画ループと画面なしのテストが同じ手順を使う (仕様 06 §6、仕様 07 §2、仕様 05 §2、仕様 10 §6)。

use web_time::Instant;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::{GameInput, GameState};
use charon_game::vitals::Vitals;
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use charon_render::EntitySprite;

use crate::player::{area_loaded, try_spawn_player};
use crate::{EnemySpawn, PlayerLoadout, StreamingState};

/// ワイヤーを、点列の間を 1 セル間隔でうめた 1×1 の点と、先端の 2×2 の点にする (仕様 10 §9.5)。
fn rope_sprites(points: &[[f32; 2]], out: &mut Vec<EntitySprite>) {
    let to16 = |v: f32| (v * 16.0).round() as i64;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        let steps = length.ceil().max(1.0) as usize;
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            out.push(EntitySprite {
                x16: to16(a[0] + (b[0] - a[0]) * t),
                y16: to16(a[1] + (b[1] - a[1]) * t),
                width: 1,
                height: 1,
            });
        }
    }
    if let Some(head) = points.last() {
        out.push(EntitySprite {
            x16: to16(head[0]) - 16,
            y16: to16(head[1]) - 16,
            width: 2,
            height: 2,
        });
    }
}

/// エンティティと飛んでいる弾の描画用の矩形。弾は 2×2 セルの点。
/// `skip` のエンティティは箱を描かない (画像のスプライトで描くとき)。
pub(crate) fn sprites(
    entities: &EntityWorld,
    game: &GameState,
    skip: &[EntityId],
) -> Vec<EntitySprite> {
    let mut out: Vec<EntitySprite> = entities
        .iter()
        .filter(|entity| !skip.contains(&entity.id))
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
        // 落ちているもの (仕様 10 §19): 2×2 セルの点。
        .chain(game.drops.iter().map(|d| EntitySprite {
            x16: d.x16 - 16,
            y16: d.y16 - 16,
            width: 2,
            height: 2,
        }))
        .collect();
    for hook in game.hooks.values() {
        if let Some(rope) = hook.rope() {
            rope_sprites(rope.points(), &mut out);
        }
    }
    // 掘る道具のレーザー (仕様 10 §12): 手から当たった点まで、1 セル間隔の点。
    for dig in game.digs.values() {
        if let Some(beam) = dig.beam() {
            rope_sprites(&[beam.from, beam.to], &mut out);
        }
    }
    out
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
    /// ゲームが渡した定義 (仕様 10 §7)。
    pub(crate) definitions: Option<&'a Definitions>,
    /// プレイヤーの装備 (仕様 10 §7.4)。なければ体力も杖も持たない。
    pub(crate) loadout: Option<&'a PlayerLoadout>,
    /// まだ生成していない敵 (仕様 10 §8.5)。周囲のチャンクが届いたものから生成して取り除く。
    pub(crate) enemy_spawns: &'a mut Vec<EnemySpawn>,
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
            // 砂や水を動かすのは、画面のまわりだけ (読み込み半径より 2 チャンク内側)。
            // 範囲の外はコールドフリーズ: 設定した更新距離 (0 = 自動) の外は、セルを更新しない (仕様 02 §9.4)。
            let radius = state.streamer.active_radius();
            self.world.set_active_region(Some((camera, radius)));
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
        self.spawn_pending_enemies();
        // 体力 0 のプレイヤーは入力を受けない (仕様 10 §6.3)。
        let alive = self.player.and_then(|id| self.game.vitals.get(id)).is_none_or(Vitals::alive);
        let input = if alive { input } else { PlayerInput::default() };
        let inputs: Vec<_> = self.player.iter().map(|&id| (id, input)).collect();
        self.entities.step_players(&inputs, self.world).map_err(|err| err.to_string())?;
        // エンティティは砂に勝つ: 楕円に重なった粉体は、近くの空きへ押し出される。
        self.entities.push_out_powder(self.world);
        // 浮遊 (仕様 10 §18): ジャンプのボタンを押している間、燃料で上へ。
        if let Some(id) = *self.player {
            self.game.step_levitation(self.entities, id, input.jump);
        }
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

    /// 周囲のチャンクが届いた敵を `spawn_enemy` で生成する (仕様 10 §8.5)。失敗した敵は報告して諦める。
    fn spawn_pending_enemies(&mut self) {
        if self.enemy_spawns.is_empty() {
            return;
        }
        let Some(definitions) = self.definitions else {
            eprintln!("敵の定義がないため、{} 体の敵を生成できません", self.enemy_spawns.len());
            self.enemy_spawns.clear();
            return;
        };
        let pending = std::mem::take(self.enemy_spawns);
        for spawn in pending {
            let Some(def) = definitions.enemy(&spawn.name) else {
                eprintln!("敵 {} は定義されていません", spawn.name);
                continue;
            };
            if !area_loaded(self.world, spawn.cell, def.width, def.height) {
                self.enemy_spawns.push(spawn);
                continue;
            }
            let cell = (spawn.cell[0], spawn.cell[1]);
            if let Err(err) =
                self.game.spawn_enemy(definitions, self.entities, self.world, &spawn.name, cell)
            {
                eprintln!("敵 {} を生成できません: {err}", spawn.name);
            }
        }
    }

    /// ゲームが渡した装備 (仕様 10 §7.4) があれば、プレイヤーに体力と杖を持たせる。
    /// 定義が足りないときは報告して、体力と杖なしで続ける。
    fn register_character(&mut self, id: EntityId) {
        let (Some(definitions), Some(loadout)) = (self.definitions, self.loadout) else { return };
        if self.game.wands.contains_key(&id) {
            return;
        }
        match (Vitals::new(loadout.max_hp), definitions.wand(&loadout.wand)) {
            (Ok(vitals), Ok(wand)) => {
                self.game.add_character(id, vitals, wand);
                // 右手の道具 (ワイヤーフック)。定義がなければ報告して、道具なしで続ける。
                if let Some(hook) = &loadout.hook
                    && let Err(err) = self.game.equip_hook(id, definitions, hook)
                {
                    eprintln!("道具 {hook} を持たせられません: {err}");
                }
                if let Some(dig) = &loadout.dig
                    && let Err(err) = self.game.equip_dig(id, definitions, dig)
                {
                    eprintln!("掘る道具 {dig} を持たせられません: {err}");
                }
                if let Some(def) = loadout.levitation {
                    self.game.equip_levitation(id, def);
                }
                // 持ち物 (掘った材質を集める、仕様 10 §13)。
                if let Err(err) = self.game.give_inventory(id) {
                    eprintln!("持ち物を持たせられません: {err}");
                }
            }
            (vitals, wand) => {
                eprintln!(
                    "プレイヤーの装備を作れません: 体力 {:?}、杖 {:?}",
                    vitals.err(),
                    wand.err()
                );
            }
        }
    }
}
