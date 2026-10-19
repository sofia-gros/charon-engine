//! 保存可能なMaterialMotion channelに対する標準・独自の力積分入口。
//!
//! 速度更新とDDA輸送を一つのWorld取引で実行できます。作者providerはWorldの
//! 読取専用状態を参照して独自式へ完全置換でき、標準式を暗黙に二重適用しません。

use super::{
    CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialBase, MaterialDef, MaterialMotion,
    MaterialMotionConfig, MaterialMotionReport, MaterialParticleHandle, MaterialParticleReport,
    World, WorldEdit, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    error::allocation,
    grid::{CellPoint, GridValue},
};

const ONE: i128 = 65536;

/// 力の対象cellの重力側へ隣接する物質。未読込は別の待機として扱います。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialForceNeighbor {
    /// 隣接素材の標準分類。
    pub base: MaterialBase,
    /// 隣接素材の1cell当たりのゲーム質量。
    pub mass_per_cell: u32,
    /// block個体の一部を占有しているか。
    pub block: bool,
}

/// providerへ渡す不変の入力。必要に応じて保存済みWorld作者状態も読めます。
pub struct MaterialForceInput<'a> {
    /// 候補開始時点の読み取り専用World。
    pub world: &'a World,
    /// セルの場合はNone、正規粒子ならWorld所属の世代付きhandle。
    /// callbackは対応する作者payloadをWorldから型付きで参照できます。
    pub particle: Option<MaterialParticleHandle>,
    /// Worldの対象layer。
    pub layer: LayerId,
    /// 候補のcell位置。
    pub point: CellPoint,
    /// 素材定義。標準以外の物性もContentに保持できます。
    pub material: &'a MaterialDef,
    /// 移動channelが所有する現在の速度と端数。
    pub motion: MaterialMotion,
    /// Q16 cell/tick²の重力ベクトル。符号は右・下が正。
    pub gravity_q16: [i64; 2],
    /// 重力側にある隣接素材。World境界はNoneです。
    pub neighbor: Option<MaterialForceNeighbor>,
}

/// 作者Providerが候補ごとに参照する空間入力の矩形境界です。
///
/// `integrate`から読むWorldのcell/channelに加え、入力の隣接素材など結果へ影響する
/// 全ての空間依存を候補cellの周囲へ対称に広げる半径として宣言します。実際の読み取りが
/// 矩形の一部だけでも全体を監視する保守的な境界です。上限は各軸64 cellで、
/// それを超える広域依存はsleepへ登録せず参照経路を使います。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialForceDependencyStencil {
    radius_x: u8,
    radius_y: u8,
}
impl MaterialForceDependencyStencil {
    /// 依存矩形の半径を構築します。各軸は0〜64 cellです。
    pub fn new(radius_x: u8, radius_y: u8) -> EngineResult<Self> {
        if radius_x > 64 || radius_y > 64 {
            return Err(invalid("力学Providerのsleep依存半径"));
        }
        Ok(Self { radius_x, radius_y })
    }
    /// 水平方向の依存半径。
    pub fn radius_x(self) -> u8 {
        self.radius_x
    }
    /// 垂直方向の依存半径。
    pub fn radius_y(self) -> u8 {
        self.radius_y
    }
}

