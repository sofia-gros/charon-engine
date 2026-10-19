//! 地形Worldの明示codec。復元は独立候補を検証して返します。
//!
//! callbackやArcのアドレスは書きません。Contentは呼出側が登録済みの同じmanifestを
//! 渡します。未知の素材を空気へ置換したり、保存値の上限を信頼して先に確保したりしません。

use super::{
    BlockInstance, CellCore, ChunkCoord, Content, GenerationStamp, LayerId, LeaseMode, MaterialId,
    Orientation, RegionLease, RegionRequest, RequestStatus, World, WorldBounds, WorldLimits,
    generation::PreparedChunk,
    invalid,
    terrain::{CELL_COUNT, Chunk, ChunkPage},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::{Arena, WorldId},
    transaction::CandidateState,
};
use std::sync::Arc;

impl StateCodec for BlockInstance {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.material.encode(out)?;
        self.layer.encode(out)?;
        self.anchor.encode(out)?;
        out.u64(self.epoch)?;
        out.u8(u8::from(self.dormant.is_some()))?;
        if let Some(hash) = self.dormant {
            out.raw(&hash)?;
        }
        match &self.properties {
            Some(store) if out.is_measuring() => out.measure_bytes(store.checkpoint_len()?)?,
            Some(store) => out.bytes(&store.checkpoint()?)?,
            None => out.bytes(&self.pending_properties)?,
        }
        out.u8(match self.orientation {
            Orientation::Original => 0,
            Orientation::Right => 1,
            Orientation::Down => 2,
            Orientation::Left => 3,
        })?;
        out.u32(u32::try_from(self.remaining.len()).map_err(|_| invalid("block mask長さ"))?)?;
        for index in &self.remaining {
            out.u32(*index)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let material = MaterialId::decode(input)?;
        let layer = LayerId::decode(input)?;
        let anchor = CellPoint::decode(input)?;
        let epoch = input.u64()?;
        let dormant = match input.u8()? {
            0 => None,
            1 => Some(
                input
                    .raw(32)?
                    .try_into()
                    .map_err(|_| invalid("休止block hash"))?,
            ),
            _ => return Err(invalid("休止block種別")),
        };
        let property_limit = input.remaining();
        let pending_properties = super::terrain::copy_slice(input.bytes(property_limit)?)?;
        let orientation = match input.u8()? {
            0 => Orientation::Original,
            1 => Orientation::Right,
            2 => Orientation::Down,
            3 => Orientation::Left,
            _ => return Err(invalid("block向き")),
        };
        let count = input.u32()? as usize;
        if count > input.remaining() / 4 {
            return Err(invalid("block maskの切断"));
        }
        let mut remaining = Vec::new();
        remaining.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            remaining.push(input.u32()?);
        }
        let value = Self {
            material,
            layer,
            anchor,
            orientation,
            remaining,
            epoch,
            dormant,
            properties: None,
            pending_properties,
        };
        value.validate()?;
        Ok(value)
    }
}
impl StateCodec for RegionLease {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.source.encode(out)?;
        self.region.encode(out)?;
        out.u8(match self.mode {
            LeaseMode::LoadOnly => 0,
            LeaseMode::Simulate => 1,
            LeaseMode::Pin => 2,
        })?;
        self.expires.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let source = StableKey::decode(input)?;
        let region = super::ChunkRegion::decode(input)?;
        let mode = match input.u8()? {
            0 => LeaseMode::LoadOnly,
            1 => LeaseMode::Simulate,
            2 => LeaseMode::Pin,
            _ => return Err(invalid("lease mode")),
        };
        Ok(Self {
            source,
            region,
            mode,
            expires: Option::<u64>::decode(input)?,
        })
    }
}
impl StateCodec for GenerationStamp {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.key.encode(out)?;
        out.u32(self.version)?;
        out.raw(&self.fingerprint)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = Self {
            key: StableKey::decode(input)?,
            version: input.u32()?,
            fingerprint: input.raw(32)?.try_into().map_err(|_| invalid("生成hash"))?,
        };
        if value.version == 0 {
            return Err(invalid("生成版"));
        }
        Ok(value)
    }
}
impl StateCodec for RegionRequest {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.source.encode(out)?;
        self.region.encode(out)?;
        self.stamp.encode(out)?;
        out.u64(self.due_tick)?;
        out.u64(self.attempt)?;
        out.raw(&self.content_hash)?;
        out.u8(match self.status {
            RequestStatus::Requested => 0,
            RequestStatus::Preparing => 1,
            RequestStatus::Ready => 2,
            RequestStatus::Failed => 3,
            RequestStatus::Cancelled => 4,
            RequestStatus::Published => 5,
        })?;
        if let Some(ready) = &self.ready {
            out.u32(ready.len() as u32)?;
            for chunk in ready {
                chunk.coordinate.encode(out)?;
                out.u32(chunk.pages.len() as u32)?;
                for page in &chunk.pages {
                    encode_page(page, out)?;
                }
            }
        }
        // 診断文やOSの詳細は正規状態に含めず、失敗の区分だけを保存します。
        out.u8(self.failure.as_ref().map_or(255, |error| code(error.code)))
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let source = StableKey::decode(input)?;
        let region = super::ChunkRegion::decode(input)?;
        let stamp = GenerationStamp::decode(input)?;
        let due_tick = input.u64()?;
        let attempt = input.u64()?;
        let content_hash = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("生成入力content hash"))?;
        let status = match input.u8()? {
            0 => RequestStatus::Requested,
            1 => RequestStatus::Preparing,
            2 => RequestStatus::Ready,
            3 => RequestStatus::Failed,
            4 => RequestStatus::Cancelled,
            5 => RequestStatus::Published,
            _ => return Err(invalid("生成要求の状態")),
        };
        let ready = if status == RequestStatus::Ready {
            let count = input.u32()? as usize;
            // 一つのchunkには少なくとも座標とlayer数が必要です。
            if count as u64 != region.chunk_count() || count > input.remaining() / 12 {
                return Err(invalid("Ready chunk数"));
            }
            let mut chunks = Vec::new();
            chunks.try_reserve_exact(count).map_err(allocation)?;
            for _ in 0..count {
                let coordinate = ChunkCoord::decode(input)?;
                let count = input.u32()? as usize;
                if count > input.remaining() / (CELL_COUNT * 8) {
                    return Err(invalid("Ready layer数"));
                }
                let mut pages = Vec::new();
                pages.try_reserve_exact(count).map_err(allocation)?;
                for _ in 0..count {
                    pages.push(Arc::new(decode_page(input)?));
                }
                chunks.push(PreparedChunk { coordinate, pages });
            }
            Some(chunks)
        } else {
            None
        };
        let failure = match input.u8()? {
            255 => None,
            value => Some(EngineError::new(decode_code(value)?, "復元した生成失敗")),
        };
        let value = Self {
            source,
            region,
            stamp,
            due_tick,
            attempt,
            content_hash,
            status,
            ready,
            failure,
        };
        value.validate()?;
        Ok(value)
    }
}

