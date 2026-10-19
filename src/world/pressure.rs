//! 位置に属する圧力proxyの参照緩和。
//!
//! 圧力から物質を発行しません。輸送は別phaseで行い、このphaseは登録済みの
//! i64 channelだけを変更します。中間計算はi128で行い、保存範囲を超えれば候補を捨てます。

use super::{
    CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialBase, World, WorldEdit, invalid,
};
use crate::foundation::{EngineError, EngineResult, StableKey, error::allocation, grid::CellPoint};

/// 標準圧力緩和の数値設定。実行するSystemが同じ設定を継続して渡します。
/// 作者が別の数式を使う場合は標準phaseを登録せず、通常channelへ独自kernelを書けます。
#[derive(Clone, Copy, Debug)]
pub struct PressureRelaxation {
    /// 圧力のi64 channel。Q16の非負値を保存し、物質追従にはしません。
    pub channel: StableKey,
    /// 同一phase内のJacobi反復数。標準は4です。ゼロ反復を拒否します。
    pub iterations: u16,
    /// 重力による単位質量・単位cell当たりの圧力差。下向きを正とします。
    pub gravity_q16: u32,
    /// 一度に扱うfluid cellの最大数。超過時に未完成の場を公開しません。
    pub cell_limit: usize,
}

/// 緩和に含まれた領域の診断値です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PressureRelaxationReport {
    /// 更新した流体cell数。
    pub cells: usize,
    /// 連結領域のうち、活動中の空cellと接する領域の数。
    pub open_components: usize,
    /// 空cellと接しない領域の数。前phaseの平均圧力を最小座標のanchorに保持します。
    pub closed_components: usize,
}

struct Sample {
    point: CellPoint,
    mass: u32,
    pressure: i64,
    surface: bool,
}
const NEIGHBORS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

