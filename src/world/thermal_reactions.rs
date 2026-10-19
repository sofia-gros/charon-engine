//! E06/Q05の整数enthalpy曲線と断熱熱移動を提供します。
//!
//! 熱はWorld状態とは別の正規recordとして扱い、呼出側がWorld StateStoreや
//! material channelへ保存します。solverは呼出側のscratchへ候補を書き、成功を
//! 確認した後でのみそのscratchを所有状態へ移せます。画面・wall clockには依存しません。

use super::{CellCore, CellQuery, ChannelQuery, ChunkCoord, LayerId, WorldEdit, invalid};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    grid::CellPoint,
    grid::GridValue,
};

/// 一つのenthalpy区間。energy範囲は`[energy_start, energy_end)`です。
///
/// 顕熱区間は温度が線形に上昇し、潜熱区間は温度を固定したまま二相の
/// 比率が変化します。隣り合う区間のenergyと温度は連続させます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnthalpyBand {
    /// 指定materialの温度をenergyから線形補間する区間。
    Sensible {
        /// energy区間の開始値。
        energy_start: i64,
        /// energy区間の終了値。含みません。
        energy_end: i64,
        /// 開始温度mK。
        temperature_start_mk: i64,
        /// 終了温度mK。
        temperature_end_mk: i64,
        /// この温度範囲のmaterial。
        material: StableKey,
    },
    /// 指定温度でfrom相からto相へ変化する潜熱区間。
    Latent {
        /// energy区間の開始値。
        energy_start: i64,
        /// energy区間の終了値。含みません。
        energy_end: i64,
        /// 相変化中の固定温度mK。
        temperature_mk: i64,
        /// 低energy側の相。
        from: StableKey,
        /// 高energy側の相。
        to: StableKey,
    },
}

impl EnthalpyBand {
    fn energy_bounds(self) -> (i64, i64) {
        match self {
            Self::Sensible {
                energy_start,
                energy_end,
                ..
            }
            | Self::Latent {
                energy_start,
                energy_end,
                ..
            } => (energy_start, energy_end),
        }
    }
    fn temperature_bounds(self) -> (i64, i64) {
        match self {
            Self::Sensible {
                temperature_start_mk,
                temperature_end_mk,
                ..
            } => (temperature_start_mk, temperature_end_mk),
            Self::Latent { temperature_mk, .. } => (temperature_mk, temperature_mk),
        }
    }
}

/// 一つの物質familyに対する凍結済みenthalpy曲線。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnthalpyFamily {
    key: StableKey,
    bands: Vec<EnthalpyBand>,
}

/// energyから得られる物質の熱状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnthalpyState {
    /// energyから整数補間した温度mK。
    pub temperature_mk: i64,
    /// 通常区間では等しい二つの相キー。潜熱区間では変化前後のキー。
    pub from: StableKey,
    /// 潜熱区間の変化先。通常区間では`from`と同じです。
    pub to: StableKey,
    /// 相変化率。通常区間では0、潜熱区間では0..=65535です。
    pub fraction_q16: u16,
}

impl EnthalpyFamily {
    /// 連続・単調なenthalpy曲線を検証して作ります。
    pub fn new(key: StableKey, bands: Vec<EnthalpyBand>) -> EngineResult<Self> {
        if bands.is_empty() {
            return Err(invalid("enthalpy familyに区間がありません"));
        }
        let mut previous: Option<((i64, i64), (i64, i64))> = None;
        let mut previous_material: Option<StableKey> = None;
        for band in &bands {
            let (energy, temperature) = (band.energy_bounds(), band.temperature_bounds());
            if energy.0 >= energy.1 || temperature.0 > temperature.1 {
                return Err(invalid("enthalpy区間の範囲"));
            }
            match *band {
                EnthalpyBand::Sensible { material, .. } if temperature.0 == temperature.1 => {
                    let _ = material;
                    return Err(invalid("顕熱区間の温度幅"));
                }
                EnthalpyBand::Latent { from, to, .. } if from == to => {
                    return Err(invalid("潜熱区間の相参照"));
                }
                _ => {}
            }
            if let Some((previous_energy, previous_temperature)) = previous {
                if previous_energy.1 != energy.0 || previous_temperature.1 != temperature.0 {
                    return Err(invalid("enthalpy区間のenergy/温度連続性"));
                }
                let current_material = match *band {
                    EnthalpyBand::Sensible { material, .. } => material,
                    EnthalpyBand::Latent { from, .. } => from,
                };
                if previous_material != Some(current_material) {
                    return Err(invalid("phase familyの相参照連続性"));
                }
            }
            previous_material = Some(match *band {
                EnthalpyBand::Sensible { material, .. } => material,
                EnthalpyBand::Latent { to, .. } => to,
            });
            previous = Some((energy, temperature));
        }
        Ok(Self { key, bands })
    }

    /// 永続family key。
    pub fn key(&self) -> StableKey {
        self.key
    }

    /// family内で熱状態として参照できる全material keyを調べます。
    pub fn contains_material(&self, material: StableKey) -> bool {
        self.bands.iter().any(|band| match *band {
            EnthalpyBand::Sensible { material: key, .. } => key == material,
            EnthalpyBand::Latent { from, to, .. } => from == material || to == material,
        })
    }

    /// energyを一度だけ区間検索し、温度・相・潜熱の進捗へ写像します。
    fn state_and_band(&self, energy: i64) -> EngineResult<(EnthalpyState, usize)> {
        let index = self
            .bands
            .partition_point(|band| band.energy_bounds().1 <= energy);
        let band = *self
            .bands
            .get(index)
            .ok_or_else(|| invalid("enthalpy energy範囲外"))?;
        let (start, end) = band.energy_bounds();
        if energy < start || energy >= end {
            return Err(invalid("enthalpy energy範囲外"));
        }
        let span = i128::from(end) - i128::from(start);
        let offset = i128::from(energy) - i128::from(start);
        match band {
            EnthalpyBand::Sensible {
                temperature_start_mk,
                temperature_end_mk,
                material,
                ..
            } => {
                let delta = i128::from(temperature_end_mk) - i128::from(temperature_start_mk);
                let interpolated_delta = offset
                    .checked_mul(delta)
                    .ok_or_else(|| invalid("enthalpy補間中間値overflow"))?
                    / span;
                let temperature = i128::from(temperature_start_mk)
                    .checked_add(interpolated_delta)
                    .ok_or_else(|| invalid("enthalpy温度中間値overflow"))?;
                Ok((
                    EnthalpyState {
                        temperature_mk: i64::try_from(temperature)
                            .map_err(|_| invalid("enthalpy温度overflow"))?,
                        from: material,
                        to: material,
                        fraction_q16: 0,
                    },
                    index,
                ))
            }
            EnthalpyBand::Latent {
                temperature_mk,
                from,
                to,
                ..
            } => {
                let fraction = offset * 65536 / span;
                Ok((
                    EnthalpyState {
                        temperature_mk,
                        from,
                        to,
                        fraction_q16: u16::try_from(fraction.min(65535))
                            .map_err(|_| invalid("潜熱相分率"))?,
                    },
                    index,
                ))
            }
        }
    }

    /// energyから温度・相状態・区間indexを一度に求めます。
    pub fn state(&self, energy: i64) -> EngineResult<EnthalpyState> {
        self.state_and_band(energy).map(|(state, _)| state)
    }

    fn validate_energy(&self, energy: i64) -> EngineResult<()> {
        if energy < self.bands[0].energy_bounds().0
            || energy >= self.bands[self.bands.len() - 1].energy_bounds().1
        {
            return Err(invalid("enthalpy energy範囲外"));
        }
        Ok(())
    }

    /// phase fractionが半分以上なら高enthalpy側の素材を返します。
    /// 正確なfractionは`state`で保持され、cellの離散material表示だけを安定に選びます。
    pub fn material_at_energy(&self, energy: i64) -> EngineResult<StableKey> {
        let state = self.state(energy)?;
        Ok(if state.fraction_q16 >= 32768 {
            state.to
        } else {
            state.from
        })
    }

    fn equilibrium_transfer(
        &self,
        hot_energy: i64,
        cold_energy: i64,
        hot_state: ThermalLookup,
        cold_state: ThermalLookup,
    ) -> EngineResult<i64> {
        let hot_temp = hot_state.temperature_mk;
        let cold_temp = cold_state.temperature_mk;
        if hot_temp <= cold_temp {
            return Ok(0);
        }
        if hot_state.band_index == cold_state.band_index
            && let Some(EnthalpyBand::Sensible {
                energy_start,
                energy_end,
                temperature_start_mk,
                temperature_end_mk,
                ..
            }) = self.bands.get(hot_state.band_index)
            && (i128::from(*temperature_end_mk) - i128::from(*temperature_start_mk))
                >= (i128::from(*energy_end) - i128::from(*energy_start))
        {
            // 温度がenergyごとに必ず増える同一顕熱区間では、energy差の半分が
            // 等温点の厳密解です。多数辺での繰返し二分探索を省きます。
            return i64::try_from((i128::from(hot_energy) - i128::from(cold_energy)) / 2)
                .map_err(|_| invalid("平衡熱量範囲"));
        }
        let (min_energy, max_energy) = (
            self.bands[0].energy_bounds().0,
            self.bands.last().unwrap().energy_bounds().1 - 1,
        );
        let max = (i128::from(hot_energy) - i128::from(min_energy))
            .min(i128::from(max_energy) - i128::from(cold_energy))
            .max(0);
        let mut low = 0_i128;
        let mut high = max;
        while low < high {
            let mid = low + (high - low + 1) / 2;
            let h =
                i64::try_from(i128::from(hot_energy) - mid).map_err(|_| invalid("平衡熱量範囲"))?;
            let c = i64::try_from(i128::from(cold_energy) + mid)
                .map_err(|_| invalid("平衡熱量範囲"))?;
            if self.state(h)?.temperature_mk >= self.state(c)?.temperature_mk {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        i64::try_from(low).map_err(|_| invalid("平衡熱量範囲"))
    }
}

/// 一tick中cellごとに一度算出し、隣接edge評価で共有する熱状態scratch。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThermalLookup {
    /// enthalpyから得た温度mK。
    pub temperature_mk: i64,
    /// 検証済みenthalpy family区間index。
    pub band_index: usize,
}

/// materialと一緒に移動する正規enthalpy payload。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThermalCellState {
    /// 現在のenthalpy energy。
    pub energy: i64,
}
impl StateCodec for ThermalCellState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.i64(self.energy)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            energy: input.i64()?,
        })
    }
}
impl GridValue for ThermalCellState {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn validate(self) -> EngineResult<()> {
        Ok(())
    }
}

