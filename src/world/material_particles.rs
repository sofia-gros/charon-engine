//! cellから所有権を受け取り、着地まで全payloadを保持する正規の物質粒子。
//!
//! 表示用の粒子とは独立し、寿命による削除経路を持ちません。一粒子は一cell分の
//! tokenを所有し、複数cellの放出は同じWorld取引で複数回呼び出して原子的に行います。
use super::{
    CellCore, CellQuery, ChunkCoord, LayerId, MaterialId, World, WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
    id::{Id, WorldId},
    transaction::CandidateState,
};

const SCALE: i64 = 65536;

/// 物質粒子arenaの種類。表示用EntityのIDとは混用できません。
pub enum MaterialParticleKind {}
/// Worldと世代を含む物質粒子の参照。除去・着地後の参照は失効します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialParticleHandle {
    world: WorldId,
    id: Id<MaterialParticleKind>,
}
impl MaterialParticleHandle {
    /// 所属World。
    pub fn world(self) -> WorldId {
        self.world
    }
    /// 保存・診断用の世代付きID。
    pub fn id(self) -> Id<MaterialParticleKind> {
        self.id
    }
}
impl StateCodec for MaterialParticleHandle {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.world.encode(out)?;
        self.id.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            world: WorldId::decode(input)?,
            id: Id::decode(input)?,
        })
    }
}

/// 保存対象の一tokenと連続位置。可変参照は公開せず、変更はWorld取引を通します。
#[derive(Debug)]
pub struct MaterialParticle {
    pub(crate) layer: LayerId,
    pub(crate) core: CellCore,
    pub(crate) epoch: u64,
    position: [i64; 2],
    velocity: [i64; 2],
    pub(crate) dormant: Option<[u8; 32]>,
    payload: Vec<(StableKey, Vec<u8>)>,
}
impl MaterialParticle {
    pub(super) fn inventory_reference(&self, key: StableKey) -> EngineResult<super::MixtureRef> {
        let bytes = self
            .payload
            .iter()
            .find(|(name, _)| *name == key)
            .ok_or_else(|| invalid("粒子の混合channelが欠落"))?;
        let mut input = Decoder::new(&bytes.1, bytes.1.len())?;
        let reference = super::MixtureRef::decode(&mut input)?;
        input.finish()?;
        Ok(reference)
    }

    pub(crate) fn evict_payload(&mut self, hash: [u8; 32]) {
        self.payload = Vec::new();
        self.dormant = Some(hash);
    }
    /// 元のcellのflags・variationを含む正規物質。
    pub fn core(&self) -> CellCore {
        self.core
    }
    /// 所属layer。
    pub fn layer(&self) -> LayerId {
        self.layer
    }
    /// 符号付きQ16の絶対位置。右・下が正で、負座標はEuclidean除算します。
    pub fn position_q16(&self) -> [i64; 2] {
        self.position
    }
    /// 符号付きQ16 cell/tickの速度。
    pub fn velocity_q16(&self) -> [i64; 2] {
        self.velocity
    }
    /// 地形と一緒に退避中ならtrue。payloadの所有者はarchiveです。
    pub fn is_archived(&self) -> bool {
        self.dormant.is_some()
    }
    /// 所属regionを決める、位置を含むcell。
    pub fn cell(&self) -> CellPoint {
        CellPoint {
            x: self.position[0].div_euclid(SCALE) as i32,
            y: self.position[1].div_euclid(SCALE) as i32,
        }
    }
    pub(crate) fn payload_bytes(&self) -> EngineResult<usize> {
        self.payload.iter().try_fold(0usize, |total, (key, value)| {
            total
                .checked_add(key.as_str().len())
                .and_then(|n| n.checked_add(8))
                .and_then(|n| n.checked_add(value.len()))
                .ok_or_else(|| invalid("粒子payload byte数overflow"))
        })
    }
    fn validate_shape(&self) -> EngineResult<()> {
        for position in self.position {
            if position.div_euclid(SCALE) < i64::from(i32::MIN)
                || position.div_euclid(SCALE) > i64::from(i32::MAX)
            {
                return Err(invalid("粒子の位置がcell座標範囲外"));
            }
        }
        if self.core.material == MaterialId::AIR
            || self.core.flags & !7 != 0
            || self.payload.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || (self.dormant.is_some() && !self.payload.is_empty())
        {
            return Err(invalid("粒子の物質・payload辞書・退避状態"));
        }
        Ok(())
    }
}
impl CandidateState for MaterialParticle {
    fn try_copy(&self) -> EngineResult<Self> {
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(self.payload.len())
            .map_err(allocation)?;
        for (key, bytes) in &self.payload {
            payload.push((*key, super::terrain::copy_slice(bytes)?));
        }
        Ok(Self {
            layer: self.layer,
            core: self.core,
            epoch: self.epoch,
            position: self.position,
            velocity: self.velocity,
            dormant: self.dormant,
            payload,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        self.validate_shape()
    }
}
impl StateCodec for MaterialParticle {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_shape()?;
        self.layer.encode(out)?;
        self.core.encode(out)?;
        out.u64(self.epoch)?;
        for value in self.position {
            out.i64(value)?;
        }
        for value in self.velocity {
            out.i64(value)?;
        }
        out.u8(u8::from(self.dormant.is_some()))?;
        if let Some(hash) = self.dormant {
            out.raw(&hash)?;
        }
        out.u32(u32::try_from(self.payload.len()).map_err(|_| invalid("粒子channel数"))?)?;
        for (key, bytes) in &self.payload {
            key.encode(out)?;
            out.bytes(bytes)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let layer = LayerId::decode(input)?;
        let core = CellCore::decode(input)?;
        let epoch = input.u64()?;
        let position = [input.i64()?, input.i64()?];
        let velocity = [input.i64()?, input.i64()?];
        let dormant = match input.u8()? {
            0 => None,
            1 => Some(
                input
                    .raw(32)?
                    .try_into()
                    .map_err(|_| invalid("粒子archive hash"))?,
            ),
            _ => return Err(invalid("粒子archive tag")),
        };
        let count = input.u32()? as usize;
        if count > input.remaining() / 11 {
            return Err(invalid("粒子payload列の切断"));
        }
        let mut payload = Vec::new();
        payload.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            let key = StableKey::decode(input)?;
            let limit = input.remaining();
            payload.push((key, super::terrain::copy_slice(input.bytes(limit)?)?));
        }
        let value = Self {
            layer,
            core,
            epoch,
            position,
            velocity,
            dormant,
            payload,
        };
        value.validate_shape()?;
        Ok(value)
    }
}

/// 粒子phaseの診断。失敗時はphase全体がrollbackされ、部分結果は公開されません。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialParticleReport {
    /// 活動region内で処理した粒子数。
    pub processed: usize,
    /// cellを跨いで移動した粒子数。
    pub moved: usize,
    /// cellへ所有権を返した粒子数。
    pub landed: usize,
    /// 未読込・非活動域または先行経路の予約により保持した粒子数。
    pub waiting: usize,
    /// DDAで検査した境界の数。同時交差では両直交cellも検査します。
    pub checked_steps: usize,
}

