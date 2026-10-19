//! 作者MaterialForceProviderが宣言した依存境界を使う保守的なsleep/wake。
//!
//! 標準Provider用trackerとは保存形式を分けます。作者Providerが明示的なstencilと
//! tickごとの不変条件を提供しない場合、この経路は必ず通常の参照実行へ戻ります。

use super::{
    ChangeQuery, ChunkCoord, LayerId, MaterialForceConfig, MaterialForceDependencyStencil,
    MaterialForceMotionReport, MaterialForceProvider, MaterialId, MaterialMotion,
    MaterialMotionConfig, World, invalid,
};
use crate::foundation::{
    EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 作者Provider用の参照結果とsleep/wake診断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialProviderSleepReport {
    /// 参照実装と同じ候補数・変更数の結果。
    pub reference: MaterialForceMotionReport,
    /// このtickにProvider callbackとDDAを省略したか。
    pub sleeping: bool,
    /// 以前の証明が失効し、参照実行へ戻ったか。
    pub woke: bool,
}

#[derive(Clone, Debug)]
struct MaterialProviderSleepProof {
    world: WorldId,
    cache_generation: Option<std::sync::Arc<()>>,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    layer: StableKey,
    channel: StableKey,
    provider: StableKey,
    provider_version: u32,
    gravity: [i64; 2],
    candidate_limit: usize,
    max_velocity: i64,
    max_distance: u32,
    step_limit: usize,
    stencil: MaterialForceDependencyStencil,
    active_chunks: Vec<(ChunkCoord, u64)>,
    dependency_chunks: Vec<ChunkCoord>,
    candidates: usize,
}

/// 作者Providerの静止証明。WorldやProviderの正規状態を所有しない派生cacheです。
///
/// trackerをWorld StateStoreやsaveへ置く場合も、decode後の証明は未認証です。
/// 最初のstepは必ずProvider参照実行を行い、現Worldで再証明します。
#[derive(Clone, Debug, Default)]
pub struct MaterialProviderSleepTracker {
    proof: Option<MaterialProviderSleepProof>,
    verified_in_process: bool,
}

impl MaterialProviderSleepTracker {
    /// 保存済み証明を破棄し、次回を必ず参照実行にします。
    pub fn wake(&mut self) {
        self.proof = None;
        self.verified_in_process = false;
    }

    /// sleep証明を保持しているかを返します。最終的な有効性は次回stepで確認します。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matching(
        &self,
        world: &World,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        provider: &dyn MaterialForceProvider,
        stencil: MaterialForceDependencyStencil,
    ) -> bool {
        if !self.verified_in_process {
            return false;
        }
        let Some(proof) = &self.proof else {
            return false;
        };
        if proof.world != world.id
            || proof.content != world.content.fingerprint()
            || proof.epoch != world.remaps.epoch()
            || world
                .content
                .layer(layer)
                .map_or(true, |definition| definition.key != proof.layer)
            || proof.channel != force.channel
            || proof.channel != motion.channel
            || proof.provider != provider.key()
            || proof.provider_version != provider.version()
            || proof.gravity != force.gravity_q16
            || proof.candidate_limit != force.candidate_limit
            || proof.max_velocity != force.max_velocity_q16
            || proof.max_distance != motion.max_distance
            || proof.step_limit != motion.step_limit
            || motion.candidate_limit != force.candidate_limit
            || proof.stencil != stencil
        {
            return false;
        }
        let generation_matches = proof
            .cache_generation
            .as_ref()
            .is_some_and(|source| std::sync::Arc::ptr_eq(source, &world.cache_generation));
        match world.changes_after(proof.revision) {
            ChangeQuery::Current if generation_matches => {}
            ChangeQuery::Current => return false,
            ChangeQuery::Delta(changes)
                if changes
                    .chunks
                    .iter()
                    .all(|coord| proof.dependency_chunks.binary_search(coord).is_err()) => {}
            _ => return false,
        }
        let mut active = world.active_chunks();
        for &(coord, revision) in &proof.active_chunks {
            if active.next() != Some(coord)
                || !world
                    .chunks
                    .get(&coord)
                    .is_some_and(|chunk| chunk.active && chunk.revision == revision)
            {
                return false;
            }
        }
        active.next().is_none()
    }

    fn prove(
        world: &World,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        provider: &dyn MaterialForceProvider,
        stencil: MaterialForceDependencyStencil,
        report: MaterialForceMotionReport,
    ) -> EngineResult<Option<MaterialProviderSleepProof>> {
        if force.channel != motion.channel
            || force.candidate_limit != motion.candidate_limit
            || report.force.changed != 0
            || report.force.waiting_for_region != 0
            || report.motion.moved != 0
            || report.motion.collisions != 0
            || report.motion.waiting_for_region != 0
            || report.motion.reservation_conflicts != 0
            || report.motion.crossings != 0
            || !provider.sleep_invariant(world, layer, force, motion)?
        {
            return Ok(None);
        }
        let definition = world.content.layer(layer)?;
        if !definition.simulated {
            return Ok(None);
        }
        let origin = world
            .channel_layout::<MaterialMotion>(force.channel)?
            .origin;
        let active_count = world.active_chunks().count();
        let mut active_chunks = Vec::new();
        active_chunks
            .try_reserve_exact(active_count)
            .map_err(allocation)?;
        let mut dependency_chunks = Vec::new();
        dependency_chunks
            .try_reserve_exact(active_count.saturating_mul(9))
            .map_err(allocation)?;
        let mut candidates = 0usize;
        for (coord, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            active_chunks.push((*coord, chunk.revision));
            let corner = coord.origin()?;
            let min_x = i64::from(corner.x) - i64::from(stencil.radius_x());
            let max_x = i64::from(corner.x) + 63 + i64::from(stencil.radius_x());
            let min_y = i64::from(corner.y) - i64::from(stencil.radius_y());
            let max_y = i64::from(corner.y) + 63 + i64::from(stencil.radius_y());
            let first = ChunkCoord::new(
                i32::try_from(min_x.div_euclid(64)).map_err(|_| invalid("Provider依存chunk"))?,
                i32::try_from(min_y.div_euclid(64)).map_err(|_| invalid("Provider依存chunk"))?,
            )?;
            let last = ChunkCoord::new(
                i32::try_from(max_x.div_euclid(64)).map_err(|_| invalid("Provider依存chunk"))?,
                i32::try_from(max_y.div_euclid(64)).map_err(|_| invalid("Provider依存chunk"))?,
            )?;
            for y in first.y..=last.y {
                for x in first.x..=last.x {
                    dependency_chunks.push(ChunkCoord { x, y });
                }
            }

            let page = &chunk.layers[layer.0 as usize];
            for (index, cell) in page.cells.iter().enumerate() {
                if cell.material == MaterialId::AIR
                    || page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                {
                    continue;
                }
                candidates = candidates
                    .checked_add(1)
                    .ok_or_else(|| invalid("Provider sleep候補数"))?;
                if candidates > force.candidate_limit {
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
                        .ok_or_else(|| invalid("Provider sleep速度channel水平端"))?,
                    y: point
                        .y
                        .checked_sub(origin.y)
                        .ok_or_else(|| invalid("Provider sleep速度channel垂直端"))?,
                };
                match world.channel_value::<MaterialMotion>(force.channel, channel_point)? {
                    super::ChannelQuery::Loaded(value) if value == MaterialMotion::default() => {}
                    _ => return Ok(None),
                }
            }
        }
        if candidates != report.force.candidates || candidates != report.motion.candidates {
            return Ok(None);
        }
        dependency_chunks.sort_unstable();
        dependency_chunks.dedup();
        Ok(Some(MaterialProviderSleepProof {
            world: world.id,
            cache_generation: Some(std::sync::Arc::clone(&world.cache_generation)),
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: world.revision(),
            layer: definition.key,
            channel: force.channel,
            provider: provider.key(),
            provider_version: provider.version(),
            gravity: force.gravity_q16,
            candidate_limit: force.candidate_limit,
            max_velocity: force.max_velocity_q16,
            max_distance: motion.max_distance,
            step_limit: motion.step_limit,
            stencil,
            active_chunks,
            dependency_chunks,
            candidates,
        }))
    }
}

