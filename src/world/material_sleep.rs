//! E05の決定論的なsleep/wake：停止している標準力学だけを安全に省略します。
//!
//! material channelはWorldの正規状態です。ここで保持する証明は高速化用の派生
//! cacheであり、失効したときは必ず参照実装へ戻します。作者独自Providerの
//! 挙動を「前tickに変更がなかった」という理由だけで省略しません。

use super::{
    ChannelQuery, ChunkCoord, LayerId, MaterialForceConfig, MaterialForceMotionReport,
    MaterialForceReport, MaterialId, MaterialMotion, MaterialMotionConfig, MaterialMotionReport,
    StandardMaterialForces, World, invalid,
};
use crate::foundation::{
    EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 参照結果と省略結果を区別できる診断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialSleepStepReport {
    /// 既存の正規force/motionと同じ候補数・変更数の結果です。
    pub reference: MaterialForceMotionReport,
    /// この呼出で全cell走査・力callback・DDAを実際に省略できたか。
    pub sleeping: bool,
    /// 保存したsleep証明の失効を検出して参照計算へ戻ったか。
    pub woke: bool,
}

/// 外部で保存可能なsleep証明。正規Worldの物質所有権は持ちません。
///
/// 「前回の全候補の速度・端数がゼロ」「各素材の標準力学による加速がゼロ」
/// 「active chunkの集合とrevisionが変化していない」のすべてが必要です。
/// custom Provider・正規粒子・圧力・変換rule等はこの証明の対象外です。
/// 別step・別sim設定へ再利用しても必ず失効として参照経路を使います。
#[derive(Clone, Debug)]
pub struct MaterialSleepProof {
    world: WorldId,
    // 純粋な派生cacheなのでStateCodecへは含めません。復元は再検証のみ。
    cache_generation: Option<std::sync::Arc<()>>,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    layer: StableKey,
    channel: StableKey,
    gravity: [i64; 2],
    candidate_limit: usize,
    max_velocity: i64,
    max_distance: u32,
    step_limit: usize,
    chunk_revisions: Vec<(ChunkCoord, u64)>,
    candidates: usize,
}

/// 標準力学の睡眠記録。World本体と分離し、保存/replay時に明示的に管理します。
#[derive(Clone, Debug, Default)]
pub struct MaterialSleepTracker {
    proof: Option<MaterialSleepProof>,
    // 保存済みの証明は入力値の整合しかdecodeでは検査できません。
    // 実Worldを走査して確認した生存中cacheのみを省略判定に使用します。
    verified_in_process: bool,
}

impl MaterialSleepTracker {
    /// 直前の正常な証明を破棄します。外部Provider・設定変更時の明示wakeにも使用。
    pub fn wake(&mut self) {
        self.proof = None;
        self.verified_in_process = false;
    }

    /// 証明を保持しているか。World失効時は呼出時に再検証します。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matching(
        &self,
        world: &World,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
    ) -> bool {
        if !self.verified_in_process {
            return false;
        }
        let Some(p) = &self.proof else { return false };
        if p.world != world.id
            || p.cache_generation
                .as_ref()
                .is_none_or(|source| !std::sync::Arc::ptr_eq(source, &world.cache_generation))
            || p.content != world.content.fingerprint()
            || p.epoch != world.remaps.epoch()
            || world.content.layer(layer).is_err()
            || world
                .content
                .layer(layer)
                .is_ok_and(|definition| definition.key != p.layer)
            || p.channel != force.channel
            || p.channel != motion.channel
            || p.gravity != force.gravity_q16
            || p.candidate_limit != force.candidate_limit
            || p.max_velocity != force.max_velocity_q16
            || p.max_distance != motion.max_distance
            || p.step_limit != motion.step_limit
            || p.candidate_limit != motion.candidate_limit
        {
            return false;
        }
        match world.changes_after(p.revision) {
            super::ChangeQuery::Current => {}
            super::ChangeQuery::Delta(delta) if delta.chunks.is_empty() => {}
            _ => return false,
        }
        let mut entries = world.active_chunks();
        for &(coordinate, revision) in &p.chunk_revisions {
            if entries.next() != Some(coordinate)
                || !world
                    .chunks
                    .get(&coordinate)
                    .is_some_and(|c| c.active && c.revision == revision)
            {
                return false;
            }
        }
        entries.next().is_none()
    }

    fn prove(
        world: &World,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        report: MaterialForceMotionReport,
    ) -> EngineResult<Option<MaterialSleepProof>> {
        if force.channel != motion.channel
            || force.candidate_limit != motion.candidate_limit
            || report.force.changed != 0
            || report.force.waiting_for_region != 0
            || report.motion.moved != 0
            || report.motion.collisions != 0
            || report.motion.waiting_for_region != 0
            || report.motion.reservation_conflicts != 0
            || report.motion.crossings != 0
        {
            return Ok(None);
        }
        let def = world.content.layer(layer)?;
        let origin = world
            .channel_layout::<MaterialMotion>(force.channel)?
            .origin;
        let mut chunks = Vec::new();
        let count = world.active_chunks().count();
        chunks.try_reserve_exact(count).map_err(allocation)?;
        let mut candidates = 0usize;
        for (coord, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            chunks.push((*coord, chunk.revision));
            let page = &chunk.layers[layer.0 as usize];
            let corner = coord.origin()?;
            for (index, cell) in page.cells.iter().enumerate() {
                if cell.material == MaterialId::AIR
                    || page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                {
                    continue;
                }
                candidates += 1;
                if candidates > force.candidate_limit {
                    return Ok(None);
                }
                let material = world
                    .content
                    .material(cell.material)?
                    .ok_or_else(|| invalid("sleep候補の空素材"))?;
                let physics = material.motion_properties();
                // 速度がゼロでも重力か浮力が残る場合、次tickには動き得ます。
                // 空気中の重力ゼロ、または重力倍率も浮力倍率もゼロだけを証明。
                if force.gravity_q16 != [0, 0]
                    && (physics.gravity_scale_q16 != 0 || physics.buoyancy_scale_q16 != 0)
                {
                    return Ok(None);
                }
                let point = CellPoint {
                    x: corner.x + (index % 64) as i32,
                    y: corner.y + (index / 64) as i32,
                };
                let channel_point = CellPoint {
                    x: point
                        .x
                        .checked_sub(origin.x)
                        .ok_or_else(|| invalid("sleep速度channel水平端"))?,
                    y: point
                        .y
                        .checked_sub(origin.y)
                        .ok_or_else(|| invalid("sleep速度channel垂直端"))?,
                };
                let value =
                    match world.channel_value::<MaterialMotion>(force.channel, channel_point)? {
                        ChannelQuery::Loaded(value) => value,
                        _ => return Ok(None),
                    };
                if value != MaterialMotion::default() {
                    return Ok(None);
                }
            }
        }
        if candidates != report.force.candidates
            || candidates != report.motion.candidates
            || !def.simulated
        {
            return Ok(None);
        }
        Ok(Some(MaterialSleepProof {
            world: world.id,
            cache_generation: Some(std::sync::Arc::clone(&world.cache_generation)),
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: world.revision(),
            layer: def.key,
            channel: force.channel,
            gravity: force.gravity_q16,
            candidate_limit: force.candidate_limit,
            max_velocity: force.max_velocity_q16,
            max_distance: motion.max_distance,
            step_limit: motion.step_limit,
            chunk_revisions: chunks,
            candidates,
        }))
    }
}