pub(crate) fn encode_page(page: &ChunkPage, out: &mut Encoder) -> EngineResult<()> {
    for cell in &page.cells {
        cell.encode(out)?;
    }
    out.u8(u8::from(page.blocks.is_some()))?;
    if let Some(blocks) = &page.blocks {
        for block in blocks {
            block.encode(out)?;
        }
    }
    Ok(())
}
pub(crate) fn decode_page(input: &mut Decoder<'_>) -> EngineResult<ChunkPage> {
    if input.remaining() < CELL_COUNT * 8 + 1 {
        return Err(invalid("chunk page切断"));
    }
    let mut cells = Vec::new();
    cells.try_reserve_exact(CELL_COUNT).map_err(allocation)?;
    for _ in 0..CELL_COUNT {
        cells.push(CellCore::decode(input)?);
    }
    let blocks = match input.u8()? {
        0 => None,
        1 => {
            if input.remaining() < CELL_COUNT {
                return Err(invalid("block sidecar切断"));
            }
            let mut blocks = Vec::new();
            blocks.try_reserve_exact(CELL_COUNT).map_err(allocation)?;
            for _ in 0..CELL_COUNT {
                blocks.push(Option::<super::BlockId>::decode(input)?);
            }
            Some(blocks)
        }
        _ => return Err(invalid("block sidecar種別")),
    };
    Ok(ChunkPage { cells, blocks })
}