/// 正方向格子辺(+x,+y)が所有する伝導端数。座標固定channelへ保存します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThermalEdgeState {
    /// +x辺のconductance分母に対する符号付き余り。
    pub residual_x: i64,
    /// +y辺のconductance分母に対する符号付き余り。
    pub residual_y: i64,
    /// +x辺に蓄積したQeq/8の端数credit（0..8未満）。
    pub equilibrium_credit_x: u8,
    /// +y辺に蓄積したQeq/8の端数credit（0..8未満）。
    pub equilibrium_credit_y: u8,
}
impl StateCodec for ThermalEdgeState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.i64(self.residual_x)?;
        out.i64(self.residual_y)?;
        out.u8(self.equilibrium_credit_x)?;
        out.u8(self.equilibrium_credit_y)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = Self {
            residual_x: input.i64()?,
            residual_y: input.i64()?,
            equilibrium_credit_x: input.u8()?,
            equilibrium_credit_y: input.u8()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl GridValue for ThermalEdgeState {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn validate(self) -> EngineResult<()> {
        if self.equilibrium_credit_x >= 8 || self.equilibrium_credit_y >= 8 {
            return Err(invalid("thermal equilibrium credit範囲"));
        }
        Ok(())
    }
}

/// 断熱4近傍熱拡散の固定設定。係数は同一tick・全辺で一定です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThermalStepConfig {
    /// conductivity×tickを表す非負整数係数。
    pub conductance_numerator: u32,
    /// conductivity比の正の分母。
    pub conductance_denominator: u32,
}

/// 断熱拡散の仕事量とenergy収支。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThermalStepReport {
    /// 評価した正方向のactive edge数。
    pub edges: u64,
    /// source/sinkを含まないため入力と同一の総energyです。
    pub total_energy: i128,
}

/// 断熱熱拡散を計算します。入力は変更せず、成功時だけ候補配列を完成させます。
///
/// 各edgeは一度だけ評価します。生fluxは端数を保持して積分し、平衡energy移動量の
/// 1/8に制限します。4近傍の同時fluxでも局所平衡を大きく飛び越えない参照solverです。
#[allow(clippy::too_many_arguments)]
pub fn diffuse_enthalpy_into<F>(
    width: usize,
    height: usize,
    family: &EnthalpyFamily,
    input: &[ThermalCellState],
    input_edges: &[ThermalEdgeState],
    active: F,
    config: ThermalStepConfig,
    delta: &mut [i128],
    lookup: &mut [ThermalLookup],
    output: &mut [ThermalCellState],
    output_edges: &mut [ThermalEdgeState],
) -> EngineResult<ThermalStepReport>
where
    F: Fn(usize) -> bool,
{
    if width == 0 || height == 0 || config.conductance_denominator == 0 {
        return Err(invalid("thermal dimensionsまたはconductance"));
    }
    let len = width
        .checked_mul(height)
        .ok_or_else(|| invalid("thermal cell数overflow"))?;
    if input.len() != len
        || input_edges.len() != len
        || delta.len() != len
        || lookup.len() != len
        || output.len() != len
        || output_edges.len() != len
    {
        return Err(invalid("thermal input/scratch寸法"));
    }
    delta.fill(0);
    output.copy_from_slice(input);
    output_edges.copy_from_slice(input_edges);
    let mut report = ThermalStepReport::default();
    for (index, (cell, edge)) in input.iter().zip(input_edges).enumerate() {
        let (state, band_index) = family.state_and_band(cell.energy)?;
        lookup[index] = ThermalLookup {
            temperature_mk: state.temperature_mk,
            band_index,
        };
        report.total_energy = report
            .total_energy
            .checked_add(i128::from(cell.energy))
            .ok_or_else(|| invalid("thermal総energyoverflow"))?;
        edge.validate()?;
        if edge.residual_x.unsigned_abs() >= u64::from(config.conductance_denominator)
            || edge.residual_y.unsigned_abs() >= u64::from(config.conductance_denominator)
        {
            return Err(invalid("thermal residual範囲"));
        }
    }
    for y in 0..height {
        for x in 0..width {
            let a = y * width + x;
            if !active(a) {
                continue;
            }
            for (b, horizontal) in [
                ((x + 1 < width).then_some(a + 1), true),
                ((y + 1 < height).then_some(a + width), false),
            ] {
                let Some(b) = b else { continue };
                if !active(b) {
                    continue;
                }
                report.edges = report
                    .edges
                    .checked_add(1)
                    .ok_or_else(|| invalid("thermal edge数overflow"))?;
                let ta = lookup[a].temperature_mk;
                let tb = lookup[b].temperature_mk;
                if ta == tb {
                    continue;
                }
                let residual = if horizontal {
                    input_edges[a].residual_x
                } else {
                    input_edges[a].residual_y
                };
                let numerator = (i128::from(ta) - i128::from(tb))
                    .checked_mul(i128::from(config.conductance_numerator))
                    .and_then(|v| v.checked_add(i128::from(residual)))
                    .ok_or_else(|| invalid("thermal flux numerator overflow"))?;
                let den = i128::from(config.conductance_denominator);
                let raw = numerator / den;
                let remainder = numerator % den;
                let (hot, cold) = if ta > tb { (a, b) } else { (b, a) };
                let equilibrium = family.equilibrium_transfer(
                    input[hot].energy,
                    input[cold].energy,
                    lookup[hot],
                    lookup[cold],
                )?;
                let (credit, next_credit) = if horizontal {
                    (
                        input_edges[a].equilibrium_credit_x,
                        &mut output_edges[a].equilibrium_credit_x,
                    )
                } else {
                    (
                        input_edges[a].equilibrium_credit_y,
                        &mut output_edges[a].equilibrium_credit_y,
                    )
                };
                let credit_units = i128::from(equilibrium)
                    .checked_add(i128::from(credit))
                    .ok_or_else(|| invalid("thermal equilibrium credit overflow"))?;
                let cap = credit_units / 8;
                *next_credit = u8::try_from(credit_units % 8)
                    .map_err(|_| invalid("thermal equilibrium credit範囲"))?;
                let amount = raw.clamp(-cap, cap);
                let stored_remainder = if amount != raw {
                    0
                } else {
                    i64::try_from(remainder).map_err(|_| invalid("thermal residual overflow"))?
                };
                if horizontal {
                    output_edges[a].residual_x = stored_remainder;
                } else {
                    output_edges[a].residual_y = stored_remainder;
                }
                delta[a] = delta[a]
                    .checked_sub(amount)
                    .ok_or_else(|| invalid("thermal delta overflow"))?;
                delta[b] = delta[b]
                    .checked_add(amount)
                    .ok_or_else(|| invalid("thermal delta overflow"))?;
            }
        }
    }
    for (index, cell) in input.iter().enumerate() {
        if !active(index) {
            continue;
        }
        let energy = i128::from(cell.energy)
            .checked_add(delta[index])
            .ok_or_else(|| invalid("thermal energy overflow"))?;
        output[index].energy = i64::try_from(energy).map_err(|_| invalid("thermal energy範囲"))?;
        family.validate_energy(output[index].energy)?;
    }
    let after = output.iter().try_fold(0_i128, |sum, cell| {
        sum.checked_add(i128::from(cell.energy))
            .ok_or_else(|| invalid("thermal総energyoverflow"))
    })?;
    if after != report.total_energy {
        return Err(EngineError::new(
            ErrorCode::Conflict,
            "断熱solverのenergy不一致",
        ));
    }
    Ok(report)
}

/// 燃料・濡れ・腐食・timerを所有する保存可能なmaterial sidecar。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReactionSidecar {
    /// 次tickまで利用可能な燃料単位。
    pub fuel: u32,
    /// 0は乾燥、`u16::MAX`は飽和した濡れ状態。
    pub wetness: u16,
    /// 作者定義単位の腐食進捗。
    pub corrosion: u32,
    /// 反応が要求する残り固定tick数。
    pub timer: u32,
}
impl StateCodec for ReactionSidecar {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.fuel)?;
        out.u16(self.wetness)?;
        out.u32(self.corrosion)?;
        out.u32(self.timer)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            fuel: input.u32()?,
            wetness: input.u16()?,
            corrosion: input.u32()?,
            timer: input.u32()?,
        })
    }
}
impl GridValue for ReactionSidecar {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn validate(self) -> EngineResult<()> {
        Ok(())
    }
}

/// 反応計算に渡す疎なcell。入力sliceは座標順で重複なく並べます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactionCell {
    /// World内の整数x座標。
    pub x: i32,
    /// World内の整数y座標。
    pub y: i32,
    /// 所有する素材key。
    pub material: StableKey,
    /// 正の物質数量。
    pub quantity: u64,
    /// cellに所有されるenthalpy。
    pub energy: i64,
    /// 燃焼等の個体進捗。
    pub state: ReactionSidecar,
    /// falseは接触候補としてだけ参照するblock等の占有cellです。
    pub can_react: bool,
}

/// 一つの作者定義反応。反応候補の安定順はpriority、key、座標です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactionRule {
    /// 安定rule key。
    pub key: StableKey,
    /// 反応を開始する素材。
    pub input: StableKey,
    /// 接触が必要な場合の素材。Noneなら接触条件なし。
    pub contact: Option<StableKey>,
    /// 優先順位。小さい値を先に処理します。
    pub priority: i32,
    /// 入力から消費する数量。
    pub consume: u64,
    /// 出力素材。
    pub output: StableKey,
    /// 出力数量。
    pub produce: u64,
    /// 出力座標のsourceからのdelta。
    pub output_offset: [i32; 2],
    /// `Some`なら正規material particleを生成し、値を初速として使います。
    pub particle_velocity_q16: Option<[i64; 2]>,
    /// 反応に必要な最低温度mK。
    pub minimum_temperature_mk: i64,
    /// 消費するsource sidecar燃料。
    pub fuel_cost: u32,
    /// 消費するsource sidecar timer。
    pub timer_cost: u32,
    /// 出力への正味enthalpy delta。
    pub energy_delta: i64,
    /// このwetness以下で反応可能。
    pub maximum_wetness: u16,
    /// source腐食進捗の増分。
    pub corrosion_delta: u32,
    /// 個体sidecarの変換方法。反応時に状態を黙って複製・消失させません。
    pub state_transfer: ReactionStateTransfer,
    /// 出力に反応が続く場合のRuntime schedule方針。
    pub chain_policy: ReactionChainPolicy,
}

