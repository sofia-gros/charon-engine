//! 正規物質粒子の静止tickだけを省略する保守的なsleep/wake。
//!
//! 粒子arenaと速度はWorldの正規状態として維持し、trackerは再計算可能な証明だけを
//! 持ちます。decode後やchunk編集後の証明は使わず、必ず通常の力積分とDDAへ戻します。

use super::{
    ChangeQuery, ChunkCoord, LayerId, MaterialForceProvider, MaterialParticleForceConfig,
    MaterialParticleForceStepReport, World, WorldEdit, invalid,
};
use crate::foundation::{
    EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

#[derive(Clone, Debug)]
struct ParticleSleepProof {
    world: WorldId,
    cache_generation: Option<std::sync::Arc<()>>,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    tick: u64,
    layer: StableKey,
    provider: StableKey,
    provider_version: u32,
    gravity_q16: [i64; 2],
    candidate_limit: usize,
    max_velocity_q16: i64,
    dda_step_limit: usize,
    report: MaterialParticleForceStepReport,
    particle_cells: Vec<CellPoint>,
    active_chunks: Vec<(ChunkCoord, u64)>,
}

/// 正規粒子の静止sleep証明を保持します。Worldの正規状態には含めない派生cacheです。
#[derive(Clone, Debug, Default)]
pub struct MaterialParticleSleepTracker {
    proof: Option<ParticleSleepProof>,
    verified_in_process: bool,
}

impl MaterialParticleSleepTracker {
    /// 現在の証明を破棄し、次回の参照実行を強制します。
    pub fn wake(&mut self) {
        self.proof = None;
        self.verified_in_process = false;
    }

    /// 失効前の証明を保持しているか返します。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matches(
        &self,
        world: &World,
        layer: LayerId,
        force: MaterialParticleForceConfig,
        dda_step_limit: usize,
        provider: &dyn MaterialForceProvider,
    ) -> bool {
        if !self.verified_in_process {
            return false;
        }
        let Some(proof) = &self.proof else {
            return false;
        };
        let Ok(definition) = world.content.layer(layer) else {
            return false;
        };
        let Some(tick_delta) = world.tick().checked_sub(proof.tick) else {
            return false;
        };
        if tick_delta > 1
            || proof.revision.checked_add(tick_delta) != Some(world.revision())
            || proof.world != world.id
            || proof
                .cache_generation
                .as_ref()
                .is_none_or(|source| !std::sync::Arc::ptr_eq(source, &world.cache_generation))
            || proof.content != world.content.fingerprint()
            || proof.epoch != world.remaps.epoch()
            || proof.layer != definition.key
            || proof.provider != provider.key()
            || proof.provider_version != provider.version()
            || proof.gravity_q16 != force.gravity_q16
            || proof.candidate_limit != force.candidate_limit
            || proof.max_velocity_q16 != force.max_velocity_q16
            || proof.dda_step_limit != dda_step_limit
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
        proof_revision: u64,
        layer: LayerId,
        force: MaterialParticleForceConfig,
        dda_step_limit: usize,
        provider: &dyn MaterialForceProvider,
        report: MaterialParticleForceStepReport,
    ) -> EngineResult<Option<ParticleSleepProof>> {
        if !world.content.layer(layer)?.simulated
            || report.force.changed != 0
            || report.force.waiting_for_region != 0
            || report.particles.moved != 0
            || report.particles.landed != 0
            || report.particles.waiting != 0
            || report.particles.checked_steps != 0
            || report.force.candidates != report.particles.processed
            || !provider.particle_sleep_invariant(world, layer, force)?
        {
            return Ok(None);
        }
        let mut particle_cells = Vec::new();
        for (_, particle) in world.material_particles() {
            if particle.layer() != layer
                || particle.is_archived()
                || !world
                    .chunks
                    .get(&ChunkCoord::at(particle.cell()))
                    .is_some_and(|chunk| chunk.active)
            {
                continue;
            }
            if particle.velocity_q16() != [0, 0] {
                return Ok(None);
            }
            particle_cells.try_reserve(1).map_err(allocation)?;
            particle_cells.push(particle.cell());
        }
        if particle_cells.len() != report.particles.processed {
            return Ok(None);
        }
        let active_count = world.active_chunks().count();
        let mut active_chunks = Vec::new();
        active_chunks
            .try_reserve_exact(active_count)
            .map_err(allocation)?;
        for (coordinate, chunk) in &world.chunks {
            if chunk.active {
                active_chunks.push((*coordinate, chunk.revision));
            }
        }
        Ok(Some(ParticleSleepProof {
            world: world.id,
            cache_generation: Some(std::sync::Arc::clone(&world.cache_generation)),
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: proof_revision,
            tick: world.tick(),
            layer: world.content.layer(layer)?.key,
            provider: provider.key(),
            provider_version: provider.version(),
            gravity_q16: force.gravity_q16,
            candidate_limit: force.candidate_limit,
            max_velocity_q16: force.max_velocity_q16,
            dda_step_limit,
            report,
            particle_cells,
            active_chunks,
        }))
    }
}

