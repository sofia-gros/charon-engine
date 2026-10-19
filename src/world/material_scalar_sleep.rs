//! 標準の粉体・液体・気体scalar輸送に対する保存可能な静止証明。
//!
//! 両tick parityを保守的に満たす「一つも交換候補がない」状態だけをsleepし、
//! 作者ruleや別phaseの進行を勝手に省略しません。

use super::{
    CellQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, MaterialTransportReport, World,
    invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 標準scalarの静止証明。正規Worldやchannelの所有物にはなりません。
#[derive(Clone)]
struct ScalarSleepProof {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    layer: StableKey,
    candidate_limit: usize,
    candidates: usize,
    /// page identityは保存しない。実行中の同一枝だけで証明を再利用する。
    active: Vec<(
        ChunkCoord,
        u64,
        Option<std::sync::Arc<super::terrain::ChunkPage>>,
    )>,
}
impl std::fmt::Debug for ScalarSleepProof {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScalarSleepProof")
            .field("world", &self.world)
            .field("content", &self.content)
            .field("epoch", &self.epoch)
            .field("revision", &self.revision)
            .field("layer", &self.layer)
            .field("candidate_limit", &self.candidate_limit)
            .field("candidates", &self.candidates)
            .field("active_chunks", &self.active.len())
            .finish()
    }
}

/// 標準scalar専用の派生sleep/wakeトラッカー。
///
/// 新たなWorldへrestoreした証明は未認証として扱い、必ず参照stepを実行して
/// 地形・密度・活性域を再検証します。独自Systemからもwakeを明示できます。
#[derive(Clone, Debug, Default)]
pub struct ScalarMaterialSleepTracker {
    proof: Option<ScalarSleepProof>,
    candidate_cache: Option<ScalarCandidateCache>,
    verified: bool,
}

/// revisionが保たれている固体のみchunkの候補走査を省く派生cache。
#[derive(Clone)]
struct ScalarCandidateCache {
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    candidate_limit: usize,
    chunks: Vec<CachedScalarChunk>,
}
impl std::fmt::Debug for ScalarCandidateCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScalarCandidateCache")
            .field("content", &self.content)
            .field("epoch", &self.epoch)
            .field("layer", &self.layer)
            .field("candidate_limit", &self.candidate_limit)
            .field("chunks", &self.chunks)
            .finish()
    }
}

#[derive(Clone)]
struct CachedScalarChunk {
    coord: ChunkCoord,
    revision: u64,
    page: std::sync::Arc<super::terrain::ChunkPage>,
    candidates: usize,
    /// 空気以外の非solid候補。休止中も保持し、wake時の全cell走査を避ける。
    points: Vec<CellPoint>,
    /// 隣接1cellの依存状態と内部候補の停止を検証済みか。
    sleeping: bool,
    /// 1cell haloの非既定値だけを座標順で保持します。
    halo: Vec<(u16, u64)>,
}
impl std::fmt::Debug for CachedScalarChunk {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CachedScalarChunk")
            .field("coord", &self.coord)
            .field("revision", &self.revision)
            .field("candidates", &self.candidates)
            .field("points", &self.points.len())
            .field("sleeping", &self.sleeping)
            .field("halo", &self.halo.len())
            .finish()
    }
}

/// 標準scalar輸送と同じ変更・候補数、および実際のsleep有無。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarMaterialSleepReport {
    /// 省略しなかった場合の参照輸送結果と同一。
    pub reference: MaterialTransportReport,
    /// 静止証明によって全cell走査を省略できたか。
    pub sleeping: bool,
    /// 参照輸送を省いたchunk数。全体sleepでない局所skipも数えます。
    pub sleeping_chunks: usize,
    /// 直前の証明が外部変更または設定変更によって失効したか。
    pub woke: bool,
}

impl ScalarMaterialSleepTracker {
    /// 外部の作者ruleや未申告の依存関係から手動で起こす場合にも使用します。
    pub fn wake(&mut self) {
        self.proof = None;
        self.candidate_cache = None;
        self.verified = false;
    }