/// 反応出力が次のruleを起動するときのtick境界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionChainPolicy {
    /// 次tickへ継続を保存します。
    Deferred,
    /// Runtimeの同tick予算内で続きを実行します。
    SameTick,
}
impl StateCodec for ReactionChainPolicy {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u8(match self {
            Self::Deferred => 0,
            Self::SameTick => 1,
        })
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        match input.u8()? {
            0 => Ok(Self::Deferred),
            1 => Ok(Self::SameTick),
            _ => Err(invalid("reaction chain policy tag")),
        }
    }
}

impl ReactionRule {
    /// 熱familyと照合し、Job化前の静的なrule不変条件を検証します。
    pub fn validate(&self, family: &EnthalpyFamily) -> EngineResult<()> {
        if !family.contains_material(self.input) || !family.contains_material(self.output) {
            return Err(invalid(
                "reaction input/outputがenthalpy familyにありません",
            ));
        }
        if self.consume == 0 || self.produce == 0 || self.output_offset == [0, 0] {
            return Err(invalid("reaction quantityまたはoutput位置"));
        }
        Ok(())
    }
}

/// 構築時に静的条件と一意なrule keyを検証した不変反応定義表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionRuleSet {
    family: EnthalpyFamily,
    rules: Vec<ReactionRule>,
    build_order: Vec<usize>,
    chain_inputs: Vec<StableKey>,
}

impl ReactionRuleSet {
    /// familyと照合して全ruleを一度だけ検証し、key順に凍結します。
    pub fn new(family: &EnthalpyFamily, mut rules: Vec<ReactionRule>) -> EngineResult<Self> {
        for rule in &rules {
            rule.validate(family)?;
        }
        rules.sort_unstable_by_key(|rule| rule.key);
        if rules.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(invalid("reaction rule key重複"));
        }
        let mut build_order = Vec::new();
        build_order
            .try_reserve_exact(rules.len())
            .map_err(crate::foundation::error::allocation)?;
        build_order.extend(0..rules.len());
        build_order.sort_unstable_by_key(|index| (rules[*index].priority, rules[*index].key));
        let mut chain_inputs = Vec::new();
        chain_inputs
            .try_reserve_exact(rules.len())
            .map_err(crate::foundation::error::allocation)?;
        chain_inputs.extend(rules.iter().map(|rule| rule.input));
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        Ok(Self {
            family: family.clone(),
            rules,
            build_order,
            chain_inputs,
        })
    }

    fn get(&self, key: StableKey) -> Option<&ReactionRule> {
        self.rules
            .binary_search_by_key(&key, |rule| rule.key)
            .ok()
            .map(|index| &self.rules[index])
    }

    fn can_chain_from(&self, material: StableKey) -> bool {
        self.chain_inputs.binary_search(&material).is_ok()
    }

    /// 登録済みの反応rule数。
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// ruleが登録されていないかを返します。
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// 反応に伴う個体sidecarの所有権移行規則。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionStateTransfer {
    /// sourceに数量が残る場合だけ成立し、出力へ状態を複製しません。
    RetainSource,
    /// source全量を消費し、進捗状態を出力へ移します。
    MigrateToOutput,
    /// source全量を消費し、進捗状態を明示的に破棄します。
    Drop,
    /// 進捗状態も入力と一緒に消費し、出力へ移しません。
    Consume,
}
impl StateCodec for ReactionStateTransfer {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u8(match self {
            Self::RetainSource => 0,
            Self::MigrateToOutput => 1,
            Self::Drop => 2,
            Self::Consume => 3,
        })
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        match input.u8()? {
            0 => Ok(Self::RetainSource),
            1 => Ok(Self::MigrateToOutput),
            2 => Ok(Self::Drop),
            3 => Ok(Self::Consume),
            _ => Err(invalid("reaction state transfer tag")),
        }
    }
}

/// 検証済みtick候補。rule値とsource期待値を保存し、復元時に再照合します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactionCandidate {
    /// ruleの安定順key。
    pub rule: StableKey,
    /// priority。
    pub priority: i32,
    /// source座標。
    pub source: [i32; 2],
    /// 接触素材を確認した隣接座標。
    pub contact: Option<[i32; 2]>,
    /// 接触座標で再検証する素材。
    pub contact_material: Option<StableKey>,
    /// output座標。
    pub destination: [i32; 2],
    /// 粒子出力時の初速。cell出力ならNoneです。
    pub particle_velocity_q16: Option<[i64; 2]>,
    /// 候補作成時に空だった出力座標。競合反応の後続候補を次tickへ残さず棄却します。
    pub destination_expected_empty: bool,
    /// sourceの期待素材。
    pub input: StableKey,
    /// sourceの期待数量。
    pub expected_quantity: u64,
    /// sourceの期待energy。
    pub expected_energy: i64,
    /// sourceの期待sidecar。
    pub expected_state: ReactionSidecar,
    /// 消費量。
    pub consume: u64,
    /// 出力素材。
    pub output: StableKey,
    /// 生成量。
    pub produce: u64,
    /// 最低温度mK。
    pub minimum_temperature_mk: i64,
    /// 燃料消費量。
    pub fuel_cost: u32,
    /// timer消費量。
    pub timer_cost: u32,
    /// 出力energy delta。
    pub energy_delta: i64,
    /// 最大wetness。
    pub maximum_wetness: u16,
    /// 腐食進捗増分。
    pub corrosion_delta: u32,
    /// sidecar移行方法。
    pub state_transfer: ReactionStateTransfer,
    /// Runtimeへ返すtick境界。
    pub chain_policy: ReactionChainPolicy,
}

/// tickを越えて保存できる、順序固定の反応候補Job。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionJob {
    candidates: Vec<ReactionCandidate>,
    cursor: usize,
    same_tick_chain_pending: bool,
}

/// 反応job一回分のcommitと延期件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReactionStepReport {
    /// 新たに確定した反応件数。
    pub committed: u32,
    /// 出力空間・pool・条件が足りず次回へ残した件数。
    pub deferred: u32,
    /// cursorが進んだ候補数。
    pub visited: u32,
}

impl ReactionJob {
    /// ルールとcontact pairから疎な候補を構築します。件数上限を超えたら登録前に失敗します。
    pub fn build(
        cells: &[ReactionCell],
        rules: &ReactionRuleSet,
        candidate_limit: usize,
    ) -> EngineResult<Self> {
        validate_reaction_cells(cells)?;
        let family = &rules.family;
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(cells.len().min(candidate_limit))
            .map_err(crate::foundation::error::allocation)?;
        for rule_index in &rules.build_order {
            let rule = &rules.rules[*rule_index];
            for source in cells {
                if !source.can_react || source.material != rule.input {
                    continue;
                }
                if source.quantity < rule.consume
                    || source.state.fuel < rule.fuel_cost
                    || source.state.timer < rule.timer_cost
                    || source.state.wetness > rule.maximum_wetness
                {
                    continue;
                }
                if (rule.state_transfer == ReactionStateTransfer::RetainSource
                    && rule.consume == source.quantity)
                    || (rule.state_transfer == ReactionStateTransfer::MigrateToOutput
                        && rule.consume != source.quantity)
                {
                    continue;
                }
                if family.state(source.energy)?.temperature_mk < rule.minimum_temperature_mk {
                    continue;
                }
                let contact = if let Some(material) = rule.contact {
                    let found = adjacent_points(source.x, source.y)
                        .into_iter()
                        .find(|(x, y)| {
                            find_cell(cells, *x, *y).is_some_and(|cell| cell.material == material)
                        });
                    let Some(found) = found else { continue };
                    Some([found.0, found.1])
                } else {
                    None
                };
                let destination = [
                    source
                        .x
                        .checked_add(rule.output_offset[0])
                        .ok_or_else(|| invalid("reaction x座標overflow"))?,
                    source
                        .y
                        .checked_add(rule.output_offset[1])
                        .ok_or_else(|| invalid("reaction y座標overflow"))?,
                ];
                if destination == [source.x, source.y] {
                    continue;
                }
                if candidates.len() >= candidate_limit {
                    return Err(EngineError::capacity(
                        "reaction candidate数",
                        candidates.len() as u64 + 1,
                        candidate_limit as u64,
                    ));
                }
                candidates
                    .try_reserve(1)
                    .map_err(crate::foundation::error::allocation)?;
                candidates.push(ReactionCandidate {
                    rule: rule.key,
                    priority: rule.priority,
                    source: [source.x, source.y],
                    contact,
                    contact_material: rule.contact,
                    destination,
                    particle_velocity_q16: rule.particle_velocity_q16,
                    destination_expected_empty: find_index(cells, destination[0], destination[1])
                        .is_none(),
                    input: rule.input,
                    expected_quantity: source.quantity,
                    expected_energy: source.energy,
                    expected_state: source.state,
                    consume: rule.consume,
                    output: rule.output,
                    produce: rule.produce,
                    minimum_temperature_mk: rule.minimum_temperature_mk,
                    fuel_cost: rule.fuel_cost,
                    timer_cost: rule.timer_cost,
                    energy_delta: rule.energy_delta,
                    maximum_wetness: rule.maximum_wetness,
                    corrosion_delta: rule.corrosion_delta,
                    state_transfer: rule.state_transfer,
                    chain_policy: rule.chain_policy,
                });
            }
        }
        Ok(Self {
            candidates,
            cursor: 0,
            same_tick_chain_pending: false,
        })
    }

    /// 完了状況を返します。
    pub fn is_complete(&self) -> bool {
        self.cursor == self.candidates.len()
    }

    /// 未処理候補のruleに従い、Runtime schedulerへ継続状態を返します。
    pub fn work_status(&self) -> crate::runtime::schedule::WorkStatus {
        if self.cursor < self.candidates.len()
            && self.candidates[self.cursor].chain_policy == ReactionChainPolicy::SameTick
        {
            crate::runtime::schedule::WorkStatus::ContinueSameTick
        } else if self.cursor < self.candidates.len() {
            crate::runtime::schedule::WorkStatus::Deferred
        } else if self.same_tick_chain_pending {
            crate::runtime::schedule::WorkStatus::ContinueSameTick
        } else {
            crate::runtime::schedule::WorkStatus::Complete
        }
    }

    /// 進捗中候補数を返します。
    pub fn remaining(&self) -> usize {
        self.candidates.len() - self.cursor
    }

    fn try_copy(&self) -> EngineResult<Self> {
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(self.candidates.len())
            .map_err(crate::foundation::error::allocation)?;
        candidates.extend_from_slice(&self.candidates);
        Ok(Self {
            candidates,
            cursor: self.cursor,
            same_tick_chain_pending: self.same_tick_chain_pending,
        })
    }