impl WorldEdit<'_> {
    /// 活動領域の液体・気体へ、重力項付きJacobi緩和を適用します。
    ///
    /// 空cellとの接触は自由表面で相対圧0、固体・block・活動外・未読込はno-flowです。
    /// 閉領域は前回の平均を安定anchorへ固定します。圧力channelは保存・区域退避の
    /// 既存経路に含まれ、中間反復は独立配列に保持して最後に一括で公開します。
    ///
    /// この操作は気体の温度・体積sourceやポンプの生成規則を自動適用しません。
    /// そのsourceは作者の設定に応じて別phaseで圧力channelへ与えます。
    pub fn relax_pressure(
        &mut self,
        layer: LayerId,
        config: PressureRelaxation,
    ) -> EngineResult<PressureRelaxationReport> {
        self.run(|editor| {
            if config.iterations == 0 || !editor.world.content.layer(layer)?.simulated {
                return Err(invalid("圧力の反復数または対象layer"));
            }
            let layout = editor.world.channel_layout::<i64>(config.channel)?;
            if layout.x_scale.ratio() != (1, 1)
                || layout.y_scale.ratio() != (1, 1)
                || editor
                    .world
                    .channel_material_layer::<i64>(config.channel)?
                    .is_some()
            {
                return Err(invalid("圧力には位置固定の1対1 channelが必要"));
            }
            let mut samples = Vec::new();
            for (coord, chunk) in &editor.world.chunks {
                if !chunk.active {
                    continue;
                }
                let origin = coord.origin()?;
                let page = &chunk.layers[layer.0 as usize];
                for (index, core) in page.cells.iter().enumerate() {
                    if page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                    {
                        continue;
                    }
                    let Some(material) = editor.world.content.material(core.material)? else {
                        continue;
                    };
                    if !matches!(material.base(), MaterialBase::Liquid | MaterialBase::Gas) {
                        continue;
                    }
                    if samples.len() >= config.cell_limit {
                        return Err(EngineError::capacity(
                            "圧力の流体cell数",
                            samples.len() as u64 + 1,
                            config.cell_limit as u64,
                        ));
                    }
                    let point = CellPoint {
                        x: origin.x + (index % 64) as i32,
                        y: origin.y + (index / 64) as i32,
                    };
                    let pressure = match editor.world.channel_value::<i64>(
                        config.channel,
                        channel_point(point, layout.origin)?,
                    )? {
                        ChannelQuery::Loaded(value) if value >= 0 => value,
                        _ => return Err(invalid("圧力の保存値またはcoverage")),
                    };
                    samples.try_reserve(1).map_err(allocation)?;
                    samples.push(Sample {
                        point,
                        mass: material.mass_per_cell(),
                        pressure,
                        surface: false,
                    });
                }
            }
            samples.sort_unstable_by_key(|sample| (sample.point.x, sample.point.y));
            // 各cellの辺を固定順で作り、反復中の読込や動的な列挙順へ依存させません。
            let mut edges = Vec::new();
            edges.try_reserve_exact(samples.len()).map_err(allocation)?;
            for sample in &samples {
                let mut adjacent = [None; 4];
                let mut surface = false;
                for (edge, (dx, dy)) in NEIGHBORS.into_iter().enumerate() {
                    let Some(point) = neighbor(sample.point, dx, dy) else {
                        continue;
                    };
                    if let Ok(index) = find(&samples, point) {
                        adjacent[edge] = Some(index);
                    } else if active_empty(editor.world, layer, point)? {
                        surface = true;
                    }
                }
                edges.push((adjacent, surface));
            }
            for (sample, (_, surface)) in samples.iter_mut().zip(&edges) {
                sample.surface = *surface;
            }
            let mut seen = Vec::new();
            seen.try_reserve_exact(samples.len()).map_err(allocation)?;
            seen.resize(samples.len(), false);
            let mut anchors = Vec::new();
            anchors
                .try_reserve_exact(samples.len())
                .map_err(allocation)?;
            anchors.resize(samples.len(), None);
            let mut queue = Vec::new();
            queue.try_reserve_exact(samples.len()).map_err(allocation)?;
            let mut report = PressureRelaxationReport {
                cells: samples.len(),
                ..Default::default()
            };
            for root in 0..samples.len() {
                if seen[root] {
                    continue;
                }
                queue.clear();
                queue.push(root);
                seen[root] = true;
                let mut cursor = 0;
                let mut total = 0i128;
                let mut open = false;
                while cursor < queue.len() {
                    let index = queue[cursor];
                    cursor += 1;
                    total += i128::from(samples[index].pressure);
                    open |= samples[index].surface;
                    for next in edges[index].0.into_iter().flatten() {
                        if !seen[next] {
                            seen[next] = true;
                            queue.push(next);
                        }
                    }
                }
                if open {
                    report.open_components += 1;
                } else {
                    report.closed_components += 1;
                    anchors[root] = Some((total / queue.len() as i128) as i64);
                }
            }
            let mut old = Vec::new();
            old.try_reserve_exact(samples.len()).map_err(allocation)?;
            old.extend(samples.iter().map(|sample| sample.pressure));
            let mut next = Vec::new();
            next.try_reserve_exact(samples.len()).map_err(allocation)?;
            next.resize(samples.len(), 0);
            for _ in 0..config.iterations {
                for (index, sample) in samples.iter().enumerate() {
                    if sample.surface {
                        next[index] = 0;
                        continue;
                    }
                    if let Some(value) = anchors[index] {
                        next[index] = value;
                        continue;
                    }
                    let mut sum = 0i128;
                    let mut count = 0;
                    for other in edges[index].0.into_iter().flatten() {
                        let density =
                            (i128::from(sample.mass) + i128::from(samples[other].mass)) / 2;
                        sum += i128::from(old[other])
                            + density
                                * i128::from(config.gravity_q16)
                                * (i128::from(sample.point.y) - i128::from(samples[other].point.y));
                        count += 1;
                    }
                    next[index] = if count == 0 {
                        old[index]
                    } else {
                        i64::try_from((sum / count).max(0)).map_err(|_| invalid("圧力のi64上限"))?
                    };
                }
                std::mem::swap(&mut old, &mut next);
            }
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(samples.len())
                .map_err(allocation)?;
            for (sample, pressure) in samples.into_iter().zip(old) {
                updates.push((channel_point(sample.point, layout.origin)?, pressure));
            }
            let default = editor
                .world
                .validate_channel_updates(config.channel, &updates)?;
            // 最終容量だけを予約し、既定値へ戻る出力を先に反映します。位置の走査順で
            // 不要な同時確保を要求しません。既定値が0以外のchannelにも同じ規則を適用します。
            for clearing in [true, false] {
                for &(point, pressure) in &updates {
                    if (pressure == default) == clearing {
                        editor.set_channel(config.channel, point, pressure)?;
                    }
                }
            }
            Ok(report)
        })
    }
}
fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("圧力channel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("圧力channel垂直座標"))?,
    })
}
fn neighbor(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}
fn find(samples: &[Sample], point: CellPoint) -> Result<usize, usize> {
    samples.binary_search_by_key(&(point.x, point.y), |sample| {
        (sample.point.x, sample.point.y)
    })
}
fn active_empty(world: &World, layer: LayerId, point: CellPoint) -> EngineResult<bool> {
    if !world
        .chunks
        .get(&ChunkCoord::at(point))
        .is_some_and(|chunk| chunk.active)
    {
        return Ok(false);
    }
    Ok(
        matches!(world.cell(layer,point)?,CellQuery::Loaded(cell) if cell.block.is_none() && cell.core.material==super::MaterialId::AIR),
    )
}