/// 参照particle force+DDAとsleepの結果を比較できる診断です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialParticleSleepReport {
    /// 参照実行と同じ候補数・移動数を返します。
    pub reference: MaterialParticleForceStepReport,
    /// Provider callbackとparticle DDAを省略した場合にtrueです。
    pub sleeping: bool,
    /// 以前の証明が失効して参照経路へ戻った場合にtrueです。
    pub woke: bool,
}

impl World {
    /// 静止をProviderが保証し、active Worldに変更がないtickだけ正規粒子処理を省略します。
    ///
    /// tracker復元後、設定・Provider・Content・remap・active chunk・World枝の変化では
    /// 参照経路へ戻ります。通常tick境界の空revision更新だけは許可します。sleep時も空の
    /// World取引を進め、参照実行とrevision推移を揃えます。
    pub fn step_material_particle_force_motion_sleeping(
        &mut self,
        tracker: &mut MaterialParticleSleepTracker,
        layer: LayerId,
        force: MaterialParticleForceConfig,
        dda_step_limit: usize,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialParticleSleepReport> {
        let had_proof = tracker.has_proof();
        let mut next_tracker = tracker.clone();
        if next_tracker.matches(self, layer, force, dda_step_limit, provider)
            && provider
                .particle_sleep_invariant(self, layer, force)
                .unwrap_or(false)
        {
            let report = next_tracker.proof.as_ref().expect("照合済み").report;
            // 参照DDAが候補粒子ごとに予約する編集予算を保ち、容量境界をsleepで回避しません。
            self.edit(|edit| edit.reserve_writes(report.particles.processed))?;
            let proof = next_tracker.proof.as_mut().expect("照合済み");
            proof.revision = self.revision();
            proof.tick = self.tick();
            *tracker = next_tracker;
            return Ok(MaterialParticleSleepReport {
                reference: report,
                sleeping: true,
                woke: false,
            });
        }

        next_tracker.wake();
        let report = self.edit(|edit| {
            if !edit.world.content.layer(layer)?.simulated {
                return Err(invalid("particle sleep対象はsimulated layerのみ"));
            }
            edit.step_material_particle_forces_in_layer(layer, force, provider, dda_step_limit)
        })?;
        next_tracker.proof = Self::proof_after_step(
            self,
            self.revision(),
            layer,
            force,
            dda_step_limit,
            provider,
            report,
        )?;
        next_tracker.verified_in_process = next_tracker.proof.is_some();
        *tracker = next_tracker;
        Ok(MaterialParticleSleepReport {
            reference: report,
            sleeping: false,
            woke: had_proof,
        })
    }

    fn proof_after_step(
        world: &World,
        proof_revision: u64,
        layer: LayerId,
        force: MaterialParticleForceConfig,
        dda_step_limit: usize,
        provider: &dyn MaterialForceProvider,
        report: MaterialParticleForceStepReport,
    ) -> EngineResult<Option<ParticleSleepProof>> {
        MaterialParticleSleepTracker::prove(
            world,
            proof_revision,
            layer,
            force,
            dda_step_limit,
            provider,
            report,
        )
    }
}

impl WorldEdit<'_> {
    /// Materials phase内で正規粒子をsleepし、共通予約へ不動粒子のsourceを渡します。
    pub(super) fn step_material_particle_force_motion_sleeping_reserved(
        &mut self,
        tracker: &mut MaterialParticleSleepTracker,
        layer: LayerId,
        force: MaterialParticleForceConfig,
        dda_step_limit: usize,
        provider: &dyn MaterialForceProvider,
        mut reservations: Option<&mut super::transport::CellReservations>,
    ) -> EngineResult<MaterialParticleSleepReport> {
        self.run(|edit| {
            let had_proof = tracker.has_proof();
            let mut next_tracker = tracker.clone();
            let reservation_conflict = next_tracker.proof.as_ref().is_some_and(|proof| {
                reservations.as_deref().is_some_and(|cells| {
                    proof
                        .particle_cells
                        .iter()
                        .any(|point| cells.contains(*point))
                })
            });
            if !reservation_conflict
                && next_tracker.matches(edit.world, layer, force, dda_step_limit, provider)
                && provider
                    .particle_sleep_invariant(edit.world, layer, force)
                    .unwrap_or(false)
            {
                let proof = next_tracker.proof.as_ref().expect("照合済み");
                let report = proof.report;
                edit.reserve_writes(report.particles.processed)?;
                if let Some(cells) = reservations.as_deref_mut() {
                    for point in &proof.particle_cells {
                        cells.insert(*point)?;
                    }
                }
                let proof_revision = edit
                    .world
                    .revision()
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revision上限"))?;
                let proof = next_tracker.proof.as_mut().expect("照合済み");
                proof.revision = proof_revision;
                proof.tick = edit.world.tick();
                *tracker = next_tracker;
                return Ok(MaterialParticleSleepReport {
                    reference: report,
                    sleeping: true,
                    woke: false,
                });
            }

            next_tracker.wake();
            let report = if let Some(cells) = reservations {
                edit.step_material_particle_forces_in_layer_reserved(
                    layer,
                    force,
                    provider,
                    dda_step_limit,
                    cells,
                )?
            } else {
                edit.step_material_particle_forces_in_layer(layer, force, provider, dda_step_limit)?
            };
            let proof_revision = edit
                .world
                .revision()
                .checked_add(1)
                .ok_or_else(|| invalid("World revision上限"))?;
            next_tracker.proof = MaterialParticleSleepTracker::prove(
                edit.world,
                proof_revision,
                layer,
                force,
                dda_step_limit,
                provider,
                report,
            )?;
            next_tracker.verified_in_process = next_tracker.proof.is_some();
            *tracker = next_tracker;
            Ok(MaterialParticleSleepReport {
                reference: report,
                sleeping: false,
                woke: had_proof,
            })
        })
    }
}

