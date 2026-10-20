//! 疎なchunk、layerごとのcell配列、世代付きblock個体。
//!
//! 正規Worldへ直接書き込む参照は返しません。editはCOWの独立候補を作り、
//! 配置先全体と容量を検査してから公開します。失敗を作者が無視しても候補は公開しません。

use super::{
    ArchivedChunk, Content, GenerationStamp, LayerId, LeaseKind, MaterialHandle, MaterialId,
    MaterialRemapTable, RegionLease, RegionRequest, RequestKind, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::{Arena, BodyId, Id, WorldId},
    transaction::CandidateState,
};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) const CELL_COUNT: usize = 4096;
const MIN_CHUNK: i32 = i32::MIN / 64;
const MAX_CHUNK: i32 = i32::MAX / 64;

/// 64×64cellのchunk座標。cell座標と同じi32全域を許す型ではありません。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkCoord {
    /// 水平chunk座標。
    pub x: i32,
    /// 垂直chunk座標。
    pub y: i32,
}
impl ChunkCoord {
    /// 実在するi32 cell領域へ写せる範囲だけ受け取ります。
    pub fn new(x: i32, y: i32) -> EngineResult<Self> {
        let value = Self { x, y };
        value.validate()?;
        Ok(value)
    }
    /// cellからEuclidean除算で所属chunkを取得します。
    pub fn at(point: CellPoint) -> Self {
        Self {
            x: point.x.div_euclid(64),
            y: point.y.div_euclid(64),
        }
    }
    /// chunkの左上cell。負座標と端点も正確に表現します。
    pub fn origin(self) -> EngineResult<CellPoint> {
        self.validate()?;
        Ok(CellPoint {
            x: self.x * 64,
            y: self.y * 64,
        })
    }
    pub(crate) fn validate(self) -> EngineResult<()> {
        if !(MIN_CHUNK..=MAX_CHUNK).contains(&self.x) || !(MIN_CHUNK..=MAX_CHUNK).contains(&self.y)
        {
            return Err(invalid("chunk座標"));
        }
        Ok(())
    }
}
impl StateCodec for ChunkCoord {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.i32(self.x)?;
        out.i32(self.y)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(input.i32()?, input.i32()?)
    }
}

/// 半開矩形のWorld境界。maxはi32::MAX+1を保持できます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldBounds {
    min: CellPoint,
    max: [i64; 2],
}
impl WorldBounds {
    /// 空の矩形、逆転、cell座標の表現範囲外を拒否します。
    pub fn new(min: CellPoint, max_x: i64, max_y: i64) -> EngineResult<Self> {
        crate::foundation::units::CellRect::new(min, max_x, max_y)?;
        Ok(Self {
            min,
            max: [max_x, max_y],
        })
    }
    /// 必要なchunkを後から生成できる、i32 cell座標の全域です。
    pub fn expandable() -> Self {
        Self {
            min: CellPoint {
                x: i32::MIN,
                y: i32::MIN,
            },
            max: [i64::from(i32::MAX) + 1; 2],
        }
    }
    /// 含まれる最小cell。
    pub fn min(self) -> CellPoint {
        self.min
    }
    /// 含まれない最大端点。
    pub fn max(self) -> [i64; 2] {
        self.max
    }
    /// cellがWorld内かを返します。residentとは独立しています。
    pub fn contains(self, point: CellPoint) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && i64::from(point.x) < self.max[0]
            && i64::from(point.y) < self.max[1]
    }
    /// chunkとWorldに一つ以上の共通cellがあるか。
    pub fn intersects_chunk(self, chunk: ChunkCoord) -> bool {
        if chunk.validate().is_err() {
            return false;
        }
        let x = i64::from(chunk.x) * 64;
        let y = i64::from(chunk.y) * 64;
        x < self.max[0]
            && y < self.max[1]
            && x + 64 > i64::from(self.min.x)
            && y + 64 > i64::from(self.min.y)
    }
}
impl StateCodec for WorldBounds {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.min.encode(out)?;
        out.i64(self.max[0])?;
        out.i64(self.max[1])
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(CellPoint::decode(input)?, input.i64()?, input.i64()?)
    }
}

/// 8byteの正規cell本体。block参照や作業stampをこのbitへ押し込みません。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct CellCore {
    /// このWorldのContentに属する素材ID。
    pub material: MaterialId,
    /// bit0=moving、bit1=burning、bit2=wet。未定義bitは拒否します。
    pub flags: u16,
    /// 作者の外観variation。描画pixelや占有形状を変更しません。
    pub variation: u16,
}
impl CellCore {
    /// 全ての値がゼロの空cell。
    pub const AIR: Self = Self {
        material: MaterialId::AIR,
        flags: 0,
        variation: 0,
    };
    pub(crate) fn validate(self, content: &Content) -> EngineResult<()> {
        content.material(self.material)?;
        if self.flags & !7 != 0 || (self.material == MaterialId::AIR && self != Self::AIR) {
            return Err(invalid("cellの予約flagまたは空cellの付加状態"));
        }
        Ok(())
    }
}
impl StateCodec for CellCore {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.material.encode(out)?;
        out.u16(self.flags)?;
        out.u16(self.variation)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            material: MaterialId::decode(input)?,
            flags: input.u16()?,
            variation: input.u16()?,
        })
    }
}
/// block個体用の型識別子。
pub enum BlockKind {}
/// World内部の世代付き個体ID。World間の受渡しにはBlockRefを使います。
pub type BlockId = Id<BlockKind>;
/// 所属Worldも検査するblock参照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef {
    /// 所属World。
    pub world: WorldId,
    /// そのWorld内の個体。
    pub id: BlockId,
}
/// 90度単位の配置向き。画像の回転は別の表示設定です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// 素材で定義した向き。
    #[default]
    Original,
    /// 時計回り90度。
    Right,
    /// 180度。
    Down,
    /// 反時計回り90度。
    Left,
}
impl Orientation {
    fn offset(self, x: u16, y: u16, size: [u16; 2]) -> [i64; 2] {
        let [w, h] = size.map(i64::from);
        let x = i64::from(x);
        let y = i64::from(y);
        match self {
            Self::Original => [x, y],
            Self::Right => [h - 1 - y, x],
            Self::Down => [w - 1 - x, h - 1 - y],
            Self::Left => [y, w - 1 - x],
        }
    }
}
/// 一つの配置個体。残存maskは素材の元footprintにおけるrow-major indexです。
pub struct BlockInstance {
    pub(crate) material: MaterialId,
    pub(crate) layer: LayerId,
    pub(crate) anchor: CellPoint,
    pub(crate) orientation: Orientation,
    pub(crate) remaining: Vec<u32>,
    pub(crate) epoch: u64,
    pub(crate) dormant: Option<[u8; 32]>,
    pub(crate) properties: Option<crate::foundation::state::StateStore>,
    pub(crate) pending_properties: Vec<u8>,
}
impl BlockInstance {
    /// 素材ID。
    pub fn material(&self) -> MaterialId {
        self.material
    }
    /// 所属layer。
    pub fn layer(&self) -> LayerId {
        self.layer
    }
    /// 回転後のfootprint左上cell。
    pub fn anchor(&self) -> CellPoint {
        self.anchor
    }
    /// 配置向き。
    pub fn orientation(&self) -> Orientation {
        self.orientation
    }
    /// 残っているcell数。
    pub fn remaining_cells(&self) -> usize {
        self.remaining.len()
    }
    /// 現在の残存mask indexをblock定義順で返します。
    pub fn remaining_mask(&self) -> &[u32] {
        &self.remaining
    }
    /// 作者StateStoreが復元済みの場合、その不変参照を返します。
    pub fn properties(&self) -> Option<&crate::foundation::state::StateStore> {
        self.properties.as_ref()
    }
    /// 破砕後の一fragmentに属するmaskと、作者が変換した状態から新しい個体を作ります。
    ///
    /// 元個体のmaterial・layer・配置metadataを引き継ぎます。maskは元の残存maskの非空部分集合で、
    /// 昇順かつ重複なしでなければなりません。作者StateStoreはfragmentごとに渡し直すため、
    /// 呼出側が状態を複製・分割・集約する規則を選びます。状態を落とす場合も明示的に`None`を渡します。
    pub fn with_fragment_state(
        &self,
        remaining: Vec<u32>,
        properties: Option<crate::foundation::state::StateStore>,
    ) -> EngineResult<Self> {
        if remaining.is_empty()
            || remaining.windows(2).any(|pair| pair[0] >= pair[1])
            || remaining
                .iter()
                .any(|index| self.remaining.binary_search(index).is_err())
        {
            return Err(invalid("fragment block残存mask"));
        }
        let fragment = Self {
            material: self.material,
            layer: self.layer,
            anchor: self.anchor,
            orientation: self.orientation,
            remaining,
            epoch: self.epoch,
            dormant: None,
            properties,
            pending_properties: Vec::new(),
        };
        fragment.validate()?;
        Ok(fragment)
    }
    pub(crate) fn point(&self, index: u32, size: [u16; 2]) -> EngineResult<CellPoint> {
        let x = (index % u32::from(size[0])) as u16;
        let y = (index / u32::from(size[0])) as u16;
        let [dx, dy] = self.orientation.offset(x, y, size);
        Ok(CellPoint {
            x: i32::try_from(i64::from(self.anchor.x) + dx).map_err(|_| invalid("配置x端点"))?,
            y: i32::try_from(i64::from(self.anchor.y) + dy).map_err(|_| invalid("配置y端点"))?,
        })
    }
}
impl CandidateState for BlockInstance {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            material: self.material,
            layer: self.layer,
            anchor: self.anchor,
            orientation: self.orientation,
            remaining: copy_slice(&self.remaining)?,
            epoch: self.epoch,
            dormant: self.dormant,
            properties: self
                .properties
                .as_ref()
                .map(CandidateState::try_copy)
                .transpose()?,
            pending_properties: copy_slice(&self.pending_properties)?,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if self.remaining.is_empty() != self.dormant.is_some()
            || self.remaining.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid("block残存mask"));
        }
        Ok(())
    }
}

