//! 固定解像度に縛られない作者channelと、64×64単位の疎な格納。
//!
//! 値が既定値だけのchunkは確保しません。活動中のchunkでは型付き配列を使い、
//! 各cellへHashMapやBoxを置かずに連続したbatchを読めます。

use super::codec::{Decoder, Encoder, StateCodec, invalid};
use super::error::{EngineError, EngineResult, ErrorCode, allocation};
use super::key::StableKey;

/// 一つのchunkの辺長。負座標にもEuclidean除算を適用します。
pub const CHUNK_SIDE: i32 = 64;
const CELLS: usize = 4096;

/// channel値の正規な同一性と値域を宣言する契約です。
///
/// 通常のPartialEqでは浮動小数点の+0と-0を区別できません。そのため
/// 正規状態の省略判定にはこの契約を使用し、保存でbit情報を失わないようにします。
pub trait GridValue: StateCodec + Copy {
    /// 保存上同じ値ならtrue。作者recordでは全ての正規fieldを比較します。
    fn same(self, other: Self) -> bool;
    /// 値域を検証します。参照や外部資源を持つ値は別storeのIDを使います。
    fn validate(self) -> EngineResult<()>;
}

macro_rules! integer_grid {
    ($($ty:ty),* $(,)?) => {$ (
        impl GridValue for $ty {
            fn same(self, other: Self) -> bool { self == other }
            fn validate(self) -> EngineResult<()> { Ok(()) }
        }
    )*};
}
integer_grid!(u8, u16, u32, u64, i32, i64, bool);

macro_rules! float_grid {
    ($($ty:ty),* $(,)?) => {$ (
        impl GridValue for $ty {
            fn same(self, other: Self) -> bool { self.to_bits() == other.to_bits() }
            fn validate(self) -> EngineResult<()> {
                if self.is_finite() { Ok(()) } else { Err(EngineError::new(ErrorCode::InvalidValue, "channelの非有限値")) }
            }
        }
    )*};
}
float_grid!(f32, f64);

/// channel内の整数座標です。画面pixelとWorldの連続座標とは別の型です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPoint {
    /// 右方向が正の座標。
    pub x: i32,
    /// 下方向が正の座標。
    pub y: i32,
}

impl CellPoint {
    /// chunk座標と、配列内のrow-major indexへ分解します。
    pub fn chunk_local(self) -> ((i32, i32), usize) {
        (
            (self.x.div_euclid(CHUNK_SIDE), self.y.div_euclid(CHUNK_SIDE)),
            (self.y.rem_euclid(CHUNK_SIDE) * CHUNK_SIDE + self.x.rem_euclid(CHUNK_SIDE)) as usize,
        )
    }
}

impl StateCodec for CellPoint {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.i32(self.x)?;
        out.i32(self.y)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            x: input.i32()?,
            y: input.i32()?,
        })
    }
}

/// World cellに対するchannel一要素の幅または高さを表す有理数です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridScale {
    numerator: u32,
    denominator: u32,
}

impl GridScale {
    /// 正の比を約分して保持します。ゼロ幅・ゼロ分母を拒否します。
    pub fn new(mut numerator: u32, mut denominator: u32) -> EngineResult<Self> {
        if numerator == 0 || denominator == 0 {
            return Err(EngineError::new(ErrorCode::InvalidValue, "channelの格子比"));
        }
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        numerator /= a;
        denominator /= a;
        Ok(Self {
            numerator,
            denominator,
        })
    }
    /// 約分済みの分子と分母を返します。
    pub fn ratio(self) -> (u32, u32) {
        (self.numerator, self.denominator)
    }
}

impl StateCodec for GridScale {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.numerator)?;
        out.u32(self.denominator)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let numerator = input.u32()?;
        let denominator = input.u32()?;
        let scale = Self::new(numerator, denominator)?;
        if scale.ratio() != (numerator, denominator) {
            return Err(invalid());
        }
        Ok(scale)
    }
}