impl World {
    /// arenaの生存粒子。退避中はmetadataのみを返し、archive payloadを複製しません。
    pub fn material_particles(
        &self,
    ) -> impl Iterator<Item = (MaterialParticleHandle, &MaterialParticle)> {
        self.particles
            .iter()
            .map(|(id, value)| (MaterialParticleHandle { world: self.id, id }, value))
    }
    /// World・世代・生存を照合して粒子を借ります。
    pub fn material_particle(
        &self,
        handle: MaterialParticleHandle,
    ) -> EngineResult<&MaterialParticle> {
        if handle.world != self.id {
            return Err(EngineError::new(ErrorCode::WrongScope, "粒子のWorld"));
        }
        self.particles.get(handle.id)
    }
    /// 粒子が所有する作者channelを元の型で読みます。位置固定場や退避中は拒否します。
    pub fn particle_channel_value<T: GridValue + Send + Sync + 'static>(
        &self,
        handle: MaterialParticleHandle,
        key: StableKey,
    ) -> EngineResult<T> {
        self.channel_layout::<T>(key)?;
        let particle = self.material_particle(handle)?;
        if particle.dormant.is_some() {
            return Err(conflict("退避中の粒子payload"));
        }
        let bytes = particle
            .payload
            .iter()
            .find(|(name, _)| *name == key)
            .ok_or_else(|| invalid("粒子に所属しないchannel"))?;
        let mut input = Decoder::new(&bytes.1, self.limits.particle_bytes)?;
        let value = T::decode(&mut input)?;
        input.finish()?;
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate_particle(&self, particle: &MaterialParticle) -> EngineResult<()> {
        particle.validate_shape()?;
        particle.core.validate(&self.content)?;
        self.content.layer(particle.layer)?;
        if particle.epoch > self.remaps.epoch() || !self.bounds.contains(particle.cell()) {
            return Err(invalid("粒子のepoch・World境界"));
        }
        if let Some(hash) = particle.dormant {
            if !self
                .archived
                .get(&ChunkCoord::at(particle.cell()))
                .is_some_and(|a| a.hash == hash)
            {
                return Err(invalid("粒子の退避先なし"));
            }
            return Ok(());
        }
        if !self.chunks.contains_key(&ChunkCoord::at(particle.cell())) {
            return Err(invalid("resident粒子のregionが未読込"));
        }
        if particle.epoch != self.remaps.epoch() {
            return Err(invalid("resident粒子に未適用のremap"));
        }
        self.validate_particle_payload(particle)
    }

    /// archive監査でも全channelの型・値・キー列を検査し、混合参照だけの検査にしません。
    pub(super) fn validate_particle_payload(
        &self,
        particle: &MaterialParticle,
    ) -> EngineResult<()> {
        let layer = self.content.layer(particle.layer)?.key;
        let mut payload = particle.payload.iter();
        for channel in self
            .channels
            .iter()
            .filter(|channel| channel.material_layer() == Some(layer))
        {
            let Some((key, bytes)) = payload.next() else {
                return Err(invalid("粒子payloadの欠落"));
            };
            if *key != channel.key() {
                return Err(invalid("粒子payloadのchannel不一致"));
            }
            channel.validate_payload(bytes)?;
        }
        if payload.next().is_some() {
            return Err(invalid("粒子payloadの余分なchannel"));
        }
        Ok(())
    }
    pub(crate) fn validate_particles(&self) -> EngineResult<()> {
        if self.particles.capacity() != self.limits.particles {
            return Err(invalid("粒子arenaとWorldのslot容量が不一致"));
        }
        let mut bytes = 0usize;
        for (_, particle) in self.particles.iter() {
            self.validate_particle(particle)?;
            bytes = bytes
                .checked_add(particle.payload_bytes()?)
                .ok_or_else(|| invalid("全粒子byte数overflow"))?;
        }
        if bytes > self.limits.particle_bytes {
            return Err(EngineError::capacity(
                "粒子payload byte数",
                bytes as u64,
                self.limits.particle_bytes as u64,
            ));
        }
        Ok(())
    }
}