// baseは生成器の実装に依存せず復元できる不変payloadとして保存します。変更が少ないlayerは
// cell indexと値だけを書き、多い場合やblock sidecar変更では完全pageへ切り替えます。
// この切替は符号化だけの選択であり、復元後の正規状態や生成履歴の意味を変えません。
pub(crate) fn encode_chunk_pages(chunk: &Chunk, out: &mut Encoder) -> EngineResult<()> {
    chunk.baseline.is_some().encode(out)?;
    out.u32(chunk.layers.len() as u32)?;
    for (layer, current) in chunk.layers.iter().enumerate() {
        let Some(base) = chunk.baseline.as_ref().map(|pages| &pages[layer]) else {
            encode_page(current, out)?;
            continue;
        };
        encode_page(base, out)?;
        let changed = base
            .cells
            .iter()
            .zip(&current.cells)
            .filter(|(a, b)| a != b)
            .count();
        if base.blocks == current.blocks && changed <= CELL_COUNT * 8 / 10 {
            out.u8(0)?;
            out.u32(changed as u32)?;
            for (index, (a, b)) in base.cells.iter().zip(&current.cells).enumerate() {
                if a != b {
                    out.u16(index as u16)?;
                    b.encode(out)?;
                }
            }
        } else {
            out.u8(1)?;
            encode_page(current, out)?;
        }
    }
    Ok(())
}

type ChunkPages = Vec<Arc<ChunkPage>>;
pub(crate) fn decode_chunk_pages(
    input: &mut Decoder<'_>,
    expected_layers: usize,
) -> EngineResult<(ChunkPages, Option<ChunkPages>)> {
    let has_base = bool::decode(input)?;
    let count = input.u32()? as usize;
    if count != expected_layers || count > input.remaining() / (CELL_COUNT * 8 + 1) {
        return Err(invalid("chunk layer数または切断"));
    }
    let mut layers = Vec::new();
    let mut baseline = Vec::new();
    layers.try_reserve_exact(count).map_err(allocation)?;
    if has_base {
        baseline.try_reserve_exact(count).map_err(allocation)?;
    }
    for _ in 0..count {
        let base = Arc::new(decode_page(input)?);
        if !has_base {
            layers.push(base);
            continue;
        }
        let current = match input.u8()? {
            0 => {
                let changes = input.u32()? as usize;
                if changes > CELL_COUNT || changes > input.remaining() / 10 {
                    return Err(invalid("生成base差分数"));
                }
                if changes == 0 {
                    Arc::clone(&base)
                } else {
                    let mut page = base.try_copy()?;
                    let mut previous = None;
                    for _ in 0..changes {
                        let index = input.u16()? as usize;
                        if index >= CELL_COUNT || previous.is_some_and(|old| old >= index) {
                            return Err(invalid("生成base差分の順序・範囲"));
                        }
                        previous = Some(index);
                        let value = CellCore::decode(input)?;
                        if value == base.cells[index] {
                            return Err(invalid("生成baseと同じ値の差分"));
                        }
                        page.cells[index] = value;
                    }
                    Arc::new(page)
                }
            }
            1 => {
                let page = decode_page(input)?;
                if page.cells == base.cells && page.blocks == base.blocks {
                    Arc::clone(&base)
                } else {
                    Arc::new(page)
                }
            }
            _ => return Err(invalid("生成base差分の符号化方式")),
        };
        baseline.push(base);
        layers.push(current);
    }
    Ok((layers, has_base.then_some(baseline)))
}

impl World {
    /// 現在の確定状態を保存するためのbyte上限を返します。
    ///
    /// 通常の保存と同じcodecを計数専用writerで通し、地形・全arena・作者状態・
    /// remap・archiveの管理情報を含めます。channelのみ登録されたcodec上限を使います。
    /// 保存bufferは確保しませんが、地形やrecordを走査する費用がかかります。
    /// 状態が変化した後は再計算してください。
    pub fn checkpoint_limit(&self) -> EngineResult<usize> {
        let mut out = Encoder::measuring();
        self.encode_checkpoint(&mut out, usize::MAX)?;
        Ok(out.written_len())
    }

    /// 全正規地形、allocator、lease、未完了要求を保存します。呼出側は確定tickで取得します。
    pub fn checkpoint(&self, byte_limit: usize) -> EngineResult<Vec<u8>> {
        self.validate()?;
        let mut out = Encoder::new(byte_limit);
        self.encode_checkpoint(&mut out, byte_limit)?;
        Ok(out.finish())
    }