/// 整数World原点と各軸の格子比です。物理・表示へ渡す変換を保存できます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridLayout {
    /// channel座標(0,0)が位置するWorldの整数cell原点。
    pub origin: CellPoint,
    /// channel一要素のWorldでの幅。
    pub x_scale: GridScale,
    /// channel一要素のWorldでの高さ。
    pub y_scale: GridScale,
}

impl StateCodec for GridLayout {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.origin.encode(out)?;
        self.x_scale.encode(out)?;
        self.y_scale.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            origin: CellPoint::decode(input)?,
            x_scale: GridScale::decode(input)?,
            y_scale: GridScale::decode(input)?,
        })
    }
}

/// channelの確保と保存に対する、呼出側が所有する上限です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridLimits {
    /// 同時に非既定値を持つchunkの最大個数。
    pub chunks: u32,
    /// 保存bufferの上限。
    pub checkpoint_bytes: usize,
}

struct Chunk<T> {
    coordinate: (i32, i32),
    values: Vec<T>,
    non_default: u16,
}

/// 独自の液量・配線・field等を持つ型付きgridです。
///
/// 値はCopyの固定幅recordとし、可変長データはStateStoreのIDで参照します。
/// loaded/activeの管理はWorld streamingが所有し、この格納型だけでは判定しません。
pub struct GridChannel<T> {
    key: StableKey,
    version: u32,
    layout: GridLayout,
    default: T,
    chunks: Vec<Chunk<T>>,
    limits: GridLimits,
    revision: u64,
}

