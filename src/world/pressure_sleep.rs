//! 参照圧力緩和が固定したfieldだけを省略する保守的なsleep経路。
//!
//! 圧力field自体はWorldの正規channelです。ここに保存するのは反復を省くための
//! 派生証明だけであり、設定・活動chunk・World枝・Contentが変わったときは参照
//! 緩和を実行します。保存から復元した証明も実Worldで再確認するまでは使いません。

use super::{
    ChangeQuery, ChunkCoord, LayerId, PressureRelaxation, PressureRelaxationReport, World, invalid,
};
use crate::foundation::{
    EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    id::WorldId,
};

/// 参照実行とsleepを区別する圧力phaseの結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PressureSleepStepReport {
    /// 参照圧力solverと同じ診断値です。
    pub reference: PressureRelaxationReport,
    /// この呼出で圧力field走査とJacobi反復を省略したか。
    pub sleeping: bool,
    /// 保持していた証明が失効し、参照solverへ戻ったか。
    pub woke: bool,
}

#[derive(Clone, Debug)]
struct PressureSleepProof {
    world: WorldId,
    cache_generation: Option<std::sync::Arc<()>>,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    layer: StableKey,
    config: PressureRelaxation,
    report: PressureRelaxationReport,
    active_chunks: Vec<(ChunkCoord, u64)>,
}

/// 圧力sleep証明を管理します。Worldの正規channelとは別に保存してください。
#[derive(Clone, Debug, Default)]
pub struct PressureSleepTracker {
    proof: Option<PressureSleepProof>,
    verified_in_process: bool,
}

impl PressureSleepTracker {
    /// 現在の証明を明示的に破棄します。
    pub fn wake(&mut self) {
        self.proof = None;
        self.verified_in_process = false;
    }

    /// 失効前の証明を保持しているか返します。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matches(&self, world: &World, layer: LayerId, config: PressureRelaxation) -> bool {
        if !self.verified_in_process {
            return false;
        }
        let Some(proof) = &self.proof else {
            return false;
        };
        let Ok(definition) = world.content.layer(layer) else {
            return false;
        };
        if proof.world != world.id
            || proof
                .cache_generation
                .as_ref()
                .is_none_or(|source| !std::sync::Arc::ptr_eq(source, &world.cache_generation))
            || proof.content != world.content.fingerprint()
            || proof.epoch != world.remaps.epoch()
            || proof.layer != definition.key
            || proof.config.channel != config.channel
            || proof.config.iterations != config.iterations
            || proof.config.gravity_q16 != config.gravity_q16
            || proof.config.cell_limit != config.cell_limit
        {
            return false;
        }
        match world.changes_after(proof.revision) {
            ChangeQuery::Current => {}
            ChangeQuery::Delta(delta) if delta.chunks.is_empty() => {}
            _ => return false,
        }
        let mut active = world.active_chunks();
        for &(coordinate, revision) in &proof.active_chunks {
            if active.next() != Some(coordinate)
                || !world
                    .chunks
                    .get(&coordinate)
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
        config: PressureRelaxation,
        report: PressureRelaxationReport,
        changed: usize,
    ) -> EngineResult<Option<PressureSleepProof>> {
        // 反復の出力が入力fieldと完全一致したときだけ省略可能です。
        if changed != 0 || !world.content.layer(layer)?.simulated {
            return Ok(None);
        }
        let count = world.active_chunks().count();
        let mut active_chunks = Vec::new();
        active_chunks.try_reserve_exact(count).map_err(allocation)?;
        for (coordinate, chunk) in &world.chunks {
            if chunk.active {
                active_chunks.push((*coordinate, chunk.revision));
            }
        }
        Ok(Some(PressureSleepProof {
            world: world.id,
            cache_generation: Some(std::sync::Arc::clone(&world.cache_generation)),
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: world.revision(),
            layer: world.content.layer(layer)?.key,
            config,
            report,
            active_chunks,
        }))
    }
}

