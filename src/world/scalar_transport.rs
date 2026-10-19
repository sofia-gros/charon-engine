//! 粉体・液体・気体の単一走査による参照輸送。
//!
//! 更新対象はSimulate leaseで活動している地形だけです。表示範囲、描画速度、
//! workerの完了順には依存しません。後から追加する圧力処理や最適化の比較基準として、
//! この局所移動phaseの走査順と予約規則を明示します。

use super::{CellQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, WorldEdit, invalid};
use crate::foundation::{EngineError, EngineResult, error::allocation, grid::CellPoint};

/// 一回の局所輸送phaseの診断。圧力、熱、反応の実行回数は含みません。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialTransportReport {
    /// phase開始時に活動領域から列挙した、block以外の非空cell数。
    pub candidates: usize,
    /// 原子的交換を行った回数。異物質交換も一回として数えます。
    pub exchanges: usize,
}

impl WorldEdit<'_> {
    /// 活動中の指定layerに対し、標準の局所輸送を一度行います。
    ///
    /// 下から上へ、同じ行はWorld tickの偶奇で左右を交互に走査します。
    /// 粉体は下・斜め下、液体は下・横、気体は上・斜め上・横を試します。
    /// 交換の両端を予約するため、同じ物質はこの呼出内で二度動きません。
    /// 未読込・活動外・World境界・blockを移動先に使いません。
    ///
    /// `candidate_limit`はこのphaseで保持できる候補数です。予約scratchには活動chunk
    /// 一つにつき4096bitを使います。候補数の超過では
    /// 一部だけ進めず取引全体を失敗させます。探索は活動chunkの4096cellを読み、
    /// 書込上限にはWorldLimitsのedit_cellsも適用します。大規模な分割処理には
    /// 別の保存可能なJobが必要で、この呼出が残作業を黙って次tickへ送りません。
    ///
    /// World時計は進めません。Runtimeでは作者Systemを一回のphaseに登録し、
    /// 時計とbarrierの管理をRuntimeへ任せます。独自の動きにはexchange_cellsや
    /// transport_pathを使え、標準の重力や密度則は自動適用されません。
    pub fn transport_materials(
        &mut self,
        layer: LayerId,
        candidate_limit: usize,
    ) -> EngineResult<MaterialTransportReport> {
        self.run(|editor| {
            let points = editor.material_candidates(layer, candidate_limit)?;
            let left_first = editor.world.tick().is_multiple_of(2);
            let mut report = MaterialTransportReport {
                candidates: points.len(),
                exchanges: 0,
            };
            let mut reserved = super::transport::CellReservations::new(editor.world)?;
            let side = if left_first { -1 } else { 1 };
            for source in points {
                if reserved.contains(source) {
                    continue;
                }
                let core = editor.loaded(layer, source)?.core;
                let material = editor
                    .world
                    .content
                    .material(core.material)?
                    .ok_or_else(|| invalid("輸送元の素材が空"))?;
                let base = material.base();
                let mass = material.mass_per_cell();
                let choices: &[(i32, i32)] = match base {
                    MaterialBase::Solid => &[],
                    MaterialBase::Powder => &[(0, 1), (side, 1), (-side, 1)],
                    MaterialBase::Liquid => &[(0, 1), (side, 0), (-side, 0)],
                    MaterialBase::Gas => &[(0, -1), (side, -1), (-side, -1), (side, 0), (-side, 0)],
                };
                for &(dx, dy) in choices {
                    let Some(destination) = offset(source, dx, dy) else {
                        continue;
                    };
                    if reserved.contains(destination)
                        || !editor.active_transport_cell(layer, destination)?
                    {
                        continue;
                    }
                    let target = editor.loaded(layer, destination)?.core;
                    let movable = if target.material == MaterialId::AIR {
                        true
                    } else {
                        let other = editor
                            .world
                            .content
                            .material(target.material)?
                            .ok_or_else(|| invalid("交換先の素材が空"))?;
                        // 下降する重い物質と、上昇する軽い気体だけが異物質を押し退けます。
                        // 固体・粉体を流体の密度交換で崩したり、横移動で同じ物質を交換しません。
                        matches!(other.base(), MaterialBase::Liquid | MaterialBase::Gas)
                            && target.material != core.material
                            && ((dy > 0 && mass > other.mass_per_cell())
                                || (dy < 0 && mass < other.mass_per_cell()))
                    };
                    if !movable {
                        continue;
                    }
                    if dx != 0 && dy != 0 {
                        // 二つの直交隣接点が共に塞がっている角は通り抜けません。
                        let horizontal = offset(source, dx, 0);
                        let vertical = offset(source, 0, dy);
                        if !editor.empty_transport_cell(layer, horizontal)?
                            && !editor.empty_transport_cell(layer, vertical)?
                        {
                            continue;
                        }
                    }
                    editor.exchange_cells(layer, source, destination)?;
                    for point in [source, destination] {
                        reserved.insert(point)?;
                    }
                    report.exchanges += 1;
                    break;
                }
            }
            Ok(report)
        })
    }
    pub(super) fn material_candidates(
        &self,
        layer: LayerId,
        candidate_limit: usize,
    ) -> EngineResult<Vec<CellPoint>> {
        if !self.world.content.layer(layer)?.simulated {
            return Err(invalid("物質輸送の対象layerはsimulatedでなければならない"));
        }
        let mut points = Vec::new();
        for (coord, chunk) in &self.world.chunks {
            if !chunk.active {
                continue;
            }
            let page = &chunk.layers[layer.0 as usize];
            let origin = coord.origin()?;
            for (index, core) in page.cells.iter().enumerate() {
                if core.material == MaterialId::AIR
                    || page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                {
                    continue;
                }
                if points.len() >= candidate_limit {
                    return Err(EngineError::capacity(
                        "物質輸送の候補数",
                        points.len() as u64 + 1,
                        candidate_limit as u64,
                    ));
                }
                points.try_reserve(1).map_err(allocation)?;
                points.push(CellPoint {
                    x: origin.x + (index % 64) as i32,
                    y: origin.y + (index / 64) as i32,
                });
            }
        }
        let left_first = self.world.tick().is_multiple_of(2);
        points.sort_unstable_by(|a, b| {
            b.y.cmp(&a.y).then_with(|| {
                if left_first {
                    a.x.cmp(&b.x)
                } else {
                    b.x.cmp(&a.x)
                }
            })
        });
        Ok(points)
    }
    fn active_transport_cell(&self, layer: LayerId, point: CellPoint) -> EngineResult<bool> {
        if !self
            .world
            .chunks
            .get(&ChunkCoord::at(point))
            .is_some_and(|chunk| chunk.active)
        {
            return Ok(false);
        }
        Ok(matches!(self.world.cell(layer,point)?,CellQuery::Loaded(cell) if cell.block.is_none()))
    }
    fn empty_transport_cell(&self, layer: LayerId, point: Option<CellPoint>) -> EngineResult<bool> {
        let Some(point) = point else {
            return Ok(false);
        };
        Ok(self.active_transport_cell(layer, point)?
            && self.loaded(layer, point)?.core.material == MaterialId::AIR)
    }
}
fn offset(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}
