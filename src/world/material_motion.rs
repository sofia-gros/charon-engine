//! 物質トークンの速度・未消化の端数と、整数格子上の連続経路検査。
//!
//! このphaseは作者が指定した速度を積分し、空cellを通る経路を検査します。
//! 力の積分、密度交換、一般形状の衝突queryを暗黙には実行しません。

use super::{CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialId, WorldEdit, invalid};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
};

const SCALE: i128 = 65536;

/// 一つの物質と一緒に輸送・保存する速度と未消化の端数です。
///
/// 原点は占有cellで、端数はまだ整数cellの移動に使っていない変位です。
/// 表示用の連続座標や一般形状のTransformとは区別します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialMotion {
    /// 各軸の符号付きQ16 cell/tick。右・下を正とします。
    pub velocity_q16: [i64; 2],
    /// 各軸の未消化Q16変位。絶対値65536未満で、負方向も0方向丸めで蓄積します。
    pub remainder_q16: [i32; 2],
}
impl GridValue for MaterialMotion {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn validate(self) -> EngineResult<()> {
        if self
            .remainder_q16
            .iter()
            .any(|value| !(-65535..=65535).contains(value))
        {
            return Err(invalid("物質移動の端数範囲"));
        }
        Ok(())
    }
}
impl StateCodec for MaterialMotion {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        for value in self.velocity_q16 {
            out.i64(value)?;
        }
        for value in self.remainder_q16 {
            out.i32(value)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let result = Self {
            velocity_q16: [input.i64()?, input.i64()?],
            remainder_q16: [input.i32()?, input.i32()?],
        };
        result.validate()?;
        Ok(result)
    }
}

/// 速度輸送phaseの明示的な入力と仕事量の上限です。
/// 同じsimulation profileでは同じ設定を使い、作者が設定を保存・版管理します。
#[derive(Clone, Copy, Debug)]
pub struct MaterialMotionConfig {
    /// 対象layerへ所属するMaterialMotionの1対1物質channel。
    pub channel: StableKey,
    /// phase開始時に列挙できる非block・非空cell数。
    pub candidate_limit: usize,
    /// 一つのトークンが各軸で移動できる整数距離。標準の局所設定では8を使います。
    /// 超過は速度を丸めず容量エラーとします。大規模な継続Jobとは別の一括処理です。
    pub max_distance: u32,
    /// 全トークンを通して検査できる格子境界の件数。斜めの同時交差は一件です。
    pub step_limit: usize,
}

/// 速度輸送の診断。失敗したphaseの部分結果は返しません。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialMotionReport {
    /// 列挙した非block・非空cell数。
    pub candidates: usize,
    /// 少なくとも一つのcell境界を越えたトークン数。
    pub moved: usize,
    /// 実際に越えた境界数。斜めの同時交差は一件です。
    pub crossings: usize,
    /// 検査した境界候補数。塞がっていた最後の候補も含みます。
    pub checked_steps: usize,
    /// 占有cell・block・World境界で止めたトークン数。速度と端数を0へ戻します。
    pub collisions: usize,
    /// 未読込・非活動域で止まったトークン数。速度と計算済み端数は保持します。
    pub waiting_for_region: usize,
    /// 先行トークンの予約で延期した数。始点予約と経路途中の予約を含みます。
    pub reservation_conflicts: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Passage {
    Free,
    Collision,
    Waiting,
    Reserved,
}