impl World {
    /// Jacobi pressure solverが固定したfieldだけを、参照結果を保って省略します。
    ///
    /// sleepは同じWorld取引を一度進め、revision推移を通常の参照実行と揃えます。
    /// channel・地形・活動chunkの変更、設定変更、別World枝、Content/remap変更では
    /// wakeして参照緩和を実行します。復元したtrackerは必ず一度参照実行して再証明します。
    pub fn relax_pressure_sleeping(
        &mut self,
        tracker: &mut PressureSleepTracker,
        layer: LayerId,
        config: PressureRelaxation,
    ) -> EngineResult<PressureSleepStepReport> {
        let had_proof = tracker.has_proof();
        if tracker.matches(self, layer, config) {
            let report = tracker.proof.as_ref().expect("照合済み").report;
            self.edit(|_| Ok(()))?;
            tracker.proof.as_mut().expect("検証済み").revision = self.revision();
            return Ok(PressureSleepStepReport {
                reference: report,
                sleeping: true,
                woke: false,
            });
        }
        tracker.wake();
        let (report, changed) =
            self.edit(|edit| edit.relax_pressure_with_change_count(layer, config))?;
        tracker.proof = PressureSleepTracker::prove(self, layer, config, report, changed)
            .ok()
            .flatten();
        tracker.verified_in_process = tracker.proof.is_some();
        Ok(PressureSleepStepReport {
            reference: report,
            sleeping: false,
            woke: had_proof,
        })
    }
}

impl StateCodec for PressureSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHPSLP01")?;
        let Some(proof) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        proof.world.encode(out)?;
        out.raw(&proof.content)?;
        out.u64(proof.epoch)?;
        out.u64(proof.revision)?;
        proof.layer.encode(out)?;
        proof.config.channel.encode(out)?;
        out.u16(proof.config.iterations)?;
        out.u32(proof.config.gravity_q16)?;
        out.u64(u64::try_from(proof.config.cell_limit).map_err(|_| invalid("圧力sleep容量"))?)?;
        out.u64(u64::try_from(proof.report.cells).map_err(|_| invalid("圧力sleep cell数"))?)?;
        out.u64(
            u64::try_from(proof.report.open_components)
                .map_err(|_| invalid("圧力sleep開領域数"))?,
        )?;
        out.u64(
            u64::try_from(proof.report.closed_components)
                .map_err(|_| invalid("圧力sleep閉領域数"))?,
        )?;
        out.u32(
            u32::try_from(proof.active_chunks.len()).map_err(|_| invalid("圧力sleep chunk数"))?,
        )?;
        for (coordinate, revision) in &proof.active_chunks {
            coordinate.encode(out)?;
            out.u64(*revision)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHPSLP01" {
            return Err(invalid("圧力sleep保存形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("圧力sleep存在bit")),
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("圧力sleep Content hash"))?;
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let layer = StableKey::decode(input)?;
        let channel = StableKey::decode(input)?;
        let iterations = input.u16()?;
        let gravity_q16 = input.u32()?;
        let cell_limit = usize::try_from(input.u64()?).map_err(|_| invalid("圧力sleep容量"))?;
        let report = PressureRelaxationReport {
            cells: usize::try_from(input.u64()?).map_err(|_| invalid("圧力sleep cell数"))?,
            open_components: usize::try_from(input.u64()?)
                .map_err(|_| invalid("圧力sleep開領域数"))?,
            closed_components: usize::try_from(input.u64()?)
                .map_err(|_| invalid("圧力sleep閉領域数"))?,
        };
        let count = input.u32()? as usize;
        let components = report
            .open_components
            .checked_add(report.closed_components)
            .ok_or_else(|| invalid("圧力sleep連結領域数"))?;
        if iterations == 0
            || report.cells > cell_limit
            || report.open_components > report.cells
            || report.closed_components > report.cells
            || components > report.cells
            || count > input.remaining() / 16
        {
            return Err(invalid("圧力sleep保存容量と設定"));
        }
        let mut active_chunks = Vec::new();
        active_chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            active_chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        if active_chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(invalid("圧力sleep活動chunk保存順"));
        }
        Ok(Self {
            proof: Some(PressureSleepProof {
                world,
                cache_generation: None,
                content,
                epoch,
                revision,
                layer,
                config: PressureRelaxation {
                    channel,
                    iterations,
                    gravity_q16,
                    cell_limit,
                },
                report,
                active_chunks,
            }),
            verified_in_process: false,
        })
    }
}