impl World {
    /// 標準Q16力学・DDAの結果一致が証明できるtickだけ走査を省略します。
    ///
    /// 独自Providerを受け取らず、author stateや時間に依存する力を無条件に
    /// sleepさせません。外部のWorld編集・channel変更・region load/evict/
    /// active状態変更・material remapはchunk revisionまたはepochでwake。
    /// sleep中も参照stepと同様に成功したWorld編集のrevisionを一つ進めます。
    pub fn step_standard_material_force_motion_sleeping(
        &mut self,
        tracker: &mut MaterialSleepTracker,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
    ) -> EngineResult<MaterialSleepStepReport> {
        let slept_before = tracker.has_proof();
        if tracker.matching(self, layer, force, motion) {
            // 正規のWorld::editを通すため、参照経路とのrevision進行が一致します。
            let candidates = tracker.proof.as_ref().expect("照合済み").candidates;
            self.edit(|_| Ok(()))?;
            tracker.proof.as_mut().expect("検証済み").revision = self.revision();
            return Ok(MaterialSleepStepReport {
                reference: MaterialForceMotionReport {
                    force: MaterialForceReport {
                        candidates,
                        changed: 0,
                        waiting_for_region: 0,
                    },
                    motion: MaterialMotionReport {
                        candidates,
                        ..Default::default()
                    },
                },
                sleeping: true,
                woke: false,
            });
        }
        // 失効した証明は先に破棄し、失敗時にも古い証明を使いません。
        tracker.wake();
        let reference = self.edit(|edit| {
            edit.step_material_force_motion(layer, force, motion, &StandardMaterialForces)
        })?;
        // キャッシュの確保失敗はシミュレーションの失敗ではありません。
        tracker.proof = MaterialSleepTracker::prove(self, layer, force, motion, reference)
            .ok()
            .flatten();
        tracker.verified_in_process = tracker.proof.is_some();
        Ok(MaterialSleepStepReport {
            reference,
            sleeping: false,
            woke: slept_before,
        })
    }
}