/// 読込済みcellのview。個体参照の保持権は含みません。
#[derive(Clone, Copy, Debug)]
pub struct CellView {
    /// 正規cell本体。
    pub core: CellCore,
    /// blockとして配置されたcellなら、その個体。
    pub block: Option<BlockRef>,
}
/// 空・未読込・境界外を明確に分けるquery結果。
#[derive(Clone, Copy, Debug)]
pub enum CellQuery {
    /// 空cellを含む読込済み領域。
    Loaded(CellView),
    /// World内だがまだ準備されていないchunk。
    Unavailable(ChunkCoord),
    /// Worldの半開境界外。
    WorldBoundary,
}

pub(crate) struct ChunkPage {
    pub(crate) cells: Vec<CellCore>,
    pub(crate) blocks: Option<Vec<Option<BlockId>>>,
}
impl ChunkPage {
    pub(crate) fn empty() -> EngineResult<Self> {
        Ok(Self {
            cells: filled(CellCore::AIR, CELL_COUNT)?,
            blocks: None,
        })
    }
    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            cells: copy_slice(&self.cells)?,
            blocks: self.blocks.as_ref().map(|v| copy_slice(v)).transpose()?,
        })
    }
    pub(crate) fn bytes(&self) -> usize {
        self.cells.capacity() * size_of::<CellCore>()
            + self
                .blocks
                .as_ref()
                .map_or(0, |v| v.capacity() * size_of::<Option<BlockId>>())
    }
}
pub(crate) struct Chunk {
    pub(crate) layers: Vec<Arc<ChunkPage>>,
    // 生成時の不変page。編集前はlayersと共有し、最初の編集で対象layerだけを分離します。
    // 保存はこのbaseと編集差分を区別し、生成器が更新されても既存地形を再生成しません。
    pub(crate) baseline: Option<Vec<Arc<ChunkPage>>>,
    pub(crate) revision: u64,
    pub(crate) edited: bool,
    pub(crate) active: bool,
    pub(crate) generation: Option<GenerationStamp>,
    pub(crate) epoch: u64,
}
impl Chunk {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            layers: copy_slice(&self.layers)?,
            baseline: self
                .baseline
                .as_ref()
                .map(|pages| copy_slice(pages))
                .transpose()?,
            revision: self.revision,
            edited: self.edited,
            active: self.active,
            generation: self.generation,
            epoch: self.epoch,
        })
    }
    fn bytes(&self) -> usize {
        self.layers.iter().map(|page| page.bytes()).sum::<usize>()
            + self.baseline.as_ref().map_or(0, |pages| {
                pages
                    .iter()
                    .zip(&self.layers)
                    .filter(|(base, current)| !Arc::ptr_eq(base, current))
                    .map(|(base, _)| base.bytes())
                    .sum::<usize>()
            })
    }
}

/// Worldの正規格納と通常編集の容量。表現限界以内で作者が変更できます。
#[derive(Clone, Copy, Debug)]
pub struct WorldLimits {
    /// 同時resident chunk数。
    pub resident_chunks: u32,
    /// ディスクへ退避したchunkのindex上限。payloadのRAMは保持しません。
    pub archived_chunks: u32,
    /// 同時active chunk数。
    pub active_chunks: u32,
    /// cell本体とblock sidecarの合計上限。候補の一時コピーは別途見積もります。
    pub terrain_bytes: usize,
    /// 独自channelの値配列の合計byte上限。
    pub channel_bytes: usize,
    /// World全体の作者record数。
    pub state_records: u32,
    /// World全体の作者payload上限。
    pub state_bytes: usize,
    /// block個体ごとの作者record数。
    pub block_state_records: u32,
    /// block個体ごとの作者payload上限。
    pub block_state_bytes: usize,
    /// block個体arenaの最大slot数。
    pub blocks: u32,
    /// 正規のregion lease数。
    pub leases: u32,
    /// 同時に記録する生成・読込要求数。
    pub requests: u32,
    /// 同時に保持する正規の大規模地形Job数。
    pub jobs: u32,
    /// 一つのJobの最大cell編集数。
    pub job_cells: usize,
    /// 入力と準備pageの合計byte上限。
    pub job_bytes: usize,
    /// 保存する素材変換epoch数。
    pub remaps: u32,
    /// 一回の通常取引で触るcell数。巨大操作には分割Jobを使用します。
    pub edit_cells: usize,
    /// 物質粒子の世代付きslot数。退避中もIDを予約します。
    pub particles: u32,
    /// resident物質粒子のchannel辞書・長さheader・値のcodec byte合計上限。
    pub particle_bytes: usize,
}
impl Default for WorldLimits {
    fn default() -> Self {
        Self {
            resident_chunks: 2048,
            archived_chunks: 1_048_576,
            active_chunks: 512,
            terrain_bytes: 256 * 1024 * 1024,
            channel_bytes: 64 * 1024 * 1024,
            state_records: 4096,
            state_bytes: 16 * 1024 * 1024,
            block_state_records: 32,
            block_state_bytes: 4096,
            blocks: 65536,
            leases: 4096,
            requests: 4096,
            jobs: 4096,
            job_cells: 1_048_576,
            job_bytes: 64 * 1024 * 1024,
            remaps: 4096,
            edit_cells: 65536,
            particles: 262144,
            particle_bytes: 16 * 1024 * 1024,
        }
    }
}

