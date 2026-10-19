//! 高速物質の広域整数DDAを、保存可能な固定予算Jobへ分割します。
//!
//! 複数chunkをまたぐ検査の途中ではWorldを変更せず、確認済み経路と各chunkの
//! revisionをjob内に保存します。Readyになってから地形と全物質channelを
//! 既存transport_pathで原子的に輸送します。

use super::{
    CellCore, CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialId, MaterialMotion, World,
    WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
    id::WorldId,
};

const SCALE: i128 = 65536;

/// 保存可能な広域移動の検査段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialMotionJobStatus {
    /// まだ目標移動量の経路検査を終えていません。
    Computing,
    /// 入力revisionが変わっていなければ、原子取引で公開できます。
    Ready,
}

/// 分割DDAの完了時の診断。外部入力の失効と処理途中の未公開状態を区別します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialMotionJobReport {
    /// 走査した格子境界数。
    pub checked_steps: u64,
    /// 輸送する整数cell経路の境界数。
    pub crossed_cells: usize,
    /// 障害物・World端で止まった場合。
    pub collision: bool,
    /// 未読込・非活動領域により先へ進めず延期した場合。
    pub waiting_for_region: bool,
}

/// 同じtick候補で確定した広域移動Job群の診断です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialMotionBatchReport {
    /// 原子的に公開した移動Job数。
    pub jobs: usize,
    /// 1cell以上の経路を持ち、carrierを移動したJob数。
    pub moved_jobs: usize,
    /// 公開した全経路の交差cell数。
    pub crossed_cells: usize,
    /// 原子的に公開した圧力押出し探索数。
    pub pressure_pushes: usize,
}

enum PreparedMaterialJob<'a> {
    Motion(&'a MaterialMotionJob),
    PressurePush(&'a super::PressurePushSearch, Option<Vec<CellPoint>>),
}

impl PreparedMaterialJob<'_> {
    fn layer(&self) -> StableKey {
        match self {
            Self::Motion(job) => job.layer,
            Self::PressurePush(search, _) => search.batch_order_key().0,
        }
    }

    fn order_key(&self) -> (StableKey, i32, i32, u8, Option<StableKey>) {
        match self {
            Self::Motion(job) => (job.layer, job.source.y, job.source.x, 0, Some(job.channel)),
            Self::PressurePush(search, _) => {
                let (layer, y, x) = search.batch_order_key();
                (layer, y, x, 1, None)
            }
        }
    }
}

/// 正規状態へ保存できる、特定cellに所属する広域DDA進捗。
///
/// 他の物質との交換や複数候補間の優先順位は決めず、単一トークンの空cell経路を
/// 原子移動する。複数Jobは `WorldEdit::publish_material_motion_and_pressure_pushes` で
/// 競合を検査し、経路が重なる場合は全体を拒否する。固定budget一単位を一交差で数え、
/// 限定した仕事量を超えて同じ呼出で終了したと報告しない。
#[derive(Clone, Debug)]
pub struct MaterialMotionJob {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    channel: StableKey,
    source: CellPoint,
    source_core: CellCore,
    before: MaterialMotion,
    after: MaterialMotion,
    counts: [u64; 2],
    signs: [i32; 2],
    progressed: [u64; 2],
    path_limit: usize,
    path: Vec<CellPoint>,
    // 復元したJobは最初のstepで再構築する。保存せず、publish境界でもWorldから再確認する。
    occupied_particles: Option<Vec<CellPoint>>,
    chunks: Vec<(ChunkCoord, u64)>,
    finished: bool,
    blocked: u8,
    checked_steps: u64,
}

/// 活動中の同layer正規粒子が占有するcellを座標順に収集します。
fn active_particle_cells(world: &World, layer: LayerId) -> EngineResult<Vec<CellPoint>> {
    let mut points = Vec::new();
    for (_, particle) in world.material_particles() {
        if particle.is_archived() || particle.layer != layer {
            continue;
        }
        let point = particle.cell();
        if !world
            .chunks
            .get(&ChunkCoord::at(point))
            .is_some_and(|chunk| chunk.active)
        {
            continue;
        }
        points.try_reserve(1).map_err(allocation)?;
        points.push(point);
    }
    points.sort_unstable_by_key(|point| (point.x, point.y));
    points.dedup();
    Ok(points)
}

fn particle_occupies(points: &[CellPoint], point: CellPoint) -> bool {
    points
        .binary_search_by_key(&(point.x, point.y), |candidate| (candidate.x, candidate.y))
        .is_ok()
}

/// 保存経路が粒子を通らないことを、Worldの正規粒子集合で確認します。
fn validate_particle_path(path: &[CellPoint], occupied: &[CellPoint]) -> EngineResult<()> {
    if path
        .first()
        .is_some_and(|point| particle_occupies(occupied, *point))
    {
        return Err(conflict("広域DDAのsourceが正規粒子に占有"));
    }
    for pair in path.windows(2) {
        let from = pair[0];
        let to = pair[1];
        if from.x != to.x && from.y != to.y {
            let guard_x = CellPoint { x: to.x, y: from.y };
            let guard_y = CellPoint { x: from.x, y: to.y };
            if particle_occupies(occupied, guard_x) || particle_occupies(occupied, guard_y) {
                return Err(conflict("広域DDAの角検査cellが正規粒子に占有"));
            }
        }
        if particle_occupies(occupied, to) {
            return Err(conflict("広域DDAの経路が正規粒子に占有"));
        }
    }
    Ok(())
}