    fn encode_checkpoint(&self, out: &mut Encoder, byte_limit: usize) -> EngineResult<()> {
        out.raw(b"CHWORLD2")?;
        self.id.encode(out)?;
        out.u64(self.seed)?;
        out.u64(self.tick)?;
        out.u64(self.revision)?;
        self.bounds.encode(out)?;
        out.raw(&self.content.fingerprint())?;
        encode_limits(self.limits, out)?;
        out.u32(self.chunks.len() as u32)?;
        for (coord, chunk) in &self.chunks {
            coord.encode(out)?;
            out.u64(chunk.revision)?;
            chunk.edited.encode(out)?;
            chunk.active.encode(out)?;
            chunk.generation.encode(out)?;
            out.u64(chunk.epoch)?;
            encode_chunk_pages(chunk, out)?;
        }
        if out.is_measuring() {
            out.measure_bytes(self.blocks.checkpoint_len()?)?;
        } else {
            out.bytes(&self.blocks.checkpoint(byte_limit)?)?;
        }
        if out.is_measuring() {
            out.measure_bytes(self.leases.checkpoint_len()?)?;
        } else {
            out.bytes(&self.leases.checkpoint(byte_limit)?)?;
        }
        if out.is_measuring() {
            out.measure_bytes(self.requests.checkpoint_len()?)?;
        } else {
            out.bytes(&self.requests.checkpoint(byte_limit)?)?;
        }
        self.remaps.encode(out)?;
        out.u32(self.archived.len() as u32)?;
        for (coord, archive) in &self.archived {
            coord.encode(out)?;
            out.raw(&archive.hash)?;
            archive.region.encode(out)?;
        }
        if out.is_measuring() {
            out.measure_bytes(self.jobs.checkpoint_len()?)?;
        } else {
            out.bytes(&self.jobs.checkpoint(byte_limit)?)?;
        }
        if out.is_measuring() {
            out.measure_bytes(self.state.checkpoint_len()?)?;
        } else {
            out.bytes(&self.state.checkpoint()?)?;
        }
        out.u32(self.channels.len() as u32)?;
        for channel in &self.channels {
            channel.key().encode(out)?;
            if out.is_measuring() {
                out.measure_bytes(channel.checkpoint_limit().min(u32::MAX as usize))?;
            } else {
                out.bytes(&channel.checkpoint()?)?;
            }
        }
        out.u32(self.limits.particles)?;
        out.u64(self.limits.particle_bytes as u64)?;
        if out.is_measuring() {
            out.measure_bytes(self.particles.checkpoint_len()?)?;
        } else {
            out.bytes(&self.particles.checkpoint(byte_limit)?)?;
        }
        Ok(())
    }
    /// 必須定義と各容量を確認し、完全な候補だけを返します。既存Worldの書換えは行いません。
    pub fn restore(
        bytes: &[u8],
        content: Arc<Content>,
        caps: WorldLimits,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, byte_limit)?;
        let format = input.raw(8)?;
        let with_particles = format == b"CHWORLD2";
        if !with_particles && format != b"CHWORLD1" {
            return Err(invalid("World保存形式"));
        }
        let id = WorldId::decode(&mut input)?;
        let seed = input.u64()?;
        let tick = input.u64()?;
        let revision = input.u64()?;
        let bounds = WorldBounds::decode(&mut input)?;
        let saved_content: [u8; 32] = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("保存content hash"))?;
        if !content.accepts_content(saved_content) {
            return Err(EngineError::new(
                ErrorCode::MissingDependency,
                "保存したContentの定義と不一致",
            ));
        }
        let mut limits = decode_limits(&mut input)?;
        // 旧版に粒子は存在しません。新しい容量は呼出側の上限から採用し、
        // 新版では末尾の明示容量とarenaのallocatorを合わせて復元します。
        limits.particles = caps.particles;
        limits.particle_bytes = caps.particle_bytes;
        validate_caps(limits, caps)?;
        let mut world = World::new(id, seed, bounds, content, limits)?;
        world.tick = tick;
        world.revision = revision;
        let count = input.u32()? as usize;
        if count > limits.resident_chunks as usize || count > input.remaining() / 12 {
            return Err(invalid("保存chunk数"));
        }
        let mut previous = None;
        for _ in 0..count {
            let coordinate = ChunkCoord::decode(&mut input)?;
            if previous.is_some_and(|old| old >= coordinate) {
                return Err(invalid("chunk辞書の順序または重複"));
            }
            previous = Some(coordinate);
            let revision = input.u64()?;
            let edited = bool::decode(&mut input)?;
            let active = bool::decode(&mut input)?;
            let generation = Option::<GenerationStamp>::decode(&mut input)?;
            let epoch = input.u64()?;
            let (layers, baseline) = decode_chunk_pages(&mut input, world.content.layers.len())?;
            world.chunks.insert(
                coordinate,
                Chunk {
                    layers,
                    baseline,
                    revision,
                    edited,
                    active,
                    generation,
                    epoch,
                },
            );
        }
        world.blocks = Arena::restore(input.bytes(byte_limit)?, limits.blocks, byte_limit)?;
        world.leases = Arena::restore(input.bytes(byte_limit)?, limits.leases, byte_limit)?;
        world.requests = Arena::restore(input.bytes(byte_limit)?, limits.requests, byte_limit)?;
        world.remaps = super::MaterialRemapTable::decode(&mut input)?;
        let count = input.u32()? as usize;
        if count > limits.archived_chunks as usize || count > input.remaining() / 64 {
            return Err(invalid("archive index数"));
        }
        let mut previous = None;
        for _ in 0..count {
            let coord = ChunkCoord::decode(&mut input)?;
            if previous.is_some_and(|old| old >= coord) {
                return Err(invalid("archive index順序"));
            }
            previous = Some(coord);
            let hash = input
                .raw(32)?
                .try_into()
                .map_err(|_| invalid("archive index hash"))?;
            let region = super::ChunkRegion::decode(&mut input)?;
            world
                .archived
                .insert(coord, super::ArchivedChunk { hash, region });
        }
        world.jobs = Arena::restore(input.bytes(byte_limit)?, limits.jobs, byte_limit)?;
        world.state = super::state::restore(
            input.bytes(byte_limit)?,
            &world.content,
            world.limits,
            false,
        )?;
        let count = input.u32()? as usize;
        if count != world.content.channels.len() {
            return Err(invalid("保存channel数"));
        }
        world.channels.clear();
        for factory in &world.content.channels {
            if StableKey::decode(&mut input)? != factory.key() {
                return Err(invalid("保存channel辞書"));
            }
            world
                .channels
                .push(factory.restore(input.bytes(byte_limit)?)?);
        }
        if with_particles {
            world.limits.particles = input.u32()?;
            world.limits.particle_bytes =
                usize::try_from(input.u64()?).map_err(|_| invalid("保存粒子byte容量"))?;
            validate_caps(world.limits, caps)?;
            world.particles =
                Arena::restore(input.bytes(byte_limit)?, world.limits.particles, byte_limit)?;
        }
        input.finish()?;
        world.connect_block_states()?;
        world.validate()?;
        world.validate_restored_jobs()?;
        Ok(world)
    }
    /// 完全な正規snapshotのBLAKE3。表示cacheやOS時刻は含みません。
    pub fn canonical_hash(&self, byte_limit: usize) -> EngineResult<[u8; 32]> {
        Ok(*blake3::hash(&self.checkpoint(byte_limit)?).as_bytes())
    }
}