    /// 保存された候補をworldへ再照合し、上限内の候補を一つずつ取引として確定します。
    ///
    /// 入力cell配列は座標順である必要があります。候補のread setが変わっていた場合は
    /// staleとして延期し、燃料・timer・入力を消費しません。出力は空座標だけに作り、
    /// `max_cells`またはcell poolが足りない候補も延期します。
    pub fn step(
        &mut self,
        cells: &mut Vec<ReactionCell>,
        rules: &ReactionRuleSet,
        occupied: &[[i32; 2]],
        max_cells: usize,
        budget: usize,
    ) -> EngineResult<ReactionStepReport> {
        self.step_with_particle_outputs(cells, rules, occupied, max_cells, budget, None)
    }

    fn step_with_particle_outputs(
        &mut self,
        cells: &mut Vec<ReactionCell>,
        rules: &ReactionRuleSet,
        occupied: &[[i32; 2]],
        max_cells: usize,
        budget: usize,
        mut particle_outputs: Option<&mut Vec<([i32; 2], [i64; 2])>>,
    ) -> EngineResult<ReactionStepReport> {
        validate_reaction_cells(cells)?;
        let family = &rules.family;
        if budget == 0 {
            return Err(invalid("reaction step budget"));
        }
        let end = self
            .cursor
            .saturating_add(budget)
            .min(self.candidates.len());
        let output_count = self.candidates[self.cursor..end].len();
        cells
            .try_reserve(output_count)
            .map_err(crate::foundation::error::allocation)?;
        let mut report = ReactionStepReport::default();
        while self.cursor < end {
            let candidate = self.candidates[self.cursor];
            report.visited += 1;
            let Some(rule) = rules.get(candidate.rule) else {
                return Err(invalid("restored reaction ruleが未登録"));
            };
            let offset = [
                candidate.destination[0].checked_sub(candidate.source[0]),
                candidate.destination[1].checked_sub(candidate.source[1]),
            ];
            if rule.priority != candidate.priority
                || rule.input != candidate.input
                || rule.consume != candidate.consume
                || rule.output != candidate.output
                || rule.produce != candidate.produce
                || rule.minimum_temperature_mk != candidate.minimum_temperature_mk
                || rule.fuel_cost != candidate.fuel_cost
                || rule.timer_cost != candidate.timer_cost
                || rule.energy_delta != candidate.energy_delta
                || rule.maximum_wetness != candidate.maximum_wetness
                || rule.corrosion_delta != candidate.corrosion_delta
                || rule.state_transfer != candidate.state_transfer
                || rule.chain_policy != candidate.chain_policy
                || offset != [Some(rule.output_offset[0]), Some(rule.output_offset[1])]
                || candidate.contact_material != rule.contact
                || candidate.particle_velocity_q16 != rule.particle_velocity_q16
            {
                return Err(invalid("restored reaction candidateとrule不一致"));
            }
            if let Some(point) = candidate.contact {
                let dx = point[0].checked_sub(candidate.source[0]);
                let dy = point[1].checked_sub(candidate.source[1]);
                if !matches!(
                    (dx, dy),
                    (Some(0), Some(-1))
                        | (Some(-1), Some(0))
                        | (Some(1), Some(0))
                        | (Some(0), Some(1))
                ) {
                    return Err(invalid("reaction contact座標"));
                }
            }
            let Some(source_index) = find_index(cells, candidate.source[0], candidate.source[1])
            else {
                self.cursor += 1;
                report.deferred += 1;
                continue;
            };
            let source = cells[source_index];
            if source.material != candidate.input
                || source.quantity != candidate.expected_quantity
                || source.energy != candidate.expected_energy
                || source.state != candidate.expected_state
                || source.quantity < candidate.consume
                || source.state.fuel < candidate.fuel_cost
                || source.state.timer < candidate.timer_cost
                || source.state.wetness > candidate.maximum_wetness
                || family.state(source.energy)?.temperature_mk < candidate.minimum_temperature_mk
            {
                self.cursor += 1;
                report.deferred += 1;
                continue;
            }
            if candidate.contact.is_some_and(|point| {
                find_cell(cells, point[0], point[1])
                    .is_none_or(|cell| Some(cell.material) != candidate.contact_material)
            }) {
                self.cursor += 1;
                report.deferred += 1;
                continue;
            }
            let destination_index =
                find_index(cells, candidate.destination[0], candidate.destination[1]);
            if destination_index.is_some() && candidate.destination_expected_empty {
                self.cursor += 1;
                report.deferred += 1;
                continue;
            }
            let source_quantity = source.quantity - candidate.consume;
            if (candidate.state_transfer == ReactionStateTransfer::RetainSource
                && source_quantity == 0)
                || (candidate.state_transfer == ReactionStateTransfer::MigrateToOutput
                    && source_quantity != 0)
            {
                self.cursor += 1;
                report.deferred += 1;
                continue;
            }
            if destination_index.is_some()
                || occupied.binary_search(&candidate.destination).is_ok()
                || (cells.len() >= max_cells && source_quantity != 0)
            {
                report.deferred += 1;
                break;
            }
            let consumed_energy = i128::from(source.energy) * i128::from(candidate.consume)
                / i128::from(source.quantity);
            let source_energy = i64::try_from(i128::from(source.energy) - consumed_energy)
                .map_err(|_| invalid("reaction source energy overflow"))?;
            let output_energy = i64::try_from(consumed_energy + i128::from(candidate.energy_delta))
                .map_err(|_| invalid("reaction output energy overflow"))?;
            if family.state(source_energy).is_err() || family.state(output_energy).is_err() {
                report.deferred += 1;
                break;
            }
            let new_fuel = source.state.fuel - candidate.fuel_cost;
            let new_timer = source.state.timer - candidate.timer_cost;
            let new_corrosion = source
                .state
                .corrosion
                .checked_add(candidate.corrosion_delta)
                .ok_or_else(|| invalid("reaction corrosion overflow"))?;
            // 失敗し得る算術をすべて終えてからsourceと出力を一括して変更します。
            if source_quantity == 0 {
                cells.remove(source_index);
            } else {
                let src = &mut cells[source_index];
                src.quantity = source_quantity;
                src.energy = source_energy;
                src.state.fuel = new_fuel;
                src.state.timer = new_timer;
                src.state.corrosion = new_corrosion;
            }
            let insertion =
                find_insert_index(cells, candidate.destination[0], candidate.destination[1]);
            cells.insert(
                insertion,
                ReactionCell {
                    x: candidate.destination[0],
                    y: candidate.destination[1],
                    material: candidate.output,
                    quantity: candidate.produce,
                    energy: output_energy,
                    state: ReactionSidecar {
                        fuel: if candidate.state_transfer == ReactionStateTransfer::MigrateToOutput
                        {
                            new_fuel
                        } else {
                            0
                        },
                        wetness: if candidate.state_transfer
                            == ReactionStateTransfer::MigrateToOutput
                        {
                            source.state.wetness
                        } else {
                            0
                        },
                        corrosion: if candidate.state_transfer
                            == ReactionStateTransfer::MigrateToOutput
                        {
                            new_corrosion
                        } else {
                            0
                        },
                        timer: if candidate.state_transfer == ReactionStateTransfer::MigrateToOutput
                        {
                            new_timer
                        } else {
                            0
                        },
                    },
                    can_react: true,
                },
            );
            if let (Some(velocity), Some(outputs)) = (
                candidate.particle_velocity_q16,
                particle_outputs.as_deref_mut(),
            ) {
                outputs.push((candidate.destination, velocity));
            }
            if candidate.chain_policy == ReactionChainPolicy::SameTick
                && rules.can_chain_from(candidate.output)
            {
                self.same_tick_chain_pending = true;
            }
            self.cursor += 1;
            report.committed += 1;
        }
        Ok(report)
    }
}