/// 疎な格納を持つ汎用の地形World。画面の大きさから境界や活動域を決めません。
pub struct World {
    pub(crate) id: WorldId,
    /// 保存しない、実Worldの派生cache世代。try_copyでは参照を共有し、
    /// 物質配置や活動領域が変化したcommitだけで別世代へ進めます。
    /// これにより、同WorldId・同revisionの別分岐Worldとcacheを混同しません。
    pub(crate) cache_generation: Arc<()>,
    pub(crate) seed: u64,
    pub(crate) tick: u64,
    pub(crate) revision: u64,
    pub(crate) content: Arc<Content>,
    pub(crate) bounds: WorldBounds,
    pub(crate) limits: WorldLimits,
    pub(crate) chunks: BTreeMap<ChunkCoord, Chunk>,
    pub(crate) archived: BTreeMap<ChunkCoord, ArchivedChunk>,
    pub(crate) blocks: Arena<BlockInstance, BlockKind>,
    pub(crate) leases: Arena<RegionLease, LeaseKind>,
    pub(crate) requests: Arena<RegionRequest, RequestKind>,
    pub(crate) remaps: MaterialRemapTable,
    pub(crate) channels: Vec<Box<dyn super::channels::ChannelState>>,
    pub(crate) state: crate::foundation::state::StateStore,
    pub(crate) jobs: Arena<super::TerrainJob, super::TerrainJobKind>,
    pub(crate) changes: super::WorldChanges,
    pub(crate) particles: Arena<super::MaterialParticle, super::MaterialParticleKind>,
    /// E07の正規backend状態。未使用Worldでは未確保のままにします。
    pub(crate) body_physics: Option<super::BodyPhysics>,
}
impl World {
    /// 定義を共有して空のWorldを作ります。論理世界全体のcellを確保しません。
    pub fn new(
        id: WorldId,
        seed: u64,
        bounds: WorldBounds,
        content: Arc<Content>,
        limits: WorldLimits,
    ) -> EngineResult<Self> {
        if limits.resident_chunks == 0
            || limits.active_chunks > limits.resident_chunks
            || limits.edit_cells == 0
            || limits.terrain_bytes == 0
        {
            return Err(invalid("Worldの容量設定"));
        }
        let mut channels = Vec::new();
        channels
            .try_reserve_exact(content.channels.len())
            .map_err(allocation)?;
        for factory in &content.channels {
            channels.push(factory.create()?);
        }
        let state = super::state::empty(&content, limits, false)?;
        Ok(Self {
            id,
            cache_generation: Arc::new(()),
            seed,
            tick: 0,
            revision: 0,
            bounds,
            content,
            limits,
            chunks: BTreeMap::new(),
            archived: BTreeMap::new(),
            blocks: Arena::new(limits.blocks)?,
            leases: Arena::new(limits.leases)?,
            requests: Arena::new(limits.requests)?,
            remaps: MaterialRemapTable { rules: Vec::new() },
            state,
            jobs: super::jobs::empty(limits.jobs)?,
            changes: super::WorldChanges::empty(),
            channels,
            particles: Arena::new(limits.particles)?,
            body_physics: None,
        })
    }
    /// 所属ID。描画側のScene IDとは別です。
    pub fn id(&self) -> WorldId {
        self.id
    }
    /// 世界の生成seed。
    pub fn seed(&self) -> u64 {
        self.seed
    }
    /// 最後に確定した正規tick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 編集・読込・活動変更の公開revision。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// 不変の素材・layer定義。
    pub fn content(&self) -> &Arc<Content> {
        &self.content
    }
    /// 境界とresidentは独立しています。
    pub fn bounds(&self) -> WorldBounds {
        self.bounds
    }
    /// 同時に読み込んでいるchunk数。
    pub fn resident_chunks(&self) -> usize {
        self.chunks.len()
    }
    /// 正規地形配列の使用量。COWによる旧snapshotの保持分は含みません。
    pub fn terrain_bytes(&self) -> usize {
        self.chunks.values().map(Chunk::bytes).sum()
    }
    /// 読込済みchunkの生成履歴。手動作成chunkはNone、未読込は明示エラーです。
    /// 保存した版のbaseを使うため、呼出側が新しい生成器を登録しても結果は変わりません。
    pub fn generation_stamp(&self, coord: ChunkCoord) -> EngineResult<Option<GenerationStamp>> {
        self.chunks
            .get(&coord)
            .map(|chunk| chunk.generation)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "生成履歴のchunkが未読込"))
    }
    /// 編集前の生成cell。手動作成chunkにはbaseがなくNoneを返します。
    /// 境界外・未読込はエラーとし、空cellやbase不在とは区別します。
    pub fn generation_base_cell(
        &self,
        layer: LayerId,
        point: CellPoint,
    ) -> EngineResult<Option<CellCore>> {
        self.content.layer(layer)?;
        if !self.bounds.contains(point) {
            return Err(invalid("生成base照会がWorld境界外"));
        }
        let (coord, index) = local(point);
        let chunk = self
            .chunks
            .get(&coord)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "生成baseが未読込"))?;
        Ok(chunk
            .baseline
            .as_ref()
            .map(|pages| pages[layer.0 as usize].cells[index]))
    }
    /// 現在生存するblock個体数。
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    /// 領域を空気へ丸めず問い合わせます。存在しないlayerはエラーです。
    pub fn cell(&self, layer: LayerId, point: CellPoint) -> EngineResult<CellQuery> {
        self.content.layer(layer)?;
        if !self.bounds.contains(point) {
            return Ok(CellQuery::WorldBoundary);
        }
        let (coord, index) = local(point);
        let Some(chunk) = self.chunks.get(&coord) else {
            return Ok(CellQuery::Unavailable(coord));
        };
        let page = &chunk.layers[layer.0 as usize];
        Ok(CellQuery::Loaded(CellView {
            core: page.cells[index],
            block: page
                .blocks
                .as_ref()
                .and_then(|v| v[index])
                .map(|id| BlockRef { world: self.id, id }),
        }))
    }

    pub(crate) fn is_cell_in_bounds(&self, point: CellPoint) -> bool {
        self.bounds.contains(point)
    }

    pub(crate) fn is_chunk_resident(&self, coord: ChunkCoord) -> bool {
        self.chunks.contains_key(&coord)
    }
    /// 生存・世代・World所属が一致する個体だけを返します。
    pub fn block(&self, reference: BlockRef) -> EngineResult<&BlockInstance> {
        if reference.world != self.id {
            return Err(EngineError::new(ErrorCode::WrongScope, "blockの所属World"));
        }
        let block = self.blocks.get(reference.id)?;
        if block.dormant.is_some() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "blockの所属regionは退避中",
            ));
        }
        Ok(block)
    }
    /// 一つの原子的編集。callback内で一度でも失敗した候補は公開しません。
    ///
    /// chunk pageは共有し、書込みがあるpageだけコピーします。個体arenaは独立候補を
    /// 作るため複製します。多数の操作は一つのeditへまとめるとこの費用を共有できます。
    pub fn edit<R>(
        &mut self,
        action: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> EngineResult<R> {
        let mut candidate = self.try_copy()?;
        let mut editor = WorldEdit {
            world: &mut candidate,
            error: None,
            writes: 0,
        };
        let result = action(&mut editor)?;
        if let Some(error) = editor.error {
            return Err(error);
        }
        candidate.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("World revisionの上限"))?;
        candidate.validate()?;
        candidate.changes = super::WorldChanges::between(self, &candidate)?;
        if !candidate.changes.chunks.is_empty() {
            candidate.cache_generation = Arc::new(());
        }
        if let (Some(source), Some(target)) =
            (self.body_physics.as_mut(), candidate.body_physics.as_mut())
        {
            target.inherit_step_scratch(source);
        }
        *self = candidate;
        Ok(result)
    }

    /// Worldが所有する正規の剛体状態を読み取ります。未使用WorldではNoneです。
    pub fn body_physics(&self) -> Option<&super::BodyPhysics> {
        self.body_physics.as_ref()
    }

    pub(crate) fn apply_body_material_remaps(&mut self) -> EngineResult<()> {
        if let Some(physics) = &mut self.body_physics {
            physics.apply_material_remaps(&self.remaps, &self.content, self.limits)?;
            physics.validate_content(&self.content, self.limits)?;
            physics.sync_material_mass_properties(&self.content)?;
        }
        Ok(())
    }

    /// 保存可能な標準剛体providerを初期化します。設定後のWorldは再設定できません。
    pub fn configure_body_physics(
        &mut self,
        settings: super::BodyPhysicsSettings,
    ) -> EngineResult<()> {
        if self.body_physics.is_some() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "BodyPhysicsは初期化済みです",
            ));
        }
        let candidate = super::BodyPhysics::new(settings)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("World revisionの上限"))?;
        self.body_physics = Some(candidate);
        self.revision = revision;
        Ok(())
    }

    /// 剛体・jointの変更を独立候補で検証し、Worldへ原子的に公開します。
    pub fn edit_bodies<R>(
        &mut self,
        action: impl FnOnce(&mut super::BodyPhysics) -> EngineResult<R>,
    ) -> EngineResult<R> {
        let mut candidate = match &self.body_physics {
            Some(physics) => physics.try_copy()?,
            None => super::BodyPhysics::new(Default::default())?,
        };
        let result = action(&mut candidate)?;
        candidate.apply_material_remaps(&self.remaps, &self.content, self.limits)?;
        candidate.validate_content(&self.content, self.limits)?;
        candidate.sync_material_mass_properties(&self.content)?;
        if let Some(source) = self.body_physics.as_mut() {
            candidate.inherit_step_scratch(source);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("World revisionの上限"))?;
        self.body_physics = Some(candidate);
        self.revision = revision;
        Ok(result)
    }

    /// 独立したCollisionQueries providerでread-onlyのWorld/physics候補をqueryします。
    ///
    /// queryはWorldを変更せず、未対応shapeと未準備regionを`Unsupported`/`Incomplete`として
    /// 呼出側へ返します。結果の有限値・coverage・hit上限を検証し、stable orderを要求した場合は
    /// 決定順へ整列します。provider identityと読取revisionをreportへ含めます。
    pub fn query_body_collisions(
        &self,
        request: &super::CollisionQueryRequest,
        provider: &dyn super::CollisionQueriesProvider,
    ) -> EngineResult<super::CollisionQueryReport> {
        super::validate_collision_query_request(request)?;
        self.content.layer(request.layer)?;
        let stamp = super::CollisionQueryProviderStamp {
            key: provider.key(),
            version: provider.version(),
        };
        if stamp.version == 0 {
            return Err(invalid("CollisionQueries provider version"));
        }
        let mut result = provider.query(self, self.body_physics.as_ref(), request)?;
        super::validate_collision_query(self, self.body_physics.as_ref(), request, &mut result)?;
        Ok(super::CollisionQueryReport {
            provider: stamp,
            world_revision: self.revision,
            body_step: self
                .body_physics
                .as_ref()
                .map_or(0, super::BodyPhysics::step_index),
            result,
        })
    }

    /// resident terrainを同期し、指定policyを満たすbody teleportを原子的に確定します。
    ///
    /// `Check`はRapier/Parryの回転を含むshape castで経路上のsolid colliderを調べます。
    /// `Ignore`は移動途中を調べず到着poseだけを検査します。到着重なりを許す場合は
    /// `overlap: Allow`を指定します。未読込chunkへの経路・到着、shape cast未対応、
    /// overlap拒否、容量・Content不整合のどれかで失敗した場合はWorldとphysicsを保ちます。
    /// 成功時も速度、蓄積force/torque、joint接続は保持します。移動前にactiveだったcontactは
    /// `BodyContactExitReason::Teleported`としてこの編集境界で通知し、次のfixed stepでは重複通知しません。
    /// 候補physicsの複製を要するため、
    /// この操作は明示的な低頻度移動向けであり、毎tickの通常移動経路には使いません。
    pub fn teleport_body(
        &mut self,
        body: BodyId,
        destination: super::BodyPose,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        options: super::BodyTeleportOptions,
    ) -> EngineResult<super::BodyTeleportReport> {
        self.teleport_body_inner(
            body,
            destination,
            collision_layer,
            world_units_per_cell,
            options,
            None,
        )
    }

    /// `CollisionQueries`を作者providerへ置換したbody teleportを原子的に確定します。
    ///
    /// providerのClear判定は指定されたpath/overlap optionsを満たす責務を持ちます。
    /// Engineはresident region、body pose、physics state、World commitを検証し、
    /// reportへprovider key/versionを残します。Blocked/Incomplete/Unsupportedは失敗し、
    /// Worldとphysics候補を公開しません。
    pub fn teleport_body_with_provider(
        &mut self,
        body: BodyId,
        destination: super::BodyPose,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        options: super::BodyTeleportOptions,
        provider: &dyn super::BodyTeleportQueryProvider,
    ) -> EngineResult<super::BodyTeleportReport> {
        self.teleport_body_inner(
            body,
            destination,
            collision_layer,
            world_units_per_cell,
            options,
            Some(provider),
        )
    }

    fn teleport_body_inner(
        &mut self,
        body: BodyId,
        destination: super::BodyPose,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        options: super::BodyTeleportOptions,
        query_provider: Option<&dyn super::BodyTeleportQueryProvider>,
    ) -> EngineResult<super::BodyTeleportReport> {
        let source = self
            .body_physics
            .as_ref()
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?;
        let mut candidate = source.try_copy()?;
        candidate.sync_solid_terrain_for_step(self, collision_layer, world_units_per_cell)?;
        let report = candidate.teleport_in_world(
            source,
            body,
            destination,
            self,
            world_units_per_cell,
            options,
            query_provider,
        )?;
        candidate.apply_material_remaps(&self.remaps, &self.content, self.limits)?;
        candidate.validate_content(&self.content, self.limits)?;
        candidate.sync_material_mass_properties(&self.content)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("World revisionの上限"))?;
        candidate.acknowledge_world_revision(revision);
        self.body_physics = Some(candidate);
        self.revision = revision;
        Ok(report)
    }

    /// Worldを変更せず、resident solid colliderと所有剛体fixed stepを原子的に更新します。
    ///
    /// fluid接触がないstepではWorld chunk/channel/block arenaを複製しません。fluidとの
    /// 重なりまたは境界接触がある場合だけ、world/physicsを同時rollbackできるWorld候補を作成します。
    /// stepが失敗した場合は独立したBodyPhysics候補を破棄します。terrain変更と同じcommitに
    /// する場合は`edit_and_step_bodies`を使います。固定step内では物体の予測移動と実測移動が
    /// 選択済みの移動profileを守るsubstep数を選び、`BodyPhysicsSettings::max_substeps`を超える場合は拒否します。
    pub fn step_bodies(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
    ) -> EngineResult<super::BodyStepReport> {
        let mut candidate = match self.body_physics.as_ref() {
            Some(physics) => physics.try_copy()?,
            None => super::BodyPhysics::new(Default::default())?,
        };
        candidate.sync_solid_terrain_for_step(self, collision_layer, world_units_per_cell)?;
        if let Some(source) = self.body_physics.as_mut() {
            candidate.inherit_step_scratch(source);
        }
        let report = match self.body_physics.as_ref() {
            Some(physics) => {
                candidate.step_isolated_in_world(physics, self, world_units_per_cell)?
            }
            None => candidate.step_in_world(self, world_units_per_cell)?,
        };
        if candidate.has_fluid_contact(self, collision_layer)?
            || candidate.has_releasable_exclusion_reservoir(self, collision_layer)?
        {
            let mut world_candidate = self.try_copy_with_body_physics(Some(candidate))?;
            let mut fluid_editor = WorldEdit {
                world: &mut world_candidate,
                error: None,
                writes: 0,
            };
            fluid_editor.apply_body_fluid_coupling(collision_layer, world_units_per_cell)?;
            fluid_editor.displace_body_fluid_overlaps(
                collision_layer,
                world_units_per_cell,
                None,
            )?;
            if let Some(error) = fluid_editor.error.clone() {
                return Err(error);
            }
            let fluid_writes = fluid_editor.writes;
            drop(fluid_editor);
            if fluid_writes != 0 {
                world_candidate.revision = self
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revisionの上限"))?;
                world_candidate.validate()?;
                world_candidate.changes = super::WorldChanges::between(self, &world_candidate)?;
                if !world_candidate.changes.chunks.is_empty() {
                    world_candidate.cache_generation = Arc::new(());
                }
                if let Some(physics) = world_candidate.body_physics.as_mut() {
                    physics.acknowledge_world_revision(world_candidate.revision);
                }
            }
            *self = world_candidate;
        } else {
            self.body_physics = Some(candidate);
        }
        Ok(report)
    }

    /// 作者BodyPhysics solverで固定stepし、World・physics・fluid連成を同時にcommitします。
    ///
    /// providerは候補pose/velocityとBodyPhysics snapshotに保存する作者stateを更新します。
    /// Engineは候補terrain colliderを同期し、数値・resident sweep・fluid displacement・World容量を検査します。
    /// fluid変更がない場合はWorld全体を複製せずphysics候補だけをcommitします。callbackが失敗しても
    /// WorldとBodyPhysicsを保持します。標準Rapier経路は呼び出しません。
    pub fn step_bodies_with_provider(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        provider: &dyn super::BodyPhysicsStepProvider,
    ) -> EngineResult<super::BodyStepReport> {
        if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
            return Err(invalid("作者剛体solverのWorld cell size"));
        }
        let default_before = if self.body_physics.is_none() {
            Some(super::BodyPhysics::new(Default::default())?)
        } else {
            None
        };
        let mut physics = match self.body_physics.as_ref() {
            Some(source) => source.try_copy()?,
            None => super::BodyPhysics::new(Default::default())?,
        };
        if let Some(source) = self.body_physics.as_mut() {
            physics.inherit_step_scratch(source);
        }
        let before = self
            .body_physics
            .as_ref()
            .or(default_before.as_ref())
            .ok_or_else(|| invalid("作者剛体solverのstep前state"))?;
        physics.sync_solid_terrain(self, collision_layer, world_units_per_cell)?;
        let report = physics.step_with_provider(before, self, world_units_per_cell, provider)?;
        physics.validate_content(&self.content, self.limits)?;
        physics.sync_material_mass_properties(&self.content)?;
        let has_fluid_work = physics.has_fluid_contact(self, collision_layer)?
            || physics.has_releasable_exclusion_reservoir(self, collision_layer)?;
        if has_fluid_work {
            let mut candidate = self.try_copy_with_body_physics(Some(physics))?;
            let mut editor = WorldEdit {
                world: &mut candidate,
                error: None,
                writes: 0,
            };
            editor.apply_body_fluid_coupling(collision_layer, world_units_per_cell)?;
            editor.displace_body_fluid_overlaps(collision_layer, world_units_per_cell, None)?;
            if let Some(error) = editor.error.clone() {
                return Err(error);
            }
            let writes = editor.writes;
            drop(editor);
            if writes != 0 {
                candidate.revision = self
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revisionの上限"))?;
            }
            candidate.validate()?;
            if writes != 0 {
                candidate.changes = super::WorldChanges::between(self, &candidate)?;
                if !candidate.changes.chunks.is_empty() {
                    candidate.cache_generation = Arc::new(());
                }
            }
            if let Some(physics) = candidate.body_physics.as_mut() {
                physics.acknowledge_world_revision(candidate.revision);
            }
            *self = candidate;
        } else {
            physics.acknowledge_world_revision(self.revision);
            self.body_physics = Some(physics);
        }
        Ok(report)
    }

    /// 標準Rapier fixed stepの後、指定CouplingProviderで流体接触を更新します。
    ///
    /// callbackへ渡すWorld viewはstep開始時の読取専用snapshotです。現在のbody pose/velocityは
    /// `BodyCouplingContext`から渡し、fluid/material照会はこのsnapshotへ行います。物理step、
    /// provider state、流体更新、重なり排除は一つの候補でcommitし、provider選択後は標準経路
    /// から連成則を暗黙に切り替えられません。
    pub fn step_bodies_with_coupling_provider(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        provider: &dyn super::CouplingProvider,
    ) -> EngineResult<super::BodyStepReport> {
        if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
            return Err(invalid("CouplingProviderのWorld cell size"));
        }
        let mut candidate = match self.body_physics.as_ref() {
            Some(physics) => physics.try_copy()?,
            None => super::BodyPhysics::new(Default::default())?,
        };
        if let Some(source) = self.body_physics.as_mut() {
            candidate.inherit_step_scratch(source);
        }
        let selected = candidate.prepare_coupling_step();
        candidate.sync_solid_terrain_for_step(self, collision_layer, world_units_per_cell)?;
        let before = self.body_physics.as_ref();
        let mut report = match before {
            Some(physics) => {
                candidate.step_isolated_in_world(physics, self, world_units_per_cell)?
            }
            None => candidate.step_in_world(self, world_units_per_cell)?,
        };
        let stamp = super::BodyCouplingProviderStamp {
            key: provider.key(),
            version: provider.version(),
        };
        candidate.select_coupling_provider(stamp)?;
        if let Some(previous) = selected
            && previous != stamp
        {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "保存済みCouplingProviderとkey/versionが一致しません",
            ));
        }
        report.coupling_provider = Some(stamp);

        let custom_fluid_contacts = provider.fluid_contacts(
            &candidate,
            self,
            collision_layer,
            world_units_per_cell,
            self.limits.job_cells,
        )?;
        if let Some(contacts) = &custom_fluid_contacts
            && contacts.len() > self.limits.job_cells
        {
            return Err(EngineError::capacity(
                "CouplingProvider fluid contacts",
                contacts.len() as u64,
                self.limits.job_cells as u64,
            ));
        }
        let custom_fluid_occupancy = provider.fluid_occupancy(
            &candidate,
            self,
            collision_layer,
            world_units_per_cell,
            self.limits.job_cells,
        )?;
        if let Some(occupancy) = &custom_fluid_occupancy
            && occupancy.len() > self.limits.job_cells
        {
            return Err(EngineError::capacity(
                "CouplingProvider fluid occupancy",
                occupancy.len() as u64,
                self.limits.job_cells as u64,
            ));
        }
        let prepared_custom_occupancy = custom_fluid_occupancy
            .map(|occupancy| {
                super::prepare_body_fluid_occupancy(&candidate, self, collision_layer, occupancy)
            })
            .transpose()?;
        let has_fluid_boundary_work = match &custom_fluid_contacts {
            Some(contacts) => !contacts.is_empty(),
            None => candidate.has_fluid_contact(self, collision_layer)?,
        };
        let has_fluid_work = has_fluid_boundary_work
            || prepared_custom_occupancy
                .as_ref()
                .is_some_and(|prepared| !prepared.overlaps.is_empty())
            || candidate.has_releasable_exclusion_reservoir(self, collision_layer)?;
        if has_fluid_work {
            let mut world_candidate = self.try_copy_with_body_physics(Some(candidate))?;
            let mut editor = WorldEdit {
                world: &mut world_candidate,
                error: None,
                writes: 0,
            };
            editor.apply_body_fluid_coupling_with_provider(
                collision_layer,
                world_units_per_cell,
                provider,
                self,
                custom_fluid_contacts,
            )?;
            editor.displace_body_fluid_overlaps(
                collision_layer,
                world_units_per_cell,
                prepared_custom_occupancy,
            )?;
            if let Some(error) = editor.error.clone() {
                return Err(error);
            }
            let writes = editor.writes;
            drop(editor);
            if writes != 0 {
                world_candidate.revision = self
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revisionの上限"))?;
                world_candidate.changes = super::WorldChanges::between(self, &world_candidate)?;
                if !world_candidate.changes.chunks.is_empty() {
                    world_candidate.cache_generation = Arc::new(());
                }
            }
            world_candidate.validate()?;
            if let Some(physics) = world_candidate.body_physics.as_mut() {
                physics.acknowledge_world_revision(world_candidate.revision);
            }
            *self = world_candidate;
        } else {
            candidate.acknowledge_world_revision(self.revision);
            self.body_physics = Some(candidate);
        }
        Ok(report)
    }

    /// World edit、resident solid collider更新、剛体fixed stepを一つのcommitにします。
    ///
    /// World候補またはBodyPhysics候補のどの段階で失敗しても、呼出元のWorldと物理状態を
    /// ともに保持します。描画周期や外部clockは参照せず、物理stepはBodyPhysicsの固定dtです。
    /// 固定stepは`BodyPhysicsSettings::max_substep_displacement_cells`の移動予算からsubstep数を選び、callbackがWorldを書き換えないstepでは
    /// World revisionを進めず、物理状態だけを更新します。
    pub fn edit_and_step_bodies<R>(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        action: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> EngineResult<(R, super::BodyStepReport)> {
        self.edit_and_step_bodies_with_post_step(
            collision_layer,
            world_units_per_cell,
            action,
            |_| Ok(()),
        )
        .map(|(result, (), report)| (result, report))
    }

    /// World edit、resident terrain・fluid連成、剛体fixed step、step後処理を一つの候補で確定します。
    ///
    /// `action`はfixed step前、`post_step`は物理・流体連成後に実行されます。後処理が反応や
    /// 接触結果を読む必要がある場合に使い、いずれかの段階で失敗した候補は公開しません。
    pub fn edit_and_step_bodies_with_post_step<R, P>(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        action: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<R>,
        post_step: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<P>,
    ) -> EngineResult<(R, P, super::BodyStepReport)> {
        let mut candidate = self.try_copy()?;
        let mut editor = WorldEdit {
            world: &mut candidate,
            error: None,
            writes: 0,
        };
        let result = action(&mut editor)?;
        if let Some(error) = editor.error {
            return Err(error);
        }
        let writes = editor.writes;
        drop(editor);
        if writes != 0 {
            candidate.revision = self
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("World revisionの上限"))?;
            candidate.validate()?;
            candidate.changes = super::WorldChanges::between(self, &candidate)?;
            if !candidate.changes.chunks.is_empty() {
                candidate.cache_generation = Arc::new(());
            }
        }

        let mut candidate_physics = match candidate.body_physics.take() {
            Some(physics) => physics,
            None => super::BodyPhysics::new(Default::default())?,
        };
        candidate_physics.sync_solid_terrain_for_step(
            &candidate,
            collision_layer,
            world_units_per_cell,
        )?;
        if let Some(source) = self.body_physics.as_mut() {
            candidate_physics.inherit_step_scratch(source);
        }
        let report = match self.body_physics.as_ref() {
            Some(physics) => candidate_physics.step_isolated_in_world(
                physics,
                &candidate,
                world_units_per_cell,
            )?,
            None => candidate_physics.step_in_world(&candidate, world_units_per_cell)?,
        };
        candidate.body_physics = Some(candidate_physics);
        let has_fluid_contact = candidate
            .body_physics
            .as_ref()
            .ok_or_else(|| invalid("BodyPhysics candidate is missing"))?
            .has_fluid_contact(&candidate, collision_layer)?;
        let mut fluid_editor = WorldEdit {
            world: &mut candidate,
            error: None,
            writes,
        };
        if has_fluid_contact {
            fluid_editor.apply_body_fluid_coupling(collision_layer, world_units_per_cell)?;
        }
        fluid_editor.displace_body_fluid_overlaps(collision_layer, world_units_per_cell, None)?;
        if let Some(error) = fluid_editor.error.clone() {
            return Err(error);
        }
        let fluid_writes = fluid_editor.writes.saturating_sub(writes);
        drop(fluid_editor);

        let mut post_editor = WorldEdit {
            world: &mut candidate,
            error: None,
            writes: writes.saturating_add(fluid_writes),
        };
        let post_result = post_step(&mut post_editor)?;
        if let Some(error) = post_editor.error {
            return Err(error);
        }
        let total_writes = post_editor.writes;
        drop(post_editor);

        if total_writes != 0 {
            if writes == 0 {
                candidate.revision = self
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("World revisionの上限"))?;
            }
            candidate.validate()?;
            candidate.changes = super::WorldChanges::between(self, &candidate)?;
            if !candidate.changes.chunks.is_empty() {
                candidate.cache_generation = Arc::new(());
            }
            if let Some(physics) = candidate.body_physics.as_mut() {
                physics.acknowledge_world_revision(candidate.revision);
            }
        }
        *self = candidate;
        Ok((result, post_result, report))
    }
}
impl World {
    /// callerが物理候補を既に作成した場合、旧BodyPhysicsを複製せずWorld候補へ差し込みます。
    fn try_copy_with_body_physics(
        &self,
        body_physics: Option<super::BodyPhysics>,
    ) -> EngineResult<Self> {
        let mut chunks = BTreeMap::new();
        for (key, value) in &self.chunks {
            chunks.insert(*key, value.try_copy()?);
        }
        let mut channels = Vec::new();
        channels
            .try_reserve_exact(self.channels.len())
            .map_err(allocation)?;
        for channel in &self.channels {
            channels.push(channel.try_copy()?);
        }
        Ok(Self {
            id: self.id,
            cache_generation: Arc::clone(&self.cache_generation),
            seed: self.seed,
            tick: self.tick,
            revision: self.revision,
            content: Arc::clone(&self.content),
            bounds: self.bounds,
            limits: self.limits,
            chunks,
            archived: self.archived.clone(),
            blocks: self.blocks.try_copy()?,
            leases: self.leases.try_copy()?,
            requests: self.requests.try_copy()?,
            remaps: self.remaps.try_copy()?,
            state: self.state.try_copy()?,
            jobs: self.jobs.try_copy()?,
            changes: self.changes.try_copy()?,
            particles: self.particles.try_copy()?,
            channels,
            body_physics,
        })
    }
}
impl CandidateState for World {
    fn try_copy(&self) -> EngineResult<Self> {
        let body_physics = self
            .body_physics
            .as_ref()
            .map(super::BodyPhysics::try_copy)
            .transpose()?;
        self.try_copy_with_body_physics(body_physics)
    }
    fn validate(&self) -> EngineResult<()> {
        self.validate_particles()?;
        self.validate_jobs()?;
        self.validate_state()?;
        self.validate_channels()?;
        self.validate_archives()?;
        self.validate_remaps()?;
        self.validate_generation()?;
        self.validate_activity()?;
        if self
            .chunks
            .values()
            .any(|chunk| chunk.epoch != self.remaps.epoch())
            || self
                .blocks
                .iter()
                .any(|(_, block)| block.dormant.is_none() && block.epoch != self.remaps.epoch())
        {
            return Err(invalid("residentに未適用のmaterial remapが存在"));
        }
        if self.chunks.len() > self.limits.resident_chunks as usize {
            return Err(EngineError::capacity(
                "resident chunk数",
                self.chunks.len() as u64,
                self.limits.resident_chunks.into(),
            ));
        }
        let bytes = self.terrain_bytes();
        if bytes > self.limits.terrain_bytes {
            return Err(EngineError::capacity(
                "地形byte数",
                bytes as u64,
                self.limits.terrain_bytes as u64,
            ));
        }
        let active = self.chunks.values().filter(|v| v.active).count();
        if active > self.limits.active_chunks as usize {
            return Err(EngineError::capacity(
                "active chunk数",
                active as u64,
                self.limits.active_chunks.into(),
            ));
        }
        let mut references = 0usize;
        for (coord, chunk) in &self.chunks {
            if chunk.generation.is_some() != chunk.baseline.is_some() {
                return Err(invalid("生成履歴と不変baseの対応"));
            }
            if let Some(base) = &chunk.baseline {
                if base.len() != self.content.layers.len() {
                    return Err(invalid("生成baseのlayer数"));
                }
                let origin = coord.origin()?;
                for page in base {
                    if page.cells.len() != CELL_COUNT || page.blocks.is_some() {
                        return Err(invalid("生成baseの寸法・個体参照"));
                    }
                    for (index, cell) in page.cells.iter().enumerate() {
                        cell.validate(&self.content)?;
                        let point = CellPoint {
                            x: origin.x + (index % 64) as i32,
                            y: origin.y + (index / 64) as i32,
                        };
                        if !self.bounds.contains(point) && *cell != CellCore::AIR {
                            return Err(invalid("生成baseがWorld境界外"));
                        }
                    }
                }
            }
            if !self.bounds.intersects_chunk(*coord)
                || chunk.layers.len() != self.content.layers.len()
            {
                return Err(invalid("chunkのWorld所属またはlayer数"));
            }
            let origin = coord.origin()?;
            for (layer, page) in chunk.layers.iter().enumerate() {
                if page.cells.len() != CELL_COUNT
                    || page.blocks.as_ref().is_some_and(|v| v.len() != CELL_COUNT)
                {
                    return Err(invalid("chunk pageの寸法"));
                }
                for (index, cell) in page.cells.iter().enumerate() {
                    cell.validate(&self.content)?;
                    let point = CellPoint {
                        x: origin.x + (index % 64) as i32,
                        y: origin.y + (index / 64) as i32,
                    };
                    if !self.bounds.contains(point) && *cell != CellCore::AIR {
                        return Err(invalid("World境界外のcell"));
                    }
                    if let Some(id) = page.blocks.as_ref().and_then(|v| v[index]) {
                        references += 1;
                        let block = self.blocks.get(id)?;
                        if block.material != cell.material || block.layer.0 as usize != layer {
                            return Err(invalid("block sidecarの対応"));
                        }
                    }
                }
            }
        }
        let mut expected = 0usize;
        for (id, block) in self.blocks.iter() {
            block.validate()?;
            if block.dormant.is_some() {
                continue;
            }
            let def = self
                .content
                .material(block.material)?
                .ok_or_else(|| invalid("空素材のblock"))?;
            let size = def.block_size();
            for index in &block.remaining {
                if *index >= u32::from(size[0]) * u32::from(size[1])
                    || !def.shape().occupied(
                        (*index % u32::from(size[0])) as u16,
                        (*index / u32::from(size[0])) as u16,
                        size,
                    )
                {
                    return Err(invalid("block maskの範囲"));
                }
                match self.cell(block.layer, block.point(*index, size)?)? {
                    CellQuery::Loaded(value)
                        if value.block == Some(BlockRef { world: self.id, id }) => {}
                    _ => return Err(invalid("blockの残存cell参照")),
                }
                expected += 1;
            }
        }
        if references != expected {
            return Err(invalid("重複したblock参照"));
        }
        Ok(())
    }
}