/// 書換え可能な力学処理。キー・versionを作者のsim設定とともに保存してください。
///
/// callbackはWorldの参照だけを受け取り、結果を返します。追加の作者状態は
/// World StateStoreなどへ保存し、callback自身の隠れた時間状態に依存させません。
pub trait MaterialForceProvider: Send + Sync {
    /// 正規の実装キー。
    fn key(&self) -> StableKey;
    /// 係数・アルゴリズム・読み取りstencilを変更したら更新する版。
    fn version(&self) -> u32;
    /// 一候補のQ16速度と端数の更新。別のcellを書き換えません。
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion>;
    /// 独自sleepへ参加する場合のcell依存境界。既定は常に参照実行です。
    fn sleep_stencil(&self) -> Option<MaterialForceDependencyStencil> {
        None
    }
    /// 保存済み作者状態・tick・外部入力を含め、全候補が次回も静止する条件です。
    ///
    /// `true`は「現在のactive候補のintegrateが速度・端数を変えず、宣言stencil内の
    /// cellまたは作者状態の変更がない間、その結果が続く」というProviderの保証です。
    /// この関数はsleep中も毎tick一度呼ばれます。未実装・未証明なら`false`を返します。
    fn sleep_invariant(
        &self,
        _world: &World,
        _layer: LayerId,
        _force: MaterialForceConfig,
        _motion: MaterialMotionConfig,
    ) -> EngineResult<bool> {
        Ok(false)
    }

    /// 正規粒子の静止sleepへ参加するProviderのtick不変条件です。
    ///
    /// `true`は、現在の対象粒子が静止しているとき次回も速度を0のまま保つことを
    /// 保証します。chunk編集は標準trackerが全てwakeし、World状態や外部入力への
    /// 依存はProvider自身がこの判定に含めます。既定は参照実行です。
    fn particle_sleep_invariant(
        &self,
        _world: &World,
        _layer: LayerId,
        _force: MaterialParticleForceConfig,
    ) -> EngineResult<bool> {
        Ok(false)
    }
}

/// 標準Q16参照式。独自providerに置き換えれば標準式は適用されません。
pub struct StandardMaterialForces;
impl MaterialForceProvider for StandardMaterialForces {
    fn key(&self) -> StableKey {
        StableKey::parse("engine:standard-material-forces").expect("固定された標準providerキー")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        let properties = input.material.motion_properties();
        properties.validate()?;
        let mut updated = input.motion;
        let mut velocity = input.motion.velocity_q16.map(i128::from);
        for (axis, component) in velocity.iter_mut().enumerate() {
            // 抗力は速度の既存符号を変えず、0方向への整数除算で丸めます。
            *component = *component * (ONE - i128::from(properties.drag_q16)) / ONE;
            *component += i128::from(input.gravity_q16[axis])
                * i128::from(properties.gravity_scale_q16)
                / ONE;
        }
        let gravity_axis =
            usize::from(input.gravity_q16[1].unsigned_abs() >= input.gravity_q16[0].unsigned_abs());
        if let Some(neighbor) = input.neighbor {
            if matches!(neighbor.base, MaterialBase::Liquid | MaterialBase::Gas)
                && neighbor.mass_per_cell > input.material.mass_per_cell()
            {
                // 密度差の比を有理のまま計算し、重力と逆向きの浮力へ換算します。
                let difference =
                    i128::from(neighbor.mass_per_cell - input.material.mass_per_cell());
                let buoyancy = i128::from(input.gravity_q16[gravity_axis])
                    * i128::from(properties.buoyancy_scale_q16)
                    * difference
                    / (ONE * i128::from(neighbor.mass_per_cell));
                velocity[gravity_axis] -= buoyancy;
            }
            if neighbor.base == MaterialBase::Solid || neighbor.block {
                let tangent = 1 - gravity_axis;
                velocity[tangent] =
                    velocity[tangent] * (ONE - i128::from(properties.friction_q16)) / ONE;
            }
        }
        for (axis, component) in velocity.into_iter().enumerate() {
            updated.velocity_q16[axis] =
                i64::try_from(component).map_err(|_| invalid("物性速度Q16の値域"))?;
        }
        updated.validate()?;
        Ok(updated)
    }

    fn particle_sleep_invariant(
        &self,
        _world: &World,
        _layer: LayerId,
        force: MaterialParticleForceConfig,
    ) -> EngineResult<bool> {
        // 静止粒子の集合と速度は証明作成時だけ走査し、sleep中のtickごとに
        // 全粒子を走査しません。粒子状態の編集は所属chunkのrevisionでwakeします。
        Ok(force.gravity_q16 == [0, 0])
    }
}