fn encode_limits(value: WorldLimits, out: &mut Encoder) -> EngineResult<()> {
    out.u32(value.resident_chunks)?;
    out.u32(value.archived_chunks)?;
    out.u32(value.active_chunks)?;
    out.u64(value.terrain_bytes as u64)?;
    out.u64(value.channel_bytes as u64)?;
    out.u32(value.state_records)?;
    out.u64(value.state_bytes as u64)?;
    out.u32(value.block_state_records)?;
    out.u64(value.block_state_bytes as u64)?;
    out.u32(value.blocks)?;
    out.u32(value.leases)?;
    out.u32(value.requests)?;
    out.u32(value.jobs)?;
    out.u64(value.job_cells as u64)?;
    out.u64(value.job_bytes as u64)?;
    out.u32(value.remaps)?;
    out.u64(value.edit_cells as u64)
}
fn decode_limits(input: &mut Decoder<'_>) -> EngineResult<WorldLimits> {
    Ok(WorldLimits {
        resident_chunks: input.u32()?,
        archived_chunks: input.u32()?,
        active_chunks: input.u32()?,
        terrain_bytes: usize::try_from(input.u64()?).map_err(|_| invalid("保存地形byte数"))?,
        channel_bytes: usize::try_from(input.u64()?).map_err(|_| invalid("保存channel byte数"))?,
        state_records: input.u32()?,
        state_bytes: usize::try_from(input.u64()?).map_err(|_| invalid("保存state byte数"))?,
        block_state_records: input.u32()?,
        block_state_bytes: usize::try_from(input.u64()?)
            .map_err(|_| invalid("保存block state byte数"))?,
        blocks: input.u32()?,
        leases: input.u32()?,
        requests: input.u32()?,
        jobs: input.u32()?,
        job_cells: usize::try_from(input.u64()?).map_err(|_| invalid("保存Job cell数"))?,
        job_bytes: usize::try_from(input.u64()?).map_err(|_| invalid("保存Job byte数"))?,
        remaps: input.u32()?,
        edit_cells: usize::try_from(input.u64()?).map_err(|_| invalid("保存編集cell数"))?,
        particles: WorldLimits::default().particles,
        particle_bytes: WorldLimits::default().particle_bytes,
    })
}
fn validate_caps(value: WorldLimits, caps: WorldLimits) -> EngineResult<()> {
    for (name, value, cap) in [
        (
            "粒子数",
            u64::from(value.particles),
            u64::from(caps.particles),
        ),
        (
            "粒子byte数",
            value.particle_bytes as u64,
            caps.particle_bytes as u64,
        ),
        ("Job数", u64::from(value.jobs), u64::from(caps.jobs)),
        ("Job cell数", value.job_cells as u64, caps.job_cells as u64),
        ("Job byte数", value.job_bytes as u64, caps.job_bytes as u64),
        (
            "World state record数",
            u64::from(value.state_records),
            u64::from(caps.state_records),
        ),
        (
            "World state byte数",
            value.state_bytes as u64,
            caps.state_bytes as u64,
        ),
        (
            "block state record数",
            u64::from(value.block_state_records),
            u64::from(caps.block_state_records),
        ),
        (
            "block state byte数",
            value.block_state_bytes as u64,
            caps.block_state_bytes as u64,
        ),
        (
            "channel byte数",
            value.channel_bytes as u64,
            caps.channel_bytes as u64,
        ),
        (
            "archive chunk数",
            u64::from(value.archived_chunks),
            u64::from(caps.archived_chunks),
        ),
        (
            "remap epoch数",
            u64::from(value.remaps),
            u64::from(caps.remaps),
        ),
        (
            "resident chunk数",
            u64::from(value.resident_chunks),
            u64::from(caps.resident_chunks),
        ),
        (
            "active chunk数",
            value.active_chunks.into(),
            caps.active_chunks.into(),
        ),
        (
            "地形byte数",
            value.terrain_bytes as u64,
            caps.terrain_bytes as u64,
        ),
        ("block数", value.blocks.into(), caps.blocks.into()),
        ("lease数", value.leases.into(), caps.leases.into()),
        ("生成要求数", value.requests.into(), caps.requests.into()),
        (
            "編集cell数",
            value.edit_cells as u64,
            caps.edit_cells as u64,
        ),
    ] {
        if value > cap {
            return Err(EngineError::capacity(name, value, cap));
        }
    }
    Ok(())
}