impl<T: GridValue> GridChannel<T> {
    /// 空のchannelを作ります。値のcodec版と格子変換も正規状態になります。
    pub fn new(
        key: StableKey,
        version: u32,
        layout: GridLayout,
        default: T,
        limits: GridLimits,
    ) -> EngineResult<Self> {
        if version == 0 || limits.chunks == 0 || limits.checkpoint_bytes == 0 {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "channelの版・容量・既定値",
            ));
        }
        default.validate()?;
        Ok(Self {
            key,
            version,
            layout,
            default,
            chunks: Vec::new(),
            limits,
            revision: 0,
        })
    }

    /// channelの安定キーです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 正規の変更revisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// 格子からWorldへの変換定義です。
    pub fn layout(&self) -> GridLayout {
        self.layout
    }
    /// 確保しているchunk数です。
    pub fn allocated_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// 輸送前の容量計算用に、配列を確保せず非既定値の件数を返します。
    pub(crate) fn non_default_count(&self, coordinate: (i32, i32)) -> usize {
        self.find(coordinate)
            .map(|index| usize::from(self.chunks[index].non_default))
            .unwrap_or(0)
    }

    /// 値を読みます。chunk未確保は既定値を意味し、World未読込の意味ではありません。
    pub fn get(&self, point: CellPoint) -> T {
        let (coordinate, index) = point.chunk_local();
        match self.find(coordinate) {
            Ok(chunk) => self.chunks[chunk].values[index],
            Err(_) => self.default,
        }
    }

    /// 一つの値を変更します。確保・revision上限の失敗では値を変更しません。
    /// 複数channelを同時変更する際は、候補を作ってWorldのbarrierで公開します。
    pub fn set(&mut self, point: CellPoint, value: T) -> EngineResult<()> {
        value.validate()?;
        let (coordinate, index) = point.chunk_local();
        let location = self.find(coordinate);
        let old = location
            .as_ref()
            .map(|chunk| self.chunks[*chunk].values[index])
            .unwrap_or(self.default);
        if old.same(value) {
            return Ok(());
        }
        let next = self
            .revision
            .checked_add(1)
            .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "channel revision"))?;
        match location {
            Ok(chunk) => {
                let chunk_data = &mut self.chunks[chunk];
                if old.same(self.default) {
                    chunk_data.non_default += 1;
                }
                if value.same(self.default) {
                    chunk_data.non_default -= 1;
                }
                chunk_data.values[index] = value;
                if chunk_data.non_default == 0 {
                    self.chunks.remove(chunk);
                }
            }
            Err(position) => {
                if self.chunks.len() >= self.limits.chunks as usize {
                    return Err(EngineError::capacity(
                        "channel chunk数",
                        self.chunks.len() as u64 + 1,
                        u64::from(self.limits.chunks),
                    ));
                }
                let mut values = Vec::new();
                values.try_reserve_exact(CELLS).map_err(allocation)?;
                values.resize(CELLS, self.default);
                values[index] = value;
                self.chunks.try_reserve(1).map_err(allocation)?;
                self.chunks.insert(
                    position,
                    Chunk {
                        coordinate,
                        values,
                        non_default: 1,
                    },
                );
            }
        }
        self.revision = next;
        Ok(())
    }

    /// 確保済みchunkを安定座標順で連続配列として読みます。
    pub fn batches(&self) -> impl Iterator<Item = ((i32, i32), &[T])> {
        self.chunks
            .iter()
            .map(|chunk| (chunk.coordinate, chunk.values.as_slice()))
    }

    /// 一つのchunkを連続bufferで編集し、全値の検証後に一括反映します。
    ///
    /// 参照実装は4096要素を候補へ複製します。作者kernelの失敗や不正値で
    /// 元配列を壊さず、全値が既定値になったchunkは回収します。
    /// 外側の複合取引に含める場合も同じAPIを候補GridChannelへ呼びます。
    pub fn edit_chunk(
        &mut self,
        coordinate: (i32, i32),
        edit: impl FnOnce(&mut [T]) -> EngineResult<()>,
    ) -> EngineResult<()> {
        // i32 cellで表せないchunkを空き領域として発行しません。
        let min_chunk = i32::MIN.div_euclid(CHUNK_SIDE);
        let max_chunk = i32::MAX.div_euclid(CHUNK_SIDE);
        if !(min_chunk..=max_chunk).contains(&coordinate.0)
            || !(min_chunk..=max_chunk).contains(&coordinate.1)
        {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "channelのchunk座標",
            ));
        }
        let location = self.find(coordinate);
        let mut candidate = Vec::new();
        candidate.try_reserve_exact(CELLS).map_err(allocation)?;
        match location {
            Ok(index) => candidate.extend_from_slice(&self.chunks[index].values),
            Err(_) => candidate.resize(CELLS, self.default),
        }
        edit(&mut candidate)?;
        let mut count = 0_u16;
        let mut changed = false;
        for (index, value) in candidate.iter().enumerate() {
            value.validate()?;
            if !value.same(self.default) {
                count += 1;
            }
            let old = location
                .as_ref()
                .map(|chunk| self.chunks[*chunk].values[index])
                .unwrap_or(self.default);
            changed |= !value.same(old);
        }
        if !changed {
            return Ok(());
        }
        let next = self
            .revision
            .checked_add(1)
            .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "channel revision"))?;
        if location.is_err() && count != 0 {
            if self.chunks.len() >= self.limits.chunks as usize {
                return Err(EngineError::capacity(
                    "channel chunk数",
                    self.chunks.len() as u64 + 1,
                    u64::from(self.limits.chunks),
                ));
            }
            self.chunks.try_reserve(1).map_err(allocation)?;
        }
        match location {
            Ok(index) if count == 0 => {
                self.chunks.remove(index);
            }
            Ok(index) => {
                self.chunks[index] = Chunk {
                    coordinate,
                    values: candidate,
                    non_default: count,
                }
            }
            Err(index) if count != 0 => self.chunks.insert(
                index,
                Chunk {
                    coordinate,
                    values: candidate,
                    non_default: count,
                },
            ),
            Err(_) => {}
        }
        self.revision = next;
        Ok(())
    }

    /// 候補用の独立した複製です。いずれかの確保に失敗したら全候補を破棄します。
    pub fn try_copy(&self) -> EngineResult<Self> {
        let mut candidate = Self::new(
            self.key,
            self.version,
            self.layout,
            self.default,
            self.limits,
        )?;
        candidate
            .chunks
            .try_reserve_exact(self.chunks.len())
            .map_err(allocation)?;
        for chunk in &self.chunks {
            let mut values = Vec::new();
            values.try_reserve_exact(CELLS).map_err(allocation)?;
            values.extend_from_slice(&chunk.values);
            candidate.chunks.push(Chunk {
                coordinate: chunk.coordinate,
                values,
                non_default: chunk.non_default,
            });
        }
        candidate.revision = self.revision;
        Ok(candidate)
    }

    fn find(&self, coordinate: (i32, i32)) -> Result<usize, usize> {
        self.chunks
            .binary_search_by_key(&coordinate, |chunk| chunk.coordinate)
    }
}

