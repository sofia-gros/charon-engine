//! セル本体と物質に所属する独自状態を、同じ候補の中で輸送します。
//!
//! 物理則の判定は呼出側のsolverが行い、この層は輸送の原子性を保証します。
//! 未読込区域を空きとみなしたり、block個体をcellだけに分解したりしません。

use super::{ChunkCoord, LayerId, World, WorldEdit, conflict, invalid};
use crate::foundation::{EngineError, EngineResult, error::allocation, grid::CellPoint};

/// 活動chunkごとの4096bit。cell数に比例したsorted Vecへの挿入を避け、
/// 参照走査でも予約確認をchunk検索と一つのbit演算で行います。正規状態ではなく
/// 参照輸送のscratchや探索の再構築可能な索引で、sleep判定や保存stateの代わりにはしません。
pub(super) struct CellReservations(Vec<(ChunkCoord, [u64; 64])>);
impl CellReservations {
    pub(super) fn new(world: &World) -> EngineResult<Self> {
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(world.chunks.values().filter(|chunk| chunk.active).count())
            .map_err(allocation)?;
        for (coord, chunk) in &world.chunks {
            if chunk.active {
                chunks.push((*coord, [0; 64]));
            }
        }
        Ok(Self(chunks))
    }
    pub(super) fn for_chunks(chunks: &[(ChunkCoord, u64)]) -> EngineResult<Self> {
        let mut values = Vec::new();
        values.try_reserve_exact(chunks.len()).map_err(allocation)?;
        values.extend(chunks.iter().map(|(coord, _)| (*coord, [0; 64])));
        Ok(Self(values))
    }
    pub(super) fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self(super::terrain::copy_slice(&self.0)?))
    }
    pub(super) fn remove(&mut self, point: CellPoint) {
        let (coord, index) = super::terrain::local(point);
        if let Ok(chunk) = self.0.binary_search_by_key(&coord, |(coord, _)| *coord) {
            self.0[chunk].1[index / 64] &= !(1u64 << (index % 64));
        }
    }
    pub(super) fn contains(&self, point: CellPoint) -> bool {
        let (coord, index) = super::terrain::local(point);
        self.0
            .binary_search_by_key(&coord, |(coord, _)| *coord)
            .ok()
            .is_some_and(|chunk| self.0[chunk].1[index / 64] & (1u64 << (index % 64)) != 0)
    }
    pub(super) fn insert(&mut self, point: CellPoint) -> EngineResult<()> {
        let (coord, index) = super::terrain::local(point);
        let chunk = self
            .0
            .binary_search_by_key(&coord, |(coord, _)| *coord)
            .map_err(|_| invalid("輸送予約の対象chunkが活動外"))?;
        self.0[chunk].1[index / 64] |= 1u64 << (index % 64);
        Ok(())
    }
}

impl WorldEdit<'_> {
    /// 隣接した二つのセルと、そのlayerに登録された全物質channelを交換します。
    ///
    /// 物質の重さや移動可能性はsolverが先に判定します。この操作自体は作者の
    /// 独自挙動にも使えるため、標準の密度則を強制しません。失敗を無視しても
    /// 外側の`World::edit`は失敗し、先行する編集も公開されません。
    pub fn exchange_cells(
        &mut self,
        layer: LayerId,
        first: CellPoint,
        second: CellPoint,
    ) -> EngineResult<()> {
        self.transport_path(layer, &[first, second])
    }

    /// 連続した経路に沿ってセルと付随状態を一つ先へ移し、末尾を先頭へ戻します。
    ///
    /// 押出しでは末尾に空cellを含めます。全点の読込・重複・隣接性・block参照と
    /// 編集予算を検査してから、全経路を予約して循環置換します。斜め隣接も許し、
    /// 斜面や角の通過可否はsolverの形状判定が担います。離れた点への飛越しは禁止です。
    /// 地形とchannelの容量検査を含む確定は外側の取引でまとめて行われます。
    pub fn transport_path(&mut self, layer: LayerId, path: &[CellPoint]) -> EngineResult<()> {
        self.run(|editor| {
            if path.len() < 2 {
                return Err(invalid("輸送経路は2cell以上必要"));
            }
            let layer_key = editor.world.content.layer(layer)?.key;
            // 地形だけを数えると、多数の付随channelを登録して編集予算を迂回できます。
            // 同じ点への地形書込と各channel書込を一つずつ数えて先に予約します。
            let channels = editor
                .world
                .channels
                .iter()
                .filter(|channel| channel.material_layer() == Some(layer_key))
                .count();
            let writes = path
                .len()
                .checked_mul(channels + 1)
                .ok_or_else(|| invalid("物質輸送の書込数overflow"))?;
            editor.reserve_writes(writes)?;
            let mut ordered = Vec::new();
            ordered.try_reserve_exact(path.len()).map_err(allocation)?;
            ordered.extend(path.iter().map(|point| (point.x, point.y)));
            ordered.sort_unstable();
            if ordered.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(conflict("輸送経路のcell重複"));
            }
            for pair in path.windows(2) {
                let dx = (i64::from(pair[0].x) - i64::from(pair[1].x)).abs();
                let dy = (i64::from(pair[0].y) - i64::from(pair[1].y)).abs();
                if dx > 1 || dy > 1 {
                    return Err(invalid("輸送経路に非隣接cellがある"));
                }
            }
            let mut cores = Vec::new();
            cores.try_reserve_exact(path.len()).map_err(allocation)?;
            for point in path {
                let cell = editor.loaded(layer, *point)?;
                if cell.block.is_some() {
                    return Err(conflict("block個体は明示的な分解前に輸送できない"));
                }
                cores.push(cell.core);
            }
            // 全channelの最終容量を先に検査します。縮むchannelから処理すると、
            // 途中の総確保量も開始時と終了時の最大値を超えません。
            let mut plans = Vec::new();
            plans
                .try_reserve_exact(editor.world.channels.len())
                .map_err(allocation)?;
            let mut required = 0usize;
            for (index, channel) in editor.world.channels.iter().enumerate() {
                let bytes = channel.transport_bytes(layer_key, path)?;
                required = required
                    .checked_add(bytes)
                    .ok_or_else(|| invalid("輸送後のchannel byte数overflow"))?;
                plans.push((bytes > channel.bytes(), index));
            }
            if required > editor.world.limits.channel_bytes {
                return Err(EngineError::capacity(
                    "channel byte数",
                    required as u64,
                    editor.world.limits.channel_bytes as u64,
                ));
            }
            plans.sort_unstable();
            for (_, index) in plans {
                editor.world.channels[index].transport(layer_key, path)?;
            }
            // 各始点の旧値はcoresに保持済みです。途中の上書きで元の値を失わず、
            // 一時的な二重占有も候補の外へ公開しません。
            for (index, core) in cores.into_iter().enumerate() {
                editor.write(layer, path[(index + 1) % path.len()], core, None)?;
            }
            Ok(())
        })
    }
}