impl MaterialMotionJob {
    /// 1体の高速物質を検査するJobを生成します。
    ///
    /// 活動chunk集合とrevisionを採取し、全候補の一括読込はしません。
    /// `step`と`publish`は同じ活動集合を保守的に照合します。
    pub fn new(
        world: &World,
        layer: LayerId,
        source: CellPoint,
        channel: StableKey,
        max_distance: u32,
        max_path_cells: usize,
        max_active_chunks: usize,
    ) -> EngineResult<Self> {
        if max_distance == 0 || max_path_cells == 0 {
            return Err(invalid("広域移動の移動距離・経路容量"));
        }
        let layer_key = world.content.layer(layer)?.key;
        if world.channel_material_layer::<MaterialMotion>(channel)? != Some(layer_key) {
            return Err(invalid("広域移動channelの所有layer"));
        }
        let chunk = world
            .chunks
            .get(&ChunkCoord::at(source))
            .ok_or_else(|| invalid("広域移動の未読込source"))?;
        if !chunk.active {
            return Err(conflict("広域移動の非活動source"));
        }
        let cell = match world.cell(layer, source)? {
            CellQuery::Loaded(cell)
                if cell.core.material != MaterialId::AIR && cell.block.is_none() =>
            {
                cell
            }
            _ => return Err(conflict("広域移動の空cell・block source")),
        };
        let origin = world.channel_layout::<MaterialMotion>(channel)?.origin;
        let coord = channel_point(source, origin)?;
        let before = match world.channel_value::<MaterialMotion>(channel, coord)? {
            ChannelQuery::Loaded(value) => value,
            _ => return Err(invalid("広域移動channel coverage")),
        };
        before.validate()?;
        let mut after = before;
        let mut counts = [0u64; 2];
        let mut signs = [0i32; 2];
        for axis in 0..2 {
            let q16 =
                i128::from(before.velocity_q16[axis]) + i128::from(before.remainder_q16[axis]);
            let integral = q16 / SCALE;
            if integral.unsigned_abs() > u128::from(max_distance) {
                return Err(EngineError::capacity(
                    "広域移動の軸距離",
                    integral.unsigned_abs() as u64,
                    max_distance as u64,
                ));
            }
            counts[axis] = integral.unsigned_abs() as u64;
            signs[axis] = integral.signum() as i32;
            after.remainder_q16[axis] = (q16 % SCALE) as i32;
        }
        let mut chunks = Vec::new();
        for (position, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            if chunks.len() == max_active_chunks {
                return Err(EngineError::capacity(
                    "広域移動の活動chunk記録数",
                    chunks.len() as u64 + 1,
                    max_active_chunks as u64,
                ));
            }
            chunks.try_reserve(1).map_err(allocation)?;
            chunks.push((*position, chunk.revision));
        }
        let mut path = Vec::new();
        path.try_reserve(1).map_err(allocation)?;
        path.push(source);
        let occupied_particles = active_particle_cells(world, layer)?;
        if particle_occupies(&occupied_particles, source) {
            return Err(conflict("広域移動のsourceが正規粒子に占有"));
        }
        let value = Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: layer_key,
            channel,
            source,
            source_core: cell.core,
            before,
            after,
            counts,
            signs,
            progressed: [0; 2],
            path_limit: max_path_cells,
            path,
            occupied_particles: Some(occupied_particles),
            chunks,
            finished: counts == [0, 0],
            blocked: 0,
            checked_steps: 0,
        };
        value.validate_progress()?;
        Ok(value)
    }

    /// 検査途中の保存・復元をしても同じ順序でReadyへ進みます。
    pub fn status(&self) -> MaterialMotionJobStatus {
        if self.finished {
            MaterialMotionJobStatus::Ready
        } else {
            MaterialMotionJobStatus::Computing
        }
    }
    /// DDA検査の進捗・移動距離・障害の診断。
    pub fn report(&self) -> MaterialMotionJobReport {
        MaterialMotionJobReport {
            checked_steps: self.checked_steps,
            crossed_cells: self.path.len() - 1,
            collision: self.blocked == 1,
            waiting_for_region: self.blocked == 2,
        }
    }
    /// 一交差を一budgetとし、Worldを変更せずに整数DDAを進めます。
    pub fn step(&mut self, world: &World, budget: usize) -> EngineResult<MaterialMotionJobStatus> {
        self.validate_context(world)?;
        if self.occupied_particles.is_none() {
            let layer = world.content.layer_id(self.layer)?;
            let occupied = active_particle_cells(world, layer)?;
            validate_particle_path(&self.path, &occupied)?;
            self.occupied_particles = Some(occupied);
        }
        for _ in 0..budget {
            if self.finished {
                break;
            }
            self.step_one(world)?;
        }
        Ok(self.status())
    }
    fn step_one(&mut self, world: &World) -> EngineResult<()> {
        let p = self.progressed;
        let horizontal = u128::from(2 * p[0] + 1) * u128::from(self.counts[1]);
        let vertical = u128::from(2 * p[1] + 1) * u128::from(self.counts[0]);
        let move_x = p[0] < self.counts[0] && (p[1] == self.counts[1] || horizontal <= vertical);
        let move_y = p[1] < self.counts[1] && (p[0] == self.counts[0] || vertical <= horizontal);
        if !move_x && !move_y {
            return Err(invalid("広域DDAの進捗が停止"));
        }
        let current = *self.path.last().expect("経路に始点あり");
        let dx = if move_x { self.signs[0] } else { 0 };
        let dy = if move_y { self.signs[1] } else { 0 };
        let target = offset(current, dx, dy)?;
        let mut points = [None; 3];
        if move_x && move_y {
            points[0] = Some(offset(current, dx, 0)?);
            points[1] = Some(offset(current, 0, dy)?);
        }
        points[2] = Some(target);
        for check in points.into_iter().flatten() {
            if self
                .occupied_particles
                .as_ref()
                .is_some_and(|occupied| particle_occupies(occupied, check))
            {
                self.blocked = 1;
                self.after = MaterialMotion::default();
                self.finished = true;
                self.checked_steps += 1;
                return Ok(());
            }
            match world.cell(world.content.layer_id(self.layer)?, check)? {
                CellQuery::WorldBoundary => {
                    self.blocked = 1;
                    self.after = MaterialMotion::default();
                    self.finished = true;
                    self.checked_steps += 1;
                    return Ok(());
                }
                CellQuery::Loaded(cell) => {
                    if !world
                        .chunks
                        .get(&ChunkCoord::at(check))
                        .is_some_and(|chunk| chunk.active)
                    {
                        self.blocked = 2;
                        self.finished = true;
                        self.checked_steps += 1;
                        return Ok(());
                    }
                    if cell.block.is_some() || cell.core.material != MaterialId::AIR {
                        self.blocked = 1;
                        self.after = MaterialMotion::default();
                        self.finished = true;
                        self.checked_steps += 1;
                        return Ok(());
                    }
                }
                _ => {
                    self.blocked = 2;
                    self.finished = true;
                    self.checked_steps += 1;
                    return Ok(());
                }
            }
        }
        if self.path.len() == self.path_limit {
            return Err(EngineError::capacity(
                "広域DDAの経路記録数",
                self.path.len() as u64 + 1,
                self.path_limit as u64,
            ));
        }
        self.path.try_reserve(1).map_err(allocation)?;
        self.path.push(target);
        self.progressed[0] += u64::from(move_x);
        self.progressed[1] += u64::from(move_y);
        self.checked_steps += 1;
        self.finished = self.progressed == self.counts;
        Ok(())
    }

    /// Readyの経路を一つのWorld取引で公開します。途中状態は一切書き込みません。
    pub fn publish(&self, editor: &mut WorldEdit<'_>) -> EngineResult<MaterialMotionJobReport> {
        editor.run(|edit| {
            // 全経路の構造検証は計算の途中では繰り返さず、確定入口で一度だけ行う。
            self.validate_progress()?;
            self.validate_context(edit.world)?;
            if !self.finished {
                return Err(conflict("広域移動Jobが未完成"));
            }
            let layer = edit.world.content.layer_id(self.layer)?;
            let occupied = active_particle_cells(edit.world, layer)?;
            validate_particle_path(&self.path, &occupied)?;
            self.publish_validated(edit)
        })
    }

    fn publish_validated(
        &self,
        editor: &mut WorldEdit<'_>,
    ) -> EngineResult<MaterialMotionJobReport> {
        let layer = editor.world.content.layer_id(self.layer)?;
        if self.path.len() > 1 {
            let mut reversed = Vec::new();
            reversed
                .try_reserve_exact(self.path.len())
                .map_err(allocation)?;
            reversed.extend(self.path.iter().rev().copied());
            editor.transport_path(layer, &reversed)?;
        }
        if self.after != self.before {
            let origin = editor
                .world
                .channel_layout::<MaterialMotion>(self.channel)?
                .origin;
            let target = *self.path.last().expect("始点を含む経路");
            editor.set_channel(self.channel, channel_point(target, origin)?, self.after)?;
        }
        Ok(self.report())
    }

    fn validate_context(&self, world: &World) -> EngineResult<()> {
        // progress自体はnew/復元の際に一度検証済みであり、privateなstepしか
        // 変更しません。毎stepにpath全体を再走査するとbudget=1でもO(N²)
        // になるので、ここではWorldの入力失効のみ確認します。
        if world.id != self.world
            || world.content.fingerprint() != self.content
            || world.remaps.epoch() != self.epoch
            || world.content.layer_id(self.layer).is_err()
            || world.channel_material_layer::<MaterialMotion>(self.channel)? != Some(self.layer)
        {
            return Err(conflict("広域移動JobのWorld・Content・epoch・channel"));
        }
        let active_count = world.chunks.values().filter(|chunk| chunk.active).count();
        if active_count != self.chunks.len() {
            return Err(conflict("広域移動Jobの活動集合が変化"));
        }
        for &(coord, revision) in &self.chunks {
            if !world
                .chunks
                .get(&coord)
                .is_some_and(|chunk| chunk.active && chunk.revision == revision)
            {
                return Err(conflict("広域移動Jobの入力revisionが失効"));
            }
        }
        let layer = world.content.layer_id(self.layer)?;
        let cell = match world.cell(layer, self.source)? {
            CellQuery::Loaded(cell) => cell,
            _ => return Err(conflict("広域移動Jobの始点coverage")),
        };
        if cell.block.is_some() || cell.core != self.source_core {
            return Err(conflict("広域移動Jobの始点が変更"));
        }
        let origin = world.channel_layout::<MaterialMotion>(self.channel)?.origin;
        if !matches!(
            world.channel_value::<MaterialMotion>(self.channel, channel_point(self.source, origin)?)?,
            ChannelQuery::Loaded(value) if value == self.before
        ) {
            return Err(conflict("広域移動Jobの速度入力が変更"));
        }
        Ok(())
    }
    fn validate_particle_occupancy(&self, world: &World) -> EngineResult<()> {
        let layer = world.content.layer_id(self.layer)?;
        let occupied = active_particle_cells(world, layer)?;
        validate_particle_path(&self.path, &occupied)
    }
    fn validate_progress(&self) -> EngineResult<()> {
        self.before.validate()?;
        self.after.validate()?;
        if self.path_limit == 0
            || self.path.len() > self.path_limit
            || self.path.first() != Some(&self.source)
            || self.path.is_empty()
            || self.blocked > 2
            || self.progressed[0] > self.counts[0]
            || self.progressed[1] > self.counts[1]
            || self.signs.iter().any(|s| !(-1..=1).contains(s))
            || (self.finished && self.blocked == 0 && self.progressed != self.counts)
            || (!self.finished && (self.blocked != 0 || self.progressed == self.counts))
            || self.chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(invalid("広域移動Jobの保存進捗"));
        }
        let mut crossed = [0u64; 2];
        for pair in self.path.windows(2) {
            let dx = i64::from(pair[1].x) - i64::from(pair[0].x);
            let dy = i64::from(pair[1].y) - i64::from(pair[0].y);
            if dx.unsigned_abs() > 1
                || dy.unsigned_abs() > 1
                || (dx == 0 && dy == 0)
                || (dx != 0 && dx.signum() as i32 != self.signs[0])
                || (dy != 0 && dy.signum() as i32 != self.signs[1])
            {
                return Err(invalid("広域DDAの保存経路"));
            }
            // 外部から読んだcodecに斜め角抜け等を仕込めないよう、
            // 純粋な整数DDAの交差順と経路を先頭から再照合します。
            let horizontal = u128::from(2 * crossed[0] + 1) * u128::from(self.counts[1]);
            let vertical = u128::from(2 * crossed[1] + 1) * u128::from(self.counts[0]);
            let expected_x = crossed[0] < self.counts[0]
                && (crossed[1] == self.counts[1] || horizontal <= vertical);
            let expected_y = crossed[1] < self.counts[1]
                && (crossed[0] == self.counts[0] || vertical <= horizontal);
            if (dx != 0) != expected_x || (dy != 0) != expected_y {
                return Err(invalid("広域DDAの保存経路と整数交差順"));
            }
            crossed[0] += u64::from(dx != 0);
            crossed[1] += u64::from(dy != 0);
        }
        if crossed != self.progressed {
            return Err(invalid("広域DDAのcursorと保存経路"));
        }
        let expected_steps = self.path.len() as u64 - 1 + u64::from(self.blocked != 0);
        if self.checked_steps != expected_steps
            || (self.blocked != 0 && !self.finished)
            || (self.blocked == 1 && self.after != MaterialMotion::default())
            || (self.blocked != 0 && self.progressed == self.counts)
        {
            return Err(invalid("広域DDAの検査数・終了理由・速度"));
        }
        Ok(())
    }
}