impl World {
    /// 宣言stencilと作者不変条件がsleepを許すtickだけ、独自Provider callbackとDDAを省略します。
    ///
    /// Providerがstencilまたは不変条件を宣言しない場合、証明を破棄して参照stepを実行します。
    /// 不変条件はsleep中も毎tick呼び、作者状態・時間・非cell入力に依存する場合はそこで
    /// falseを返します。依存範囲のcell変更、active chunk変更、枝World、設定変更ではwakeします。
    /// 証明作成時のscanと依存chunk scratchは候補数とactive chunk数に比例し、sleep時は
    /// Providerの不変条件callbackと直近変更chunk数だけを追加確認します。
    pub fn step_material_force_motion_sleeping_with_provider(
        &mut self,
        tracker: &mut MaterialProviderSleepTracker,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialProviderSleepReport> {
        let slept_before = tracker.has_proof();
        let Some(stencil) = provider.sleep_stencil() else {
            tracker.wake();
            let reference =
                self.edit(|edit| edit.step_material_force_motion(layer, force, motion, provider))?;
            return Ok(MaterialProviderSleepReport {
                reference,
                sleeping: false,
                woke: slept_before,
            });
        };
        if provider.version() == 0 {
            return Err(invalid("力学Provider版0"));
        }
        if tracker.matching(self, layer, force, motion, provider, stencil)
            && provider
                .sleep_invariant(self, layer, force, motion)
                .unwrap_or(false)
        {
            let candidates = tracker.proof.as_ref().expect("照合済み").candidates;
            self.edit(|_| Ok(()))?;
            tracker.proof.as_mut().expect("検証済み").revision = self.revision();
            return Ok(MaterialProviderSleepReport {
                reference: MaterialForceMotionReport {
                    force: super::MaterialForceReport {
                        candidates,
                        changed: 0,
                        waiting_for_region: 0,
                    },
                    motion: super::MaterialMotionReport {
                        candidates,
                        ..Default::default()
                    },
                },
                sleeping: true,
                woke: false,
            });
        }
        tracker.wake();
        let reference =
            self.edit(|edit| edit.step_material_force_motion(layer, force, motion, provider))?;
        tracker.proof = MaterialProviderSleepTracker::prove(
            self, layer, force, motion, provider, stencil, reference,
        )
        .ok()
        .flatten();
        tracker.verified_in_process = tracker.proof.is_some();
        Ok(MaterialProviderSleepReport {
            reference,
            sleeping: false,
            woke: slept_before,
        })
    }
}

impl StateCodec for MaterialProviderSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHMPSLP1")?;
        let Some(proof) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        proof.world.encode(out)?;
        out.raw(&proof.content)?;
        out.u64(proof.epoch)?;
        out.u64(proof.revision)?;
        proof.layer.encode(out)?;
        proof.channel.encode(out)?;
        proof.provider.encode(out)?;
        out.u32(proof.provider_version)?;
        out.i64(proof.gravity[0])?;
        out.i64(proof.gravity[1])?;
        out.u64(
            u64::try_from(proof.candidate_limit).map_err(|_| invalid("Provider sleep候補容量"))?,
        )?;
        out.i64(proof.max_velocity)?;
        out.u32(proof.max_distance)?;
        out.u64(u64::try_from(proof.step_limit).map_err(|_| invalid("Provider sleep検査容量"))?)?;
        out.u8(proof.stencil.radius_x())?;
        out.u8(proof.stencil.radius_y())?;
        out.u64(u64::try_from(proof.candidates).map_err(|_| invalid("Provider sleep候補数"))?)?;
        out.u32(
            u32::try_from(proof.active_chunks.len())
                .map_err(|_| invalid("Provider sleep活動chunk数"))?,
        )?;
        for (coord, revision) in &proof.active_chunks {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        out.u32(
            u32::try_from(proof.dependency_chunks.len())
                .map_err(|_| invalid("Provider sleep依存chunk数"))?,
        )?;
        for coord in &proof.dependency_chunks {
            coord.encode(out)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHMPSLP1" {
            return Err(invalid("Provider sleep保存形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("Provider sleep存在bit")),
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("Provider sleepContent hash"))?;
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let layer = StableKey::decode(input)?;
        let channel = StableKey::decode(input)?;
        let provider = StableKey::decode(input)?;
        let provider_version = input.u32()?;
        let gravity = [input.i64()?, input.i64()?];
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("Provider sleep候補容量"))?;
        let max_velocity = input.i64()?;
        let max_distance = input.u32()?;
        let step_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("Provider sleep検査容量"))?;
        let stencil = MaterialForceDependencyStencil::new(input.u8()?, input.u8()?)?;
        let candidates =
            usize::try_from(input.u64()?).map_err(|_| invalid("Provider sleep候補数"))?;
        let active_count = input.u32()? as usize;
        if active_count > input.remaining() / 16 {
            return Err(invalid("Provider sleep活動chunk byte数"));
        }
        let mut active_chunks = Vec::new();
        active_chunks
            .try_reserve_exact(active_count)
            .map_err(allocation)?;
        for _ in 0..active_count {
            active_chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let dependency_count = input.u32()? as usize;
        if dependency_count > input.remaining() / 8 {
            return Err(invalid("Provider sleep依存chunk byte数"));
        }
        let mut dependency_chunks = Vec::new();
        dependency_chunks
            .try_reserve_exact(dependency_count)
            .map_err(allocation)?;
        for _ in 0..dependency_count {
            dependency_chunks.push(ChunkCoord::decode(input)?);
        }
        if provider_version == 0
            || max_velocity <= 0
            || max_distance == 0
            || candidates > candidate_limit
            || active_chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || dependency_chunks.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid("Provider sleep保存内容"));
        }
        Ok(Self {
            proof: Some(MaterialProviderSleepProof {
                world,
                cache_generation: None,
                content,
                epoch,
                revision,
                layer,
                channel,
                provider,
                provider_version,
                gravity,
                candidate_limit,
                max_velocity,
                max_distance,
                step_limit,
                stencil,
                active_chunks,
                dependency_chunks,
                candidates,
            }),
            verified_in_process: false,
        })
    }
}