impl WorldEdit<'_> {
    /// 独自providerが粒子所有のtimer・energy・作者状態を同じWorld取引で変更します。
    /// 登録済みの物質channelだけを受理し、型・値・全粒子byte容量を検査します。
    /// 粒子本体の速度と作者の速度channelを連動させる場合は、この入口と
    /// set_particle_velocityを同じ取引で呼び、どちらを正とするか明示します。
    pub fn set_particle_channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        handle: MaterialParticleHandle,
        key: StableKey,
        value: T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            editor.world.channel_layout::<T>(key)?;
            value.validate()?;
            let particle = editor.world.material_particle(handle)?;
            if particle.is_archived() {
                return Err(conflict("退避粒子のchannel変更"));
            }
            let index = particle
                .payload
                .iter()
                .position(|(name, _)| *name == key)
                .ok_or_else(|| invalid("粒子に所属しないchannel"))?;
            let previous = particle.payload[index].1.len();
            let point = particle.cell();
            let used = editor
                .world
                .particles
                .iter()
                .try_fold(0usize, |total, (_, p)| {
                    total
                        .checked_add(p.payload_bytes()?)
                        .ok_or_else(|| invalid("粒子byte数overflow"))
                })?;
            let available = editor
                .world
                .limits
                .particle_bytes
                .checked_sub(used)
                .and_then(|n| n.checked_add(previous))
                .ok_or_else(|| invalid("粒子channel変更容量"))?;
            let mut out = Encoder::new(available);
            value.encode(&mut out)?;
            editor.reserve_writes(1)?;
            editor.world.particles.get_mut(handle.id)?.payload[index].1 = out.finish();
            editor.touch_particle_chunks(point, point)?;
            Ok(())
        })
    }
    fn touch_particle_chunks(
        &mut self,
        source: CellPoint,
        destination: CellPoint,
    ) -> EngineResult<()> {
        for (index, coord) in [ChunkCoord::at(source), ChunkCoord::at(destination)]
            .into_iter()
            .enumerate()
        {
            if index == 1 && coord == ChunkCoord::at(source) {
                continue;
            }
            let chunk = self
                .world
                .chunks
                .get_mut(&coord)
                .ok_or_else(|| invalid("粒子変更のchunkが未読込"))?;
            chunk.revision = chunk
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("粒子変更のchunk revision上限"))?;
        }
        Ok(())
    }
    /// cell一つと全物質channelを粒子poolへ移します。pool不足ではcellを消費しません。
    /// block個体は分解ruleが必要なため、この入口では拒否します。位置固定場は保持します。
    pub fn emit_material_particle(
        &mut self,
        layer: LayerId,
        point: CellPoint,
        velocity_q16: [i64; 2],
    ) -> EngineResult<MaterialParticleHandle> {
        self.run(|editor| {
            let cell = editor.loaded(layer, point)?;
            if cell.block.is_some() || cell.core.material == MaterialId::AIR {
                return Err(conflict("粒子へ移すcellが空またはblock個体"));
            }
            if editor.world.particles.iter().any(|(_, particle)| {
                !particle.is_archived() && particle.layer == layer && particle.cell() == point
            }) {
                return Err(conflict("放出先に別の物質粒子が存在"));
            }
            let layer_key = editor.world.content.layer(layer)?.key;
            let channels = editor
                .world
                .channels
                .iter()
                .filter(|channel| channel.material_layer() == Some(layer_key));
            let mut payload = Vec::new();
            let count = channels.clone().count();
            payload.try_reserve_exact(count).map_err(allocation)?;
            let used = editor
                .world
                .particles
                .iter()
                .try_fold(0usize, |total, (_, p)| {
                    total
                        .checked_add(p.payload_bytes()?)
                        .ok_or_else(|| invalid("粒子byte数overflow"))
                })?;
            let mut remaining = editor
                .world
                .limits
                .particle_bytes
                .checked_sub(used)
                .ok_or_else(|| invalid("粒子byte容量"))?;
            for channel in channels {
                let overhead = channel.key().as_str().len() + 8;
                remaining = remaining.checked_sub(overhead).ok_or_else(|| {
                    EngineError::capacity(
                        "粒子channel辞書byte数",
                        overhead as u64,
                        remaining as u64,
                    )
                })?;
                let bytes = channel.material_payload(point, remaining)?;
                remaining -= bytes.len();
                payload.push((channel.key(), bytes));
            }
            editor.reserve_writes(count + 2)?;
            let particle = MaterialParticle {
                layer,
                core: cell.core,
                epoch: editor.world.remaps.epoch(),
                position: [
                    i64::from(point.x) * SCALE + SCALE / 2,
                    i64::from(point.y) * SCALE + SCALE / 2,
                ],
                velocity: velocity_q16,
                dormant: None,
                payload,
            };
            let id = editor.world.particles.insert(particle)?;
            for channel in &mut editor.world.channels {
                channel.reset_material(layer_key, &[point])?;
            }
            editor.write(layer, point, CellCore::AIR, None)?;
            Ok(MaterialParticleHandle {
                world: editor.world.id,
                id,
            })
        })
    }

    /// 独自providerが速度を更新する入口。暗黙の重力・摩擦や寿命は適用しません。
    pub fn set_particle_velocity(
        &mut self,
        handle: MaterialParticleHandle,
        velocity_q16: [i64; 2],
    ) -> EngineResult<()> {
        self.run(|editor| {
            if editor.world.material_particle(handle)?.dormant.is_some() {
                return Err(conflict("退避粒子の速度変更"));
            }
            editor.reserve_writes(1)?;
            editor.world.particles.get_mut(handle.id)?.velocity = velocity_q16;
            let point = editor.world.particles.get(handle.id)?.cell();
            editor.touch_particle_chunks(point, point)?;
            Ok(())
        })
    }

    /// 粒子の現在cellへ着地します。宛先の物質payloadも既定値であることを要求し、
    /// 空cell上の作者状態を黙って捨てません。容量不足・失効は粒子を含め全体rollbackします。
    pub fn land_material_particle(&mut self, handle: MaterialParticleHandle) -> EngineResult<()> {
        self.run(|editor| {
            let particle = editor.world.material_particle(handle)?;
            if editor.world.particles.iter().any(|(id, other)| {
                id != handle.id
                    && !other.is_archived()
                    && other.layer == particle.layer
                    && other.cell() == particle.cell()
            }) {
                return Err(conflict("着地先に別の物質粒子が存在"));
            }
            editor.land_uncontested_particle(handle)
        })
    }

    // 公開入口では他粒子を走査し、phase内では開始位置の索引と経路予約で同じ条件を保証します。
    // phaseの全着地でpool全体を再走査する二乗費用を避け、payload検査と原子的変更は共用します。
    fn land_uncontested_particle(&mut self, handle: MaterialParticleHandle) -> EngineResult<()> {
        self.run(|editor| {
            let particle = editor.world.material_particle(handle)?.try_copy()?;
            if particle.dormant.is_some() {
                return Err(conflict("退避粒子の着地"));
            }
            let point = particle.cell();
            let cell = editor.loaded(particle.layer, point)?;
            if cell.block.is_some() || cell.core != CellCore::AIR {
                return Err(conflict("粒子の着地先が占有中"));
            }
            editor.world.validate_particle(&particle)?;
            let layer = editor.world.content.layer(particle.layer)?.key;
            let mut required = 0usize;
            for channel in &editor.world.channels {
                let bytes = if channel.material_layer() == Some(layer) {
                    if !channel.payload_is_default(point)? {
                        return Err(conflict("着地先に物質payloadが残存"));
                    }
                    let bytes = particle
                        .payload
                        .iter()
                        .find(|(key, _)| *key == channel.key())
                        .expect("payload検証済み");
                    channel.payload_bytes(point, &bytes.1)?
                } else {
                    channel.bytes()
                };
                required = required
                    .checked_add(bytes)
                    .ok_or_else(|| invalid("着地channel byte数overflow"))?;
            }
            if required > editor.world.limits.channel_bytes {
                return Err(EngineError::capacity(
                    "着地channel byte数",
                    required as u64,
                    editor.world.limits.channel_bytes as u64,
                ));
            }
            editor.reserve_writes(particle.payload.len() + 2)?;
            for channel in &mut editor.world.channels {
                if let Some((_, bytes)) = particle
                    .payload
                    .iter()
                    .find(|(key, _)| *key == channel.key())
                {
                    channel.put_payload(point, bytes)?;
                }
            }
            editor.write(particle.layer, point, particle.core, None)?;
            editor.world.particles.remove(handle.id)?;
            Ok(())
        })
    }

    /// 活動regionの粒子を整数DDAで一tick分進めます。境界の比較は整数の交差積で行い、
    /// 同時交差ではX側・Y側・斜め先を検査します。有限半径のshape sweepではありません。
    /// 通過cellと角の検査cellを予約し、後続粒子の交差を延期します。障害の直前の
    /// 空cellへ着地できない場合も粒子を保持します。件数超過は全phaseをrollbackします。
    pub fn step_material_particles(
        &mut self,
        candidate_limit: usize,
        step_limit: usize,
    ) -> EngineResult<MaterialParticleReport> {
        self.step_material_particles_filtered(None, candidate_limit, step_limit, None)
    }

    /// 指定layerだけの活動正規粒子を整数DDAで進めます。未処理の他layerの
    /// 粒子位置も予約として保持し、異layerの処理順に依存しない結果にします。
    pub fn step_material_particles_in_layer(
        &mut self,
        layer: LayerId,
        candidate_limit: usize,
        step_limit: usize,
    ) -> EngineResult<MaterialParticleReport> {
        self.step_material_particles_filtered(Some(layer), candidate_limit, step_limit, None)
    }

    /// Cell motion/scalarで先に使ったpath予約を粒子DDAも避ける統合入口です。
    pub(super) fn step_material_particles_in_layer_reserved(
        &mut self,
        layer: LayerId,
        candidate_limit: usize,
        step_limit: usize,
        reserved_cells: &super::transport::CellReservations,
    ) -> EngineResult<MaterialParticleReport> {
        self.step_material_particles_filtered(
            Some(layer),
            candidate_limit,
            step_limit,
            Some(reserved_cells),
        )
    }

    fn step_material_particles_filtered(
        &mut self,
        layer_filter: Option<LayerId>,
        candidate_limit: usize,
        step_limit: usize,
        reserved_cells: Option<&super::transport::CellReservations>,
    ) -> EngineResult<MaterialParticleReport> {
        self.run(|editor| {
            if let Some(layer) = layer_filter
                && !editor.world.content.layer(layer)?.simulated
            {
                return Err(invalid("正規粒子の輸送対象はsimulated layerのみ"));
            }
            let mut ids = Vec::new();
            for (id, particle) in editor.world.particles.iter() {
                if layer_filter.is_some_and(|layer| particle.layer != layer) {
                    continue;
                }
                // 非参加layerは全域呼出でも動かさない。粒子の所有権は保持する。
                if !editor.world.content.layer(particle.layer)?.simulated {
                    continue;
                }
                if particle.dormant.is_none() && editor.world.chunks.get(&ChunkCoord::at(particle.cell())).is_some_and(|c| c.active) {
                    if ids.len() == candidate_limit { return Err(EngineError::capacity("活動粒子数", ids.len() as u64 + 1, candidate_limit as u64)); }
                    ids.try_reserve(1).map_err(allocation)?;
                    ids.push(id);
                }
            }
            // 未処理・非活動の粒子も現在位置を所有します。先行粒子が後続の始点を
            // 奪わないよう、開始時の位置は移動後もこのphase中は解放しません。
            if ids.is_empty() { return Ok(MaterialParticleReport::default()); }
            let mut occupied = Vec::new();
            occupied.try_reserve_exact(editor.world.particles.len()).map_err(allocation)?;
            for (_, particle) in editor.world.particles.iter().filter(|(_, p)| !p.is_archived()) {
                let point = particle.cell();
                occupied.push((particle.layer.0, point.x, point.y));
            }
            occupied.sort_unstable();
            let mut reserved = std::collections::BTreeSet::new();
            let mut report = MaterialParticleReport::default();
            for id in ids {
                let handle = MaterialParticleHandle { world: editor.world.id, id };
                let particle = editor.world.material_particle(handle)?;
                let start = particle.position;
                let velocity = particle.velocity;
                let layer = particle.layer;
                let source = particle.cell();
                report.processed += 1;
                if reserved_cells.is_some_and(|cells| cells.contains(source)) {
                    report.waiting += 1;
                    continue;
                }
                if reserved.contains(&(layer.0, source.x, source.y)) { report.waiting += 1; continue; }
                let mut end = [0i64; 2];
                for axis in 0..2 {
                    end[axis] = start[axis].checked_add(velocity[axis]).ok_or_else(|| invalid("粒子変位overflow"))?;
                    if end[axis].div_euclid(SCALE) < i64::from(i32::MIN) || end[axis].div_euclid(SCALE) > i64::from(i32::MAX) {
                        return Err(invalid("粒子変位がcell座標範囲外"));
                    }
                }
                let target = CellPoint { x: end[0].div_euclid(SCALE) as i32, y: end[1].div_euclid(SCALE) as i32 };
                let mut current = source;
                let sign = [velocity[0].signum() as i32, velocity[1].signum() as i32];
                let speed = [velocity[0].unsigned_abs() as u128, velocity[1].unsigned_abs() as u128];
                let mut stopped = velocity == [0, 0];
                let mut waiting = false;
                let mut path = vec![source];
                while current != target {
                    if report.checked_steps == step_limit { return Err(EngineError::capacity("粒子DDA検査数", report.checked_steps as u64 + 1, step_limit as u64)); }
                    report.checked_steps += 1;
                    let coord = [current.x, current.y];
                    let distance: [u128; 2] = std::array::from_fn(|axis| {
                        let boundary = (i64::from(coord[axis]) + i64::from(sign[axis] > 0)) * SCALE;
                        (boundary - start[axis]).unsigned_abs() as u128
                    });
                    let x = current.x != target.x && (current.y == target.y || distance[0] * speed[1] <= distance[1] * speed[0]);
                    let y = current.y != target.y && (current.x == target.x || distance[1] * speed[0] <= distance[0] * speed[1]);
                    let next = CellPoint { x: current.x.checked_add(if x { sign[0] } else { 0 }).ok_or_else(|| invalid("粒子DDA水平端"))?,
                        y: current.y.checked_add(if y { sign[1] } else { 0 }).ok_or_else(|| invalid("粒子DDA垂直端"))? };
                    let mut guards = Vec::new();
                    if x && y { guards.push(CellPoint { x: next.x, y: current.y }); guards.push(CellPoint { x: current.x, y: next.y }); }
                    guards.push(next);
                    for point in &guards {
                        if reserved_cells.is_some_and(|cells| cells.contains(*point))
                            || occupied.binary_search(&(layer.0, point.x, point.y)).is_ok()
                            || reserved.contains(&(layer.0, point.x, point.y))
                        {
                            waiting = true;
                            break;
                        }
                        match editor.world.cell(layer, *point)? {
                            CellQuery::WorldBoundary => { stopped = true; break; }
                            CellQuery::Loaded(cell) => {
                                if !editor.world.chunks[&ChunkCoord::at(*point)].active { waiting = true; break; }
                                if cell.block.is_some() || cell.core.material != MaterialId::AIR { stopped = true; break; }
                            }
                            _ => { waiting = true; break; }
                        }
                    }
                    if stopped || waiting { break; }
                    path.try_reserve(guards.len()).map_err(allocation)?;
                    path.extend(guards);
                    current = next;
                }
                for point in path { reserved.insert((layer.0, point.x, point.y)); }
                editor.reserve_writes(1)?;
                // 有効な無変更tickはchunk revisionを汚さず、静止sleepの証明を維持します。
                // 移動、停止による速度変更、または接触後の位置補正だけが空間変更です。
                let next_position = if (stopped || waiting) && current == source {
                    start
                } else if stopped || waiting {
                    [i64::from(current.x) * SCALE + SCALE / 2, i64::from(current.y) * SCALE + SCALE / 2]
                } else {
                    end
                };
                let state_changed = next_position != start || (stopped && velocity != [0, 0]);
                let particle = editor.world.particles.get_mut(id)?;
                // 接触で先のcellを越えないよう、最後の空cellの中心を正規停止位置とします。
                // 未準備域でも未検査の整数距離を借金へ変えず、速度を保持して次phaseへ待機します。
                particle.position = next_position;
                if current != source { report.moved += 1; }
                if waiting { report.waiting += 1; }
                if stopped {
                    particle.velocity = [0, 0];
                    // 空cellのpayloadが残る場合は保持します。容量エラーは外側取引を失敗させ、
                    // 先行粒子の移動も戻すため、入力粒子を消す経路にはなりません。
                    let layer_key = editor.world.content.layer(layer)?.key;
                    let empty_payload = editor.world.channels.iter().filter(|c| c.material_layer() == Some(layer_key))
                        .try_fold(true, |empty, c| Ok::<_, EngineError>(empty && c.payload_is_default(current)?))?;
                    let source_key = (layer.0, source.x, source.y);
                    let shared_source = current == source && occupied.partition_point(|key| *key <= source_key)
                        - occupied.partition_point(|key| *key < source_key) > 1;
                    if shared_source { report.waiting += 1; }
                    if !shared_source && matches!(editor.world.cell(layer, current)?, CellQuery::Loaded(cell) if cell.core == CellCore::AIR && cell.block.is_none()) && empty_payload {
                        editor.land_uncontested_particle(handle)?;
                        report.landed += 1;
                    }
                }
                if state_changed {
                    editor.touch_particle_chunks(source, current)?;
                }
            }
            Ok(report)
        })
    }
}