    /// 以前の評価で証明された候補があるか。復元後の未認証状態も含みます。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matching(&self, world: &World, layer: LayerId, limit: usize) -> bool {
        if has_layer_particles(world, layer) {
            return false;
        }
        if !self.verified {
            return false;
        }
        let Some(proof) = &self.proof else {
            return false;
        };
        if proof.world != world.id
            || proof.content != world.content.fingerprint()
            || proof.epoch != world.remaps.epoch()
            || proof.candidate_limit != limit
            || world.content.layer(layer).is_err()
            || world
                .content
                .layer(layer)
                .is_ok_and(|definition| definition.key != proof.layer)
        {
            return false;
        }
        match world.changes_after(proof.revision) {
            super::ChangeQuery::Current => {}
            super::ChangeQuery::Delta(delta) if delta.chunks.is_empty() => {}
            _ => return false,
        }
        let mut current = world.active_chunks();
        for (coord, revision, page) in &proof.active {
            if current.next() != Some(*coord)
                || !world.chunks.get(coord).is_some_and(|chunk| {
                    chunk.active
                        && chunk.revision == *revision
                        && page.as_ref().is_some_and(|page| {
                            std::sync::Arc::ptr_eq(
                                &chunk.layers[layer_index(world, proof.layer)],
                                page,
                            )
                        })
                })
            {
                return false;
            }
        }
        current.next().is_none()
    }

    /// 静止の証明は走査順と無関係な全候補の局所交換可能性を検査します。
    ///
    /// 候補が一つでも移動できる場合はどちらのparityで最後に動くかを
    /// 推測せず参照輸送へ戻すため、sleepしたtickの並び替えが結果を変えません。
    fn prove(
        world: &World,
        layer: LayerId,
        limit: usize,
        report: MaterialTransportReport,
        proof_revision: u64,
    ) -> EngineResult<Option<ScalarSleepProof>> {
        if report.exchanges != 0
            || has_layer_particles(world, layer)
            || !world.content.layer(layer)?.simulated
        {
            return Ok(None);
        }
        let mut active = Vec::new();
        active
            .try_reserve_exact(world.active_chunks().count())
            .map_err(allocation)?;
        let mut candidates = 0usize;
        for (coord, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            active.push((
                *coord,
                chunk.revision,
                Some(std::sync::Arc::clone(&chunk.layers[layer.0 as usize])),
            ));
            let origin = coord.origin()?;
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
                candidates += 1;
                if candidates > limit {
                    return Ok(None);
                }
                let material = world
                    .content
                    .material(cell.material)?
                    .ok_or_else(|| invalid("scalar sleepの未定義素材"))?;
                let position = CellPoint {
                    x: origin.x + (index % 64) as i32,
                    y: origin.y + (index / 64) as i32,
                };
                let choices: &[(i32, i32)] = match material.base() {
                    MaterialBase::Solid => &[],
                    MaterialBase::Powder => &[(0, 1), (-1, 1), (1, 1)],
                    MaterialBase::Liquid => &[(0, 1), (-1, 0), (1, 0)],
                    MaterialBase::Gas => &[(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)],
                };
                for &(dx, dy) in choices {
                    let Some(target) = add(position, dx, dy) else {
                        continue;
                    };
                    let Some(dst) = resident_active(world, layer, target)? else {
                        continue;
                    };
                    if dst.block.is_some() {
                        continue;
                    }
                    let can_swap = if dst.core.material == MaterialId::AIR {
                        true
                    } else {
                        let other = world
                            .content
                            .material(dst.core.material)?
                            .ok_or_else(|| invalid("scalar sleepの未定義交換先"))?;
                        matches!(other.base(), MaterialBase::Liquid | MaterialBase::Gas)
                            && dst.core.material != cell.material
                            && ((dy > 0 && material.mass_per_cell() > other.mass_per_cell())
                                || (dy < 0 && material.mass_per_cell() < other.mass_per_cell()))
                    };
                    if !can_swap {
                        continue;
                    }
                    if dx != 0
                        && dy != 0
                        && !free_corner(world, layer, add(position, dx, 0))?
                        && !free_corner(world, layer, add(position, 0, dy))?
                    {
                        continue;
                    }
                    // 一候補でも交換できればこの活動集合はまだ休止できません。
                    return Ok(None);
                }
            }
        }
        if candidates != report.candidates {
            return Ok(None);
        }
        Ok(Some(ScalarSleepProof {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: proof_revision,
            layer: world.content.layer(layer)?.key,
            candidate_limit: limit,
            candidates,
            active,
        }))
    }
}