impl WorldEdit<'_> {
    /// 同じ初期Worldから計算した広域移動Jobを、衝突検査後に一括公開します。
    ///
    /// 全Jobの入力revision・source・速度・進捗を、最初の書込みより前に照合します。
    /// 同一layer内で経路、source/destination、斜めDDAの角検査cellが重なるJobは
    /// conflictとして拒否します。候補順に依存した勝者選択はせず、衝突があれば全件を
    /// rollbackします。互いに独立したJobはlayer・source座標・channelの固定順で公開し、
    /// 成功時は一つのWorldEdit候補として確定できます。
    ///
    /// 同時件数はWorldLimits.jobs、active chunk入力照合・保存node・経路検査の合計は
    /// WorldLimits.job_cells、経路Vec・bitset予約・順序VecのscratchはWorldLimits.job_bytes
    /// で制限します。検査用予約は活動chunk数に比例するbitsetを一layerずつ保持します。
    /// 計算中の進捗stepは各Jobで事前にbudget分割してください。
    pub fn publish_material_motion_jobs(
        &mut self,
        jobs: &[MaterialMotionJob],
    ) -> EngineResult<MaterialMotionBatchReport> {
        self.publish_material_motion_and_pressure_pushes(jobs, &[])
    }

    /// 広域DDA移動と圧力押出し探索を同じ予約・原子取引で公開します。
    ///
    /// 両種別の全入力を未変更Worldから事前検証し、同一layer上で経路・角確認点が重なる
    /// 場合は全体を拒否します。失敗した探索や一方だけの公開は残りません。圧力押出しの
    /// 探索条件はすでに完成したPressurePushSearchの入力snapshotに基づきます。この入口は
    /// 圧力緩和のPressureJobを同時実行したり、そこで更新する圧力値から押出し探索を作り直す
    /// 機能ではありません。
    pub fn publish_material_motion_and_pressure_pushes(
        &mut self,
        motion_jobs: &[MaterialMotionJob],
        pressure_pushes: &[super::PressurePushSearch],
    ) -> EngineResult<MaterialMotionBatchReport> {
        self.run(|editor| {
            let item_count = motion_jobs
                .len()
                .checked_add(pressure_pushes.len())
                .ok_or_else(|| invalid("広域移動Job数overflow"))?;
            if item_count > editor.world.limits.jobs as usize {
                return Err(EngineError::capacity(
                    "広域移動Job数",
                    item_count as u64,
                    editor.world.limits.jobs.into(),
                ));
            }
            let active_chunks = editor
                .world
                .chunks
                .values()
                .filter(|chunk| chunk.active)
                .count();
            let mut validation_work = motion_jobs
                .len()
                .checked_mul(active_chunks)
                .ok_or_else(|| invalid("広域移動Jobの入力検証量overflow"))?;
            for job in motion_jobs {
                let path_work = job
                    .path
                    .len()
                    .checked_mul(8)
                    .ok_or_else(|| invalid("広域移動Jobの経路検証量overflow"))?;
                validation_work = validation_work
                    .checked_add(path_work)
                    .ok_or_else(|| invalid("広域移動Jobの検証量overflow"))?;
            }
            for search in pressure_pushes {
                let search_work = search
                    .node_count()
                    .checked_mul(8)
                    .ok_or_else(|| invalid("圧力押出しJobの検証量overflow"))?;
                validation_work = validation_work
                    .checked_add(search_work)
                    .ok_or_else(|| invalid("圧力押出しJob群の検証量overflow"))?;
            }
            if validation_work > editor.world.limits.job_cells {
                return Err(EngineError::capacity(
                    "広域移動Job群の検証予算",
                    validation_work as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            let prepared_path_points =
                pressure_pushes.iter().try_fold(0usize, |total, search| {
                    total
                        .checked_add(search.node_count())
                        .ok_or_else(|| invalid("圧力押出し経路scratch数overflow"))
                })?;
            let path_scratch_bytes = prepared_path_points
                .checked_mul(std::mem::size_of::<CellPoint>())
                .ok_or_else(|| invalid("圧力押出し経路scratch byte数overflow"))?;
            let reservation_scratch_bytes = if item_count == 0 {
                0
            } else {
                active_chunks
                    .checked_mul(std::mem::size_of::<(ChunkCoord, [u64; 64])>())
                    .ok_or_else(|| invalid("広域移動予約scratch byte数overflow"))?
            };
            let scratch_bytes = item_count
                .checked_mul(std::mem::size_of::<PreparedMaterialJob<'_>>())
                .and_then(|value| value.checked_add(path_scratch_bytes))
                .and_then(|value| value.checked_add(reservation_scratch_bytes))
                .ok_or_else(|| invalid("広域移動Job scratch byte数overflow"))?;
            if scratch_bytes > editor.world.limits.job_bytes {
                return Err(EngineError::capacity(
                    "広域移動Job群のscratch byte数",
                    scratch_bytes as u64,
                    editor.world.limits.job_bytes as u64,
                ));
            }
            let mut ordered = Vec::new();
            ordered.try_reserve_exact(item_count).map_err(allocation)?;
            ordered.extend(motion_jobs.iter().map(PreparedMaterialJob::Motion));

            // すべてのJobは同じ未変更候補に対して検査する。公開開始後にrevisionが
            // 進んでも残りのJobの有効性を失わないよう、照合を先に完了させる。
            for job in motion_jobs {
                job.validate_progress()?;
                job.validate_context(editor.world)?;
                if !job.finished {
                    return Err(conflict("広域移動Job群に未完成Jobがある"));
                }
                job.validate_particle_occupancy(editor.world)?;
            }
            for search in pressure_pushes {
                ordered.push(PreparedMaterialJob::PressurePush(search, None));
            }
            ordered.sort_by_key(PreparedMaterialJob::order_key);

            let mut report = MaterialMotionBatchReport {
                jobs: motion_jobs.len(),
                pressure_pushes: pressure_pushes.len(),
                ..MaterialMotionBatchReport::default()
            };
            let mut reservations: Option<(StableKey, super::transport::CellReservations)> = None;
            for job in &mut ordered {
                let layer = job.layer();
                let is_motion = matches!(job, PreparedMaterialJob::Motion(_));
                if reservations
                    .as_ref()
                    .is_none_or(|(current, _)| *current != layer)
                {
                    // 次layerの予約scratchを確保する前に前layer分を解放し、
                    // 活動chunk数に比例するbitsetが一時的に二重化するのを防ぐ。
                    drop(reservations.take());
                    reservations = Some((
                        layer,
                        super::transport::CellReservations::new(editor.world)?,
                    ));
                }
                let scratch = &mut reservations.as_mut().expect("予約scratchを作成済み").1;
                let path = match job {
                    PreparedMaterialJob::Motion(motion) => &motion.path,
                    PreparedMaterialJob::PressurePush(search, stored_path) => {
                        let (verified_layer, path) = search.prepared_publish_path(editor.world)?;
                        if verified_layer != search.batch_order_key().0 {
                            return Err(conflict("圧力押出しJobのlayerが変化"));
                        }
                        *stored_path = Some(path);
                        stored_path.as_ref().expect("経路を準備済み")
                    }
                };
                for &point in path {
                    reserve_batch_point(scratch, point)?;
                }
                for pair in path.windows(2) {
                    let dx = pair[1].x - pair[0].x;
                    let dy = pair[1].y - pair[0].y;
                    if is_motion && dx != 0 && dy != 0 {
                        reserve_batch_point(scratch, offset(pair[0], dx, 0)?)?;
                        reserve_batch_point(scratch, offset(pair[0], 0, dy)?)?;
                    }
                }
                if is_motion {
                    report.crossed_cells = report
                        .crossed_cells
                        .checked_add(path.len() - 1)
                        .ok_or_else(|| invalid("広域移動Job群の交差数overflow"))?;
                }
                report.moved_jobs += usize::from(path.len() > 1);
            }

            for job in ordered {
                match job {
                    PreparedMaterialJob::Motion(job) => {
                        job.publish_validated(editor)?;
                    }
                    PreparedMaterialJob::PressurePush(search, Some(path)) => {
                        let layer = search.batch_order_key().0;
                        editor.transport_path(editor.world.content.layer_id(layer)?, &path)?;
                    }
                    PreparedMaterialJob::PressurePush(_, None) => {
                        return Err(invalid("圧力押出しJobの経路予約がない"));
                    }
                }
            }
            Ok(report)
        })
    }

    /// PressureJob、広域DDA、圧力押出しを一つのWorldEdit候補へ確定します。
    ///
    /// 三種の入力と経路をすべて最初の書込み前に準備し、job件数と検証workの合計を
    /// WorldLimitsで制限します。移動/押出しを固定順で適用した後、位置固定の圧力出力を
    /// 公開します。scratch byte数もWorldLimits.job_bytes以下にします。
    /// PressurePushSearchは保存時点の圧力snapshotで作られた経路を使い、
    /// このPressureJobの出力から再探索はしません。圧力出力は位置固定channelなので、
    /// 物質輸送のpayloadには含まれません。
    pub fn publish_material_motion_pressure_jobs_and_pushes(
        &mut self,
        motion_jobs: &[MaterialMotionJob],
        pressure_pushes: &[super::PressurePushSearch],
        pressure_jobs: &[super::PressureJob],
    ) -> EngineResult<(
        MaterialMotionBatchReport,
        Vec<super::PressureRelaxationReport>,
    )> {
        self.run(|editor| {
            let count = motion_jobs
                .len()
                .checked_add(pressure_pushes.len())
                .and_then(|value| value.checked_add(pressure_jobs.len()))
                .ok_or_else(|| invalid("E05広域Job件数overflow"))?;
            if count > editor.world.limits.jobs as usize {
                return Err(EngineError::capacity(
                    "E05広域Job数",
                    count as u64,
                    editor.world.limits.jobs.into(),
                ));
            }

            // 全jobを同じ初期Worldで準備し、異種batchの合計予算も書込み前に確認する。
            let prepared_pressure =
                super::pressure_job::prepare_pressure_jobs(editor.world, pressure_jobs)?;
            let active_chunks = editor
                .world
                .chunks
                .values()
                .filter(|chunk| chunk.active)
                .count();
            let mut motion_work = motion_jobs
                .len()
                .checked_mul(active_chunks)
                .ok_or_else(|| invalid("E05移動Jobの入力検証量overflow"))?;
            for job in motion_jobs {
                motion_work = motion_work
                    .checked_add(
                        job.path
                            .len()
                            .checked_mul(8)
                            .ok_or_else(|| invalid("E05移動Jobの経路検証量overflow"))?,
                    )
                    .ok_or_else(|| invalid("E05移動Jobの検証量overflow"))?;
            }
            for search in pressure_pushes {
                motion_work = motion_work
                    .checked_add(
                        search
                            .node_count()
                            .checked_mul(8)
                            .ok_or_else(|| invalid("E05押出しJobの検証量overflow"))?,
                    )
                    .ok_or_else(|| invalid("E05広域Jobの検証量overflow"))?;
            }
            let total_work = motion_work
                .checked_add(prepared_pressure.validation_work)
                .ok_or_else(|| invalid("E05広域Jobの検証量overflow"))?;
            if total_work > editor.world.limits.job_cells {
                return Err(EngineError::capacity(
                    "E05異種Job群の検証予算",
                    total_work as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            let prepared_path_points =
                pressure_pushes.iter().try_fold(0usize, |total, search| {
                    total
                        .checked_add(search.node_count())
                        .ok_or_else(|| invalid("E05押出し経路scratch数overflow"))
                })?;
            let movement_items = motion_jobs
                .len()
                .checked_add(pressure_pushes.len())
                .ok_or_else(|| invalid("E05移動Job数overflow"))?;
            let path_scratch_bytes = prepared_path_points
                .checked_mul(std::mem::size_of::<CellPoint>())
                .ok_or_else(|| invalid("E05経路scratch byte数overflow"))?;
            let reservation_scratch_bytes = if movement_items == 0 {
                0
            } else {
                active_chunks
                    .checked_mul(std::mem::size_of::<(ChunkCoord, [u64; 64])>())
                    .ok_or_else(|| invalid("E05予約scratch byte数overflow"))?
            };
            let movement_scratch_bytes = movement_items
                .checked_mul(std::mem::size_of::<PreparedMaterialJob<'_>>())
                .and_then(|value| value.checked_add(path_scratch_bytes))
                .and_then(|value| value.checked_add(reservation_scratch_bytes))
                .ok_or_else(|| invalid("E05移動scratch byte数overflow"))?;
            let scratch_bytes = prepared_pressure
                .scratch_bytes
                .checked_add(movement_scratch_bytes)
                .ok_or_else(|| invalid("E05異種Job scratch byte数overflow"))?;
            if scratch_bytes > editor.world.limits.job_bytes {
                return Err(EngineError::capacity(
                    "E05異種Job群のscratch byte数",
                    scratch_bytes as u64,
                    editor.world.limits.job_bytes as u64,
                ));
            }

            // 既存の移動batchは経路競合を検査して一候補へ公開する。PressureJob出力は
            // 位置固定channelのため、輸送後のchannel状態へ原子的に適用できる。
            let motion_report =
                editor.publish_material_motion_and_pressure_pushes(motion_jobs, pressure_pushes)?;
            let pressure_reports =
                super::pressure_job::apply_pressure_outputs(editor, prepared_pressure)?;
            Ok((motion_report, pressure_reports))
        })
    }
}

/// MaterialPhaseと同じtickに公開する広域Jobの経路を、標準solverより先に予約します。
///
/// Job batch本体も公開前に同じ入力を検証するため、この関数は追加の検証仕事量を返します。
/// 統合入口は二重検証を含めてWorldLimits.job_cellsへ収めてから呼び出します。
pub(super) fn reserve_material_job_paths(
    world: &World,
    motion_jobs: &[MaterialMotionJob],
    pressure_pushes: &[super::PressurePushSearch],
    layer: StableKey,
    reservations: &mut super::transport::CellReservations,
) -> EngineResult<(usize, bool)> {
    let active_chunks = world.chunks.values().filter(|chunk| chunk.active).count();
    let mut validation_work = motion_jobs
        .len()
        .checked_mul(active_chunks)
        .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
    let mut has_reservations = false;
    for job in motion_jobs {
        job.validate_progress()?;
        job.validate_context(world)?;
        if !job.finished {
            return Err(conflict("MaterialPhaseに未完成の広域移動Jobがある"));
        }
        job.validate_particle_occupancy(world)?;
        let path_work = job
            .path
            .len()
            .checked_mul(8)
            .ok_or_else(|| invalid("MaterialPhase経路検証量overflow"))?;
        validation_work = validation_work
            .checked_add(path_work)
            .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
        if job.layer != layer {
            continue;
        }
        reserve_material_job_path(reservations, &job.path, true)?;
        has_reservations |= !job.path.is_empty();
    }
    for search in pressure_pushes {
        let path_work = search
            .node_count()
            .checked_mul(8)
            .ok_or_else(|| invalid("MaterialPhase押出し検証量overflow"))?;
        validation_work = validation_work
            .checked_add(path_work)
            .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
        let (search_layer, path) = search.prepared_publish_path(world)?;
        if search_layer == layer {
            reserve_material_job_path(reservations, &path, false)?;
            has_reservations |= !path.is_empty();
        }
    }
    Ok((validation_work, has_reservations))
}

fn reserve_material_job_path(
    reservations: &mut super::transport::CellReservations,
    path: &[CellPoint],
    reserve_diagonal_corners: bool,
) -> EngineResult<()> {
    for &point in path {
        reserve_batch_point(reservations, point)?;
    }
    if reserve_diagonal_corners {
        for pair in path.windows(2) {
            let dx = pair[1].x - pair[0].x;
            let dy = pair[1].y - pair[0].y;
            if dx != 0 && dy != 0 {
                reserve_batch_point(reservations, offset(pair[0], dx, 0)?)?;
                reserve_batch_point(reservations, offset(pair[0], 0, dy)?)?;
            }
        }
    }
    Ok(())
}

pub(super) fn material_job_validation_work(
    world: &World,
    motion_jobs: &[MaterialMotionJob],
    pressure_pushes: &[super::PressurePushSearch],
) -> EngineResult<usize> {
    let active_chunks = world.chunks.values().filter(|chunk| chunk.active).count();
    let mut work = motion_jobs
        .len()
        .checked_mul(active_chunks)
        .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
    for job in motion_jobs {
        work = work
            .checked_add(
                job.path
                    .len()
                    .checked_mul(8)
                    .ok_or_else(|| invalid("MaterialPhase経路検証量overflow"))?,
            )
            .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
    }
    for search in pressure_pushes {
        work = work
            .checked_add(
                search
                    .node_count()
                    .checked_mul(8)
                    .ok_or_else(|| invalid("MaterialPhase押出し検証量overflow"))?,
            )
            .ok_or_else(|| invalid("MaterialPhase広域Job検証量overflow"))?;
    }
    Ok(work)
}

/// 統合phaseの予約bitsetを加えたJob batch scratchの事前見積りです。
pub(super) fn integrated_material_job_scratch_bytes(
    world: &World,
    motion_jobs: &[MaterialMotionJob],
    pressure_pushes: &[super::PressurePushSearch],
    reserve_phase_cells: bool,
) -> EngineResult<usize> {
    let path_points = pressure_pushes.iter().try_fold(0usize, |total, search| {
        total
            .checked_add(search.node_count())
            .ok_or_else(|| invalid("MaterialPhase経路scratch数overflow"))
    })?;
    integrated_material_job_scratch_bytes_bounded(
        world,
        motion_jobs,
        pressure_pushes.len(),
        path_points,
        reserve_phase_cells,
    )
}

pub(super) fn integrated_material_job_scratch_bytes_bounded(
    world: &World,
    motion_jobs: &[MaterialMotionJob],
    pressure_push_count: usize,
    pressure_push_path_points: usize,
    reserve_phase_cells: bool,
) -> EngineResult<usize> {
    let items = motion_jobs
        .len()
        .checked_add(pressure_push_count)
        .ok_or_else(|| invalid("MaterialPhase広域Job数overflow"))?;
    let active_chunks = world.chunks.values().filter(|chunk| chunk.active).count();
    let batch_reservations = if items == 0 {
        0
    } else {
        active_chunks
            .checked_mul(std::mem::size_of::<(ChunkCoord, [u64; 64])>())
            .ok_or_else(|| invalid("MaterialPhase batch予約byte数overflow"))?
    };
    let phase_reservations = if reserve_phase_cells && items != 0 {
        active_chunks
            .checked_mul(std::mem::size_of::<(ChunkCoord, [u64; 64])>())
            .ok_or_else(|| invalid("MaterialPhase共有予約byte数overflow"))?
    } else {
        0
    };
    let motion_path_points =
        motion_jobs
            .iter()
            .try_fold(pressure_push_path_points, |total, job| {
                total
                    .checked_add(job.path.len())
                    .ok_or_else(|| invalid("MaterialPhase経路scratch数overflow"))
            })?;
    items
        .checked_mul(std::mem::size_of::<PreparedMaterialJob<'_>>())
        .and_then(|bytes| {
            motion_path_points
                .checked_mul(std::mem::size_of::<CellPoint>())
                .and_then(|path_bytes| bytes.checked_add(path_bytes))
        })
        .and_then(|bytes| bytes.checked_add(batch_reservations))
        .and_then(|bytes| bytes.checked_add(phase_reservations))
        .ok_or_else(|| invalid("MaterialPhase統合Job scratch byte数overflow"))
}

fn reserve_batch_point(
    reservations: &mut super::transport::CellReservations,
    point: CellPoint,
) -> EngineResult<()> {
    if reservations.contains(point) {
        return Err(conflict("同tick広域移動Jobの経路予約が競合"));
    }
    reservations.insert(point)
}

fn offset(point: CellPoint, dx: i32, dy: i32) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_add(dx)
            .ok_or_else(|| invalid("広域DDAの水平端"))?,
        y: point
            .y
            .checked_add(dy)
            .ok_or_else(|| invalid("広域DDAの垂直端"))?,
    })
}

fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("広域DDAのchannel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("広域DDAのchannel垂直座標"))?,
    })
}

impl StateCodec for MaterialMotionJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_progress()?;
        out.raw(b"CHMMJOB1")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        self.layer.encode(out)?;
        self.channel.encode(out)?;
        self.source.encode(out)?;
        self.source_core.encode(out)?;
        self.before.encode(out)?;
        self.after.encode(out)?;
        for count in self.counts {
            out.u64(count)?;
        }
        for sign in self.signs {
            out.i32(sign)?;
        }
        for value in self.progressed {
            out.u64(value)?;
        }
        out.u64(u64::try_from(self.path_limit).map_err(|_| invalid("広域DDA保存容量"))?)?;
        out.u8(u8::from(self.finished))?;
        out.u8(self.blocked)?;
        out.u64(self.checked_steps)?;
        out.u32(u32::try_from(self.chunks.len()).map_err(|_| invalid("広域DDA chunk数"))?)?;
        for &(coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(revision)?;
        }
        out.u32(u32::try_from(self.path.len()).map_err(|_| invalid("広域DDA経路長"))?)?;
        for point in &self.path {
            point.encode(out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHMMJOB1" {
            return Err(invalid("広域DDAの形式"));
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("広域DDA Content hash"))?;
        let epoch = input.u64()?;
        let layer = StableKey::decode(input)?;
        let channel = StableKey::decode(input)?;
        let source = CellPoint::decode(input)?;
        let source_core = CellCore::decode(input)?;
        let before = MaterialMotion::decode(input)?;
        let after = MaterialMotion::decode(input)?;
        let counts = [input.u64()?, input.u64()?];
        let signs = [input.i32()?, input.i32()?];
        let progressed = [input.u64()?, input.u64()?];
        let path_limit = usize::try_from(input.u64()?).map_err(|_| invalid("広域DDA経路容量"))?;
        let finished = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("広域DDA完成bit")),
        };
        let blocked = input.u8()?;
        let checked_steps = input.u64()?;
        let chunk_count = input.u32()? as usize;
        if chunk_count > input.remaining() / 16 {
            return Err(invalid("広域DDA chunk件数"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(chunk_count).map_err(allocation)?;
        for _ in 0..chunk_count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = input.u32()? as usize;
        if count > path_limit || count > input.remaining() / 8 {
            return Err(invalid("広域DDA経路件数"));
        }
        let mut path = Vec::new();
        path.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            path.push(CellPoint::decode(input)?);
        }
        let job = Self {
            world,
            content,
            epoch,
            layer,
            channel,
            source,
            source_core,
            before,
            after,
            counts,
            signs,
            progressed,
            path_limit,
            path,
            occupied_particles: None,
            chunks,
            finished,
            blocked,
            checked_steps,
        };
        job.validate_progress()?;
        Ok(job)
    }
}