impl WorldEdit<'_> {
    /// 活動中の物質の指定速度を一度積分し、途中のcellを検査してから輸送します。
    ///
    /// 下から上、同一行はtickの左右交互順です。端数を足した速度を0方向へ丸め、
    /// 得た整数変位のcell中心間線分をDDAで検査します。角の同時交差ではX側、Y側、
    /// 斜め先の順に検査し、両直交cellが空であることを要求します。
    /// 始点・全通過点・角の検査点を予約し、同phaseの再移動・交差を防ぎます。
    ///
    /// 占有物との密度交換は別の局所輸送phaseです。この経路は空cellだけを通り、
    /// 物理的な障害で速度を全軸0へ戻します。未準備域・予約競合では速度を保持し、
    /// 未実行の整数変位を借金として蓄積しません。端数はそのtickの積分結果です。
    /// 容量・値域エラーは先行する移動と端数更新も含めWorld取引全体を破棄します。
    /// 力と時計は呼出側が管理します。検査済み経路を逆順にtransport_pathへ渡し、
    /// 全物質channelを一括で追従させ、途中の不要な同時確保も避けます。
    pub fn transport_material_motion(
        &mut self,
        layer: LayerId,
        config: MaterialMotionConfig,
    ) -> EngineResult<MaterialMotionReport> {
        self.run(|editor| {
            let layer_key = editor.world.content.layer(layer)?.key;
            if editor
                .world
                .channel_material_layer::<MaterialMotion>(config.channel)?
                != Some(layer_key)
            {
                return Err(invalid("速度channelは対象layerの物質に所属する必要がある"));
            }
            if config.max_distance == 0 {
                return Err(invalid("速度輸送の移動距離上限"));
            }
            let origin = editor
                .world
                .channel_layout::<MaterialMotion>(config.channel)?
                .origin;
            let points = editor.material_candidates(layer, config.candidate_limit)?;
            let mut report = MaterialMotionReport {
                candidates: points.len(),
                ..Default::default()
            };
            let mut reserved = super::transport::CellReservations::new(editor.world)?;
            for source in points {
                if reserved.contains(source) {
                    report.reservation_conflicts += 1;
                    continue;
                }
                let coordinate = channel_point(source, origin)?;
                let mut motion = match editor
                    .world
                    .channel_value::<MaterialMotion>(config.channel, coordinate)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("速度channelのcoverage")),
                };
                motion.validate()?;
                let previous = motion;
                let mut displacement = [0i64; 2];
                for (axis, distance) in displacement.iter_mut().enumerate() {
                    let total = i128::from(motion.velocity_q16[axis])
                        + i128::from(motion.remainder_q16[axis]);
                    let integer = total / SCALE;
                    if integer.abs() > i128::from(config.max_distance) {
                        return Err(EngineError::capacity(
                            "速度輸送の軸移動距離",
                            integer.unsigned_abs() as u64,
                            config.max_distance.into(),
                        ));
                    }
                    *distance = integer as i64;
                    motion.remainder_q16[axis] = (total % SCALE) as i32;
                }
                let mut path = Vec::new();
                let mut guards = Vec::new();
                path.try_reserve(1).map_err(allocation)?;
                path.push(source);
                let counts = [
                    displacement[0].unsigned_abs(),
                    displacement[1].unsigned_abs(),
                ];
                let signs = [
                    displacement[0].signum() as i32,
                    displacement[1].signum() as i32,
                ];
                let mut progressed = [0u64; 2];
                let mut current = source;
                while progressed != counts {
                    if report.checked_steps >= config.step_limit {
                        return Err(EngineError::capacity(
                            "速度輸送の境界検査数",
                            report.checked_steps as u64 + 1,
                            config.step_limit as u64,
                        ));
                    }
                    report.checked_steps += 1;
                    let horizontal = u128::from(2 * progressed[0] + 1) * u128::from(counts[1]);
                    let vertical = u128::from(2 * progressed[1] + 1) * u128::from(counts[0]);
                    let move_x = progressed[0] < counts[0]
                        && (progressed[1] == counts[1] || horizontal <= vertical);
                    let move_y = progressed[1] < counts[1]
                        && (progressed[0] == counts[0] || vertical <= horizontal);
                    let dx = if move_x { signs[0] } else { 0 };
                    let dy = if move_y { signs[1] } else { 0 };
                    let destination = offset(current, dx, dy);
                    let mut passage = Passage::Free;
                    if move_x && move_y {
                        for point in [offset(current, dx, 0), offset(current, 0, dy)] {
                            passage = editor.motion_passage(layer, point, &reserved)?;
                            if passage != Passage::Free {
                                break;
                            }
                        }
                    }
                    if passage == Passage::Free {
                        passage = editor.motion_passage(layer, destination, &reserved)?;
                    }
                    if passage != Passage::Free {
                        match passage {
                            Passage::Collision => {
                                motion = MaterialMotion::default();
                                report.collisions += 1;
                            }
                            Passage::Waiting => report.waiting_for_region += 1,
                            Passage::Reserved => report.reservation_conflicts += 1,
                            Passage::Free => unreachable!(),
                        }
                        break;
                    }
                    if move_x && move_y {
                        guards.try_reserve(2).map_err(allocation)?;
                        guards.push(offset(current, dx, 0).expect("角の検査済み"));
                        guards.push(offset(current, 0, dy).expect("角の検査済み"));
                    }
                    current = destination.expect("移動先の検査済み");
                    path.try_reserve(1).map_err(allocation)?;
                    path.push(current);
                    progressed[0] += u64::from(move_x);
                    progressed[1] += u64::from(move_y);
                }
                if path.len() > 1 {
                    for &point in path.iter().chain(&guards) {
                        reserved.insert(point)?;
                    }
                }
                if path.len() > 1 {
                    // 逆向きの循環置換により、始点のトークンを末尾へ直接送り、
                    // 通過した空cellの旧値は一つずつ始点側へ戻します。
                    path.reverse();
                    editor.transport_path(layer, &path)?;
                }
                if motion != previous {
                    editor.set_channel(config.channel, channel_point(current, origin)?, motion)?;
                }
                if path.len() > 1 {
                    report.moved += 1;
                    report.crossings += path.len() - 1;
                }
            }
            Ok(report)
        })
    }
    fn motion_passage(
        &self,
        layer: LayerId,
        point: Option<CellPoint>,
        reserved: &super::transport::CellReservations,
    ) -> EngineResult<Passage> {
        let Some(point) = point else {
            return Ok(Passage::Collision);
        };
        if reserved.contains(point) {
            return Ok(Passage::Reserved);
        }
        let cell = match self.world.cell(layer, point)? {
            CellQuery::WorldBoundary => return Ok(Passage::Collision),
            CellQuery::Loaded(cell) => cell,
            _ => return Ok(Passage::Waiting),
        };
        if !self
            .world
            .chunks
            .get(&ChunkCoord::at(point))
            .is_some_and(|chunk| chunk.active)
        {
            return Ok(Passage::Waiting);
        }
        if cell.block.is_some() || cell.core.material != MaterialId::AIR {
            return Ok(Passage::Collision);
        }
        Ok(Passage::Free)
    }
}
fn offset(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}
fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("速度channel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("速度channel垂直座標"))?,
    })
}