impl WorldEdit<'_> {
    /// 保存可能なmaterial channelを使い、指定矩形のMaterials候補内で熱を一tick進めます。
    ///
    /// energy channelは対象layerへmaterial追従で登録します。辺端数channelは位置固定です。
    /// active chunk同士だけが熱交換し、inactive/unavailable境界は断熱になります。
    /// すべての出力channelを一つのWorldEdit候補へ書き、失敗時は候補全体をrollbackします。
    #[allow(clippy::too_many_arguments)]
    pub fn step_thermal_channel(
        &mut self,
        layer: LayerId,
        energy_channel: StableKey,
        edge_channel: StableKey,
        family: &EnthalpyFamily,
        origin: CellPoint,
        width: usize,
        height: usize,
        config: ThermalStepConfig,
    ) -> EngineResult<ThermalStepReport> {
        self.run(|editor| {
            if !editor.world.content.layer(layer)?.simulated {
                return Err(invalid("熱solverはsimulated layerのみ"));
            }
            let layout = editor.world.channel_layout::<ThermalCellState>(energy_channel)?;
            if layout.x_scale.ratio() != (1, 1)
                || layout.y_scale.ratio() != (1, 1)
                || layout.origin != (CellPoint { x: 0, y: 0 })
                || editor
                    .world
                    .channel_material_layer::<ThermalCellState>(energy_channel)?
                    != Some(editor.world.content.layer(layer)?.key)
            {
                return Err(invalid("熱channelは対象layerに追従する1:1格子が必要"));
            }
            let edge_layout = editor.world.channel_layout::<ThermalEdgeState>(edge_channel)?;
            if edge_layout.origin != (CellPoint { x: 0, y: 0 }) || edge_layout.x_scale.ratio() != (1, 1)
                || edge_layout.y_scale.ratio() != (1, 1)
                || editor.world.channel_material_layer::<ThermalEdgeState>(edge_channel)?.is_some()
                || energy_channel == edge_channel {
                return Err(invalid("熱edge channelは位置固定1:1格子が必要"));
            }
            let len = width
                .checked_mul(height)
                .ok_or_else(|| invalid("熱矩形cell数overflow"))?;
            let scratch_bytes = len
                .checked_mul(
                    std::mem::size_of::<ThermalCellState>() * 2
                        + std::mem::size_of::<ThermalEdgeState>() * 2
                        + std::mem::size_of::<i128>()
                        + std::mem::size_of::<ThermalLookup>()
                        + std::mem::size_of::<bool>()
                        + std::mem::size_of::<super::CellCore>(),
                )
                .ok_or_else(|| invalid("熱scratch byte数overflow"))?;
            let max_writes = len.checked_mul(3).ok_or_else(|| invalid("熱edit write数overflow"))?;
            if len == 0
                || len > editor.world.limits.job_cells
                || max_writes > editor.world.limits.edit_cells
                || scratch_bytes > editor.world.limits.job_bytes
            {
                return Err(EngineError::capacity(
                    "熱solver範囲・scratch",
                    len as u64,
                    editor
                        .world
                        .limits
                        .job_cells
                        .min(editor.world.limits.edit_cells) as u64,
                ));
            }
            let mut input = Vec::new();
            let mut output = Vec::new();
            let mut edges = Vec::new();
            let mut output_edges = Vec::new();
            let mut delta = Vec::new();
            let mut lookup = Vec::new();
            let mut active = Vec::new();
            let mut cores = Vec::new();
            input
                .try_reserve_exact(len)
                .map_err(crate::foundation::error::allocation)?;
            output
                .try_reserve_exact(len)
                .map_err(crate::foundation::error::allocation)?;
            edges.try_reserve_exact(len).map_err(crate::foundation::error::allocation)?;
            output_edges.try_reserve_exact(len).map_err(crate::foundation::error::allocation)?;
            delta
                .try_reserve_exact(len)
                .map_err(crate::foundation::error::allocation)?;
            lookup
                .try_reserve_exact(len)
                .map_err(crate::foundation::error::allocation)?;
            active
                .try_reserve_exact(len)
                .map_err(crate::foundation::error::allocation)?;
            cores.try_reserve_exact(len).map_err(crate::foundation::error::allocation)?;
            delta.resize(len, 0);
            lookup.resize(len, ThermalLookup::default());
            output.resize(len, ThermalCellState::default());
            output_edges.resize(len, ThermalEdgeState::default());
            for index in 0..len {
                let dx = i32::try_from(index % width).map_err(|_| invalid("熱矩形x寸法"))?;
                let dy = i32::try_from(index / width).map_err(|_| invalid("熱矩形y寸法"))?;
                let point = CellPoint {
                    x: origin
                        .x
                        .checked_add(dx)
                        .ok_or_else(|| invalid("熱矩形x座標overflow"))?,
                    y: origin
                        .y
                        .checked_add(dy)
                        .ok_or_else(|| invalid("熱矩形y座標overflow"))?,
                };
                let view = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(view) => view,
                    CellQuery::Unavailable(_) => {
                        input.push(ThermalCellState::default());
                        edges.push(ThermalEdgeState::default());
                        active.push(false);
                        cores.push(super::CellCore::AIR);
                        continue;
                    }
                    CellQuery::WorldBoundary => return Err(invalid("熱矩形がWorld境界外")),
                };
                let state = match editor
                    .world
                    .channel_value::<ThermalCellState>(energy_channel, point)?
                {
                    ChannelQuery::Loaded(value) => value,
                    ChannelQuery::Unavailable(_) | ChannelQuery::WorldBoundary => {
                        return Err(invalid("熱channel coverageが未準備"));
                    }
                };
                let edge = match editor.world.channel_value::<ThermalEdgeState>(edge_channel, point)? {
                    ChannelQuery::Loaded(value) => value,
                    ChannelQuery::Unavailable(_) | ChannelQuery::WorldBoundary => return Err(invalid("thermal edge coverage未準備")),
                };
                let definition = editor
                    .world
                    .content
                    .material(view.core.material)?
                    .ok_or_else(|| invalid("熱cell material定義"))?;
                if !family.contains_material(definition.key()) {
                    return Err(invalid("熱cell materialがenthalpy family外"));
                }
                let enthalpy_state = family.state(state.energy)?;
                if definition.key() != enthalpy_state.from && definition.key() != enthalpy_state.to {
                    return Err(invalid("cell materialとenthalpy phase不一致"));
                }
                let chunk_coord = ChunkCoord {
                    x: point.x.div_euclid(64),
                    y: point.y.div_euclid(64),
                };
                let is_active = editor
                    .world
                    .chunks
                    .get(&chunk_coord)
                    .is_some_and(|chunk| chunk.active);
                input.push(state);
                edges.push(edge);
                active.push(is_active);
                cores.push(view.core);
            }
            let report = diffuse_enthalpy_into(
                width,
                height,
                family,
                &input,
                &edges,
                |index| active[index],
                config,
                &mut delta,
                &mut lookup,
                &mut output,
                &mut output_edges,
            )?;
            for (index, ((&before, &after), (&edge_before, &edge_after))) in input.iter().zip(&output).zip(edges.iter().zip(&output_edges)).enumerate() {
                if !active[index] {
                    continue;
                }
                if before == after && edge_before == edge_after { continue; }
                let dx = i32::try_from(index % width).map_err(|_| invalid("熱矩形x寸法"))?;
                let dy = i32::try_from(index / width).map_err(|_| invalid("熱矩形y寸法"))?;
                let point = CellPoint {
                        x: origin
                            .x
                            .checked_add(dx)
                            .ok_or_else(|| invalid("熱矩形x座標overflow"))?,
                        y: origin
                            .y
                            .checked_add(dy)
                            .ok_or_else(|| invalid("熱矩形y座標overflow"))?,
                    };
                let material_key = family.material_at_energy(after.energy)?;
                let next_material = editor.world.content.material_id(super::MaterialHandle(material_key))?;
                if next_material != cores[index].material {
                    if matches!(editor.world.cell(layer, point)?, CellQuery::Loaded(view) if view.block.is_some()) {
                        return Err(invalid("熱相変化によるblock素材移行は専用migration ruleが必要"));
                    }
                    editor.set_cell(layer, point, super::CellCore { material: next_material, ..cores[index] })?;
                }
                if before != after {
                    editor.set_channel(energy_channel, point, after)?;
                }
                if edge_before != edge_after {
                    editor.set_channel(edge_channel, point, edge_after)?;
                }
            }
            Ok(report)
        })
    }

    /// energy・数量・燃料sidecar channelを一つのWorld候補で反応処理します。
    ///
    /// 3つのchannelは同じsimulated layerへ1:1 material追従で登録します。`resume`を
    /// 渡す場合はStateStoreから復元したReactionJobを使い、同じedit内で進捗recordを
    /// 更新します。Noneなら現在のactive rectangleから候補を構築します。容量不足の
    /// 出力候補はJob cursorを進めず延期するため、空きができたtickに再試行できます。
    #[allow(clippy::too_many_arguments)]
    pub fn step_reaction_channels(
        &mut self,
        layer: LayerId,
        thermal_channel: StableKey,
        quantity_channel: StableKey,
        sidecar_channel: StableKey,
        rules: &ReactionRuleSet,
        origin: CellPoint,
        width: usize,
        height: usize,
        candidate_limit: usize,
        budget: usize,
        resume: Option<ReactionJob>,
        job_key: StableKey,
        job_schema: StableKey,
    ) -> EngineResult<(ReactionJob, ReactionStepReport)> {
        self.run(|editor| {
            let layer_def = editor.world.content.layer(layer)?;
            if !layer_def.simulated {
                return Err(invalid("reaction solverはsimulated layerのみ"));
            }
            for key_and_type in [
                (thermal_channel, 0_u8),
                (quantity_channel, 1_u8),
                (sidecar_channel, 2_u8),
            ] {
                let layout = match key_and_type.1 {
                    0 => editor
                        .world
                        .channel_layout::<ThermalCellState>(key_and_type.0)?,
                    1 => editor.world.channel_layout::<u64>(key_and_type.0)?,
                    _ => editor
                        .world
                        .channel_layout::<ReactionSidecar>(key_and_type.0)?,
                };
                let material_layer = match key_and_type.1 {
                    0 => editor
                        .world
                        .channel_material_layer::<ThermalCellState>(key_and_type.0)?,
                    1 => editor.world.channel_material_layer::<u64>(key_and_type.0)?,
                    _ => editor
                        .world
                        .channel_material_layer::<ReactionSidecar>(key_and_type.0)?,
                };
                if layout.origin != (CellPoint { x: 0, y: 0 })
                    || layout.x_scale.ratio() != (1, 1)
                    || layout.y_scale.ratio() != (1, 1)
                    || material_layer != Some(layer_def.key)
                {
                    return Err(invalid("reaction channelは対象layer追従の1:1格子が必要"));
                }
            }
            if thermal_channel == quantity_channel
                || thermal_channel == sidecar_channel
                || quantity_channel == sidecar_channel
            {
                return Err(invalid("reaction channel key重複"));
            }
            let area = width
                .checked_mul(height)
                .ok_or_else(|| invalid("reaction矩形面積overflow"))?;
            let work = area
                .checked_mul(rules.len())
                .ok_or_else(|| invalid("reaction候補work overflow"))?;
            let scratch = area
                .checked_mul(
                    std::mem::size_of::<ReactionCell>() * 2
                        + std::mem::size_of::<[i32; 2]>()
                        + std::mem::size_of::<bool>(),
                )
                .and_then(|bytes| {
                    bytes.checked_add(
                        candidate_limit.checked_mul(std::mem::size_of::<ReactionCandidate>())?,
                    )
                })
                .ok_or_else(|| invalid("reaction scratch byte数overflow"))?;
            let max_write_work = budget
                .checked_mul(8)
                .ok_or_else(|| invalid("reaction write work overflow"))?;
            if area == 0
                || width > i32::MAX as usize
                || height > i32::MAX as usize
                || rules.len() > editor.world.limits.jobs as usize
                || work > editor.world.limits.job_cells
                || candidate_limit > editor.world.limits.job_cells
                || scratch > editor.world.limits.job_bytes
                || max_write_work > editor.world.limits.edit_cells
            {
                return Err(EngineError::capacity(
                    "reaction候補・scratch・write budget",
                    work as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            let mut cells = Vec::new();
            let mut occupied = Vec::new();
            cells
                .try_reserve_exact(area)
                .map_err(crate::foundation::error::allocation)?;
            occupied
                .try_reserve_exact(area)
                .map_err(crate::foundation::error::allocation)?;
            for index in 0..area {
                let point = CellPoint {
                    x: origin
                        .x
                        .checked_add(
                            i32::try_from(index % width).map_err(|_| invalid("reaction x寸法"))?,
                        )
                        .ok_or_else(|| invalid("reaction x座標overflow"))?,
                    y: origin
                        .y
                        .checked_add(
                            i32::try_from(index / width).map_err(|_| invalid("reaction y寸法"))?,
                        )
                        .ok_or_else(|| invalid("reaction y座標overflow"))?,
                };
                let coord = ChunkCoord {
                    x: point.x.div_euclid(64),
                    y: point.y.div_euclid(64),
                };
                if !editor
                    .world
                    .chunks
                    .get(&coord)
                    .is_some_and(|chunk| chunk.active)
                {
                    occupied.push([point.x, point.y]);
                    continue;
                }
                let view = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(value) => value,
                    CellQuery::Unavailable(_) | CellQuery::WorldBoundary => {
                        occupied.push([point.x, point.y]);
                        continue;
                    }
                };
                if view.block.is_some() {
                    occupied.push([point.x, point.y]);
                    if let Some(material) = editor.world.content.material(view.core.material)? {
                        cells.push(ReactionCell {
                            x: point.x,
                            y: point.y,
                            material: material.key(),
                            quantity: 1,
                            energy: 0,
                            state: ReactionSidecar::default(),
                            can_react: false,
                        });
                    }
                    continue;
                }
                if view.core.material == super::MaterialId::AIR {
                    continue;
                }
                let thermal = match editor
                    .world
                    .channel_value::<ThermalCellState>(thermal_channel, point)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("reaction thermal coverage未準備")),
                };
                let quantity = match editor.world.channel_value::<u64>(quantity_channel, point)? {
                    ChannelQuery::Loaded(value) if value > 0 => value,
                    _ => return Err(invalid("material quantity channelは正数で初期化が必要")),
                };
                let state = match editor
                    .world
                    .channel_value::<ReactionSidecar>(sidecar_channel, point)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("reaction sidecar coverage未準備")),
                };
                let material = editor
                    .world
                    .content
                    .material(view.core.material)?
                    .ok_or_else(|| invalid("reaction material参照"))?;
                if !rules.family.contains_material(material.key()) {
                    return Err(invalid("reaction inputがenthalpy family外"));
                }
                cells.push(ReactionCell {
                    x: point.x,
                    y: point.y,
                    material: material.key(),
                    quantity,
                    energy: thermal.energy,
                    state,
                    can_react: true,
                });
            }
            occupied.sort_unstable();
            let mut before = Vec::new();
            before
                .try_reserve_exact(cells.len())
                .map_err(crate::foundation::error::allocation)?;
            before.extend_from_slice(&cells);
            let mut job = match resume {
                Some(job) => job.try_copy()?,
                None => ReactionJob::build(&cells, rules, candidate_limit)?,
            };
            if job.candidates.len() > candidate_limit {
                return Err(EngineError::capacity(
                    "restored reaction candidate数",
                    job.candidates.len() as u64,
                    candidate_limit as u64,
                ));
            }
            for candidate in job.candidates.iter().skip(job.cursor) {
                let point = candidate.destination;
                if point[0] < origin.x
                    || point[1] < origin.y
                    || i64::from(point[0]) >= i64::from(origin.x) + width as i64
                    || i64::from(point[1]) >= i64::from(origin.y) + height as i64
                {
                    occupied.push(point);
                }
            }
            occupied.sort_unstable();
            occupied.extend(
                editor
                    .world
                    .particles
                    .iter()
                    .filter(|(_, particle)| !particle.is_archived() && particle.layer() == layer)
                    .map(|(_, particle)| {
                        let point = particle.cell();
                        [point.x, point.y]
                    }),
            );
            if editor.world.particles.len() >= editor.world.limits.particles as usize {
                occupied.extend(
                    job.candidates
                        .iter()
                        .skip(job.cursor)
                        .take(budget)
                        .filter(|candidate| candidate.particle_velocity_q16.is_some())
                        .map(|candidate| candidate.destination),
                );
            }
            occupied.sort_unstable();
            occupied.dedup();
            let temporary_cell_limit = area
                .checked_add(budget)
                .ok_or_else(|| invalid("reaction temporary cell capacity overflow"))?;
            let mut particle_outputs = Vec::new();
            particle_outputs
                .try_reserve_exact(budget)
                .map_err(crate::foundation::error::allocation)?;
            let report = job.step_with_particle_outputs(
                &mut cells,
                rules,
                &occupied,
                temporary_cell_limit,
                budget,
                Some(&mut particle_outputs),
            )?;
            // 二つの安定順疎配列をmergeし、差分cellだけをWorldEditへ書きます。
            let (mut old_index, mut new_index) = (0, 0);
            while old_index < before.len() || new_index < cells.len() {
                let old = before.get(old_index).copied();
                let new = cells.get(new_index).copied();
                let old_key = old.map(|cell| (cell.y, cell.x));
                let new_key = new.map(|cell| (cell.y, cell.x));
                let (point, old, new) = match (old_key, new_key) {
                    (Some(a), Some(b)) if a == b => {
                        old_index += 1;
                        new_index += 1;
                        (CellPoint { x: a.1, y: a.0 }, old, new)
                    }
                    (Some(a), Some(b)) if a < b => {
                        old_index += 1;
                        (CellPoint { x: a.1, y: a.0 }, old, None)
                    }
                    (Some(_), Some(b)) => {
                        new_index += 1;
                        (CellPoint { x: b.1, y: b.0 }, None, new)
                    }
                    (Some(a), None) => {
                        old_index += 1;
                        (CellPoint { x: a.1, y: a.0 }, old, None)
                    }
                    (None, Some(b)) => {
                        new_index += 1;
                        (CellPoint { x: b.1, y: b.0 }, None, new)
                    }
                    (None, None) => break,
                };
                if old == new {
                    continue;
                }
                let core = match new {
                    Some(cell) => CellCore {
                        material: editor
                            .world
                            .content
                            .material_id(super::MaterialHandle(cell.material))?,
                        flags: old
                            .and_then(|old| (old.material == cell.material).then_some(0))
                            .unwrap_or(0),
                        variation: 0,
                    },
                    None => CellCore::AIR,
                };
                if old.is_none_or(|old| new.is_none_or(|new| old.material != new.material)) {
                    editor.set_cell(layer, point, core)?;
                }
                let (thermal, quantity, sidecar) = match new {
                    Some(cell) => (
                        ThermalCellState {
                            energy: cell.energy,
                        },
                        cell.quantity,
                        cell.state,
                    ),
                    None => (ThermalCellState::default(), 0, ReactionSidecar::default()),
                };
                editor.set_channel(thermal_channel, point, thermal)?;
                editor.set_channel(quantity_channel, point, quantity)?;
                editor.set_channel(sidecar_channel, point, sidecar)?;
            }
            for (position, velocity) in particle_outputs {
                editor.emit_material_particle(
                    layer,
                    CellPoint {
                        x: position[0],
                        y: position[1],
                    },
                    velocity,
                )?;
            }
            editor.set_state(job_key, job_schema, &job)?;
            Ok((job, report))
        })
    }
}

