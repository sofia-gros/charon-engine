//! 満杯の流体経路を通した、保存可能な押出し探索。
//!
//! 探索状態は作者のWorld StateStoreへ登録して保存できます。探索中の正規地形は
//! 変更せず、全経路の完成後にだけ地形と付随channelをまとめて循環輸送します。

use super::{
    CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, World, WorldEdit,
    conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
    transaction::CandidateState,
};

/// 探索の保存対象となる段階です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressurePushStatus {
    /// 次の固定件数stepを待っています。まだ物質を動かしていません。
    Searching,
    /// 圧力差を満たす空き先までの経路が完成し、原子的な公開が可能です。
    Ready,
    /// 読込済み活動領域内に、条件を満たす空き先がありませんでした。
    NoOutlet,
}

/// 圧力から押出し候補を選ぶ条件。solverの呼出側が明示します。
#[derive(Clone, Copy, Debug)]
pub struct PressurePushConfig {
    /// 保存された非負Q16圧力の、位置に固定されたi64 channel。
    pub channel: StableKey,
    /// 単位質量・単位高さ当たりの重力圧力差。下向きが正です。
    pub gravity_q16: u32,
    /// 経路全体のdriveがこの値より大きい場合だけ採用します。
    pub threshold_q16: u64,
    /// 保存する探索nodeの上限。上限到達は容量エラーであり、出口なしとは別です。
    pub nodes: u32,
}

/// 一つのWorldに所属する幅優先探索。可変状態をproviderのglobalへ隠しません。
///
/// `StateCodec`を実装しているので、作者はschemaを登録しWorld StateStoreへ格納できます。
/// 入力した全活動chunkのrevisionを保持する保守的な失効方針です。無関係な地形編集でも
/// 同じ活動集合内なら再探索が必要ですが、古い探索結果を新しい地形へ公開しません。
pub struct PressurePushSearch {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    material: MaterialId,
    config: PressurePushConfig,
    source_pressure: i64,
    mass: u32,
    chunks: Vec<(ChunkCoord, u64)>,
    // 活動中の同layer正規粒子が占有するcellを、探索開始時に一度だけ整列して保持します。
    // 各nodeで粒子arena全体を走査せず二分探索し、chunk revisionで失効を検出します。
    // 旧codecから復元したJobだけは次のstepで索引を作り、全nodeをbudget付きで再検査します。
    occupied_particles: Option<Vec<CellPoint>>,
    nodes: Vec<(CellPoint, u32)>,
    cursor: usize,
    goal: Option<u32>,
    // 訪問bitsetはnode列から復元できる索引で、保存形式には含めません。
    visited: super::transport::CellReservations,
    // 復元直後の実地形検査もstep予算に含めます。新規追加nodeは発見時に検査済みです。
    validated_nodes: usize,
}