impl StateCodec for MaterialParticleSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHPSLP02")?;
        let Some(proof) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        proof.world.encode(out)?;
        out.raw(&proof.content)?;
        out.u64(proof.epoch)?;
        out.u64(proof.revision)?;
        out.u64(proof.tick)?;
        proof.layer.encode(out)?;
        proof.provider.encode(out)?;
        out.u32(proof.provider_version)?;
        out.i64(proof.gravity_q16[0])?;
        out.i64(proof.gravity_q16[1])?;
        out.u64(u64::try_from(proof.candidate_limit).map_err(|_| invalid("粒子sleep候補上限"))?)?;
        out.i64(proof.max_velocity_q16)?;
        out.u64(u64::try_from(proof.dda_step_limit).map_err(|_| invalid("粒子sleep DDA上限"))?)?;
        encode_report(out, proof.report)?;
        out.u32(
            u32::try_from(proof.particle_cells.len()).map_err(|_| invalid("粒子sleep粒子数"))?,
        )?;
        for point in &proof.particle_cells {
            point.encode(out)?;
        }
        out.u32(
            u32::try_from(proof.active_chunks.len()).map_err(|_| invalid("粒子sleep chunk数"))?,
        )?;
        for &(coordinate, revision) in &proof.active_chunks {
            coordinate.encode(out)?;
            out.u64(revision)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHPSLP02" {
            return Err(invalid("粒子sleep tracker形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("粒子sleep tracker tag")),
        }
        let world = WorldId::decode(input)?;
        let mut content = [0; 32];
        content.copy_from_slice(input.raw(32)?);
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let tick = input.u64()?;
        let layer = StableKey::decode(input)?;
        let provider = StableKey::decode(input)?;
        let provider_version = input.u32()?;
        let gravity_q16 = [input.i64()?, input.i64()?];
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("粒子sleep候補上限"))?;
        let max_velocity_q16 = input.i64()?;
        let dda_step_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("粒子sleep DDA上限"))?;
        let report = decode_report(input)?;
        let particle_count = input.u32()? as usize;
        if particle_count > 1_000_000 || particle_count != report.particles.processed {
            return Err(invalid("粒子sleep particle数上限・report不一致"));
        }
        let mut particle_cells = Vec::new();
        particle_cells
            .try_reserve_exact(particle_count)
            .map_err(allocation)?;
        for _ in 0..particle_count {
            particle_cells.push(CellPoint::decode(input)?);
        }
        let count = input.u32()? as usize;
        if count > 1_000_000 {
            return Err(invalid("粒子sleep chunk数上限"));
        }
        let mut active_chunks = Vec::new();
        active_chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            active_chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        if active_chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(invalid("粒子sleep chunk順序"));
        }
        Ok(Self {
            proof: Some(ParticleSleepProof {
                world,
                cache_generation: None,
                content,
                epoch,
                revision,
                tick,
                layer,
                provider,
                provider_version,
                gravity_q16,
                candidate_limit,
                max_velocity_q16,
                dda_step_limit,
                report,
                particle_cells,
                active_chunks,
            }),
            verified_in_process: false,
        })
    }
}

fn encode_report(out: &mut Encoder, report: MaterialParticleForceStepReport) -> EngineResult<()> {
    for value in [
        report.force.candidates,
        report.force.waiting_for_region,
        report.force.changed,
        report.particles.processed,
        report.particles.moved,
        report.particles.landed,
        report.particles.waiting,
        report.particles.checked_steps,
    ] {
        out.u64(u64::try_from(value).map_err(|_| invalid("粒子sleep report値"))?)?;
    }
    Ok(())
}

fn decode_report(input: &mut Decoder<'_>) -> EngineResult<MaterialParticleForceStepReport> {
    let mut values = [0usize; 8];
    for value in &mut values {
        *value = usize::try_from(input.u64()?).map_err(|_| invalid("粒子sleep report値"))?;
    }
    Ok(MaterialParticleForceStepReport {
        force: super::MaterialParticleForceReport {
            candidates: values[0],
            waiting_for_region: values[1],
            changed: values[2],
        },
        particles: super::MaterialParticleReport {
            processed: values[3],
            moved: values[4],
            landed: values[5],
            waiting: values[6],
            checked_steps: values[7],
        },
    })
}