fn validate_reaction_cells(cells: &[ReactionCell]) -> EngineResult<()> {
    for (index, cell) in cells.iter().enumerate() {
        if cell.quantity == 0
            || (index > 0 && (cells[index - 1].y, cells[index - 1].x) >= (cell.y, cell.x))
        {
            return Err(invalid("reaction cell順序・数量"));
        }
    }
    Ok(())
}
fn find_index(cells: &[ReactionCell], x: i32, y: i32) -> Option<usize> {
    cells
        .binary_search_by_key(&(y, x), |cell| (cell.y, cell.x))
        .ok()
}
fn find_cell(cells: &[ReactionCell], x: i32, y: i32) -> Option<&ReactionCell> {
    find_index(cells, x, y).map(|index| &cells[index])
}
fn find_insert_index(cells: &[ReactionCell], x: i32, y: i32) -> usize {
    cells.partition_point(|cell| (cell.y, cell.x) < (y, x))
}
fn adjacent_points(x: i32, y: i32) -> [(i32, i32); 4] {
    [
        (x, y.saturating_sub(1)),
        (x.saturating_sub(1), y),
        (x.saturating_add(1), y),
        (x, y.saturating_add(1)),
    ]
}

impl StateCodec for ReactionCandidate {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.rule.encode(out)?;
        out.i32(self.priority)?;
        out.i32(self.source[0])?;
        out.i32(self.source[1])?;
        match self.contact {
            Some(point) => {
                out.u8(1)?;
                out.i32(point[0])?;
                out.i32(point[1])?;
            }
            None => out.u8(0)?,
        }
        self.contact_material.encode(out)?;
        out.i32(self.destination[0])?;
        out.i32(self.destination[1])?;
        match self.particle_velocity_q16 {
            Some(velocity) => {
                out.u8(1)?;
                out.i64(velocity[0])?;
                out.i64(velocity[1])?;
            }
            None => out.u8(0)?,
        }
        out.u8(u8::from(self.destination_expected_empty))?;
        self.input.encode(out)?;
        out.u64(self.expected_quantity)?;
        out.i64(self.expected_energy)?;
        self.expected_state.encode(out)?;
        out.u64(self.consume)?;
        self.output.encode(out)?;
        out.u64(self.produce)?;
        out.i64(self.minimum_temperature_mk)?;
        out.u32(self.fuel_cost)?;
        out.u32(self.timer_cost)?;
        out.i64(self.energy_delta)?;
        out.u16(self.maximum_wetness)?;
        out.u32(self.corrosion_delta)?;
        self.state_transfer.encode(out)?;
        self.chain_policy.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let rule = StableKey::decode(input)?;
        let priority = input.i32()?;
        let source = [input.i32()?, input.i32()?];
        let contact = match input.u8()? {
            0 => None,
            1 => Some([input.i32()?, input.i32()?]),
            _ => return Err(invalid("reaction contact tag")),
        };
        let contact_material = Option::<StableKey>::decode(input)?;
        let candidate = Self {
            rule,
            priority,
            source,
            contact,
            contact_material,
            destination: [input.i32()?, input.i32()?],
            particle_velocity_q16: match input.u8()? {
                0 => None,
                1 => Some([input.i64()?, input.i64()?]),
                _ => return Err(invalid("reaction particle output tag")),
            },
            destination_expected_empty: match input.u8()? {
                0 => false,
                1 => true,
                _ => return Err(invalid("reaction destination tag")),
            },
            input: StableKey::decode(input)?,
            expected_quantity: input.u64()?,
            expected_energy: input.i64()?,
            expected_state: ReactionSidecar::decode(input)?,
            consume: input.u64()?,
            output: StableKey::decode(input)?,
            produce: input.u64()?,
            minimum_temperature_mk: input.i64()?,
            fuel_cost: input.u32()?,
            timer_cost: input.u32()?,
            energy_delta: input.i64()?,
            maximum_wetness: input.u16()?,
            corrosion_delta: input.u32()?,
            state_transfer: ReactionStateTransfer::decode(input)?,
            chain_policy: ReactionChainPolicy::decode(input)?,
        };
        if candidate.consume == 0
            || candidate.produce == 0
            || candidate.expected_quantity < candidate.consume
            || candidate.destination == candidate.source
            || candidate.contact.is_some() != candidate.contact_material.is_some()
        {
            return Err(invalid("reaction candidate invariant"));
        }
        Ok(candidate)
    }
}
impl StateCodec for ReactionJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(
            u32::try_from(self.candidates.len())
                .map_err(|_| invalid("reaction candidate count"))?,
        )?;
        out.u32(u32::try_from(self.cursor).map_err(|_| invalid("reaction cursor"))?)?;
        out.u8(u8::from(self.same_tick_chain_pending))?;
        for candidate in &self.candidates {
            candidate.encode(out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let count = input.u32()? as usize;
        let cursor = input.u32()? as usize;
        let same_tick_chain_pending = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("reaction chain pending tag")),
        };
        if cursor > count || count > 1_000_000 {
            return Err(invalid("reaction job range"));
        }
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(count)
            .map_err(crate::foundation::error::allocation)?;
        for _ in 0..count {
            candidates.push(ReactionCandidate::decode(input)?);
        }
        if candidates.windows(2).any(|pair| {
            (
                pair[0].priority,
                pair[0].rule,
                pair[0].source[1],
                pair[0].source[0],
            ) > (
                pair[1].priority,
                pair[1].rule,
                pair[1].source[1],
                pair[1].source[0],
            )
        }) {
            return Err(invalid("reaction job順序"));
        }
        Ok(Self {
            candidates,
            cursor,
            same_tick_chain_pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn family() -> EnthalpyFamily {
        EnthalpyFamily::new(
            key("test:water"),
            vec![
                EnthalpyBand::Sensible {
                    energy_start: 0,
                    energy_end: 100,
                    temperature_start_mk: 0,
                    temperature_end_mk: 100,
                    material: key("test:ice"),
                },
                EnthalpyBand::Latent {
                    energy_start: 100,
                    energy_end: 200,
                    temperature_mk: 100,
                    from: key("test:ice"),
                    to: key("test:water"),
                },
                EnthalpyBand::Sensible {
                    energy_start: 200,
                    energy_end: 300,
                    temperature_start_mk: 100,
                    temperature_end_mk: 200,
                    material: key("test:water"),
                },
                EnthalpyBand::Latent {
                    energy_start: 300,
                    energy_end: 400,
                    temperature_mk: 200,
                    from: key("test:water"),
                    to: key("test:ash"),
                },
                EnthalpyBand::Sensible {
                    energy_start: 400,
                    energy_end: 500,
                    temperature_start_mk: 200,
                    temperature_end_mk: 300,
                    material: key("test:ash"),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn enthalpy_maps_sensible_and_latent_ranges() {
        let f = family();
        assert_eq!(f.state(50).unwrap().temperature_mk, 50);
        assert_eq!(f.material_at_energy(149).unwrap(), key("test:ice"));
        assert_eq!(f.material_at_energy(150).unwrap(), key("test:water"));
        let melt = f.state(150).unwrap();
        assert_eq!(melt.temperature_mk, 100);
        assert_eq!(
            (melt.from, melt.to, melt.fraction_q16),
            (key("test:ice"), key("test:water"), 32768)
        );
        assert!(f.state(500).is_err());
    }

    #[test]
    fn phase_material_returns_after_heating_and_cooling_across_both_transitions() {
        let f = family();
        let heated = [80, 150, 250, 350, 450];
        let cooled = [450, 350, 250, 150, 80];
        let heated_materials = heated
            .into_iter()
            .map(|energy| f.material_at_energy(energy).unwrap())
            .collect::<Vec<_>>();
        let cooled_materials = cooled
            .into_iter()
            .map(|energy| f.material_at_energy(energy).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            cooled_materials,
            heated_materials.iter().rev().copied().collect::<Vec<_>>()
        );
        assert_eq!(heated_materials[0], key("test:ice"));
        assert_eq!(heated_materials[2], key("test:water"));
        assert_eq!(heated_materials[4], key("test:ash"));
    }

    #[test]
    fn strict_sensible_band_uses_exact_equilibrium_transfer() {
        let f = EnthalpyFamily::new(
            key("test:steep"),
            vec![EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 200,
                material: key("test:ice"),
            }],
        )
        .unwrap();
        let (hot, hot_band) = f.state_and_band(80).unwrap();
        let (cold, cold_band) = f.state_and_band(20).unwrap();
        let transferred = f
            .equilibrium_transfer(
                80,
                20,
                ThermalLookup {
                    temperature_mk: hot.temperature_mk,
                    band_index: hot_band,
                },
                ThermalLookup {
                    temperature_mk: cold.temperature_mk,
                    band_index: cold_band,
                },
            )
            .unwrap();
        assert_eq!(transferred, 30);
        assert_eq!(
            f.state(80 - transferred).unwrap().temperature_mk,
            f.state(20 + transferred).unwrap().temperature_mk
        );
    }

    #[test]
    fn extreme_sensible_interpolation_returns_error_instead_of_overflowing() {
        let f = EnthalpyFamily::new(
            key("test:extreme"),
            vec![EnthalpyBand::Sensible {
                energy_start: i64::MIN,
                energy_end: i64::MAX,
                temperature_start_mk: i64::MIN,
                temperature_end_mk: i64::MAX,
                material: key("test:ice"),
            }],
        )
        .unwrap();
        assert!(f.state(i64::MAX - 1).is_err());
    }

    #[test]
    fn invalid_discontinuous_curve_is_rejected() {
        let result = EnthalpyFamily::new(
            key("test:bad"),
            vec![
                EnthalpyBand::Sensible {
                    energy_start: 0,
                    energy_end: 10,
                    temperature_start_mk: 0,
                    temperature_end_mk: 10,
                    material: key("test:a"),
                },
                EnthalpyBand::Sensible {
                    energy_start: 11,
                    energy_end: 20,
                    temperature_start_mk: 10,
                    temperature_end_mk: 20,
                    material: key("test:b"),
                },
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn reaction_rule_is_validated_even_when_no_source_cell_exists() {
        let f = family();
        let mut invalid_rule = rule();
        invalid_rule.consume = 0;
        assert!(ReactionRuleSet::new(&f, vec![invalid_rule]).is_err());

        assert!(ReactionRuleSet::new(&f, vec![rule(), rule()]).is_err());
    }

    #[test]
    fn insulated_diffusion_conserves_energy_and_carries_fractional_flux() {
        let f = family();
        let input = [
            ThermalCellState { energy: 80 },
            ThermalCellState { energy: 20 },
        ];
        let input_edges = [ThermalEdgeState::default(); 2];
        let mut delta = [0_i128; 2];
        let mut lookup = [ThermalLookup::default(); 2];
        let mut output = [ThermalCellState::default(); 2];
        let mut output_edges = [ThermalEdgeState::default(); 2];
        let report = diffuse_enthalpy_into(
            2,
            1,
            &f,
            &input,
            &input_edges,
            |_| true,
            ThermalStepConfig {
                conductance_numerator: 1,
                conductance_denominator: 3,
            },
            &mut delta,
            &mut lookup,
            &mut output,
            &mut output_edges,
        )
        .unwrap();
        assert_eq!(report.total_energy, 100);
        assert_eq!(
            output.iter().map(|c| i128::from(c.energy)).sum::<i128>(),
            100
        );
        assert_eq!(output[0].energy, 77);
        let mut second = [ThermalCellState::default(); 2];
        let mut second_edges = [ThermalEdgeState::default(); 2];
        diffuse_enthalpy_into(
            2,
            1,
            &f,
            &output,
            &output_edges,
            |_| true,
            ThermalStepConfig {
                conductance_numerator: 1,
                conductance_denominator: 3,
            },
            &mut delta,
            &mut lookup,
            &mut second,
            &mut second_edges,
        )
        .unwrap();
        assert!(second[0].energy <= output[0].energy);
    }

    #[test]
    fn edge_residual_is_position_owned_and_accumulates_small_flux() {
        let f = family();
        let input = [
            ThermalCellState { energy: 20 },
            ThermalCellState { energy: 0 },
        ];
        let input_edges = [ThermalEdgeState::default(); 2];
        let mut delta = [0_i128; 2];
        let mut lookup = [ThermalLookup::default(); 2];
        let mut output = [ThermalCellState::default(); 2];
        let mut output_edges = [ThermalEdgeState::default(); 2];
        let config = ThermalStepConfig {
            conductance_numerator: 1,
            conductance_denominator: 30,
        };
        diffuse_enthalpy_into(
            2,
            1,
            &f,
            &input,
            &input_edges,
            |_| true,
            config,
            &mut delta,
            &mut lookup,
            &mut output,
            &mut output_edges,
        )
        .unwrap();
        assert_eq!(output[0].energy, 20);
        assert_eq!(output_edges[0].residual_x, 20);
        let mut next = [ThermalCellState::default(); 2];
        let mut next_edges = [ThermalEdgeState::default(); 2];
        diffuse_enthalpy_into(
            2,
            1,
            &f,
            &output,
            &output_edges,
            |_| true,
            config,
            &mut delta,
            &mut lookup,
            &mut next,
            &mut next_edges,
        )
        .unwrap();
        assert_eq!(next[0].energy + next[1].energy, 20);
        assert_eq!(next[0].energy, 19);
    }

    #[test]
    fn sub_eighth_equilibrium_credit_eventually_moves_integer_energy() {
        let f = EnthalpyFamily::new(
            key("test:small-heat"),
            vec![EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 100,
                material: key("test:ice"),
            }],
        )
        .unwrap();
        let mut input = [
            ThermalCellState { energy: 4 },
            ThermalCellState { energy: 0 },
        ];
        let mut input_edges = [ThermalEdgeState::default(); 2];
        let mut delta = [0_i128; 2];
        let mut lookup = [ThermalLookup::default(); 2];
        let mut output = [ThermalCellState::default(); 2];
        let mut output_edges = [ThermalEdgeState::default(); 2];
        let config = ThermalStepConfig {
            conductance_numerator: 1,
            conductance_denominator: 1,
        };
        for _ in 0..16 {
            diffuse_enthalpy_into(
                2,
                1,
                &f,
                &input,
                &input_edges,
                |_| true,
                config,
                &mut delta,
                &mut lookup,
                &mut output,
                &mut output_edges,
            )
            .unwrap();
            input = output;
            input_edges = output_edges;
        }
        assert_eq!(input[0].energy + input[1].energy, 4);
        assert_eq!(input[0].energy, 2);
        assert_eq!(input[1].energy, 2);
    }

    #[test]
    fn inactive_boundary_is_insulated() {
        let f = family();
        let input = [
            ThermalCellState { energy: 80 },
            ThermalCellState { energy: 20 },
        ];
        let input_edges = [ThermalEdgeState::default(); 2];
        let mut delta = [0_i128; 2];
        let mut lookup = [ThermalLookup::default(); 2];
        let mut output = [ThermalCellState::default(); 2];
        let mut output_edges = [ThermalEdgeState::default(); 2];
        diffuse_enthalpy_into(
            2,
            1,
            &f,
            &input,
            &input_edges,
            |i| i == 0,
            ThermalStepConfig {
                conductance_numerator: 1,
                conductance_denominator: 1,
            },
            &mut delta,
            &mut lookup,
            &mut output,
            &mut output_edges,
        )
        .unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn thermal_state_codec_roundtrips_and_rejects_truncation() {
        let state = ThermalCellState { energy: 123 };
        let mut encoder = Encoder::new(64);
        state.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 64).unwrap();
        assert_eq!(ThermalCellState::decode(&mut decoder).unwrap(), state);
        decoder.finish().unwrap();
        let mut truncated = Decoder::new(&bytes[..4], 64).unwrap();
        assert!(ThermalCellState::decode(&mut truncated).is_err());

        let edge = ThermalEdgeState {
            residual_x: -2,
            residual_y: 3,
            equilibrium_credit_x: 7,
            equilibrium_credit_y: 6,
        };
        let mut encoder = Encoder::new(64);
        edge.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 64).unwrap();
        assert_eq!(ThermalEdgeState::decode(&mut decoder).unwrap(), edge);
        decoder.finish().unwrap();
        assert!(
            ThermalEdgeState {
                equilibrium_credit_x: 8,
                ..ThermalEdgeState::default()
            }
            .validate()
            .is_err()
        );
    }

    fn rule() -> ReactionRule {
        ReactionRule {
            key: key("test:burn"),
            input: key("test:ice"),
            contact: None,
            priority: 1,
            consume: 1,
            output: key("test:ash"),
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 0,
            fuel_cost: 2,
            timer_cost: 1,
            energy_delta: 5,
            maximum_wetness: 10,
            corrosion_delta: 3,
            state_transfer: ReactionStateTransfer::MigrateToOutput,
            chain_policy: ReactionChainPolicy::Deferred,
        }
    }
    fn reaction_input() -> Vec<ReactionCell> {
        vec![ReactionCell {
            x: 0,
            y: 0,
            material: key("test:ice"),
            quantity: 1,
            energy: 80,
            state: ReactionSidecar {
                fuel: 4,
                wetness: 0,
                corrosion: 0,
                timer: 2,
            },
            can_react: true,
        }]
    }

    fn rule_set() -> ReactionRuleSet {
        ReactionRuleSet::new(&family(), vec![rule()]).unwrap()
    }

    #[test]
    fn reaction_job_exposes_saved_deferred_and_same_tick_policy_to_runtime() {
        use crate::runtime::schedule::WorkStatus;

        let cells = reaction_input();
        let mut deferred_rule = rule();
        let deferred = ReactionRuleSet::new(&family(), vec![deferred_rule]).unwrap();
        let deferred_job = ReactionJob::build(&cells, &deferred, 4).unwrap();
        assert_eq!(deferred_job.work_status(), WorkStatus::Deferred);

        deferred_rule.chain_policy = ReactionChainPolicy::SameTick;
        let same_tick = ReactionRuleSet::new(&family(), vec![deferred_rule]).unwrap();
        let same_tick_job = ReactionJob::build(&cells, &same_tick, 4).unwrap();
        assert_eq!(same_tick_job.work_status(), WorkStatus::ContinueSameTick);
        let mut encoded = Encoder::new(4096);
        same_tick_job.encode(&mut encoded).unwrap();
        let bytes = encoded.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let restored = ReactionJob::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(restored.work_status(), WorkStatus::ContinueSameTick);
        assert_eq!(ReactionJob::default().work_status(), WorkStatus::Complete);
    }

    #[test]
    fn same_tick_output_requests_a_saved_candidate_rebuild_and_chains() {
        use crate::runtime::schedule::WorkStatus;

        let mut producer = rule();
        producer.chain_policy = ReactionChainPolicy::SameTick;
        let mut consumer = rule();
        consumer.key = key("test:consume-ash");
        consumer.input = key("test:ash");
        consumer.output = key("test:water");
        consumer.output_offset = [0, 1];
        consumer.chain_policy = ReactionChainPolicy::SameTick;
        let rules = ReactionRuleSet::new(&family(), vec![producer, consumer]).unwrap();
        let mut cells = reaction_input();
        let mut producer_job = ReactionJob::build(&cells, &rules, 4).unwrap();
        assert_eq!(
            producer_job
                .step(&mut cells, &rules, &[], 4, 1)
                .unwrap()
                .committed,
            1
        );
        assert_eq!(producer_job.work_status(), WorkStatus::ContinueSameTick);

        let mut encoder = Encoder::new(4096);
        producer_job.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let restored = ReactionJob::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(restored.work_status(), WorkStatus::ContinueSameTick);

        let mut chained_job = ReactionJob::build(&cells, &rules, 4).unwrap();
        assert_eq!(chained_job.remaining(), 1);
        assert_eq!(
            chained_job
                .step(&mut cells, &rules, &[], 4, 1)
                .unwrap()
                .committed,
            1
        );
        assert!(chained_job.is_complete());
        assert_eq!(cells[0].material, key("test:water"));
    }

    #[test]
    fn reaction_capacity_failure_defers_without_consuming_input_or_fuel() {
        let mut cells = reaction_input();
        cells.push(ReactionCell {
            x: 1,
            y: 0,
            material: key("test:ice"),
            quantity: 1,
            energy: 20,
            state: ReactionSidecar::default(),
            can_react: true,
        });
        cells.sort_unstable_by_key(|cell| (cell.y, cell.x));
        let rules = rule_set();
        let mut job = ReactionJob::build(&cells, &rules, 16).unwrap();
        let report = job.step(&mut cells, &rules, &[], 2, 1).unwrap();
        assert_eq!(report.deferred, 1);
        assert_eq!(cells[0].quantity, 1);
        assert_eq!(cells[0].state.fuel, 4);
        assert_eq!(job.remaining(), 1);
        cells.remove(1);
        let retry = job.step(&mut cells, &rules, &[], 1, 1).unwrap();
        assert_eq!(retry.committed, 1);
        assert!(job.is_complete());
    }

    #[test]
    fn reaction_commit_conserves_energy_except_declared_delta_and_tracks_sidecar() {
        let mut cells = reaction_input();
        let rules = rule_set();
        let mut job = ReactionJob::build(&cells, &rules, 16).unwrap();
        let report = job.step(&mut cells, &rules, &[], 2, 1).unwrap();
        assert_eq!(report.committed, 1);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].material, key("test:ash"));
        assert_eq!(cells[0].energy, 85);
        assert_eq!(cells[0].state.fuel, 2);
        assert_eq!(cells[0].state.timer, 1);
        assert_eq!(cells[0].state.corrosion, 3);
    }

    #[test]
    fn competing_reactions_resolve_by_stable_source_order() {
        let f = family();
        let mut cells = reaction_input();
        let mut second = reaction_input()[0];
        second.x = 2;
        second.material = key("test:water");
        second.energy = 150;
        cells.push(second);
        let mut water_rule = rule();
        water_rule.key = key("test:burn_water");
        water_rule.input = key("test:water");
        water_rule.output_offset = [-1, 0];
        let rules = ReactionRuleSet::new(&f, vec![rule(), water_rule]).unwrap();
        let mut job = ReactionJob::build(&cells, &rules, 16).unwrap();
        let report = job.step(&mut cells, &rules, &[], 8, 8).unwrap();
        assert_eq!(report.committed, 1);
        assert_eq!(report.deferred, 1);
        assert!(job.is_complete());
        assert!(find_cell(&cells, 1, 0).is_some_and(|cell| cell.material == key("test:ash")));
        assert!(find_cell(&cells, 2, 0).is_some_and(|cell| cell.material == key("test:water")));
    }

    #[test]
    fn reaction_outputs_track_only_candidates_that_actually_committed() {
        let mut cells = reaction_input();
        let mut second = cells[0];
        second.x = 2;
        cells.push(second);
        cells.sort_unstable_by_key(|cell| (cell.y, cell.x));
        let mut cell_rule = rule();
        cell_rule.priority = 0;
        cell_rule.state_transfer = ReactionStateTransfer::Consume;
        let mut particle_rule = cell_rule;
        particle_rule.key = key("test:burn-particle");
        particle_rule.priority = 1;
        particle_rule.particle_velocity_q16 = Some([0, 0]);
        let rules = ReactionRuleSet::new(&family(), vec![particle_rule, cell_rule]).unwrap();
        let mut job = ReactionJob::build(&cells, &rules, 8).unwrap();
        let mut particle_outputs = Vec::new();
        let report = job
            .step_with_particle_outputs(&mut cells, &rules, &[], 8, 8, Some(&mut particle_outputs))
            .unwrap();
        assert_eq!(report.committed, 2);
        assert!(particle_outputs.is_empty());
        assert_eq!(cells.len(), 2);
    }

    #[test]
    fn reaction_job_save_resume_matches_uninterrupted_execution() {
        let mut initial = reaction_input();
        initial.push(ReactionCell {
            x: 2,
            y: 0,
            material: key("test:ice"),
            quantity: 1,
            energy: 80,
            state: ReactionSidecar {
                fuel: 4,
                wetness: 0,
                corrosion: 0,
                timer: 2,
            },
            can_react: true,
        });
        let rules = rule_set();
        let mut uninterrupted = ReactionJob::build(&initial, &rules, 16).unwrap();
        let mut resumed = uninterrupted.clone();
        let mut first = initial.clone();
        uninterrupted.step(&mut first, &rules, &[], 4, 2).unwrap();
        let mut second = initial;
        resumed.step(&mut second, &rules, &[], 4, 1).unwrap();
        let mut encoder = Encoder::new(4096);
        resumed.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let mut resumed = ReactionJob::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        resumed.step(&mut second, &rules, &[], 4, 1).unwrap();
        assert_eq!(second, first);
    }
}