/// 探索対象の活動chunkにいる同layer粒子位置を一度だけ収集・整列します。
/// 粒子位置はWorldのarena順に依存しないよう座標順へ正規化し、探索nodeごとは二分探索します。
fn active_particle_cells(world: &World, layer: LayerId) -> EngineResult<Vec<CellPoint>> {
    let mut points = Vec::new();
    for (_, particle) in world.particles.iter() {
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

/// 保存済みnodeを地形と粒子の両方から占有されていないか再検査します。
fn validate_node(
    world: &World,
    layer: LayerId,
    point: CellPoint,
    expected: MaterialId,
    occupied_particles: &[CellPoint],
) -> EngineResult<()> {
    if occupied_particles
        .binary_search_by_key(&(point.x, point.y), |candidate| (candidate.x, candidate.y))
        .is_ok()
        || !matches!(world.cell(layer, point)?, CellQuery::Loaded(cell)
            if cell.block.is_none() && cell.core.material == expected)
    {
        return Err(conflict("圧力探索nodeの占有が不一致"));
    }
    Ok(())
}

impl PressurePushSearch {
    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn node_capacity(&self) -> usize {
        self.config.nodes as usize
    }

    pub(super) fn batch_order_key(&self) -> (StableKey, i32, i32) {
        let source = self.nodes[0].0;
        (self.layer, source.y, source.x)
    }

    /// 活動中の液体または気体cellから探索を始めます。開始だけでは輸送しません。
    pub fn new(
        world: &World,
        layer: LayerId,
        source: CellPoint,
        config: PressurePushConfig,
    ) -> EngineResult<Self> {
        if config.nodes == 0 || !world.content.layer(layer)?.simulated {
            return Err(invalid("圧力探索の容量またはlayer"));
        }
        let layout = world.channel_layout::<i64>(config.channel)?;
        if layout.x_scale.ratio() != (1, 1)
            || layout.y_scale.ratio() != (1, 1)
            || world
                .channel_material_layer::<i64>(config.channel)?
                .is_some()
        {
            return Err(invalid("圧力探索には位置固定の1対1 channelが必要"));
        }
        let cell = match world.cell(layer, source)? {
            CellQuery::Loaded(cell) if cell.block.is_none() => cell,
            _ => return Err(invalid("圧力探索の始点が未準備またはblock")),
        };
        let material = world
            .content
            .material(cell.core.material)?
            .ok_or_else(|| invalid("圧力探索の始点が空"))?;
        if !matches!(material.base(), MaterialBase::Liquid | MaterialBase::Gas) {
            return Err(invalid("圧力探索の始点が流体ではない"));
        }
        let source_pressure = read_pressure(world, config.channel, source)?;
        let mut chunks = Vec::new();
        for (coord, chunk) in &world.chunks {
            if chunk.active {
                chunks.try_reserve(1).map_err(allocation)?;
                chunks.push((*coord, chunk.revision));
            }
        }
        if chunks
            .binary_search_by_key(&ChunkCoord::at(source), |(coord, _)| *coord)
            .is_err()
        {
            return Err(invalid("圧力探索の始点が活動外"));
        }
        let occupied_particles = active_particle_cells(world, layer)?;
        if occupied_particles
            .binary_search_by_key(&(source.x, source.y), |point| (point.x, point.y))
            .is_ok()
        {
            return Err(conflict("圧力探索の始点に正規粒子が存在"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(1).map_err(allocation)?;
        nodes.push((source, 0));
        let mut visited = super::transport::CellReservations::for_chunks(&chunks)?;
        visited.insert(source)?;
        Ok(Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: world.content.layer(layer)?.key,
            material: cell.core.material,
            config,
            source_pressure,
            mass: material.mass_per_cell(),
            chunks,
            occupied_particles: Some(occupied_particles),
            nodes,
            cursor: 0,
            goal: None,
            visited,
            validated_nodes: 1,
        })
    }
    /// 現在の段階。復元後はnodeの実地形検査が完了するまでSearchingです。
    /// 容量不足や入力失効はstepのエラーとして区別して返します。
    pub fn status(&self) -> PressurePushStatus {
        if self.validated_nodes < self.nodes.len() {
            PressurePushStatus::Searching
        } else if self.goal.is_some() {
            PressurePushStatus::Ready
        } else if self.cursor == self.nodes.len() {
            PressurePushStatus::NoOutlet
        } else {
            PressurePushStatus::Searching
        }
    }
    /// 保存済みの探索node数。始点を含みます。
    pub fn discovered_nodes(&self) -> usize {
        self.nodes.len()
    }
    /// 探索frontierを失わずにnode上限を変更します。
    ///
    /// 既存node数未満への縮小は拒否します。この呼出では追加配列を確保せず、
    /// 次のstepが必要分だけ確保します。StateStoreのrecord byte上限は独立して残るため、
    /// 上限変更だけで無制限に保存できるようにはなりません。
    pub fn set_node_capacity(&mut self, nodes: u32) -> EngineResult<()> {
        if (nodes as usize) < self.nodes.len() {
            return Err(EngineError::capacity(
                "圧力探索の既存node数",
                self.nodes.len() as u64,
                nodes.into(),
            ));
        }
        self.config.nodes = nodes;
        Ok(())
    }
    /// 正規状態を変更せず、最大budget個のnodeから四近傍を調べます。
    ///
    /// 近傍は上・左・右・下の固定順です。同じ素材の活動cellだけを通過し、空cellで
    /// `Psrc - Pdst + mass*g*(ydst-ysrc) > threshold`を満たす経路を採用します。
    /// 未読込と活動外はno-flow境界です。step失敗時には探索cursorも元のままです。
    /// 訪問bitsetは保持し、今回の追記nodeだけを失敗時に巻き戻します。復元直後は
    /// 既存nodeの実地形検査も一件ずつbudgetから消費し、完了後に探索を再開します。
    /// 読込済みchunkの活動集合と入力revisionの検査も別途行います。Vecの容量拡張は償却費用で、
    /// 保存・明示的な候補複製・公開時の全体検査はこのstep予算とは別です。
    pub fn step(&mut self, world: &World, budget: usize) -> EngineResult<PressurePushStatus> {
        self.validate_context(world)?;
        let layer = world.content.layer_id(self.layer)?;
        if self.occupied_particles.is_none() {
            self.occupied_particles = Some(active_particle_cells(world, layer)?);
            // CHPUSH01/02には粒子占有のsnapshotがありません。既存nodeを新契約で
            // 検査し直すまでは探索を再開・公開できない状態へ戻します。
            self.validated_nodes = 0;
        }
        let occupied_particles = self
            .occupied_particles
            .take()
            .ok_or_else(|| invalid("圧力探索の粒子索引"))?;
        let previous_len = self.nodes.len();
        let previous_cursor = self.cursor;
        let previous_goal = self.goal;
        let previous_validated = self.validated_nodes;
        // node列の全複製は行いません。今回の追記だけを候補として末尾へ置き、
        // 失敗時は追記nodeの訪問bitとcursorを巻き戻します。
        let result = (|| {
            for _ in 0..budget {
                if self.validated_nodes < self.nodes.len() {
                    let index = self.validated_nodes;
                    let point = self.nodes[index].0;
                    let expected = if self.goal == Some(index as u32) {
                        MaterialId::AIR
                    } else {
                        self.material
                    };
                    validate_node(world, layer, point, expected, &occupied_particles)?;
                    self.validated_nodes += 1;
                    continue;
                }
                if self.status() != PressurePushStatus::Searching {
                    break;
                }
                let parent = self.cursor;
                let point = self.nodes[parent].0;
                for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                    let (Some(x), Some(y)) = (point.x.checked_add(dx), point.y.checked_add(dy))
                    else {
                        continue;
                    };
                    let next = CellPoint { x, y };
                    if self.visited.contains(next) {
                        continue;
                    }
                    if !world
                        .chunks
                        .get(&ChunkCoord::at(next))
                        .is_some_and(|chunk| chunk.active)
                    {
                        continue;
                    }
                    if occupied_particles
                        .binary_search_by_key(&(next.x, next.y), |point| (point.x, point.y))
                        .is_ok()
                    {
                        continue;
                    }
                    let cell = match world.cell(layer, next)? {
                        CellQuery::Loaded(cell) if cell.block.is_none() => cell,
                        _ => continue,
                    };
                    let empty = cell.core.material == MaterialId::AIR;
                    if !empty && cell.core.material != self.material {
                        continue;
                    }
                    if empty {
                        // 活動中の空cellは相対圧0です。位置固定channelの旧値は
                        // 物質の移動後も残るため、出口条件へ流用しません。
                        let drive = i128::from(self.source_pressure)
                            + i128::from(self.mass)
                                * i128::from(self.config.gravity_q16)
                                * (i128::from(next.y) - i128::from(self.nodes[0].0.y));
                        if drive <= i128::from(self.config.threshold_q16) {
                            continue;
                        }
                    }
                    if self.nodes.len() >= self.config.nodes as usize {
                        return Err(EngineError::capacity(
                            "圧力探索のnode数",
                            self.nodes.len() as u64 + 1,
                            self.config.nodes.into(),
                        ));
                    }
                    self.nodes.try_reserve(1).map_err(allocation)?;
                    self.nodes.push((next, parent as u32));
                    self.visited.insert(next)?;
                    self.validated_nodes += 1;
                    if empty {
                        self.goal = Some(self.nodes.len() as u32 - 1);
                        break;
                    }
                }
                self.cursor += 1;
            }
            Ok(self.status())
        })();
        if result.is_err() {
            for (point, _) in &self.nodes[previous_len..] {
                self.visited.remove(*point);
            }
            self.nodes.truncate(previous_len);
            self.cursor = previous_cursor;
            self.goal = previous_goal;
            self.validated_nodes = previous_validated;
        }
        self.occupied_particles = Some(occupied_particles);
        result
    }
    fn validate_context(&self, world: &World) -> EngineResult<()> {
        if world.id != self.world {
            return Err(EngineError::new(ErrorCode::WrongScope, "圧力探索のWorld"));
        }
        if self.content != world.content.fingerprint()
            || self.epoch != world.remaps.epoch()
            || world.chunks.values().filter(|chunk| chunk.active).count() != self.chunks.len()
            || self.chunks.iter().any(|(coord, revision)| {
                !world
                    .chunks
                    .get(coord)
                    .is_some_and(|chunk| chunk.active && chunk.revision == *revision)
            })
        {
            return Err(conflict("圧力探索の入力が変更されたため再探索が必要"));
        }
        // codecの構造検証とは別に、実行先の物性と圧力条件を照合します。
        // 復元後の各nodeの占有検査はstep予算内、公開前は一括で行います。
        let layer = world.content.layer_id(self.layer)?;
        let layout = world.channel_layout::<i64>(self.config.channel)?;
        if layout.x_scale.ratio() != (1, 1)
            || layout.y_scale.ratio() != (1, 1)
            || world
                .channel_material_layer::<i64>(self.config.channel)?
                .is_some()
        {
            return Err(invalid("復元された圧力探索のchannel定義"));
        }
        let definition = world
            .content
            .material(self.material)?
            .ok_or_else(|| invalid("圧力探索の素材定義"))?;
        if !world.content.layer(layer)?.simulated
            || !matches!(definition.base(), MaterialBase::Liquid | MaterialBase::Gas)
            || definition.mass_per_cell() != self.mass
            || read_pressure(world, self.config.channel, self.nodes[0].0)? != self.source_pressure
        {
            return Err(conflict("圧力探索の物性または始点圧力が不一致"));
        }
        if let Some(goal) = self.goal {
            let destination = self.nodes[goal as usize].0;
            // 復元後・公開前も探索時と同じ空cellの境界条件を使います。
            let drive = i128::from(self.source_pressure)
                + i128::from(self.mass)
                    * i128::from(self.config.gravity_q16)
                    * (i128::from(destination.y) - i128::from(self.nodes[0].0.y));
            if drive <= i128::from(self.config.threshold_q16) {
                return Err(conflict("圧力探索の完成経路がdrive条件を満たさない"));
            }
        }
        Ok(())
    }
    fn validate_inputs(&self, world: &World) -> EngineResult<()> {
        self.validate_context(world)?;
        let layer = world.content.layer_id(self.layer)?;
        let computed_particles = if self.occupied_particles.is_none() {
            Some(active_particle_cells(world, layer)?)
        } else {
            None
        };
        let occupied_particles = self
            .occupied_particles
            .as_deref()
            .or(computed_particles.as_deref())
            .ok_or_else(|| invalid("圧力探索の粒子索引"))?;
        // 公開時は全経路を実地形と照合します。step予算とは別の一括取引です。
        for index in 0..self.nodes.len() {
            let point = self.nodes[index].0;
            let expected = if self.goal == Some(index as u32) {
                MaterialId::AIR
            } else {
                self.material
            };
            validate_node(world, layer, point, expected, occupied_particles)?;
        }
        Ok(())
    }

    pub(super) fn prepared_publish_path(
        &self,
        world: &World,
    ) -> EngineResult<(StableKey, Vec<CellPoint>)> {
        self.validate_inputs(world)?;
        if self.status() != PressurePushStatus::Ready {
            return Err(conflict("圧力押出し探索が公開可能状態ではない"));
        }
        let mut index = self.goal.ok_or_else(|| invalid("圧力探索の経路が未完成"))? as usize;
        let mut path_len = 1usize;
        let mut cursor = index;
        while cursor != 0 {
            cursor = self.nodes[cursor].1 as usize;
            path_len = path_len
                .checked_add(1)
                .ok_or_else(|| invalid("圧力探索の経路長overflow"))?;
        }
        let mut path = Vec::new();
        path.try_reserve_exact(path_len).map_err(allocation)?;
        loop {
            path.push(self.nodes[index].0);
            if index == 0 {
                break;
            }
            index = self.nodes[index].1 as usize;
        }
        path.reverse();
        Ok((self.layer, path))
    }
}
impl WorldEdit<'_> {
    /// 完成した押出し経路だけを公開します。未完成・失効・容量不足では一切輸送しません。
    /// 公開後はchunk revisionが変わるので、同じ探索を二度公開することも拒否されます。
    pub fn publish_pressure_push(&mut self, search: &PressurePushSearch) -> EngineResult<()> {
        self.run(|editor| {
            let (layer, path) = search.prepared_publish_path(editor.world)?;
            editor.transport_path(editor.world.content.layer_id(layer)?, &path)
        })
    }
}
impl CandidateState for PressurePushSearch {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            world: self.world,
            content: self.content,
            epoch: self.epoch,
            layer: self.layer,
            material: self.material,
            config: self.config,
            source_pressure: self.source_pressure,
            mass: self.mass,
            chunks: super::terrain::copy_slice(&self.chunks)?,
            occupied_particles: self
                .occupied_particles
                .as_ref()
                .map(|points| super::terrain::copy_slice(points))
                .transpose()?,
            nodes: super::terrain::copy_slice(&self.nodes)?,
            cursor: self.cursor,
            goal: self.goal,
            visited: self.visited.try_copy()?,
            validated_nodes: self.validated_nodes,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if self.nodes.is_empty()
            || self.nodes.len() > self.config.nodes as usize
            || self.cursor > self.nodes.len()
            || self.validated_nodes > self.nodes.len()
            || self.source_pressure < 0
            || self.mass == 0
            || self.material == MaterialId::AIR
            || self.nodes[0].1 != 0
            || self.chunks.is_empty()
            || self.chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.occupied_particles.as_ref().is_some_and(|occupied| {
                occupied
                    .windows(2)
                    .any(|pair| (pair[0].x, pair[0].y) >= (pair[1].x, pair[1].y))
                    || occupied.iter().any(|point| {
                        self.chunks
                            .binary_search_by_key(&ChunkCoord::at(*point), |(coord, _)| *coord)
                            .is_err()
                    })
            })
            || self
                .goal
                .is_some_and(|index| index == 0 || index as usize != self.nodes.len() - 1)
            || self.goal.is_some_and(|index| {
                self.cursor == 0
                    || (self.nodes[index as usize].1 as usize).checked_add(1) != Some(self.cursor)
            })
        {
            return Err(invalid("圧力探索recordの値域"));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(self.nodes.len())
            .map_err(allocation)?;
        for (index, (point, parent)) in self.nodes.iter().enumerate() {
            if self
                .chunks
                .binary_search_by_key(&ChunkCoord::at(*point), |(coord, _)| *coord)
                .is_err()
            {
                return Err(invalid("圧力探索nodeの領域"));
            }
            if index > 0 {
                if *parent as usize >= index {
                    return Err(invalid("圧力探索の親node"));
                }
                let from = self.nodes[*parent as usize].0;
                if (i64::from(from.x) - i64::from(point.x)).abs()
                    + (i64::from(from.y) - i64::from(point.y)).abs()
                    != 1
                {
                    return Err(invalid("圧力探索の辺が非隣接"));
                }
            }
            points.push((point.x, point.y));
        }
        points.sort_unstable();
        if points.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("圧力探索nodeの重複"));
        }
        Ok(())
    }
}
impl StateCodec for PressurePushSearch {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.raw(if self.occupied_particles.is_some() {
            b"CHPUSH03"
        } else {
            b"CHPUSH02"
        })?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        self.layer.encode(out)?;
        self.material.encode(out)?;
        self.config.channel.encode(out)?;
        out.u32(self.config.gravity_q16)?;
        out.u64(self.config.threshold_q16)?;
        out.u32(self.config.nodes)?;
        out.i64(self.source_pressure)?;
        out.u32(self.mass)?;
        if let Some(points) = &self.occupied_particles {
            out.u32(u32::try_from(points.len()).map_err(|_| invalid("圧力探索粒子索引件数"))?)?;
            for point in points {
                point.encode(out)?;
            }
        }
        out.u32(self.chunks.len() as u32)?;
        for (coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        out.u32(self.nodes.len() as u32)?;
        for (point, parent) in &self.nodes {
            point.encode(out)?;
            out.u32(*parent)?;
        }
        out.u32(self.cursor as u32)?;
        self.goal.encode(out)?;
        out.u32(self.validated_nodes as u32)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let version = match input.raw(8)? {
            b"CHPUSH01" => 1,
            b"CHPUSH02" => 2,
            b"CHPUSH03" => 3,
            _ => return Err(invalid("圧力探索recordの形式")),
        };
        let world = WorldId::decode(input)?;
        let mut content = [0u8; 32];
        content.copy_from_slice(input.raw(32)?);
        let epoch = input.u64()?;
        let layer = StableKey::decode(input)?;
        let material = MaterialId::decode(input)?;
        let config = PressurePushConfig {
            channel: StableKey::decode(input)?,
            gravity_q16: input.u32()?,
            threshold_q16: input.u64()?,
            nodes: input.u32()?,
        };
        let source_pressure = input.i64()?;
        let mass = input.u32()?;
        let occupied_particles = if version >= 3 {
            let count = input.u32()? as usize;
            if count > input.remaining() / 8 {
                return Err(invalid("圧力探索粒子索引の切断"));
            }
            let mut points = Vec::new();
            points.try_reserve_exact(count).map_err(allocation)?;
            for _ in 0..count {
                points.push(CellPoint::decode(input)?);
            }
            Some(points)
        } else {
            None
        };
        let count = input.u32()? as usize;
        if count > input.remaining() / 16 {
            return Err(invalid("圧力探索chunk列の切断"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 12 || count > config.nodes as usize {
            return Err(invalid("圧力探索node列の切断または容量"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            nodes.push((CellPoint::decode(input)?, input.u32()?));
        }
        let cursor = input.u32()? as usize;
        let goal = Option::<u32>::decode(input)?;
        // 旧recordは未照合から始め、新recordは照合進捗もphase間で引き継ぎます。
        // 再開時の入力revision検査と公開前の全node実地形照合は省略しません。
        let validated_nodes = if version >= 2 {
            input.u32()? as usize
        } else {
            0
        };
        let mut visited = super::transport::CellReservations::for_chunks(&chunks)?;
        for (point, _) in &nodes {
            visited.insert(*point)?;
        }
        let result = Self {
            world,
            content,
            epoch,
            layer,
            material,
            config,
            source_pressure,
            mass,
            chunks,
            occupied_particles,
            nodes,
            cursor,
            goal,
            visited,
            validated_nodes,
        };
        result.validate()?;
        Ok(result)
    }
}
fn read_pressure(world: &World, key: StableKey, point: CellPoint) -> EngineResult<i64> {
    let origin = world.channel_layout::<i64>(key)?.origin;
    let coordinate = CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("圧力探索channel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("圧力探索channel垂直座標"))?,
    };
    match world.channel_value::<i64>(key, coordinate)? {
        ChannelQuery::Loaded(value) if value >= 0 => Ok(value),
        _ => Err(invalid("圧力探索の圧力値またはcoverage")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        foundation::{
            grid::{GridLayout, GridLimits, GridScale},
            schema::Schema,
        },
        world::*,
    };
    use std::sync::Arc;

    fn key(text: &str) -> StableKey {
        StableKey::parse(text).unwrap()
    }

    fn filled_world() -> (World, LayerId) {
        let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
        registry
            .layer(LayerDef {
                key: key("test:layer"),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        registry
            .register(MaterialBuilder::new("test:water", MaterialBase::Liquid).unwrap())
            .unwrap();
        registry
            .schema(Schema::of::<i64>(key("test:pressure-value"), 1, 8).unwrap())
            .unwrap();
        registry
            .channel(ChannelDefinition {
                key: key("test:pressure"),
                version: 1,
                value_schema: key("test:pressure-value"),
                layout: GridLayout {
                    origin: CellPoint { x: 0, y: 0 },
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0i64,
                limits: GridLimits {
                    chunks: 1,
                    checkpoint_bytes: 100_000,
                },
            })
            .unwrap();
        let content = registry.freeze().unwrap();
        let layer = content.layer_id(key("test:layer")).unwrap();
        let water = content
            .material_id(MaterialHandle(key("test:water")))
            .unwrap();
        let mut world = World::new(
            WorldId::from_parts(0, 1).unwrap(),
            0,
            WorldBounds::expandable(),
            Arc::clone(&content),
            WorldLimits::default(),
        )
        .unwrap();
        world
            .edit(|edit| {
                edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                for y in 0..64 {
                    for x in 0..64 {
                        edit.set_cell(
                            layer,
                            CellPoint { x, y },
                            CellCore {
                                material: water,
                                ..CellCore::AIR
                            },
                        )?;
                    }
                }
                edit.acquire_lease(RegionLease {
                    source: key("test:lease"),
                    region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                    mode: LeaseMode::Simulate,
                    expires: None,
                })
            })
            .unwrap();
        (world, layer)
    }
    fn config(nodes: u32) -> PressurePushConfig {
        PressurePushConfig {
            channel: key("test:pressure"),
            gravity_q16: 0,
            threshold_q16: 0,
            nodes,
        }
    }

    #[test]
    fn bounded_steps_keep_existing_frontier_allocation() {
        let (world, layer) = filled_world();
        let mut search =
            PressurePushSearch::new(&world, layer, CellPoint { x: 32, y: 32 }, config(4096))
                .unwrap();
        // 容量が足りる間に既存node列を複製していないことを、時間計測なしで観測します。
        search.nodes.try_reserve_exact(4095).unwrap();
        let pointer = search.nodes.as_ptr();
        for processed in 1..=200 {
            assert_eq!(
                search.step(&world, 1).unwrap(),
                PressurePushStatus::Searching
            );
            assert_eq!(search.cursor, processed);
            assert_eq!(search.nodes.as_ptr(), pointer);
            assert_eq!(search.validated_nodes, search.nodes.len());
        }
    }

    #[test]
    fn restored_node_checks_share_the_step_budget() {
        let (world, layer) = filled_world();
        let mut search =
            PressurePushSearch::new(&world, layer, CellPoint { x: 32, y: 32 }, config(4096))
                .unwrap();
        search.step(&world, 30).unwrap();
        let mut out = Encoder::new(100_000);
        search.encode(&mut out).unwrap();
        let canonical = out.finish();
        let mut legacy_search = search.try_copy().unwrap();
        legacy_search.occupied_particles = None;
        let mut legacy = Encoder::new(100_000);
        legacy_search.encode(&mut legacy).unwrap();
        let version_two = legacy.finish();
        let mut v2_decoder = Decoder::new(&version_two, 100_000).unwrap();
        let mut restored_v2 = PressurePushSearch::decode(&mut v2_decoder).unwrap();
        assert!(restored_v2.occupied_particles.is_none());
        assert_eq!(restored_v2.validated_nodes, restored_v2.nodes.len());
        restored_v2.step(&world, 1).unwrap();
        assert_eq!(restored_v2.validated_nodes, 1);

        let mut bytes = version_two;
        bytes[..8].copy_from_slice(b"CHPUSH01");
        bytes.truncate(bytes.len() - 4);
        let mut decoder = Decoder::new(&bytes, 100_000).unwrap();
        let mut restored = PressurePushSearch::decode(&mut decoder).unwrap();
        assert_eq!(restored.validated_nodes, 0);
        let previous_cursor = restored.cursor;
        for validated in 1..=restored.nodes.len() {
            let mut out = Encoder::new(100_000);
            restored.encode(&mut out).unwrap();
            let bytes = out.finish();
            restored =
                PressurePushSearch::decode(&mut Decoder::new(&bytes, 100_000).unwrap()).unwrap();
            restored.step(&world, 1).unwrap();
            assert_eq!(restored.validated_nodes, validated);
            assert_eq!(restored.cursor, previous_cursor);
        }
        let mut after = Encoder::new(100_000);
        restored.encode(&mut after).unwrap();
        assert_eq!(canonical, after.finish());
        restored.step(&world, 1).unwrap();
        search.step(&world, 1).unwrap();
        assert_eq!(search.nodes, restored.nodes);
        assert_eq!(search.cursor, restored.cursor);
    }

    #[test]
    fn restored_no_outlet_waits_for_budgeted_node_checks() {
        let (world, layer) = filled_world();
        let mut search =
            PressurePushSearch::new(&world, layer, CellPoint { x: 32, y: 32 }, config(4096))
                .unwrap();
        assert_eq!(
            search.step(&world, 4096).unwrap(),
            PressurePushStatus::NoOutlet
        );
        search.occupied_particles = None;
        let mut out = Encoder::new(100_000);
        search.encode(&mut out).unwrap();
        let mut bytes = out.finish();
        bytes[..8].copy_from_slice(b"CHPUSH01");
        bytes.truncate(bytes.len() - 4);
        let mut input = Decoder::new(&bytes, 100_000).unwrap();
        let mut restored = PressurePushSearch::decode(&mut input).unwrap();
        assert_eq!(restored.status(), PressurePushStatus::Searching);
        assert_eq!(
            restored.step(&world, 1).unwrap(),
            PressurePushStatus::Searching
        );
        assert_eq!(restored.validated_nodes, 1);
        assert_eq!(
            restored.step(&world, 4095).unwrap(),
            PressurePushStatus::NoOutlet
        );
    }

    #[test]
    fn saved_validation_cursor_cannot_exceed_the_node_count() {
        let (world, layer) = filled_world();
        let search =
            PressurePushSearch::new(&world, layer, CellPoint { x: 32, y: 32 }, config(4096))
                .unwrap();
        let mut out = Encoder::new(100_000);
        search.encode(&mut out).unwrap();
        let mut bytes = out.finish();
        let length = bytes.len();
        bytes[length - 4..].copy_from_slice(&2u32.to_le_bytes());
        assert!(PressurePushSearch::decode(&mut Decoder::new(&bytes, 100_000).unwrap()).is_err());
    }

    #[test]
    fn failed_partial_expansion_rolls_back_visited_bits_and_cursor() {
        let (world, layer) = filled_world();
        let source = CellPoint { x: 32, y: 32 };
        let mut search = PressurePushSearch::new(&world, layer, source, config(3)).unwrap();
        assert!(search.step(&world, 1).is_err());
        assert_eq!(search.nodes, vec![(source, 0)]);
        assert_eq!(search.cursor, 0);
        assert_eq!(search.validated_nodes, 1);
        search.set_node_capacity(4096).unwrap();
        search.step(&world, 1).unwrap();
        assert_eq!(search.nodes.len(), 5);
        let mut fresh = PressurePushSearch::new(&world, layer, source, config(4096)).unwrap();
        fresh.step(&world, 1).unwrap();
        assert_eq!(search.nodes, fresh.nodes);
    }
}
