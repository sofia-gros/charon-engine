//! 1 tick の更新。描画ループと画面なしのテストが同じ手順を使う (仕様 06 §6、仕様 07 §2、仕様 05 §2)。

use std::time::Instant;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::world::World;
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::{CELLS_PER_METER, PhysicsEngine};

use crate::StreamingState;
use crate::player::try_spawn_player;

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
}

impl Sim<'_> {
    /// 順序は「チャンクの読み込み → 剛体のスタンプ (初回) → プレイヤー → 剛体とセル」。
    /// `Err` は続行できない失敗 (保存領域の読み書きなど)。物理にかかった時間 (ミリ秒) を `physics_ms` に足す。
    pub(crate) fn tick(
        &mut self,
        camera: ChunkPos,
        input: PlayerInput,
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
                Ok(Some(id)) => *self.player = Some(id),
                Ok(None) => {}
                Err(err) => {
                    // 固体と重なる位置では、地形を消さずにプレイヤーなしで続ける。
                    eprintln!("プレイヤーを生成できません: {err}");
                    *self.spawn = None;
                }
            }
        }
        let inputs: Vec<_> = self.player.iter().map(|&id| (id, input)).collect();
        self.entities.step_players(&inputs, self.world).map_err(|err| err.to_string())?;
        let started = Instant::now();
        let result = self.physics.step(self.world).map_err(|err| err.to_string());
        *physics_ms += started.elapsed().as_secs_f32() * 1000.0;
        result
    }
}