fn add(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}

fn resident_active(
    world: &World,
    layer: LayerId,
    point: CellPoint,
) -> EngineResult<Option<super::CellView>> {
    if !world
        .chunks
        .get(&ChunkCoord::at(point))
        .is_some_and(|chunk| chunk.active)
    {
        return Ok(None);
    }
    match world.cell(layer, point)? {
        CellQuery::Loaded(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn free_corner(world: &World, layer: LayerId, point: Option<CellPoint>) -> EngineResult<bool> {
    let Some(point) = point else {
        return Ok(false);
    };
    Ok(matches!(
        resident_active(world, layer, point)?,
        Some(value) if value.block.is_none() && value.core.material == MaterialId::AIR
    ))
}

fn has_layer_particles(world: &World, layer: LayerId) -> bool {
    world
        .particles
        .iter()
        .any(|(_, particle)| !particle.is_archived() && particle.layer == layer)
}

impl World {
    /// 通常scalarで両側を調べても交換できないと証明されたtickだけ休止します。
    ///
    /// 全active chunkのrevision、World/Content、remap、対象layerと上限を
    /// 保守的に照合し、失効時は何も推測せず参照solverを一度実行します。
    /// 睡眠時も通常World::edit一回分の公開revisionを進めます。
    pub fn step_scalar_materials_sleeping(
        &mut self,
        tracker: &mut ScalarMaterialSleepTracker,
        layer: LayerId,
        candidate_limit: usize,
    ) -> EngineResult<ScalarMaterialSleepReport> {
        let previously_asleep = tracker.has_proof();
        if tracker.matching(self, layer, candidate_limit) {
            let candidates = tracker.proof.as_ref().expect("検証済み").candidates;
            self.edit(|_| Ok(()))?;
            tracker.proof.as_mut().expect("検証済み").revision = self.revision();
            return Ok(ScalarMaterialSleepReport {
                reference: MaterialTransportReport {
                    candidates,
                    exchanges: 0,
                },
                sleeping: true,
                sleeping_chunks: tracker.proof.as_ref().map_or(0, |proof| proof.active.len()),
                woke: false,
            });
        }
        tracker.proof = None;
        tracker.verified = false;
        let previous = tracker.candidate_cache.take();
        let (points, candidate_count, next_cache, sleeping_chunks, woke_chunks) =
            cached_candidates(self, layer, candidate_limit, previous)?;
        let reference = self.edit(|edit| {
            edit.transport_materials_from_candidates(
                layer,
                candidate_limit,
                candidate_count,
                points,
            )
        })?;
        tracker.candidate_cache = Some(next_cache);
        tracker.proof = ScalarMaterialSleepTracker::prove(
            self,
            layer,
            candidate_limit,
            reference,
            self.revision(),
        )
        .ok()
        .flatten();
        tracker.verified = tracker.proof.is_some();
        Ok(ScalarMaterialSleepReport {
            reference,
            sleeping: false,
            sleeping_chunks,
            woke: previously_asleep || woke_chunks,
        })
    }
}

impl super::WorldEdit<'_> {
    /// Materials phase内で標準scalar sleepを実行し、同じ予約とWorld候補を共有します。
    ///
    /// トラッカーを値で受け取り、phase成功時だけ更新値を返すため、後段solverの失敗で
    /// World候補が破棄されても呼出側の証明だけが先へ進みません。返す証明のrevisionは
    /// 外側WorldEditが成功commitする次revisionです。候補cache自体は保存codecへ含めません。
    pub(super) fn step_scalar_materials_sleeping(
        &mut self,
        mut tracker: ScalarMaterialSleepTracker,
        layer: LayerId,
        candidate_limit: usize,
        reservations: &mut super::transport::CellReservations,
    ) -> EngineResult<(ScalarMaterialSleepReport, ScalarMaterialSleepTracker)> {
        self.run(|editor| {
            let previously_asleep = tracker.has_proof();
            if tracker.matching(editor.world, layer, candidate_limit) {
                let candidates = tracker.proof.as_ref().expect("検証済み").candidates;
                tracker.proof.as_mut().expect("検証済み").revision = editor
                    .world
                    .revision()
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revisionの上限"))?;
                return Ok((
                    ScalarMaterialSleepReport {
                        reference: MaterialTransportReport {
                            candidates,
                            exchanges: 0,
                        },
                        sleeping: true,
                        sleeping_chunks: tracker
                            .proof
                            .as_ref()
                            .map_or(0, |proof| proof.active.len()),
                        woke: false,
                    },
                    tracker,
                ));
            }
            tracker.proof = None;
            tracker.verified = false;
            let previous = tracker.candidate_cache.take();
            let (points, candidate_count, next_cache, sleeping_chunks, woke_chunks) =
                cached_candidates(editor.world, layer, candidate_limit, previous)?;
            let reference = editor.transport_material_candidates_from_candidates_reserved(
                layer,
                candidate_limit,
                candidate_count,
                points,
                reservations,
            )?;
            tracker.candidate_cache = Some(next_cache);
            let proof_revision = editor
                .world
                .revision()
                .checked_add(1)
                .ok_or_else(|| invalid("World revisionの上限"))?;
            tracker.proof = ScalarMaterialSleepTracker::prove(
                editor.world,
                layer,
                candidate_limit,
                reference,
                proof_revision,
            )
            .ok()
            .flatten();
            tracker.verified = tracker.proof.is_some();
            Ok((
                ScalarMaterialSleepReport {
                    reference,
                    sleeping: false,
                    sleeping_chunks,
                    woke: previously_asleep || woke_chunks,
                },
                tracker,
            ))
        })
    }
}

fn layer_index(world: &World, layer_key: StableKey) -> usize {
    world
        .content
        .layers
        .iter()
        .position(|definition| definition.key == layer_key)
        .expect("証明済みlayer")
}

fn cached_candidates(
    world: &World,
    layer: LayerId,
    limit: usize,
    previous: Option<ScalarCandidateCache>,
) -> EngineResult<(Vec<CellPoint>, usize, ScalarCandidateCache, usize, bool)> {
    let definition = world.content.layer(layer)?;
    if !definition.simulated {
        return Err(invalid("物質輸送の対象layerはsimulatedでなければならない"));
    }
    let context_matches = previous.as_ref().is_some_and(|cache| {
        cache.content == world.content.fingerprint()
            && cache.epoch == world.remaps.epoch()
            && cache.layer == definition.key
            && cache.candidate_limit == limit
    });
    let old_chunks = previous
        .filter(|_| context_matches)
        .map_or_else(Vec::new, |cache| cache.chunks)
        .into_iter();
    let mut old_chunks = old_chunks.peekable();
    let mut points = Vec::new();
    let mut chunks = Vec::new();
    let active_count = world.active_chunks().count();
    points.try_reserve(active_count).map_err(allocation)?;
    chunks.try_reserve_exact(active_count).map_err(allocation)?;
    let mut total = 0usize;
    let mut sleeping_chunks = 0usize;
    let mut woke_chunks = false;
    let particles_present = has_layer_particles(world, layer);
    for (&coord, chunk) in &world.chunks {
        if !chunk.active {
            continue;
        }
        while old_chunks.peek().is_some_and(|entry| entry.coord < coord) {
            old_chunks.next();
        }
        let old = old_chunks
            .peek()
            .is_some_and(|entry| context_matches && entry.coord == coord)
            .then(|| old_chunks.next().expect("peekした候補"));
        let current_page = &chunk.layers[layer.0 as usize];
        let old_was_sleeping = old.as_ref().is_some_and(|entry| entry.sleeping);
        let old_page_matches = old.as_ref().is_some_and(|entry| {
            entry.revision == chunk.revision && std::sync::Arc::ptr_eq(&entry.page, current_page)
        });
        let (candidate_count, local_points, mut halo, was_sleeping, halo_unchanged, halo_dynamic) =
            if old_page_matches {
                let mut entry = old.expect("page一致の候補");
                let (same, dynamic) = halo_matches_and_dynamic(world, layer, coord, &entry.halo)?;
                (
                    entry.candidates,
                    std::mem::take(&mut entry.points),
                    std::mem::take(&mut entry.halo),
                    entry.sleeping,
                    same,
                    dynamic,
                )
            } else {
                let (count, local) = scan_scalar_chunk(world, layer, coord, limit)?;
                (
                    count,
                    local,
                    Vec::new(),
                    old_was_sleeping,
                    false,
                    halo_has_dynamic_candidate(world, layer, coord)?,
                )
            };
        total = total
            .checked_add(candidate_count)
            .ok_or_else(|| invalid("scalar候補数overflow"))?;
        if total > limit {
            return Err(EngineError::capacity(
                "物質輸送の候補数",
                limit.saturating_add(1) as u64,
                limit as u64,
            ));
        }
        if was_sleeping && !halo_unchanged {
            woke_chunks = true;
        }
        let mut sleeping = was_sleeping && halo_unchanged && !particles_present;
        if was_sleeping && particles_present {
            woke_chunks = true;
        }
        if !sleeping && !halo_unchanged && !particles_present && candidate_count != 0 {
            let stopped = !halo_dynamic && chunk_sources_stopped(world, layer, &local_points)?;
            sleeping = stopped;
        }
        if sleeping && candidate_count != 0 {
            sleeping_chunks += 1;
        } else {
            points.try_reserve(local_points.len()).map_err(allocation)?;
            points.extend(local_points.iter().copied());
        }
        if !halo_unchanged {
            halo = capture_halo(world, layer, coord)?;
        }
        chunks.push(CachedScalarChunk {
            coord,
            revision: chunk.revision,
            page: std::sync::Arc::clone(&chunk.layers[layer.0 as usize]),
            candidates: candidate_count,
            points: local_points,
            sleeping,
            halo,
        });
    }
    let left_first = world.tick().is_multiple_of(2);
    points.sort_unstable_by(|a, b| {
        b.y.cmp(&a.y).then_with(|| {
            if left_first {
                a.x.cmp(&b.x)
            } else {
                b.x.cmp(&a.x)
            }
        })
    });
    Ok((
        points,
        total,
        ScalarCandidateCache {
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: definition.key,
            candidate_limit: limit,
            chunks,
        },
        sleeping_chunks,
        woke_chunks,
    ))
}

fn scan_scalar_chunk(
    world: &World,
    layer: LayerId,
    coord: ChunkCoord,
    limit: usize,
) -> EngineResult<(usize, Vec<CellPoint>)> {
    let chunk = world
        .chunks
        .get(&coord)
        .ok_or_else(|| invalid("scalar候補chunkが未読込"))?;
    let page = &chunk.layers[layer.0 as usize];
    let origin = coord.origin()?;
    let mut candidates = 0usize;
    let mut points = Vec::new();
    points.try_reserve(16).map_err(allocation)?;
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
            .ok_or_else(|| invalid("scalar chunk候補数overflow"))?;
        if candidates > limit {
            return Err(EngineError::capacity(
                "物質輸送の候補数",
                candidates as u64,
                limit as u64,
            ));
        }
        let material = world
            .content
            .material(cell.material)?
            .ok_or_else(|| invalid("scalar候補cacheの未定義素材"))?;
        if material.base() != MaterialBase::Solid {
            points.try_reserve(1).map_err(allocation)?;
            points.push(CellPoint {
                x: origin.x + (index % 64) as i32,
                y: origin.y + (index / 64) as i32,
            });
        }
    }
    Ok((candidates, points))
}

fn chunk_sources_stopped(
    world: &World,
    layer: LayerId,
    points: &[CellPoint],
) -> EngineResult<bool> {
    for &point in points {
        if scalar_can_move_from(world, layer, point)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn scalar_can_move_from(world: &World, layer: LayerId, source: CellPoint) -> EngineResult<bool> {
    let Some(source_cell) = resident_active(world, layer, source)? else {
        return Ok(false);
    };
    if source_cell.block.is_some() || source_cell.core.material == MaterialId::AIR {
        return Ok(false);
    }
    let material = world
        .content
        .material(source_cell.core.material)?
        .ok_or_else(|| invalid("scalar sleepの未定義素材"))?;
    let choices: &[(i32, i32)] = match material.base() {
        MaterialBase::Solid => &[],
        MaterialBase::Powder => &[(0, 1), (-1, 1), (1, 1)],
        MaterialBase::Liquid => &[(0, 1), (-1, 0), (1, 0)],
        MaterialBase::Gas => &[(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)],
    };
    for &(dx, dy) in choices {
        let Some(target) = add(source, dx, dy) else {
            continue;
        };
        let Some(destination) = resident_active(world, layer, target)? else {
            continue;
        };
        if destination.block.is_some() {
            continue;
        }
        let can_swap = if destination.core.material == MaterialId::AIR {
            true
        } else {
            let other = world
                .content
                .material(destination.core.material)?
                .ok_or_else(|| invalid("scalar sleepの未定義交換先"))?;
            matches!(other.base(), MaterialBase::Liquid | MaterialBase::Gas)
                && destination.core.material != source_cell.core.material
                && ((dy > 0 && material.mass_per_cell() > other.mass_per_cell())
                    || (dy < 0 && material.mass_per_cell() < other.mass_per_cell()))
        };
        if !can_swap {
            continue;
        }
        if dx != 0
            && dy != 0
            && !free_corner(world, layer, add(source, dx, 0))?
            && !free_corner(world, layer, add(source, 0, dy))?
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn halo_has_dynamic_candidate(
    world: &World,
    layer: LayerId,
    coord: ChunkCoord,
) -> EngineResult<bool> {
    Ok(halo_matches_and_dynamic(world, layer, coord, &[])?.1)
}

fn halo_state(world: &World, layer: LayerId, point: CellPoint) -> EngineResult<u64> {
    match world.cell(layer, point)? {
        CellQuery::WorldBoundary => Ok(0),
        CellQuery::Unavailable(_) => Ok(1),
        CellQuery::Loaded(cell) => {
            Ok(2 + u64::from(cell.core.material.0) * 2 + u64::from(cell.block.is_some()))
        }
    }
}

fn visit_halo(
    coord: ChunkCoord,
    mut visit: impl FnMut(u16, CellPoint) -> EngineResult<()>,
) -> EngineResult<()> {
    let origin = coord.origin()?;
    let right = origin.x.checked_add(64);
    let bottom = origin.y.checked_add(64);
    let left = origin.x.checked_sub(1);
    let top = origin.y.checked_sub(1);
    let mut index = 0u16;
    let mut emit = |point: Option<CellPoint>| -> EngineResult<()> {
        let current = index;
        index = index
            .checked_add(1)
            .ok_or_else(|| invalid("scalar halo index overflow"))?;
        if let Some(point) = point {
            visit(current, point)?;
        }
        Ok(())
    };
    for y in 0..64 {
        let y = origin.y + y;
        emit(left.map(|x| CellPoint { x, y }))?;
        emit(right.map(|x| CellPoint { x, y }))?;
    }
    for x in 0..64 {
        let x = origin.x + x;
        emit(top.map(|y| CellPoint { x, y }))?;
        emit(bottom.map(|y| CellPoint { x, y }))?;
    }
    for x in [left, right] {
        for y in [top, bottom] {
            emit(x.zip(y).map(|(x, y)| CellPoint { x, y }))?;
        }
    }
    Ok(())
}

fn halo_matches_and_dynamic(
    world: &World,
    layer: LayerId,
    coord: ChunkCoord,
    expected: &[(u16, u64)],
) -> EngineResult<(bool, bool)> {
    let mut entry_index = 0usize;
    let mut matches = true;
    let mut dynamic = false;
    visit_halo(coord, |position, point| {
        let state = halo_state(world, layer, point)?;
        if state != 2 {
            if expected.get(entry_index) != Some(&(position, state)) {
                matches = false;
            }
            entry_index += 1;
        }
        if state > 2 {
            let material = MaterialId(((state - 2) / 2) as u32);
            let blocked = state % 2 == 1;
            if !blocked && material != MaterialId::AIR {
                let definition = world
                    .content
                    .material(material)?
                    .ok_or_else(|| invalid("scalar haloの未定義素材"))?;
                dynamic |= definition.base() != MaterialBase::Solid;
            }
        }
        Ok(())
    })?;
    matches &= entry_index == expected.len();
    Ok((matches, dynamic))
}

fn capture_halo(world: &World, layer: LayerId, coord: ChunkCoord) -> EngineResult<Vec<(u16, u64)>> {
    let mut values = Vec::new();
    values.try_reserve(16).map_err(allocation)?;
    visit_halo(coord, |position, point| {
        let state = halo_state(world, layer, point)?;
        if state != 2 {
            values.try_reserve(1).map_err(allocation)?;
            values.push((position, state));
        }
        Ok(())
    })?;
    Ok(values)
}

impl StateCodec for ScalarMaterialSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHSLSCR1")?;
        let Some(p) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        p.world.encode(out)?;
        out.raw(&p.content)?;
        out.u64(p.epoch)?;
        out.u64(p.revision)?;
        p.layer.encode(out)?;
        out.u64(u64::try_from(p.candidate_limit).map_err(|_| invalid("scalar sleep候補容量"))?)?;
        out.u64(u64::try_from(p.candidates).map_err(|_| invalid("scalar sleep候補数"))?)?;
        out.u32(u32::try_from(p.active.len()).map_err(|_| invalid("scalar sleep活動数"))?)?;
        for (coord, revision, _) in &p.active {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHSLSCR1" {
            return Err(invalid("scalar sleep保存形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("scalar sleep存在bit")),
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("scalar sleep Content"))?;
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let layer = StableKey::decode(input)?;
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("scalar sleep容量"))?;
        let candidates =
            usize::try_from(input.u64()?).map_err(|_| invalid("scalar sleep候補数"))?;
        let count = input.u32()? as usize;
        if candidates > candidate_limit || count > input.remaining() / 16 {
            return Err(EngineError::capacity(
                "scalar sleep保存レコード",
                count as u64,
                input.remaining() as u64 / 16,
            ));
        }
        let mut active = Vec::new();
        active.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            active.push((ChunkCoord::decode(input)?, input.u64()?, None));
        }
        if active.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(invalid("scalar sleep活動chunk保存順"));
        }
        // 復元したcacheは未検証。world正規状態の走査後だけ再びsleepできる。
        Ok(Self {
            proof: Some(ScalarSleepProof {
                world,
                content,
                epoch,
                revision,
                layer,
                candidate_limit,
                candidates,
                active,
            }),
            candidate_cache: None,
            verified: false,
        })
    }
}