impl StateCodec for MaterialSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHSLEEP1")?;
        let Some(p) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        p.world.encode(out)?;
        out.raw(&p.content)?;
        out.u64(p.epoch)?;
        out.u64(p.revision)?;
        p.layer.encode(out)?;
        p.channel.encode(out)?;
        for value in p.gravity {
            out.i64(value)?;
        }
        out.u64(u64::try_from(p.candidate_limit).map_err(|_| invalid("sleep候補容量"))?)?;
        out.i64(p.max_velocity)?;
        out.u32(p.max_distance)?;
        out.u64(u64::try_from(p.step_limit).map_err(|_| invalid("sleep検査容量"))?)?;
        out.u64(u64::try_from(p.candidates).map_err(|_| invalid("sleep候補数"))?)?;
        out.u32(u32::try_from(p.chunk_revisions.len()).map_err(|_| invalid("sleep活動chunk数"))?)?;
        for (coord, revision) in &p.chunk_revisions {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHSLEEP1" {
            return Err(invalid("sleep保存形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("sleep存在bit")),
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("sleep Content hash"))?;
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let layer = StableKey::decode(input)?;
        let channel = StableKey::decode(input)?;
        let gravity = [input.i64()?, input.i64()?];
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("sleep候補容量"))?;
        let max_velocity = input.i64()?;
        let max_distance = input.u32()?;
        let step_limit = usize::try_from(input.u64()?).map_err(|_| invalid("sleep検査容量"))?;
        let candidates = usize::try_from(input.u64()?).map_err(|_| invalid("sleep候補数"))?;
        let count = input.u32()? as usize;
        if count > input.remaining() / 16
            || candidates > candidate_limit
            || max_velocity <= 0
            || max_distance == 0
        {
            return Err(invalid("sleep保存容量と設定"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        if chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(invalid("sleep活動chunk保存順"));
        }
        Ok(Self {
            proof: Some(MaterialSleepProof {
                world,
                cache_generation: None,
                content,
                epoch,
                revision,
                layer,
                channel,
                gravity,
                candidate_limit,
                max_velocity,
                max_distance,
                step_limit,
                chunk_revisions: chunks,
                candidates,
            }),
            verified_in_process: false,
        })
    }
}