/// 標準・独自providerを共通の候補順で実行するための設定。
#[derive(Clone, Copy, Debug)]
pub struct MaterialForceConfig {
    /// 対象layerに所有された1対1のMaterialMotion channel。
    pub channel: StableKey,
    /// 重力ベクトル。方向も作者が決めます。
    pub gravity_q16: [i64; 2],
    /// 対象tickに列挙する活動物質の最大数。
    pub candidate_limit: usize,
    /// 絶対Q16速度の上限。飽和させず、超過すると全World取引を失敗させます。
    pub max_velocity_q16: i64,
}

/// 力積分の診断。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialForceReport {
    /// 処理対象の非空・非blockセル数。
    pub candidates: usize,
    /// 重力側の未読込regionにより次tickへ延期した件数。
    pub waiting_for_region: usize,
    /// MaterialMotionが変化した件数。
    pub changed: usize,
}

/// 力の更新と連続DDA輸送の同一tick統合診断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialForceMotionReport {
    /// 力積分。
    pub force: MaterialForceReport,
    /// 速度による経路輸送。
    pub motion: MaterialMotionReport,
}

/// 正規物質粒子用の標準・独自の速度積分設定。
#[derive(Clone, Copy, Debug)]
pub struct MaterialParticleForceConfig {
    /// Q16 cell/tick²の重力ベクトル。
    pub gravity_q16: [i64; 2],
    /// このphaseで積分する最大活動粒子数。超過したら丸ごと失敗させます。
    pub candidate_limit: usize,
    /// Q16速度の許容絶対値。飽和させずrollbackします。
    pub max_velocity_q16: i64,
}

/// 正規物質粒子への力積分に関する処理結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialParticleForceReport {
    /// activeなresident粒子候補数。
    pub candidates: usize,
    /// 必須の隣接regionが未読込で更新を延期した粒子数。
    pub waiting_for_region: usize,
    /// 速度更新数。
    pub changed: usize,
}

/// 正規粒子の力積分と整数DDAを一つの候補で実行した結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialParticleForceStepReport {
    /// 力の更新。
    pub force: MaterialParticleForceReport,
    /// 整数DDA・予約・着地の結果。
    pub particles: MaterialParticleReport,
}