/// 独立候補の編集窓口。失敗は記録され、callbackで無視してもcommitを拒否します。
pub struct WorldEdit<'a> {
    pub(super) world: &'a mut World,
    error: Option<EngineError>,
    writes: usize,
}
impl WorldEdit<'_> {
    /// 候補Worldのcellを読みます。作者physics solverはこのviewからterrain inputを参照できます。
    pub fn cell(&self, layer: LayerId, point: CellPoint) -> EngineResult<CellQuery> {
        self.world.cell(layer, point)
    }

    pub(super) fn run<R>(
        &mut self,
        action: impl FnOnce(&mut Self) -> EngineResult<R>,
    ) -> EngineResult<R> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let result = action(self);
        if let Err(error) = &result {
            self.error = Some(error.clone());
        }
        result
    }
    pub(super) fn reserve_writes(&mut self, count: usize) -> EngineResult<()> {
        let next = self
            .writes
            .checked_add(count)
            .ok_or_else(|| invalid("編集cell数overflow"))?;
        if next > self.world.limits.edit_cells {
            return Err(EngineError::capacity(
                "通常取引のcell数",
                next as u64,
                self.world.limits.edit_cells as u64,
            ));
        }
        self.writes = next;
        Ok(())
    }
    /// 明示的に空のchunkを確定します。生成器の完了候補と同じく全layerを持ちます。
    /// 既存chunkを消して空にする操作ではありません。
    pub fn insert_empty_chunk(&mut self, coord: ChunkCoord) -> EngineResult<()> {
        self.run(|editor| {
            coord.validate()?;
            if !editor.world.bounds.intersects_chunk(coord) {
                return Err(invalid("World境界外のchunk"));
            }
            if editor.world.chunks.contains_key(&coord)
                || editor.world.archived.contains_key(&coord)
            {
                return Err(conflict("読込済みchunkの重複"));
            }
            if editor.world.chunks.len() >= editor.world.limits.resident_chunks as usize {
                return Err(EngineError::capacity(
                    "resident chunk数",
                    editor.world.chunks.len() as u64 + 1,
                    editor.world.limits.resident_chunks.into(),
                ));
            }
            let required = editor
                .world
                .terrain_bytes()
                .checked_add(editor.world.content.layers.len() * CELL_COUNT * size_of::<CellCore>())
                .ok_or_else(|| invalid("地形byte数overflow"))?;
            if required > editor.world.limits.terrain_bytes {
                return Err(EngineError::capacity(
                    "地形byte数",
                    required as u64,
                    editor.world.limits.terrain_bytes as u64,
                ));
            }
            let mut layers = Vec::new();
            layers
                .try_reserve_exact(editor.world.content.layers.len())
                .map_err(allocation)?;
            for _ in &editor.world.content.layers {
                layers.push(Arc::new(ChunkPage::empty()?));
            }
            editor.world.chunks.insert(
                coord,
                Chunk {
                    layers,
                    baseline: None,
                    revision: 0,
                    edited: false,
                    active: false,
                    generation: None,
                    epoch: editor.world.remaps.epoch(),
                },
            );
            Ok(())
        })
    }
    pub(super) fn loaded(&self, layer: LayerId, point: CellPoint) -> EngineResult<CellView> {
        match self.world.cell(layer, point)? {
            CellQuery::Loaded(value) => Ok(value),
            CellQuery::Unavailable(_) => Err(EngineError::new(
                ErrorCode::InvalidState,
                "編集対象chunkが未読込",
            )),
            CellQuery::WorldBoundary => Err(invalid("World境界外の編集")),
        }
    }
    pub(super) fn write(
        &mut self,
        layer: LayerId,
        point: CellPoint,
        core: CellCore,
        block: Option<BlockId>,
    ) -> EngineResult<()> {
        self.loaded(layer, point)?;
        core.validate(&self.world.content)?;
        let (coord, index) = local(point);
        // 生成baseと共有するpageは編集時に両方を保持します。コピー前に恒常保持量を予約し、
        // sidecarだけを計算してbaseの増分を見落とさないようにします。
        let chunk = &self.world.chunks[&coord];
        let page = &chunk.layers[layer.0 as usize];
        let base_extra = chunk
            .baseline
            .as_ref()
            .filter(|base| Arc::ptr_eq(&base[layer.0 as usize], page))
            .map_or(0, |_| page.bytes());
        let sidecar_extra = if block.is_some() && page.blocks.is_none() {
            CELL_COUNT * size_of::<Option<BlockId>>()
        } else {
            0
        };
        let required = self
            .world
            .terrain_bytes()
            .checked_add(base_extra)
            .and_then(|bytes| bytes.checked_add(sidecar_extra))
            .ok_or_else(|| invalid("編集pageのbyte数overflow"))?;
        if required > self.world.limits.terrain_bytes {
            return Err(EngineError::capacity(
                "地形byte数",
                required as u64,
                self.world.limits.terrain_bytes as u64,
            ));
        }
        // sidecarを確保してから容量を調べると、一時的に予算を大幅に超えられます。
        // 必要な追加量を先に計算し、候補にも上限を適用します。
        if block.is_some()
            && self.world.chunks[&coord].layers[layer.0 as usize]
                .blocks
                .is_none()
        {
            let required = self
                .world
                .terrain_bytes()
                .checked_add(CELL_COUNT * size_of::<Option<BlockId>>())
                .ok_or_else(|| invalid("block sidecar byte数overflow"))?;
            if required > self.world.limits.terrain_bytes {
                return Err(EngineError::capacity(
                    "地形byte数",
                    required as u64,
                    self.world.limits.terrain_bytes as u64,
                ));
            }
        }
        let chunk = self
            .world
            .chunks
            .get_mut(&coord)
            .expect("loadedで存在を確認");
        let page = &mut chunk.layers[layer.0 as usize];
        if Arc::strong_count(page) > 1 {
            *page = Arc::new(page.try_copy()?);
        }
        let page = Arc::get_mut(page).expect("候補専用page");
        if block.is_some() && page.blocks.is_none() {
            page.blocks = Some(filled(None, CELL_COUNT)?);
        }
        page.cells[index] = core;
        if let Some(blocks) = &mut page.blocks {
            blocks[index] = block;
        }
        chunk.revision = chunk
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("chunk revision上限"))?;
        chunk.edited = true;
        Ok(())
    }
    /// 物質1cellを置きます。block個体を暗黙に分解したり上書きしたりしません。
    pub fn set_cell(
        &mut self,
        layer: LayerId,
        point: CellPoint,
        core: CellCore,
    ) -> EngineResult<()> {
        self.run(|editor| {
            editor.reserve_writes(1)?;
            if editor.loaded(layer, point)?.block.is_some() {
                return Err(conflict("block個体のあるcellへの直接書込"));
            }
            editor.write(layer, point, core, None)
        })
    }
    /// World編集と同じ候補内で剛体・jointを編集します。
    ///
    /// bodyのlocal gridと地形・channelを一つのWorldEditで移動する作者処理に使います。
    /// callback内のエラーは`?`で返してください。World候補と物理候補のどちらかが失敗した
    /// 場合、外側の`edit_and_step_bodies`は両方を公開しません。Contentに対するlocal cellと
    /// sidecarの検証、および変更されたgridの質量特性再計算はこの境界で行います。
    pub fn edit_bodies<R>(
        &mut self,
        action: impl FnOnce(&mut super::BodyPhysics) -> EngineResult<R>,
    ) -> EngineResult<R> {
        self.run(|editor| {
            if editor.world.body_physics.is_none() {
                editor.world.body_physics = Some(super::BodyPhysics::new(Default::default())?);
            }
            let physics = editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| invalid("BodyPhysics候補の初期化失敗"))?;
            let result = action(physics)?;
            editor.world.apply_body_material_remaps()?;
            Ok(result)
        })
    }
    /// footprintと形状を検証し、全cellと個体IDを一括で作成します。
    pub fn place_block(
        &mut self,
        layer: LayerId,
        anchor: CellPoint,
        material: MaterialHandle,
        orientation: Orientation,
    ) -> EngineResult<BlockRef> {
        self.run(|editor| {
            let material = editor.world.content.material_id(material)?;
            let content = Arc::clone(&editor.world.content);
            let def = content
                .material(material)?
                .ok_or_else(|| invalid("空素材のblock配置"))?;
            editor.world.validate_placement(&super::PlacementContext {
                world: editor.world,
                layer,
                anchor,
                orientation,
                material: def,
            })?;
            let size = def.block_size();
            let area = usize::from(size[0]) * usize::from(size[1]);
            // mask全体を列挙する計算量にも予算を適用し、疎な巨大maskで上限を迂回しません。
            editor.reserve_writes(area)?;
            let mut block = BlockInstance {
                material,
                layer,
                anchor,
                orientation,
                remaining: Vec::new(),
                epoch: editor.world.remaps.epoch(),
                dormant: None,
                properties: None,
                pending_properties: Vec::new(),
            };
            block
                .remaining
                .try_reserve_exact(area)
                .map_err(allocation)?;
            for y in 0..size[1] {
                for x in 0..size[0] {
                    if def.shape().occupied(x, y, size) {
                        block
                            .remaining
                            .push(u32::from(y) * u32::from(size[0]) + u32::from(x));
                    }
                }
            }
            for index in &block.remaining {
                let existing = editor.loaded(layer, block.point(*index, size)?)?;
                if existing.core != CellCore::AIR || existing.block.is_some() {
                    return Err(conflict("block配置先の占有"));
                }
            }
            let id = editor.world.blocks.insert(block)?;
            // 個体は候補内でのみ公開されます。途中の確保・書込失敗でも元Worldへ影響しません。
            let positions: Vec<_> = editor
                .world
                .blocks
                .get(id)?
                .remaining
                .iter()
                .map(|index| editor.world.blocks.get(id)?.point(*index, size))
                .collect::<EngineResult<_>>()?;
            for point in positions {
                editor.write(
                    layer,
                    point,
                    CellCore {
                        material,
                        ..CellCore::AIR
                    },
                    Some(id),
                )?;
            }
            Ok(BlockRef {
                world: editor.world.id,
                id,
            })
        })
    }
    /// 素材のBreakModeに従ってcellまたは個体を除去します。未読込区域を変更しません。
    pub fn break_at(&mut self, layer: LayerId, point: CellPoint) -> EngineResult<()> {
        self.run(|editor| {
            let cell = editor.loaded(layer, point)?;
            let Some(reference) = cell.block else {
                editor.reserve_writes(1)?;
                return editor.write(layer, point, CellCore::AIR, None);
            };
            let block = editor.world.block(reference)?;
            let def = editor
                .world
                .content
                .material(block.material)?
                .ok_or_else(|| invalid("block素材"))?;
            let size = def.block_size();
            let indices = if def.break_mode() == super::BreakMode::Block {
                copy_slice(&block.remaining)?
            } else {
                vec![
                    *block
                        .remaining
                        .iter()
                        .find(|index| {
                            block
                                .point(**index, size)
                                .is_ok_and(|candidate| candidate == point)
                        })
                        .ok_or_else(|| invalid("block maskとcellの不整合"))?,
                ]
            };
            let positions = indices
                .iter()
                .map(|index| block.point(*index, size))
                .collect::<EngineResult<Vec<_>>>()?;
            editor.reserve_writes(indices.len())?;
            for point in positions {
                editor.write(layer, point, CellCore::AIR, None)?;
            }
            let block = editor.world.blocks.get_mut(reference.id)?;
            block
                .remaining
                .retain(|index| indices.binary_search(index).is_err());
            if block.remaining.is_empty() {
                editor.world.blocks.remove(reference.id)?;
            }
            Ok(())
        })
    }
}

pub(crate) fn local(point: CellPoint) -> (ChunkCoord, usize) {
    (
        ChunkCoord::at(point),
        (point.y.rem_euclid(64) * 64 + point.x.rem_euclid(64)) as usize,
    )
}
pub(crate) fn copy_slice<T: Clone>(source: &[T]) -> EngineResult<Vec<T>> {
    let mut output = Vec::new();
    output.try_reserve_exact(source.len()).map_err(allocation)?;
    output.extend_from_slice(source);
    Ok(output)
}
fn filled<T: Clone>(value: T, count: usize) -> EngineResult<Vec<T>> {
    let mut output = Vec::new();
    output.try_reserve_exact(count).map_err(allocation)?;
    output.resize(count, value);
    Ok(output)
}