impl<T: GridValue> super::transaction::CandidateState for GridChannel<T> {
    fn try_copy(&self) -> EngineResult<Self> {
        GridChannel::try_copy(self)
    }
    fn validate(&self) -> EngineResult<()> {
        self.default.validate()?;
        for chunk in &self.chunks {
            for value in &chunk.values {
                value.validate()?;
            }
        }
        Ok(())
    }
}

impl<T: GridValue> GridChannel<T> {
    /// 非既定値のみを座標・局所index順に保存します。格納配列のmemory dumpではありません。
    pub fn checkpoint(&self) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(self.limits.checkpoint_bytes);
        out.raw(b"CHRGRD01")?;
        self.key.encode(&mut out)?;
        out.u32(self.version)?;
        self.layout.encode(&mut out)?;
        self.default.encode(&mut out)?;
        out.u64(self.revision)?;
        out.u32(self.chunks.len() as u32)?;
        for chunk in &self.chunks {
            out.i32(chunk.coordinate.0)?;
            out.i32(chunk.coordinate.1)?;
            out.u16(chunk.non_default)?;
            for (index, value) in chunk.values.iter().enumerate() {
                if !value.same(self.default) {
                    out.u16(index as u16)?;
                    value.encode(&mut out)?;
                }
            }
        }
        Ok(out.finish())
    }

    /// 期待するkey/version/layout/defaultと外部上限で候補を復元します。
    ///
    /// 値の型の移行は呼出側schemaで先に行います。未知の版や格子の違いを
    /// 暗黙変換しません。空chunkや非正規の並びも拒否します。
    pub fn restore(
        bytes: &[u8],
        key: StableKey,
        version: u32,
        layout: GridLayout,
        default: T,
        limits: GridLimits,
    ) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, limits.checkpoint_bytes)?;
        if input.raw(8)? != b"CHRGRD01"
            || StableKey::decode(&mut input)? != key
            || input.u32()? != version
        {
            return Err(invalid());
        }
        if GridLayout::decode(&mut input)? != layout || !T::decode(&mut input)?.same(default) {
            return Err(invalid());
        }
        let revision = input.u64()?;
        let count = input.u32()?;
        if count > limits.chunks || u64::from(count) * 12 > input.remaining() as u64 {
            return Err(invalid());
        }
        let mut candidate = Self::new(key, version, layout, default, limits)?;
        let mut previous = None;
        for _ in 0..count {
            let coordinate = (input.i32()?, input.i32()?);
            if previous.is_some_and(|old| old >= coordinate) {
                return Err(invalid());
            }
            previous = Some(coordinate);
            let cells = input.u16()?;
            if cells == 0
                || usize::from(cells) > CELLS
                || usize::from(cells) * 2 > input.remaining()
            {
                return Err(invalid());
            }
            let mut last_index = None;
            for _ in 0..cells {
                let index = input.u16()?;
                if usize::from(index) >= CELLS || last_index.is_some_and(|last| last >= index) {
                    return Err(invalid());
                }
                last_index = Some(index);
                let value = T::decode(&mut input)?;
                if value.same(default) {
                    return Err(invalid());
                }
                let x = i64::from(coordinate.0) * 64 + i64::from(index % 64);
                let y = i64::from(coordinate.1) * 64 + i64::from(index / 64);
                let point = CellPoint {
                    x: i32::try_from(x).map_err(|_| invalid())?,
                    y: i32::try_from(y).map_err(|_| invalid())?,
                };
                candidate.set(point, value)?;
            }
        }
        input.finish()?;
        candidate.revision = revision;
        Ok(candidate)
    }
}