/// 正規粒子phaseを粒子件数単位で保存再開するJobの状態です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialParticleJobStatus {
    /// まだ全候補の経路検査を終えていません。
    Computing,
    /// 入力が失効していなければ一取引で公開できます。
    Ready,
}

/// 保存可能な粒子Jobが計画したphaseの診断値です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialParticleJobReport {
    /// 検査を終えた粒子数。
    pub processed: usize,
    /// cellを跨ぐ経路を持つ粒子数。
    pub moved: usize,
    /// 公開時にcellへ着地する粒子数。
    pub landed: usize,
    /// 非活動域または先行粒子の予約により延期する粒子数。
    pub waiting: usize,
    /// DDAで検査した境界数。
    pub checked_steps: usize,
}

#[derive(Clone, Copy, Debug)]
struct MaterialParticleJobCandidate {
    id: Id<MaterialParticleKind>,
    layer: LayerId,
    position: [i64; 2],
    velocity: [i64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MaterialParticleJobPlan {
    position: [i64; 2],
    cell: CellPoint,
    stopped: bool,
    waiting: bool,
    landed: bool,
}

/// 正規粒子の移動を検査時点と公開時点に分ける、保存可能な件数分割Jobです。
///
/// `new`はslot順の候補、開始時占有、活動chunk revisionを一度だけ記録します。
/// `step`は最大`budget`個の粒子について整数DDAとphase予約を更新しますが、Worldを
/// 変更しません。全候補を処理した後の`publish`だけが候補位置を変更し、停止粒子の
/// 着地まで同じWorldEdit内で実行します。入力chunkや粒子が変わった場合は公開を拒否します。
#[derive(Clone, Debug)]
pub struct MaterialParticleJob {
    world: crate::foundation::id::WorldId,
    content: [u8; 32],
    epoch: u64,
    candidates: Vec<MaterialParticleJobCandidate>,
    occupied: Vec<(u32, i32, i32)>,
    chunks: Vec<(ChunkCoord, u64)>,
    reserved: std::collections::BTreeSet<(u32, i32, i32)>,
    plans: Vec<MaterialParticleJobPlan>,
    candidate_limit: usize,
    step_limit: usize,
    checked_steps: usize,
    report: MaterialParticleJobReport,
}

impl MaterialParticleJob {
    /// 現在活動中の全simulated particleを対象にJobを作ります。
    ///
    /// `candidate_limit`は一Jobが保持する活動粒子数、`step_limit`はJob全体で許すDDA境界数です。
    /// 候補・予約・保存recordはWorldLimits.job_cells/job_bytesの両方で制限します。
    pub fn new(world: &World, candidate_limit: usize, step_limit: usize) -> EngineResult<Self> {
        if candidate_limit > world.limits.job_cells || step_limit > world.limits.job_cells {
            return Err(EngineError::capacity(
                "粒子Jobの候補・DDA上限",
                candidate_limit.max(step_limit) as u64,
                world.limits.job_cells as u64,
            ));
        }
        let mut resident_count = 0usize;
        let mut candidate_count = 0usize;
        for (_, particle) in world.particles.iter() {
            if particle.is_archived() {
                continue;
            }
            resident_count = resident_count
                .checked_add(1)
                .ok_or_else(|| invalid("粒子Job占有件数overflow"))?;
            let cell = particle.cell();
            if !world.content.layer(particle.layer)?.simulated
                || !world
                    .chunks
                    .get(&ChunkCoord::at(cell))
                    .is_some_and(|chunk| chunk.active)
            {
                continue;
            }
            candidate_count = candidate_count
                .checked_add(1)
                .ok_or_else(|| invalid("粒子Job候補件数overflow"))?;
        }
        if candidate_count > candidate_limit {
            return Err(EngineError::capacity(
                "粒子Job候補数",
                candidate_count as u64,
                candidate_limit as u64,
            ));
        }
        let active_count = world.chunks.values().filter(|chunk| chunk.active).count();
        let base_cells = candidate_count
            .checked_add(active_count)
            .ok_or_else(|| invalid("粒子Job初期cell数overflow"))?;
        if base_cells > world.limits.job_cells {
            return Err(EngineError::capacity(
                "粒子Job初期cell数",
                base_cells as u64,
                world.limits.job_cells as u64,
            ));
        }
        let base_bytes = 160usize
            .checked_add(
                candidate_count
                    .checked_mul(48)
                    .ok_or_else(|| invalid("粒子Job候補byte数overflow"))?,
            )
            .and_then(|n| n.checked_add(resident_count.checked_mul(24)?))
            .and_then(|n| n.checked_add(active_count.checked_mul(16)?))
            .ok_or_else(|| invalid("粒子Job初期byte数overflow"))?;
        if base_bytes > world.limits.job_bytes {
            return Err(EngineError::capacity(
                "粒子Job初期byte数",
                base_bytes as u64,
                world.limits.job_bytes as u64,
            ));
        }
        let mut candidates = Vec::new();
        let mut occupied = Vec::new();
        candidates
            .try_reserve_exact(candidate_count)
            .map_err(allocation)?;
        occupied
            .try_reserve_exact(resident_count)
            .map_err(allocation)?;
        for (id, particle) in world.particles.iter() {
            if particle.is_archived() {
                continue;
            }
            let cell = particle.cell();
            occupied.push((particle.layer.0, cell.x, cell.y));
            if !world.content.layer(particle.layer)?.simulated
                || !world
                    .chunks
                    .get(&ChunkCoord::at(cell))
                    .is_some_and(|chunk| chunk.active)
            {
                continue;
            }
            candidates.push(MaterialParticleJobCandidate {
                id,
                layer: particle.layer,
                position: particle.position,
                velocity: particle.velocity,
            });
        }
        occupied.sort_unstable();
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(active_count).map_err(allocation)?;
        for (coord, chunk) in &world.chunks {
            if chunk.active {
                chunks.push((*coord, chunk.revision));
            }
        }
        let mut value = Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            candidates,
            occupied,
            chunks,
            reserved: std::collections::BTreeSet::new(),
            plans: Vec::new(),
            candidate_limit,
            step_limit,
            checked_steps: 0,
            report: MaterialParticleJobReport::default(),
        };
        value.check_byte_limit(world)?;
        value
            .plans
            .try_reserve_exact(value.candidates.len())
            .map_err(allocation)?;
        Ok(value)
    }

    /// 検査状態です。全候補を処理したJobは公開可能です。
    pub fn status(&self) -> MaterialParticleJobStatus {
        if self.plans.len() == self.candidates.len() {
            MaterialParticleJobStatus::Ready
        } else {
            MaterialParticleJobStatus::Computing
        }
    }

    /// 処理済み件数とDDA診断を返します。
    pub fn report(&self) -> MaterialParticleJobReport {
        self.report
    }

    /// 最大`budget`個の候補を検査します。Worldには変更を公開しません。
    pub fn step(
        &mut self,
        world: &World,
        budget: usize,
    ) -> EngineResult<MaterialParticleJobStatus> {
        self.validate_context(world)?;
        for _ in 0..budget {
            if self.status() == MaterialParticleJobStatus::Ready {
                break;
            }
            let candidate = self.candidates[self.plans.len()];
            let base_cells = self
                .candidates
                .len()
                .checked_add(self.chunks.len())
                .and_then(|n| n.checked_add(self.reserved.len()))
                .ok_or_else(|| invalid("粒子Job予約数overflow"))?;
            let cells_left = world.limits.job_cells.saturating_sub(base_cells);
            let bytes_left = world
                .limits
                .job_bytes
                .saturating_sub(self.estimated_bytes()?)
                / 64;
            let path_limit = cells_left.min(bytes_left);
            let step_budget = self.step_limit.saturating_sub(self.checked_steps);
            let (plan, path, checked) =
                self.plan_candidate(world, candidate, &self.reserved, path_limit, step_budget)?;
            let next_steps = self
                .checked_steps
                .checked_add(checked)
                .ok_or_else(|| invalid("粒子Job DDA件数overflow"))?;
            if next_steps > self.step_limit {
                return Err(EngineError::capacity(
                    "粒子Job DDA検査数",
                    next_steps as u64,
                    self.step_limit as u64,
                ));
            }
            let additional = path.len();
            let next_cells = self
                .candidates
                .len()
                .checked_add(self.chunks.len())
                .and_then(|n| n.checked_add(self.reserved.len()))
                .and_then(|n| n.checked_add(additional))
                .ok_or_else(|| invalid("粒子Job予約数overflow"))?;
            if next_cells > world.limits.job_cells {
                return Err(EngineError::capacity(
                    "粒子Job予約数",
                    next_cells as u64,
                    world.limits.job_cells as u64,
                ));
            }
            let next_bytes = self
                .estimated_bytes()?
                .checked_add(
                    additional
                        .checked_mul(64)
                        .ok_or_else(|| invalid("粒子Job予約byte数overflow"))?,
                )
                .ok_or_else(|| invalid("粒子Job byte数overflow"))?;
            if next_bytes > world.limits.job_bytes {
                return Err(EngineError::capacity(
                    "粒子Job byte数",
                    next_bytes as u64,
                    world.limits.job_bytes as u64,
                ));
            }
            for point in path {
                self.reserved.insert((candidate.layer.0, point.x, point.y));
            }
            self.checked_steps = next_steps;
            self.report.processed = self
                .report
                .processed
                .checked_add(1)
                .ok_or_else(|| invalid("粒子Job処理件数overflow"))?;
            self.report.moved = self
                .report
                .moved
                .checked_add(usize::from(plan.cell != candidate_cell(candidate)))
                .ok_or_else(|| invalid("粒子Job移動件数overflow"))?;
            self.report.waiting = self
                .report
                .waiting
                .checked_add(usize::from(plan.waiting))
                .ok_or_else(|| invalid("粒子Job待機件数overflow"))?;
            self.report.landed = self
                .report
                .landed
                .checked_add(usize::from(plan.landed))
                .ok_or_else(|| invalid("粒子Job着地件数overflow"))?;
            self.report.checked_steps = self.checked_steps;
            self.plans.push(plan);
            self.check_byte_limit(world)?;
        }
        Ok(self.status())
    }

    /// 準備済み粒子移動を一つのWorld取引で公開します。
    pub fn publish(&self, editor: &mut WorldEdit<'_>) -> EngineResult<MaterialParticleJobReport> {
        editor.run(|edit| {
            self.validate_structure()?;
            self.validate_publish_context(edit.world)?;
            if self.status() != MaterialParticleJobStatus::Ready {
                return Err(conflict("粒子Jobが未完成"));
            }
            self.validate_plans_against_world(edit.world)?;
            for (candidate, plan) in self.candidates.iter().zip(&self.plans) {
                if plan.position != candidate.position
                    || (plan.stopped && candidate.velocity != [0, 0])
                {
                    edit.reserve_writes(1)?;
                    let particle = edit.world.particles.get_mut(candidate.id)?;
                    particle.position = plan.position;
                    if plan.stopped {
                        particle.velocity = [0, 0];
                    }
                    edit.touch_particle_chunks(candidate_cell(*candidate), plan.cell)?;
                }
            }
            for (candidate, plan) in self.candidates.iter().zip(&self.plans) {
                if plan.landed {
                    edit.land_uncontested_particle(MaterialParticleHandle {
                        world: self.world,
                        id: candidate.id,
                    })?;
                }
            }
            Ok(self.report)
        })
    }

    fn plan_candidate(
        &self,
        world: &World,
        candidate: MaterialParticleJobCandidate,
        reserved: &std::collections::BTreeSet<(u32, i32, i32)>,
        path_limit: usize,
        step_budget: usize,
    ) -> EngineResult<(MaterialParticleJobPlan, Vec<CellPoint>, usize)> {
        let start = candidate.position;
        let velocity = candidate.velocity;
        let source = candidate_cell(candidate);
        let mut end = [0i64; 2];
        for axis in 0..2 {
            end[axis] = start[axis]
                .checked_add(velocity[axis])
                .ok_or_else(|| invalid("粒子Job変位overflow"))?;
            if end[axis].div_euclid(SCALE) < i64::from(i32::MIN)
                || end[axis].div_euclid(SCALE) > i64::from(i32::MAX)
            {
                return Err(invalid("粒子Job変位がcell座標範囲外"));
            }
        }
        let target = CellPoint {
            x: end[0].div_euclid(SCALE) as i32,
            y: end[1].div_euclid(SCALE) as i32,
        };
        let mut current = source;
        let sign = [velocity[0].signum() as i32, velocity[1].signum() as i32];
        let speed = [
            velocity[0].unsigned_abs() as u128,
            velocity[1].unsigned_abs() as u128,
        ];
        let mut waiting = reserved.contains(&(candidate.layer.0, source.x, source.y));
        let mut stopped = velocity == [0, 0] && !waiting;
        let mut checked = 0usize;
        let mut path = Vec::new();
        if !waiting && path_limit == 0 {
            return Err(EngineError::capacity("粒子Job経路byte数", 1, 0));
        }
        if !waiting {
            path.try_reserve(1).map_err(allocation)?;
            path.push(source);
        }
        while current != target && !waiting {
            if checked == step_budget {
                return Err(EngineError::capacity(
                    "粒子Job DDA検査数",
                    self.checked_steps.saturating_add(checked).saturating_add(1) as u64,
                    self.step_limit as u64,
                ));
            }
            checked = checked
                .checked_add(1)
                .ok_or_else(|| invalid("粒子Job DDA件数overflow"))?;
            let coord = [current.x, current.y];
            let distance: [u128; 2] = std::array::from_fn(|axis| {
                let boundary = (i64::from(coord[axis]) + i64::from(sign[axis] > 0)) * SCALE;
                (boundary - start[axis]).unsigned_abs() as u128
            });
            let x = current.x != target.x
                && (current.y == target.y || distance[0] * speed[1] <= distance[1] * speed[0]);
            let y = current.y != target.y
                && (current.x == target.x || distance[1] * speed[0] <= distance[0] * speed[1]);
            let next = CellPoint {
                x: current
                    .x
                    .checked_add(if x { sign[0] } else { 0 })
                    .ok_or_else(|| invalid("粒子Job DDA水平端"))?,
                y: current
                    .y
                    .checked_add(if y { sign[1] } else { 0 })
                    .ok_or_else(|| invalid("粒子Job DDA垂直端"))?,
            };
            let mut guards = Vec::new();
            if x && y {
                guards.try_reserve(3).map_err(allocation)?;
                guards.push(CellPoint {
                    x: next.x,
                    y: current.y,
                });
                guards.push(CellPoint {
                    x: current.x,
                    y: next.y,
                });
            } else {
                guards.try_reserve(1).map_err(allocation)?;
            }
            guards.push(next);
            for point in &guards {
                if self
                    .occupied
                    .binary_search(&(candidate.layer.0, point.x, point.y))
                    .is_ok()
                    || reserved.contains(&(candidate.layer.0, point.x, point.y))
                {
                    waiting = true;
                    break;
                }
                match world.cell(candidate.layer, *point)? {
                    CellQuery::WorldBoundary => {
                        stopped = true;
                        break;
                    }
                    CellQuery::Loaded(cell) => {
                        if !world
                            .chunks
                            .get(&ChunkCoord::at(*point))
                            .is_some_and(|chunk| chunk.active)
                        {
                            waiting = true;
                            break;
                        }
                        if cell.block.is_some() || cell.core.material != MaterialId::AIR {
                            stopped = true;
                            break;
                        }
                    }
                    _ => {
                        waiting = true;
                        break;
                    }
                }
            }
            if stopped || waiting {
                break;
            }
            let next_path_len = path
                .len()
                .checked_add(guards.len())
                .ok_or_else(|| invalid("粒子Job経路件数overflow"))?;
            if next_path_len > path_limit {
                return Err(EngineError::capacity(
                    "粒子Job経路件数",
                    next_path_len as u64,
                    path_limit as u64,
                ));
            }
            path.try_reserve(guards.len()).map_err(allocation)?;
            path.extend(guards);
            current = next;
        }
        let position = if (stopped || waiting) && current == source {
            start
        } else if stopped || waiting {
            [
                i64::from(current.x) * SCALE + SCALE / 2,
                i64::from(current.y) * SCALE + SCALE / 2,
            ]
        } else {
            end
        };
        let mut landed = false;
        if stopped {
            let layer_key = world.content.layer(candidate.layer)?.key;
            let empty_payload = world
                .channels
                .iter()
                .filter(|channel| channel.material_layer() == Some(layer_key))
                .try_fold(true, |empty, channel| {
                    Ok::<_, EngineError>(empty && channel.payload_is_default(current)?)
                })?;
            let source_key = (candidate.layer.0, source.x, source.y);
            let shared_source = current == source
                && self.occupied.partition_point(|key| *key <= source_key)
                    - self.occupied.partition_point(|key| *key < source_key)
                    > 1;
            landed = !shared_source
                && matches!(world.cell(candidate.layer, current)?, CellQuery::Loaded(cell) if cell.core == CellCore::AIR && cell.block.is_none())
                && empty_payload;
        }
        let plan = MaterialParticleJobPlan {
            position,
            cell: current,
            stopped,
            waiting,
            landed,
        };
        Ok((plan, path, checked))
    }

    fn validate_context(&self, world: &World) -> EngineResult<()> {
        if world.id != self.world
            || world.content.fingerprint() != self.content
            || world.remaps.epoch() != self.epoch
        {
            return Err(conflict("粒子JobのWorld・Content・素材epoch"));
        }
        let cell_records = self
            .candidates
            .len()
            .checked_add(self.chunks.len())
            .and_then(|n| n.checked_add(self.reserved.len()))
            .ok_or_else(|| invalid("粒子Job保存cell数overflow"))?;
        if self.candidate_limit > world.limits.job_cells
            || self.step_limit > world.limits.job_cells
            || cell_records > world.limits.job_cells
        {
            return Err(EngineError::capacity(
                "粒子Job保存cell数",
                cell_records.max(self.candidate_limit).max(self.step_limit) as u64,
                world.limits.job_cells as u64,
            ));
        }
        self.check_byte_limit(world)?;
        let active_count = world.chunks.values().filter(|chunk| chunk.active).count();
        if active_count != self.chunks.len() {
            return Err(conflict("粒子Jobの活動chunk集合が変化"));
        }
        for &(coord, revision) in &self.chunks {
            if !world
                .chunks
                .get(&coord)
                .is_some_and(|chunk| chunk.active && chunk.revision == revision)
            {
                return Err(conflict("粒子JobのWorld入力revisionが失効"));
            }
        }
        Ok(())
    }

    fn validate_publish_context(&self, world: &World) -> EngineResult<()> {
        self.validate_context(world)?;
        for candidate in &self.candidates {
            let particle = world.particles.get(candidate.id)?;
            if particle.layer != candidate.layer
                || particle.position != candidate.position
                || particle.velocity != candidate.velocity
                || particle.is_archived()
            {
                return Err(conflict("粒子Jobの候補状態が失効"));
            }
        }
        let resident_count = world
            .particles
            .iter()
            .filter(|(_, particle)| !particle.is_archived())
            .count();
        if resident_count != self.occupied.len() {
            return Err(conflict("粒子Jobの占有snapshot件数が失効"));
        }
        let mut occupied = Vec::new();
        occupied
            .try_reserve_exact(resident_count)
            .map_err(allocation)?;
        for (_, particle) in world
            .particles
            .iter()
            .filter(|(_, particle)| !particle.is_archived())
        {
            let cell = particle.cell();
            occupied.push((particle.layer.0, cell.x, cell.y));
        }
        occupied.sort_unstable();
        if occupied != self.occupied {
            return Err(conflict("粒子Jobの占有snapshotが失効"));
        }
        Ok(())
    }

    /// 保存planが現Worldに対する候補順・DDA・予約結果と一致することを公開直前に再計算します。
    fn validate_plans_against_world(&self, world: &World) -> EngineResult<()> {
        let stored_reserved_bytes = self
            .reserved
            .len()
            .checked_mul(64)
            .ok_or_else(|| invalid("粒子Job予約byte数overflow"))?;
        let fixed_bytes = self
            .estimated_bytes()?
            .checked_sub(stored_reserved_bytes)
            .ok_or_else(|| invalid("粒子Job固定byte数不整合"))?;
        let mut reserved = std::collections::BTreeSet::new();
        let mut checked_steps = 0usize;
        for (candidate, saved_plan) in self.candidates.iter().zip(&self.plans) {
            let base_cells = self
                .candidates
                .len()
                .checked_add(self.chunks.len())
                .and_then(|count| count.checked_add(reserved.len()))
                .ok_or_else(|| invalid("粒子Job再検査cell数overflow"))?;
            let cells_left = world.limits.job_cells.saturating_sub(base_cells);
            let current_reserved_bytes = reserved
                .len()
                .checked_mul(64)
                .ok_or_else(|| invalid("粒子Job再検査byte数overflow"))?;
            let bytes_used = fixed_bytes
                .checked_add(current_reserved_bytes)
                .ok_or_else(|| invalid("粒子Job再検査byte数overflow"))?;
            let bytes_left = world.limits.job_bytes.saturating_sub(bytes_used) / 64;
            let path_limit = cells_left.min(bytes_left);
            let step_budget = self.step_limit.saturating_sub(checked_steps);
            let (plan, path, checked) =
                self.plan_candidate(world, *candidate, &reserved, path_limit, step_budget)?;
            if plan != *saved_plan {
                return Err(conflict("粒子Jobの保存計画がDDA再計算結果と不一致"));
            }
            checked_steps = checked_steps
                .checked_add(checked)
                .ok_or_else(|| invalid("粒子Job再検査DDA件数overflow"))?;
            for point in path {
                reserved.insert((candidate.layer.0, point.x, point.y));
            }
        }
        if checked_steps != self.checked_steps || reserved != self.reserved {
            return Err(conflict(
                "粒子Jobの保存進捗・経路予約がDDA再計算結果と不一致",
            ));
        }
        Ok(())
    }

    fn estimated_bytes(&self) -> EngineResult<usize> {
        160usize
            .checked_add(
                self.candidates
                    .len()
                    .checked_mul(48)
                    .ok_or_else(|| invalid("粒子Job候補byte数overflow"))?,
            )
            .and_then(|n| n.checked_add(self.occupied.len().checked_mul(24)?))
            .and_then(|n| n.checked_add(self.chunks.len().checked_mul(16)?))
            .and_then(|n| n.checked_add(self.reserved.len().checked_mul(64)?))
            .and_then(|n| n.checked_add(self.candidates.len().checked_mul(32)?))
            .ok_or_else(|| invalid("粒子Job byte数overflow"))
    }

    fn check_byte_limit(&self, world: &World) -> EngineResult<()> {
        let bytes = self.estimated_bytes()?;
        if bytes > world.limits.job_bytes {
            return Err(EngineError::capacity(
                "粒子Job byte数",
                bytes as u64,
                world.limits.job_bytes as u64,
            ));
        }
        Ok(())
    }

    /// barrierごとのStateStore保存では全snapshotをcodec化するため、保存前検証は進捗の
    /// O(1)不変条件に限定します。各要素の完全検証はdecode後と最終publish時に一度行います。
    fn validate_progress(&self) -> EngineResult<()> {
        if self.candidates.len() > self.candidate_limit
            || self.plans.len() > self.candidates.len()
            || self.report.processed != self.plans.len()
            || self.report.checked_steps != self.checked_steps
            || self.checked_steps > self.step_limit
            || self.report.moved > self.report.processed
            || self.report.landed > self.report.processed
            || self.report.waiting > self.report.processed
        {
            return Err(invalid("粒子Job進捗"));
        }
        Ok(())
    }

    fn validate_structure(&self) -> EngineResult<()> {
        self.validate_progress()?;
        let mut moved = 0usize;
        let mut landed = 0usize;
        let mut waiting = 0usize;
        for (plan, candidate) in self.plans.iter().zip(&self.candidates) {
            moved += usize::from(plan.cell != candidate_cell(*candidate));
            landed += usize::from(plan.landed);
            waiting += usize::from(plan.waiting);
        }
        if self.occupied.windows(2).any(|pair| pair[0] > pair[1])
            || self.chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self
                .candidates
                .windows(2)
                .any(|pair| pair[0].id >= pair[1].id)
            || self.report.moved != moved
            || self.report.landed != landed
            || self.report.waiting != waiting
            || self
                .plans
                .iter()
                .zip(&self.candidates)
                .any(|(plan, candidate)| {
                    plan.cell.x != plan.position[0].div_euclid(SCALE) as i32
                        || plan.cell.y != plan.position[1].div_euclid(SCALE) as i32
                        || plan.landed && (!plan.stopped || plan.waiting)
                        || !position_in_bounds(candidate.position)
                        || !position_in_bounds(plan.position)
                        || !self.reserved.contains(&(
                            candidate.layer.0,
                            candidate_cell(*candidate).x,
                            candidate_cell(*candidate).y,
                        ))
                        || !self
                            .reserved
                            .contains(&(candidate.layer.0, plan.cell.x, plan.cell.y))
                })
        {
            return Err(invalid("粒子Job進捗・snapshot"));
        }
        Ok(())
    }
}

fn candidate_cell(candidate: MaterialParticleJobCandidate) -> CellPoint {
    CellPoint {
        x: candidate.position[0].div_euclid(SCALE) as i32,
        y: candidate.position[1].div_euclid(SCALE) as i32,
    }
}

fn position_in_bounds(position: [i64; 2]) -> bool {
    position
        .into_iter()
        .all(|value| (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&value.div_euclid(SCALE)))
}

impl StateCodec for MaterialParticleJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_progress()?;
        out.raw(b"CHMPJOB1")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        out.u64(self.candidate_limit as u64)?;
        out.u64(self.step_limit as u64)?;
        out.u64(self.checked_steps as u64)?;
        out.u64(self.report.processed as u64)?;
        out.u64(self.report.moved as u64)?;
        out.u64(self.report.landed as u64)?;
        out.u64(self.report.waiting as u64)?;
        out.u64(self.report.checked_steps as u64)?;
        out.u32(u32::try_from(self.chunks.len()).map_err(|_| invalid("粒子Job chunk件数"))?)?;
        for &(coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(revision)?;
        }
        out.u32(u32::try_from(self.candidates.len()).map_err(|_| invalid("粒子Job候補件数"))?)?;
        for candidate in &self.candidates {
            candidate.id.encode(out)?;
            candidate.layer.encode(out)?;
            for value in candidate.position.into_iter().chain(candidate.velocity) {
                out.i64(value)?;
            }
        }
        out.u32(u32::try_from(self.occupied.len()).map_err(|_| invalid("粒子Job占有件数"))?)?;
        for &(layer, x, y) in &self.occupied {
            out.u32(layer)?;
            out.i32(x)?;
            out.i32(y)?;
        }
        out.u32(u32::try_from(self.reserved.len()).map_err(|_| invalid("粒子Job予約件数"))?)?;
        for &(layer, x, y) in &self.reserved {
            out.u32(layer)?;
            out.i32(x)?;
            out.i32(y)?;
        }
        out.u32(u32::try_from(self.plans.len()).map_err(|_| invalid("粒子Job計画件数"))?)?;
        for plan in &self.plans {
            for value in plan.position {
                out.i64(value)?;
            }
            plan.cell.encode(out)?;
            out.u8(u8::from(plan.stopped)
                | (u8::from(plan.waiting) << 1)
                | (u8::from(plan.landed) << 2))?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHMPJOB1" {
            return Err(invalid("粒子Job形式"));
        }
        let world = crate::foundation::id::WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("粒子Job Content hash"))?;
        let epoch = input.u64()?;
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job候補上限"))?;
        let step_limit = usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job DDA上限"))?;
        let checked_steps =
            usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job DDA進捗"))?;
        let report = MaterialParticleJobReport {
            processed: usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job処理件数"))?,
            moved: usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job移動件数"))?,
            landed: usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job着地件数"))?,
            waiting: usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job待機件数"))?,
            checked_steps: usize::try_from(input.u64()?).map_err(|_| invalid("粒子Job検査件数"))?,
        };
        let chunk_count = input.u32()? as usize;
        if chunk_count > input.remaining() / 16 {
            return Err(invalid("粒子Job chunk件数"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(chunk_count).map_err(allocation)?;
        for _ in 0..chunk_count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let candidate_count = input.u32()? as usize;
        if candidate_count > candidate_limit || candidate_count > input.remaining() / 44 {
            return Err(invalid("粒子Job候補件数"));
        }
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(candidate_count)
            .map_err(allocation)?;
        for _ in 0..candidate_count {
            let id = Id::decode(input)?;
            let layer = LayerId::decode(input)?;
            let values = [input.i64()?, input.i64()?, input.i64()?, input.i64()?];
            candidates.push(MaterialParticleJobCandidate {
                id,
                layer,
                position: [values[0], values[1]],
                velocity: [values[2], values[3]],
            });
        }
        let occupied_count = input.u32()? as usize;
        if occupied_count > input.remaining() / 12 {
            return Err(invalid("粒子Job占有件数"));
        }
        let mut occupied = Vec::new();
        occupied
            .try_reserve_exact(occupied_count)
            .map_err(allocation)?;
        for _ in 0..occupied_count {
            occupied.push((input.u32()?, input.i32()?, input.i32()?));
        }
        let reserved_count = input.u32()? as usize;
        if reserved_count > input.remaining() / 12 {
            return Err(invalid("粒子Job予約件数"));
        }
        let mut reserved = std::collections::BTreeSet::new();
        for _ in 0..reserved_count {
            let point = (input.u32()?, input.i32()?, input.i32()?);
            if !reserved.insert(point) {
                return Err(invalid("粒子Job重複予約"));
            }
        }
        let plan_count = input.u32()? as usize;
        if plan_count != report.processed
            || plan_count > candidate_count
            || plan_count > input.remaining() / 25
        {
            return Err(invalid("粒子Job計画件数"));
        }
        let mut plans = Vec::new();
        plans.try_reserve_exact(plan_count).map_err(allocation)?;
        for _ in 0..plan_count {
            let position = [input.i64()?, input.i64()?];
            let cell = CellPoint::decode(input)?;
            let flags = input.u8()?;
            if flags & !7 != 0 {
                return Err(invalid("粒子Job計画flag"));
            }
            plans.push(MaterialParticleJobPlan {
                position,
                cell,
                stopped: flags & 1 != 0,
                waiting: flags & 2 != 0,
                landed: flags & 4 != 0,
            });
        }
        let job = Self {
            world,
            content,
            epoch,
            candidates,
            occupied,
            chunks,
            reserved,
            plans,
            candidate_limit,
            step_limit,
            checked_steps,
            report,
        };
        job.validate_structure()?;
        Ok(job)
    }
}