fn code(value: ErrorCode) -> u8 {
    match value {
        ErrorCode::InvalidValue => 0,
        ErrorCode::CapacityExceeded => 1,
        ErrorCode::AllocationFailed => 2,
        ErrorCode::DuplicateKey => 3,
        ErrorCode::MissingKey => 4,
        ErrorCode::StaleId => 5,
        ErrorCode::WrongScope => 6,
        ErrorCode::SchemaMismatch => 7,
        ErrorCode::Conflict => 8,
        ErrorCode::InvalidEncoding => 9,
        ErrorCode::InvalidState => 10,
        ErrorCode::UnsupportedCapability => 11,
        ErrorCode::MissingDependency => 12,
        ErrorCode::DependencyCycle => 13,
        ErrorCode::Io => 14,
        ErrorCode::DeviceLost => 15,
    }
}
fn decode_code(value: u8) -> EngineResult<ErrorCode> {
    Ok(match value {
        0 => ErrorCode::InvalidValue,
        1 => ErrorCode::CapacityExceeded,
        2 => ErrorCode::AllocationFailed,
        3 => ErrorCode::DuplicateKey,
        4 => ErrorCode::MissingKey,
        5 => ErrorCode::StaleId,
        6 => ErrorCode::WrongScope,
        7 => ErrorCode::SchemaMismatch,
        8 => ErrorCode::Conflict,
        9 => ErrorCode::InvalidEncoding,
        10 => ErrorCode::InvalidState,
        11 => ErrorCode::UnsupportedCapability,
        12 => ErrorCode::MissingDependency,
        13 => ErrorCode::DependencyCycle,
        14 => ErrorCode::Io,
        15 => ErrorCode::DeviceLost,
        _ => return Err(invalid("生成失敗code")),
    })
}
