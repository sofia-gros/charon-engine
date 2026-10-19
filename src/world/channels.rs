//! 作者の別解像度channelとWorld domainの接続。
//!
//! GridChannel自体は疎な配列で、未確保を既定値として扱います。この層では先にWorldの
//! coverageを確認し、未読込を既定値へ置換しません。borrow中はWorld全体が不変借用され、
//! batchを読みながらunloadや再配置を実行することはRustの借用規則で禁止されます。

use super::{
    ChunkCoord, ChunkCoverage, ChunkRegion, ContentRegistry, RegionStatus, World, WorldEdit,
    invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridChannel, GridLayout, GridLimits, GridValue},
};
use std::{any::Any, sync::Arc};

/// 一つの独自channelの型・格子・既定値・容量。定義はContent freeze後に変えません。
pub struct ChannelDefinition<T: GridValue> {
    /// 独自channelキー。
    pub key: StableKey,
    /// record版。
    pub version: u32,
    /// 一要素の型schema。ContentRegistry::schemaで同じTを登録します。
    pub value_schema: StableKey,
    /// Worldのcellに対する原点と各軸の有理数格子比。
    pub layout: GridLayout,
    /// 読込済みだが配列を未確保の領域で返す値。
    pub default: T,
    /// channelの独立した確保・codec上限。
    pub limits: GridLimits,
}
pub(crate) trait ChannelFactory: Send + Sync {
    fn key(&self) -> StableKey;
    fn value_schema(&self) -> StableKey;
    fn descriptor(&self, limit: usize) -> EngineResult<Vec<u8>>;
    fn create(&self) -> EngineResult<Box<dyn ChannelState>>;
    fn restore(&self, bytes: &[u8]) -> EngineResult<Box<dyn ChannelState>>;
    fn validate_type(
        &self,
        schemas: &crate::foundation::schema::SchemaRegistry,
    ) -> EngineResult<()>;
}
pub(crate) trait ChannelState: Send + Sync {
    fn key(&self) -> StableKey;
    fn material_layer(&self) -> Option<StableKey>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn try_copy(&self) -> EngineResult<Box<dyn ChannelState>>;
    fn checkpoint(&self) -> EngineResult<Vec<u8>>;
    fn bytes(&self) -> usize;
    fn validate(&self, world: &World) -> EngineResult<()>;
    fn extract(&self, region: ChunkRegion) -> EngineResult<Vec<u8>>;
    fn clear(&mut self, region: ChunkRegion) -> EngineResult<()>;
    fn merge(&mut self, bytes: &[u8]) -> EngineResult<()>;
    fn transport_bytes(&self, layer: StableKey, path: &[CellPoint]) -> EngineResult<usize>;
    fn transport(&mut self, layer: StableKey, path: &[CellPoint]) -> EngineResult<()>;
}
struct Factory<T: GridValue + Send + Sync + 'static>(ChannelDefinition<T>, Option<StableKey>);
struct Storage<T: GridValue + Send + Sync + 'static> {
    definition: Arc<Factory<T>>,
    grid: GridChannel<T>,
}
impl<T: GridValue + Send + Sync + 'static> Factory<T> {
    fn copy(&self) -> Self {
        Self(
            ChannelDefinition {
                key: self.0.key,
                version: self.0.version,
                value_schema: self.0.value_schema,
                layout: self.0.layout,
                default: self.0.default,
                limits: self.0.limits,
            },
            self.1,
        )
    }
    fn empty(&self) -> EngineResult<GridChannel<T>> {
        GridChannel::new(
            self.0.key,
            self.0.version,
            self.0.layout,
            self.0.default,
            self.0.limits,
        )
    }
    fn decode(&self, bytes: &[u8]) -> EngineResult<GridChannel<T>> {
        GridChannel::restore(
            bytes,
            self.0.key,
            self.0.version,
            self.0.layout,
            self.0.default,
            self.0.limits,
        )
    }
}
impl<T: GridValue + Send + Sync + 'static> ChannelFactory for Factory<T> {
    fn key(&self) -> StableKey {
        self.0.key
    }
    fn value_schema(&self) -> StableKey {
        self.0.value_schema
    }
    fn descriptor(&self, limit: usize) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(limit);
        self.0.key.encode(&mut out)?;
        out.u32(self.0.version)?;
        self.0.value_schema.encode(&mut out)?;
        self.0.layout.encode(&mut out)?;
        self.0.default.encode(&mut out)?;
        out.u32(self.0.limits.chunks)?;
        out.u64(self.0.limits.checkpoint_bytes as u64)?;
        // 位置に固定された既存channelのmanifestは維持します。物質追従を指定した
        // 場合だけ識別子と所属layerを加え、所有権の変更をContent差分として検出します。
        if let Some(layer) = self.1 {
            out.raw(b"MATERIAL")?;
            layer.encode(&mut out)?;
        }
        Ok(out.finish())
    }
    fn create(&self) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Storage {
            definition: Arc::new(self.copy()),
            grid: self.empty()?,
        }))
    }
    fn restore(&self, bytes: &[u8]) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Storage {
            definition: Arc::new(self.copy()),
            grid: self.decode(bytes)?,
        }))
    }
    fn validate_type(
        &self,
        schemas: &crate::foundation::schema::SchemaRegistry,
    ) -> EngineResult<()> {
        let schema = schemas.get(self.0.value_schema)?;
        if !schema.is::<T>() {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "channel値の型schema",
            ));
        }
        let mut out = Encoder::new(schema.max_bytes());
        self.0.default.encode(&mut out)?;
        schema.validate(&out.finish())
    }
}
impl<T: GridValue + Send + Sync + 'static> Storage<T> {
    fn transport_coordinate(&self, point: CellPoint) -> EngineResult<CellPoint> {
        let origin = self.grid.layout().origin;
        Ok(CellPoint {
            x: point
                .x
                .checked_sub(origin.x)
                .ok_or_else(|| invalid("物質channel水平座標"))?,
            y: point
                .y
                .checked_sub(origin.y)
                .ok_or_else(|| invalid("物質channel垂直座標"))?,
        })
    }
    fn transport_values(&self, path: &[CellPoint]) -> EngineResult<Vec<(CellPoint, T)>> {
        let mut values = Vec::new();
        values.try_reserve_exact(path.len()).map_err(allocation)?;
        for point in path {
            let coordinate = self.transport_coordinate(*point)?;
            values.push((coordinate, self.grid.get(coordinate)));
        }
        Ok(values)
    }
    fn projected_bytes(&self, changes: &mut [((i32, i32), i64)]) -> EngineResult<usize> {
        changes.sort_unstable_by_key(|(coordinate, _)| *coordinate);
        let mut chunks = self.grid.allocated_chunks();
        let mut index = 0;
        while index < changes.len() {
            let coordinate = changes[index].0;
            let before = self.grid.non_default_count(coordinate);
            let mut after = before as i64;
            while index < changes.len() && changes[index].0 == coordinate {
                after += changes[index].1;
                index += 1;
            }
            if before == 0 && after > 0 {
                chunks += 1;
            } else if before > 0 && after == 0 {
                chunks -= 1;
            }
        }
        if chunks > self.definition.0.limits.chunks as usize {
            return Err(EngineError::capacity(
                "channel chunk数",
                chunks as u64,
                self.definition.0.limits.chunks.into(),
            ));
        }
        chunks
            .checked_mul(4096)
            .and_then(|cells| cells.checked_mul(size_of::<T>()))
            .ok_or_else(|| invalid("更新後のchannel byte数overflow"))
    }
    fn non_default(&self) -> impl Iterator<Item = (CellPoint, T)> + '_ {
        self.grid.batches().flat_map(move |((cx, cy), values)| {
            values
                .iter()
                .copied()
                .enumerate()
                .filter_map(move |(index, value)| {
                    if value.same(self.definition.0.default) {
                        return None;
                    }
                    let x = i64::from(cx) * 64 + (index % 64) as i64;
                    let y = i64::from(cy) * 64 + (index / 64) as i64;
                    Some((
                        CellPoint {
                            x: x as i32,
                            y: y as i32,
                        },
                        value,
                    ))
                })
        })
    }
    fn selected(&self, region: ChunkRegion) -> EngineResult<Vec<(CellPoint, T)>> {
        let mut result = Vec::new();
        for (point, value) in self.non_default() {
            let coverage = coverage(self.grid.layout(), point, 1)?;
            let intersects = (coverage.min().x as i64) < region.max()[0]
                && (region.min().x as i64) < coverage.max()[0]
                && (coverage.min().y as i64) < region.max()[1]
                && (region.min().y as i64) < coverage.max()[1];
            if intersects {
                if !region.contains(coverage.min())
                    || coverage.max()[0] > region.max()[0]
                    || coverage.max()[1] > region.max()[1]
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "channel値が退避regionの境界を跨ぐ",
                    ));
                }
                result.try_reserve(1).map_err(allocation)?;
                result.push((point, value));
            }
        }
        Ok(result)
    }
}
impl<T: GridValue + Send + Sync + 'static> ChannelState for Storage<T> {
    fn key(&self) -> StableKey {
        self.definition.0.key
    }
    fn material_layer(&self) -> Option<StableKey> {
        self.definition.1
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn try_copy(&self) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Self {
            definition: Arc::clone(&self.definition),
            grid: self.grid.try_copy()?,
        }))
    }
    fn checkpoint(&self) -> EngineResult<Vec<u8>> {
        self.grid.checkpoint()
    }
    fn bytes(&self) -> usize {
        self.grid.allocated_chunks() * 4096 * size_of::<T>()
    }
    fn validate(&self, world: &World) -> EngineResult<()> {
        for (point, value) in self.non_default() {
            value.validate()?;
            let (min, max) = cell_bounds(self.grid.layout(), point, 1)?;
            if min[0] < i64::from(world.bounds.min().x)
                || min[1] < i64::from(world.bounds.min().y)
                || max[0] > world.bounds.max()[0]
                || max[1] > world.bounds.max()[1]
            {
                return Err(invalid("channel値がWorld境界外"));
            }
            let region = coverage(self.grid.layout(), point, 1)?;
            world.require_coverage(region)?;
        }
        Ok(())
    }
    fn extract(&self, region: ChunkRegion) -> EngineResult<Vec<u8>> {
        let mut grid = self.definition.empty()?;
        for (point, value) in self.selected(region)? {
            grid.set(point, value)?;
        }
        grid.checkpoint()
    }
    fn clear(&mut self, region: ChunkRegion) -> EngineResult<()> {
        for (point, _) in self.selected(region)? {
            self.grid.set(point, self.definition.0.default)?;
        }
        Ok(())
    }
    fn merge(&mut self, bytes: &[u8]) -> EngineResult<()> {
        let source = Self {
            definition: Arc::clone(&self.definition),
            grid: self.definition.decode(bytes)?,
        };
        for (point, value) in source.non_default() {
            if !self.grid.get(point).same(self.definition.0.default) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "復帰channelの上書き競合",
                ));
            }
            self.grid.set(point, value)?;
        }
        Ok(())
    }
    fn transport_bytes(&self, layer: StableKey, path: &[CellPoint]) -> EngineResult<usize> {
        if self.definition.1 != Some(layer) {
            return Ok(self.bytes());
        }
        // cell配列を新規確保する前に、各chunkの非既定値の増減だけを集計します。
        // 経路外に残る値と、空になる旧chunkの回収を両方含めます。
        let mut changes = Vec::new();
        changes.try_reserve_exact(path.len()).map_err(allocation)?;
        for (index, point) in path.iter().enumerate() {
            let point = self.transport_coordinate(*point)?;
            let previous =
                self.transport_coordinate(path[(index + path.len() - 1) % path.len()])?;
            let old = self.grid.get(point);
            let incoming = self.grid.get(previous);
            let delta = i64::from(!incoming.same(self.definition.0.default))
                - i64::from(!old.same(self.definition.0.default));
            changes.push((point.chunk_local().0, delta));
        }
        self.projected_bytes(&mut changes)
    }
    fn transport(&mut self, layer: StableKey, path: &[CellPoint]) -> EngineResult<()> {
        if self.definition.1 != Some(layer) {
            return Ok(());
        }
        // 先に旧値を全て退避し、一旦既定値へ戻してから再配置します。これにより
        // chunk上限ぴったりで別chunkへ移る場合も、不要な同時確保を要求しません。
        // 途中で失敗すれば外側のWorld候補全体が破棄されます。
        let values = self.transport_values(path)?;
        for (point, _) in &values {
            self.grid.set(*point, self.definition.0.default)?;
        }
        // pathの末尾を先頭へ、残りは一つ先へ送る循環置換です。
        for index in 0..values.len() {
            let destination = (index + 1) % values.len();
            self.grid.set(values[destination].0, values[index].1)?;
        }
        Ok(())
    }
}
impl ContentRegistry {
    /// 状態・block個体・channel値に使う型schemaを登録します。
    pub fn schema(&mut self, schema: crate::foundation::schema::Schema) -> EngineResult<()> {
        self.schemas.register(schema)
    }
    /// 独自channelを登録します。型・既定値の検証と重複検査を済ませてから公開します。
    pub fn channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        definition: ChannelDefinition<T>,
    ) -> EngineResult<()> {
        self.register_channel(definition, None)
    }
    /// 指定layerの物質と一緒に移るchannelを登録します。
    ///
    /// energy、timer、混合recordや作者の個体IDを格納するための所有権宣言です。
    /// 地形の輸送取引は登録された全channelを移すため、呼出側の選択漏れで状態を
    /// 置き去りにしません。圧力など位置へ属する場には通常の`channel`を使います。
    /// この経路は1cell対1要素だけを扱います。別解像度の値の分配・合流は作者が
    /// 通常channelと明示的な変換取引で定義し、暗黙に補間しません。
    pub fn material_channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        layer: StableKey,
        definition: ChannelDefinition<T>,
    ) -> EngineResult<()> {
        if !self.has_layer(layer) {
            return Err(EngineError::new(
                ErrorCode::MissingKey,
                "物質channelのlayer",
            ));
        }
        if definition.layout.x_scale.ratio() != (1, 1)
            || definition.layout.y_scale.ratio() != (1, 1)
        {
            return Err(invalid("物質追従channelには1対1の格子が必要"));
        }
        self.register_channel(definition, Some(layer))
    }
    fn register_channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        definition: ChannelDefinition<T>,
        material_layer: Option<StableKey>,
    ) -> EngineResult<()> {
        if self.channels.len() >= self.limits.channels as usize {
            return Err(EngineError::capacity(
                "channel定義数",
                self.channels.len() as u64 + 1,
                self.limits.channels.into(),
            ));
        }
        let factory = Factory(definition, material_layer);
        factory.validate_type(&self.schemas)?;
        factory.empty()?;
        let index = match self
            .channels
            .binary_search_by_key(&factory.key(), |old| old.key())
        {
            Ok(_) => return Err(EngineError::new(ErrorCode::DuplicateKey, "channelキー")),
            Err(index) => index,
        };
        self.channels.try_reserve(1).map_err(allocation)?;
        self.channels.insert(index, Arc::new(factory));
        Ok(())
    }
}
/// 読込状態を失わない型付きchannel query。
pub enum ChannelQuery<T> {
    /// 全coverageがWorld内かつresident。既定値もこの経路で返します。
    Loaded(T),
    /// 必要なchunkに未準備部分がある。各chunkの状態を保持します。
    Unavailable(Vec<ChunkCoverage>),
    /// 要素がWorldの境界を跨ぐか、座標の表現範囲外。
    WorldBoundary,
}
/// 連続した64×64の型付きbatch。coverageは値配列と別に提供します。
pub struct ChannelBatch<'a, T> {
    /// channel内のchunk座標。
    pub coordinate: (i32, i32),
    /// row-majorの固定長値配列。
    pub values: &'a [T],
    /// Worldへの格子変換。
    pub layout: GridLayout,
    /// このbatchが必要とする各World chunk。haloは呼出側が追加regionを照会できます。
    pub coverage: Vec<ChunkCoverage>,
}
impl World {
    /// 登録済みchannelの格子変換を返します。値型が一致しない要求は拒否します。
    /// 読込状態や値は含まないため、値の取得時には引き続きcoverageを確認してください。
    pub fn channel_layout<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
    ) -> EngineResult<GridLayout> {
        Ok(self.typed_channel::<T>(key)?.grid.layout())
    }
    /// channelが物質に追従する場合の所属layer。Noneは位置に固定された場です。
    pub fn channel_material_layer<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
    ) -> EngineResult<Option<StableKey>> {
        Ok(self.typed_channel::<T>(key)?.definition.1)
    }
    fn typed_channel<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
    ) -> EngineResult<&Storage<T>> {
        let channel = self
            .channels
            .iter()
            .find(|channel| channel.key() == key)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "World channel"))?;
        channel
            .as_any()
            .downcast_ref::<Storage<T>>()
            .ok_or_else(|| EngineError::new(ErrorCode::SchemaMismatch, "channelの要求型"))
    }
    pub(crate) fn require_coverage(&self, region: ChunkRegion) -> EngineResult<()> {
        for coord in region.coordinates(self.limits.resident_chunks as usize)? {
            if !self.chunks.contains_key(&coord) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "channelの必要regionが未準備",
                ));
            }
        }
        Ok(())
    }
    /// 読込状態を照合して型付き値を返します。未読込をdefaultへ丸めません。
    pub fn channel_value<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
        point: CellPoint,
    ) -> EngineResult<ChannelQuery<T>> {
        let channel = self.typed_channel::<T>(key)?;
        let (min, max) = cell_bounds(channel.grid.layout(), point, 1)?;
        let bounds = self.bounds;
        if min[0] < i64::from(bounds.min().x)
            || min[1] < i64::from(bounds.min().y)
            || max[0] > bounds.max()[0]
            || max[1] > bounds.max()[1]
        {
            return Ok(ChannelQuery::WorldBoundary);
        }
        let region = coverage(channel.grid.layout(), point, 1)?;
        let states = self.region_state(region, self.limits.resident_chunks as usize)?;
        if states.iter().any(|state| {
            !matches!(
                state.status,
                RegionStatus::Active | RegionStatus::ResidentInactive
            )
        }) {
            return Ok(ChannelQuery::Unavailable(states));
        }
        Ok(ChannelQuery::Loaded(channel.grid.get(point)))
    }
    /// 配列をcellごとのdynamic callbackへ変換せず公開します。借用中はunloadできません。
    pub fn channel_batches<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
        coverage_limit: usize,
    ) -> EngineResult<Vec<ChannelBatch<'_, T>>> {
        let channel = self.typed_channel::<T>(key)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(channel.grid.allocated_chunks())
            .map_err(allocation)?;
        for (coordinate, values) in channel.grid.batches() {
            let point = CellPoint {
                x: i32::try_from(i64::from(coordinate.0) * 64)
                    .map_err(|_| invalid("channel batch座標"))?,
                y: i32::try_from(i64::from(coordinate.1) * 64)
                    .map_err(|_| invalid("channel batch座標"))?,
            };
            let coverage =
                self.region_state(coverage(channel.grid.layout(), point, 64)?, coverage_limit)?;
            result.push(ChannelBatch {
                coordinate,
                values,
                layout: channel.grid.layout(),
                coverage,
            });
        }
        Ok(result)
    }
    /// channelの確保済み値配列の合計byte数。
    pub fn channel_bytes(&self) -> usize {
        self.channels.iter().map(|channel| channel.bytes()).sum()
    }
    pub(crate) fn validate_channels(&self) -> EngineResult<()> {
        if self.channels.len() != self.content.channels.len() {
            return Err(invalid("World channel数"));
        }
        if self.channel_bytes() > self.limits.channel_bytes {
            return Err(EngineError::capacity(
                "channel byte数",
                self.channel_bytes() as u64,
                self.limits.channel_bytes as u64,
            ));
        }
        for (state, definition) in self.channels.iter().zip(&self.content.channels) {
            if state.key() != definition.key() {
                return Err(invalid("World channel辞書"));
            }
            state.validate(self)?;
        }
        Ok(())
    }
}
impl World {
    /// 全出力の最終容量を、channel配列を確保する前に検査します。
    /// 呼出側は返された既定値への更新を先に適用し、空chunkを回収してから再配置します。
    pub(super) fn validate_channel_updates<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
        updates: &[(CellPoint, T)],
    ) -> EngineResult<T> {
        let storage = self.typed_channel::<T>(key)?;
        let default = storage.definition.0.default;
        let mut points = Vec::new();
        let mut changes = Vec::new();
        points
            .try_reserve_exact(updates.len())
            .map_err(allocation)?;
        changes
            .try_reserve_exact(updates.len())
            .map_err(allocation)?;
        for &(point, value) in updates {
            value.validate()?;
            if !matches!(
                self.channel_value::<T>(key, point)?,
                ChannelQuery::Loaded(_)
            ) {
                return Err(invalid("channel一括更新のcoverageが未準備または境界外"));
            }
            points.push((point.x, point.y));
            let delta =
                i64::from(!value.same(default)) - i64::from(!storage.grid.get(point).same(default));
            changes.push((point.chunk_local().0, delta));
        }
        points.sort_unstable();
        if points.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("channel一括更新の座標重複"));
        }
        let final_bytes = storage.projected_bytes(&mut changes)?;
        let required = self
            .channel_bytes()
            .checked_sub(storage.bytes())
            .and_then(|bytes| bytes.checked_add(final_bytes))
            .ok_or_else(|| invalid("channel一括更新のbyte数overflow"))?;
        if required > self.limits.channel_bytes {
            return Err(EngineError::capacity(
                "channel byte数",
                required as u64,
                self.limits.channel_bytes as u64,
            ));
        }
        Ok(default)
    }
}
impl WorldEdit<'_> {
    /// 読込済みcoverageの独自値を候補へ設定します。素材や熱等へ自動変換しません。
    pub fn set_channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        key: StableKey,
        point: CellPoint,
        value: T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            if !matches!(
                editor.world.channel_value::<T>(key, point)?,
                ChannelQuery::Loaded(_)
            ) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "channel編集のcoverageが未準備または境界外",
                ));
            }
            let storage = editor.world.typed_channel::<T>(key)?;
            let allocated = storage
                .grid
                .batches()
                .any(|(coordinate, _)| coordinate == point.chunk_local().0);
            let extra = if allocated || value.same(storage.definition.0.default) {
                0
            } else {
                4096 * size_of::<T>()
            };
            let required = editor
                .world
                .channel_bytes()
                .checked_add(extra)
                .ok_or_else(|| invalid("channel byte数overflow"))?;
            if required > editor.world.limits.channel_bytes {
                return Err(EngineError::capacity(
                    "channel byte数",
                    required as u64,
                    editor.world.limits.channel_bytes as u64,
                ));
            }
            editor.reserve_writes(1)?;
            let channel = editor
                .world
                .channels
                .iter_mut()
                .find(|channel| channel.key() == key)
                .ok_or_else(|| invalid("channelが見つからない"))?;
            let storage = channel
                .as_any_mut()
                .downcast_mut::<Storage<T>>()
                .ok_or_else(|| EngineError::new(ErrorCode::SchemaMismatch, "channel型"))?;
            let region = coverage(storage.grid.layout(), point, 1)?;
            storage.grid.set(point, value)?;
            for coord in region.coordinates(editor.world.limits.resident_chunks as usize)? {
                let chunk = editor
                    .world
                    .chunks
                    .get_mut(&coord)
                    .expect("channel coverageを検査済み");
                chunk.revision = chunk
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("channelによるchunk revision上限"))?;
                chunk.edited = true;
            }
            Ok(())
        })
    }
}
fn cell_bounds(
    layout: GridLayout,
    point: CellPoint,
    side: i64,
) -> EngineResult<([i64; 2], [i64; 2])> {
    let mut min = [0; 2];
    let mut max = [0; 2];
    for (axis, (origin, coordinate, scale)) in [
        (layout.origin.x, point.x, layout.x_scale),
        (layout.origin.y, point.y, layout.y_scale),
    ]
    .into_iter()
    .enumerate()
    {
        let (n, d) = scale.ratio();
        let d = i128::from(d);
        let start = i128::from(origin) * d + i128::from(coordinate) * i128::from(n);
        let end = start + i128::from(side) * i128::from(n);
        min[axis] =
            i64::try_from(start.div_euclid(d)).map_err(|_| invalid("channel coverage開始座標"))?;
        max[axis] = i64::try_from((end + d - 1).div_euclid(d))
            .map_err(|_| invalid("channel coverage終了座標"))?;
    }
    Ok((min, max))
}
fn coverage(layout: GridLayout, point: CellPoint, side: i64) -> EngineResult<ChunkRegion> {
    let (min, max) = cell_bounds(layout, point, side)?;
    let min = ChunkCoord::new(
        i32::try_from(min[0].div_euclid(64)).map_err(|_| invalid("channel chunk開始座標"))?,
        i32::try_from(min[1].div_euclid(64)).map_err(|_| invalid("channel chunk開始座標"))?,
    )?;
    ChunkRegion::new(
        min,
        (max[0] + 63).div_euclid(64),
        (max[1] + 63).div_euclid(64),
    )
}