impl WorldEdit<'_> {
    /// 明示されたproviderだけを、活動素材の速度channelへ適用します。
    ///
    /// 複数更新は同じ候補へまとめ、providerがエラーを返した場合や速度の上限を
    /// 超えた場合は先行更新まで巻き戻します。未読込の重力側は空と仮定せず待機。
    /// 物質channelの追従・DDA・粒子stepはこの入口では実行しません。
    pub fn integrate_material_forces(
        &mut self,
        layer: LayerId,
        config: MaterialForceConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialForceReport> {
        self.integrate_material_forces_with_reservations(layer, config, provider, None)
    }

    pub(super) fn integrate_material_forces_reserved(
        &mut self,
        layer: LayerId,
        config: MaterialForceConfig,
        provider: &dyn MaterialForceProvider,
        reservations: &super::transport::CellReservations,
    ) -> EngineResult<MaterialForceReport> {
        self.integrate_material_forces_with_reservations(
            layer,
            config,
            provider,
            Some(reservations),
        )
    }

    fn integrate_material_forces_with_reservations(
        &mut self,
        layer: LayerId,
        config: MaterialForceConfig,
        provider: &dyn MaterialForceProvider,
        reservations: Option<&super::transport::CellReservations>,
    ) -> EngineResult<MaterialForceReport> {
        self.run(|editor| {
            if provider.version() == 0 || config.max_velocity_q16 <= 0 {
                return Err(invalid("物性provider版・速度上限"));
            }
            let layer_key = editor.world.content.layer(layer)?.key;
            if editor
                .world
                .channel_material_layer::<MaterialMotion>(config.channel)?
                != Some(layer_key)
            {
                return Err(invalid("物性速度channelの所有layer"));
            }
            let origin = editor
                .world
                .channel_layout::<MaterialMotion>(config.channel)?
                .origin;
            let points = editor.material_candidates(layer, config.candidate_limit)?;
            let mut report = MaterialForceReport::default();
            // 全候補のprovider評価を同一の読取りスナップショットで終えてから
            // 更新を公開します。候補順による隣接速度の参照ぶれを防ぎます。
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(points.len())
                .map_err(allocation)?;
            for point in points {
                if reservations.is_some_and(|reserved| reserved.contains(point)) {
                    continue;
                }
                report.candidates += 1;
                let cell = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(value) => value,
                    _ => return Err(invalid("物性候補のcoverage")),
                };
                let material = editor
                    .world
                    .content
                    .material(cell.core.material)?
                    .ok_or_else(|| invalid("物性候補の空素材"))?;
                let coord = CellPoint {
                    x: point
                        .x
                        .checked_sub(origin.x)
                        .ok_or_else(|| invalid("物性channel水平座標"))?,
                    y: point
                        .y
                        .checked_sub(origin.y)
                        .ok_or_else(|| invalid("物性channel垂直座標"))?,
                };
                let previous = match editor
                    .world
                    .channel_value::<MaterialMotion>(config.channel, coord)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("物性速度channelのcoverage")),
                };
                let axis = usize::from(
                    config.gravity_q16[1].unsigned_abs() >= config.gravity_q16[0].unsigned_abs(),
                );
                let sign = config.gravity_q16[axis].signum() as i32;
                let neighbor_point = if config.gravity_q16 == [0, 0] {
                    None
                } else if axis == 0 {
                    point
                        .x
                        .checked_add(sign)
                        .map(|x| CellPoint { x, y: point.y })
                } else {
                    point
                        .y
                        .checked_add(sign)
                        .map(|y| CellPoint { x: point.x, y })
                };
                let neighbor = match neighbor_point {
                    None => None,
                    Some(position) => match editor.world.cell(layer, position)? {
                        CellQuery::WorldBoundary => None,
                        CellQuery::Loaded(value) => editor
                            .world
                            .content
                            .material(value.core.material)?
                            .map(|material| MaterialForceNeighbor {
                                base: material.base(),
                                mass_per_cell: material.mass_per_cell(),
                                block: value.block.is_some(),
                            }),
                        _ => {
                            report.waiting_for_region += 1;
                            continue;
                        }
                    },
                };
                let updated = provider.integrate(MaterialForceInput {
                    world: editor.world,
                    particle: None,
                    layer,
                    point,
                    material,
                    motion: previous,
                    gravity_q16: config.gravity_q16,
                    neighbor,
                })?;
                updated.validate()?;
                if updated
                    .velocity_q16
                    .iter()
                    .any(|speed| speed.unsigned_abs() > config.max_velocity_q16 as u64)
                {
                    return Err(EngineError::capacity(
                        "物性速度上限",
                        updated
                            .velocity_q16
                            .iter()
                            .map(|v| v.unsigned_abs())
                            .max()
                            .unwrap_or(0),
                        config.max_velocity_q16 as u64,
                    ));
                }
                if updated != previous {
                    updates.push((coord, updated));
                    report.changed += 1;
                }
            }
            for (coord, updated) in updates {
                editor.set_channel(config.channel, coord, updated)?;
            }
            Ok(report)
        })
    }

    /// 力を一度だけ積分してから既存DDAを一度実行します。
    ///
    /// 一体化した取引で失敗をrollbackし、物質の全channel追従を既存の正規経路
    /// に任せます。広域移動Job、dirty/sleep、正規粒子への力は別の実装単位です。
    pub fn step_material_force_motion(
        &mut self,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialForceMotionReport> {
        self.run(|editor| {
            if force.channel != motion.channel {
                return Err(invalid("物性とDDAの速度channelが一致しない"));
            }
            let force = editor.integrate_material_forces(layer, force, provider)?;
            let motion = editor.transport_material_motion(layer, motion)?;
            Ok(MaterialForceMotionReport { force, motion })
        })
    }

    /// 活動regionの正規物質粒子へ、標準または独自Providerの力を一度だけ適用します。
    ///
    /// 粒子の位置は既にQ16で保持するため、セル輸送用の端数channelは使いません。
    /// 戻り値の端数が非0のProviderは未保存値の消失を避けるため拒否します。
    /// 候補をすべて同じWorld状態で評価してから世代付きhandleへ書き戻すので、
    /// callbackは先行粒子の新速度を読みません。未読込隣接域は延期します。
    pub fn integrate_material_particle_forces(
        &mut self,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialParticleForceReport> {
        self.integrate_material_particle_forces_filtered(None, config, provider)
    }

    /// 指定layerだけの正規粒子へ力積分します。他layerの活動粒子の速度は
    /// 更新せず、同一tick内で複数layerのphaseを呼んでも二重積分しません。
    pub fn integrate_material_particle_forces_in_layer(
        &mut self,
        layer: LayerId,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialParticleForceReport> {
        self.integrate_material_particle_forces_filtered(Some(layer), config, provider)
    }

    fn integrate_material_particle_forces_filtered(
        &mut self,
        layer_filter: Option<LayerId>,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialParticleForceReport> {
        self.run(|editor| {
            if config.max_velocity_q16 <= 0 || provider.version() == 0 {
                return Err(invalid("正規粒子の物性速度上限・provider版"));
            }
            if let Some(layer) = layer_filter
                && !editor.world.content.layer(layer)?.simulated
            {
                return Err(invalid("正規粒子の力積分対象はsimulated layerのみ"));
            }
            let mut handles = Vec::new();
            for (handle, particle) in editor.world.material_particles() {
                if layer_filter.is_some_and(|layer| particle.layer() != layer) {
                    continue;
                }
                if !editor.world.content.layer(particle.layer())?.simulated {
                    continue;
                }
                if particle.is_archived() {
                    continue;
                }
                if !editor
                    .world
                    .chunks
                    .get(&ChunkCoord::at(particle.cell()))
                    .is_some_and(|chunk| chunk.active)
                {
                    continue;
                }
                if handles.len() >= config.candidate_limit {
                    return Err(EngineError::capacity(
                        "活動正規粒子の力積分件数",
                        handles.len() as u64 + 1,
                        config.candidate_limit as u64,
                    ));
                }
                handles.try_reserve(1).map_err(allocation)?;
                handles.push(handle);
            }
            let mut report = MaterialParticleForceReport {
                candidates: handles.len(),
                ..Default::default()
            };
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(handles.len())
                .map_err(allocation)?;
            for handle in handles {
                let particle = editor.world.material_particle(handle)?;
                let point = particle.cell();
                let layer = particle.layer();
                let material = editor
                    .world
                    .content
                    .material(particle.core().material)?
                    .ok_or_else(|| invalid("正規粒子の空素材"))?;
                let gravity_axis = usize::from(
                    config.gravity_q16[1].unsigned_abs() >= config.gravity_q16[0].unsigned_abs(),
                );
                let sign = config.gravity_q16[gravity_axis].signum() as i32;
                let adjacent = if config.gravity_q16 == [0, 0] {
                    None
                } else if gravity_axis == 0 {
                    point
                        .x
                        .checked_add(sign)
                        .map(|x| CellPoint { x, y: point.y })
                } else {
                    point
                        .y
                        .checked_add(sign)
                        .map(|y| CellPoint { x: point.x, y })
                };
                let neighbor = match adjacent {
                    None => None,
                    Some(location) => match editor.world.cell(layer, location)? {
                        CellQuery::WorldBoundary => None,
                        CellQuery::Loaded(value) => editor
                            .world
                            .content
                            .material(value.core.material)?
                            .map(|definition| MaterialForceNeighbor {
                                base: definition.base(),
                                mass_per_cell: definition.mass_per_cell(),
                                block: value.block.is_some(),
                            }),
                        _ => {
                            report.waiting_for_region += 1;
                            continue;
                        }
                    },
                };
                let previous = particle.velocity_q16();
                let updated = provider.integrate(MaterialForceInput {
                    world: editor.world,
                    particle: Some(handle),
                    layer,
                    point,
                    material,
                    motion: MaterialMotion {
                        velocity_q16: previous,
                        remainder_q16: [0, 0],
                    },
                    gravity_q16: config.gravity_q16,
                    neighbor,
                })?;
                updated.validate()?;
                if updated.remainder_q16 != [0, 0] {
                    return Err(invalid("正規粒子は別の未保存端数を持たない"));
                }
                if updated
                    .velocity_q16
                    .iter()
                    .any(|speed| speed.unsigned_abs() > config.max_velocity_q16 as u64)
                {
                    return Err(EngineError::capacity(
                        "正規粒子の物性速度上限",
                        updated
                            .velocity_q16
                            .iter()
                            .map(|v| v.unsigned_abs())
                            .max()
                            .unwrap_or(0),
                        config.max_velocity_q16 as u64,
                    ));
                }
                if updated.velocity_q16 != previous {
                    updates.push((handle, updated.velocity_q16));
                    report.changed += 1;
                }
            }
            for (handle, velocity) in updates {
                editor.set_particle_velocity(handle, velocity)?;
            }
            Ok(report)
        })
    }

    /// 正規粒子の力積分・整数DDA・予約・着地を一つのWorld候補で公開します。
    /// providerの失敗や粒子DDA容量不足では、速度更新も含めてrollbackします。
    pub fn step_material_particle_forces(
        &mut self,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
        dda_step_limit: usize,
    ) -> EngineResult<MaterialParticleForceStepReport> {
        self.run(|editor| {
            let force = editor.integrate_material_particle_forces(config, provider)?;
            let particles =
                editor.step_material_particles(config.candidate_limit, dda_step_limit)?;
            Ok(MaterialParticleForceStepReport { force, particles })
        })
    }

    /// 指定layerの粒子だけを力積分・整数DDA・着地まで原子的に更新します。
    /// World全体を動かす既存APIは互換性のため残します。
    pub fn step_material_particle_forces_in_layer(
        &mut self,
        layer: LayerId,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
        dda_step_limit: usize,
    ) -> EngineResult<MaterialParticleForceStepReport> {
        self.run(|editor| {
            let force =
                editor.integrate_material_particle_forces_in_layer(layer, config, provider)?;
            let particles = editor.step_material_particles_in_layer(
                layer,
                config.candidate_limit,
                dda_step_limit,
            )?;
            Ok(MaterialParticleForceStepReport { force, particles })
        })
    }

    pub(super) fn step_material_particle_forces_in_layer_reserved(
        &mut self,
        layer: LayerId,
        config: MaterialParticleForceConfig,
        provider: &dyn MaterialForceProvider,
        dda_step_limit: usize,
        reserved_cells: &super::transport::CellReservations,
    ) -> EngineResult<MaterialParticleForceStepReport> {
        self.run(|editor| {
            let force =
                editor.integrate_material_particle_forces_in_layer(layer, config, provider)?;
            let particles = editor.step_material_particles_in_layer_reserved(
                layer,
                config.candidate_limit,
                dda_step_limit,
                reserved_cells,
            )?;
            Ok(MaterialParticleForceStepReport { force, particles })
        })
    }
}
