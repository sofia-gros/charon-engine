//! 整数比率の混合定義と、成分収支を検査する原子的な混合・分離。
//!
//! 組成はWorldの共有storeに置き、cellには固定長参照だけを置きます。組成自体は
//! 質量の所有者ではありません。占有するcarrier cellの質量を成分比へ正確に配分します。
use super::{
    ChannelQuery, Content, LayerId, MaterialCellChange, MaterialChangeReason, MaterialId,
    MaterialReceipt, MaterialReceiptJournal, MaterialReceiptKind, MaterialReceiptLog, World,
    WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
    id::WorldId,
};

/// 約分した非負の有理数量。混合cellの成分tokenを整数へ丸めません。
/// 中間積もu128の範囲を要求し、超過時は切捨てず取引を拒否します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialQuantity {
    numerator: u128,
    denominator: u128,
}
impl MaterialQuantity {
    /// 量0の一意な表現。
    pub const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };
    /// 純物質cell一つに対応するtoken数。
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };
    /// 分母0を拒否し、最大公約数で約分します。
    pub fn new(numerator: u128, denominator: u128) -> EngineResult<Self> {
        if denominator == 0 {
            return Err(invalid("物質数量の分母0"));
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }
    /// 約分後の分子。
    pub fn numerator(self) -> u128 {
        self.numerator
    }
    /// 約分後の正の分母。
    pub fn denominator(self) -> u128 {
        self.denominator
    }
    /// 最大公約数で交差積を小さくしてから加算します。
    pub fn checked_add(self, other: Self) -> EngineResult<Self> {
        let divisor = gcd(self.denominator, other.denominator);
        let a = self.denominator / divisor;
        let b = other.denominator / divisor;
        let numerator = self
            .numerator
            .checked_mul(b)
            .and_then(|value| {
                other
                    .numerator
                    .checked_mul(a)
                    .and_then(|right| value.checked_add(right))
            })
            .ok_or_else(|| invalid("物質数量の加算上限"))?;
        let denominator = a
            .checked_mul(other.denominator)
            .ok_or_else(|| invalid("物質数量の分母上限"))?;
        Self::new(numerator, denominator)
    }
    /// 非負の差を正確に求めます。負の結果と中間積の上限超過は拒否します。
    pub fn checked_sub(self, other: Self) -> EngineResult<Self> {
        if self == other {
            return Ok(Self::ZERO);
        }
        let divisor = gcd(self.denominator, other.denominator);
        let a = self.denominator / divisor;
        let b = other.denominator / divisor;
        let left = self
            .numerator
            .checked_mul(b)
            .ok_or_else(|| invalid("物質数量の減算上限"))?;
        let right = other
            .numerator
            .checked_mul(a)
            .ok_or_else(|| invalid("物質数量の減算上限"))?;
        let numerator = left
            .checked_sub(right)
            .ok_or_else(|| invalid("物質数量の負の差"))?;
        let denominator = a
            .checked_mul(other.denominator)
            .ok_or_else(|| invalid("物質数量の分母上限"))?;
        Self::new(numerator, denominator)
    }

    /// 除算の商と剰余を交互に比べ、交差積のoverflowなしで大小を比較します。
    pub fn compare(self, other: Self) -> std::cmp::Ordering {
        let (mut a, mut b, mut c, mut d) = (
            self.numerator,
            self.denominator,
            other.numerator,
            other.denominator,
        );
        let mut reversed = false;
        loop {
            let order = (a / b).cmp(&(c / d));
            if order != std::cmp::Ordering::Equal {
                return if reversed { order.reverse() } else { order };
            }
            let (left, right) = (a % b, c % d);
            if left == 0 || right == 0 {
                let order = left.cmp(&right);
                return if reversed { order.reverse() } else { order };
            }
            (a, b, c, d) = (b, left, d, right);
            reversed = !reversed;
        }
    }

    /// 質量換算等の整数倍。先に分母と約分します。
    pub fn checked_mul(self, value: u128) -> EngineResult<Self> {
        let divisor = gcd(value, self.denominator);
        Self::new(
            self.numerator
                .checked_mul(value / divisor)
                .ok_or_else(|| invalid("物質数量の積上限"))?,
            self.denominator / divisor,
        )
    }
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}
impl StateCodec for MaterialQuantity {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(&self.numerator.to_le_bytes())?;
        out.raw(&self.denominator.to_le_bytes())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let numerator =
            u128::from_le_bytes(input.raw(16)?.try_into().map_err(|_| invalid("数量分子"))?);
        let denominator =
            u128::from_le_bytes(input.raw(16)?.try_into().map_err(|_| invalid("数量分母"))?);
        let value = Self::new(numerator, denominator)?;
        if value.numerator != numerator || value.denominator != denominator {
            return Err(invalid("未約分の物質数量"));
        }
        Ok(value)
    }
}

/// 種類と整数token比率。mass比率ではなく、純物質token数の比率です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MixturePart {
    /// 空でない素材の安定キー。
    pub material: StableKey,
    /// 正の比率。登録時に同素材を合算し、全体を最大公約数で正規化します。
    pub ratio: u32,
}
/// 安定キー順で正規化した組成。carrier素材の密度・挙動は作者が別に定義します。
#[derive(Debug, PartialEq, Eq)]
pub struct MixtureState {
    parts: Vec<MixturePart>,
}
impl MixtureState {
    /// 2種類以上の正の成分を正規化します。Content参照はWorld登録時に検査します。
    pub fn new(parts: &[MixturePart]) -> EngineResult<Self> {
        let mut values = Vec::new();
        values.try_reserve_exact(parts.len()).map_err(allocation)?;
        for part in parts {
            if part.ratio == 0 {
                return Err(invalid("混合比率0"));
            }
            values.push((part.material, u128::from(part.ratio)));
        }
        values.sort_unstable_by_key(|part| part.0);
        let mut length = 0;
        for index in 0..values.len() {
            if length != 0 && values[length - 1].0 == values[index].0 {
                values[length - 1].1 = values[length - 1]
                    .1
                    .checked_add(values[index].1)
                    .ok_or_else(|| invalid("混合比率の合算上限"))?;
            } else {
                values[length] = values[index];
                length += 1;
            }
        }
        values.truncate(length);
        if values.len() < 2 {
            return Err(invalid("混合には異なる2素材以上が必要"));
        }
        let divisor = values.iter().fold(0, |value, part| gcd(value, part.1));
        let mut normalized = Vec::new();
        normalized.try_reserve_exact(length).map_err(allocation)?;
        for (material, ratio) in values {
            normalized.push(MixturePart {
                material,
                ratio: u32::try_from(ratio / divisor)
                    .map_err(|_| invalid("正規化した混合比率上限"))?,
            });
        }
        Ok(Self { parts: normalized })
    }
    /// 種類の安定順・互いに素な比率。
    pub fn parts(&self) -> &[MixturePart] {
        &self.parts
    }
    fn validate(&self) -> EngineResult<()> {
        if self.parts.len() < 2
            || self.parts.iter().any(|part| part.ratio == 0)
            || self
                .parts
                .windows(2)
                .any(|pair| pair[0].material >= pair[1].material)
            || self
                .parts
                .iter()
                .fold(0, |value, part| gcd(value, u128::from(part.ratio)))
                != 1
        {
            return Err(invalid("混合定義の正規形"));
        }
        Ok(())
    }
    fn validate_content(&self, content: &Content) -> EngineResult<()> {
        self.validate()?;
        for part in &self.parts {
            content.material_id(super::MaterialHandle(part.material))?;
        }
        Ok(())
    }
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            parts: super::terrain::copy_slice(&self.parts)?,
        })
    }
}
impl MixtureState {
    fn decode_limited(input: &mut Decoder<'_>, limit: usize) -> EngineResult<Self> {
        let count = bounded_count(input, 8)?;
        if count > limit {
            return Err(invalid("混合成分の保存上限"));
        }
        let mut parts = Vec::new();
        parts.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            parts.push(MixturePart {
                material: StableKey::decode(input)?,
                ratio: input.u32()?,
            });
        }
        let value = Self { parts };
        value.validate()?;
        Ok(value)
    }
}
impl StateCodec for MixtureState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.u32(u32::try_from(self.parts.len()).map_err(|_| invalid("混合種類数上限"))?)?;
        for part in &self.parts {
            part.material.encode(out)?;
            out.u32(part.ratio)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::decode_limited(input, usize::MAX)
    }
}

/// Worldの共有組成への参照。Noneは純物質を意味し、cell毎の可変配列を持ちません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixtureRef {
    world: Option<WorldId>,
    index: u32,
}
impl MixtureRef {
    pub(super) fn inventory_key(self) -> (Option<WorldId>, u32) {
        (self.world, self.index)
    }
    /// 組成を持たない純物質。空cellにもこの値だけを使います。
    pub const NONE: Self = Self {
        world: None,
        index: 0,
    };
    /// 純物質参照か。
    pub fn is_none(self) -> bool {
        self.world.is_none()
    }
    /// 所属World。純物質の場合はNone。
    pub fn world(self) -> Option<WorldId> {
        self.world
    }
}
impl GridValue for MixtureRef {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn validate(self) -> EngineResult<()> {
        if self.world.is_some() == (self.index != 0) {
            Ok(())
        } else {
            Err(invalid("混合参照の空値"))
        }
    }
}
impl StateCodec for MixtureRef {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        self.world.encode(out)?;
        out.u32(self.index)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = Self {
            world: Option::<WorldId>::decode(input)?,
            index: input.u32()?,
        };
        value.validate()?;
        Ok(value)
    }
}

/// 成分別の正確な量。carrier自体を成分へ重ねて二重計上しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialComponentQuantity {
    /// 純物質の素材キー。
    pub material: StableKey,
    /// 純物質cell相当のtoken数。有理数として正確に保持します。
    pub tokens: MaterialQuantity,
    /// token数とその素材のmass_per_cellの積。
    pub mass: MaterialQuantity,
}
/// 共有組成・変換履歴の容量。古い参照・receiptを自動削除しません。
#[derive(Clone, Copy, Debug)]
pub struct MixtureLimits {
    /// 共有定義数。
    pub entries: u32,
    /// 一定義の素材種類数。
    pub components: u32,
    /// 保存する成分変換witnessの件数。
    pub receipts: u32,
}
/// 成分収支を保つ混合・分離と、作者が明示した発行・消費・変換を区別します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixtureConservation {
    /// 素材別token・massが完全一致することを要求します。
    PreserveComponents,
    /// 差を認め、理由付きcell receiptと組成参照witnessを記録します。
    RecordConversion,
}
/// cell変更と組成参照変更の期待値。payload移行は既存の明示方針に従います。
#[derive(Clone, Copy, Debug)]
pub struct MixtureCellChange {
    /// flags・variation・全物質payloadの方針を含む変更。
    pub cell: MaterialCellChange,
    /// 現在の組成参照の期待値。
    pub expected: MixtureRef,
    /// 変更後の組成参照。純物質・空cellはNONE。
    pub output: MixtureRef,
}
/// 実際のcell receiptへ結び付ける、過去の組成参照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixtureReferenceChange {
    /// receipt内の座標。
    pub point: CellPoint,
    /// 変更前の組成。
    pub expected: MixtureRef,
    /// 変更後の組成。
    pub output: MixtureRef,
}
/// cell receiptの変更列と合わせて成分収支を再計算できるwitnessです。
#[derive(PartialEq, Eq)]
pub struct MixtureReceiptWitness {
    journal: StableKey,
    sequence: u64,
    channel: StableKey,
    conservation: MixtureConservation,
    changes: Vec<MixtureReferenceChange>,
}
impl MixtureReceiptWitness {
    /// 参照先journal。
    pub fn journal(&self) -> StableKey {
        self.journal
    }
    /// journal内のreceipt番号。
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// 組成を格納した物質channel。
    pub fn channel(&self) -> StableKey {
        self.channel
    }
    /// 変換時の成分保存方針。
    pub fn conservation(&self) -> MixtureConservation {
        self.conservation
    }
    /// 座標順の組成参照の変更。
    pub fn changes(&self) -> &[MixtureReferenceChange] {
        &self.changes
    }
}
/// 混合操作が使う登録済みschema・channel・journal。
#[derive(Clone, Copy, Debug)]
pub struct MixtureEditConfig {
    /// MixtureStoreのschema。recordキーはMixtureStore::keyで固定します。
    pub schema: StableKey,
    /// 対象layerへ所属するMixtureRefの物質channel。既定値はNONEを要求します。
    pub channel: StableKey,
    /// cell変更と同時に記録するjournal。
    pub journal: MaterialReceiptJournal,
}
/// 一Worldで共有する不変組成と成分変換の履歴。組成に質量所有権を持たせません。
/// 定義を削除・再利用しないため、退避中の区域に残る参照も失効させません。
pub struct MixtureStore {
    world: WorldId,
    content: [u8; 32],
    limits: MixtureLimits,
    entries: Vec<MixtureState>,
    receipts: Vec<MixtureReceiptWitness>,
}
impl MixtureStore {
    /// 標準storeの一意なrecordキー。独自方式は別schema・channel・recordを使えます。
    pub fn key() -> StableKey {
        StableKey::parse("engine:material-mixtures").expect("固定の混合storeキー")
    }
    /// 未初期化のWorldへ作ります。使用中storeを作り直して参照を再利用しません。
    pub fn new(world: &World, limits: MixtureLimits) -> EngineResult<Self> {
        if world
            .state
            .records()
            .any(|record| record.key == Self::key())
        {
            return Err(conflict("混合storeは初期化済み"));
        }
        validate_limits(limits)?;
        Ok(Self {
            world: world.id(),
            content: world.content().fingerprint(),
            limits,
            entries: Vec::new(),
            receipts: Vec::new(),
        })
    }
    /// 登録済み組成数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// 組成がまだないか。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// 定義と履歴を失わず容量を変更します。schemaのbyte上限は独立して残ります。
    pub fn set_limits(&mut self, limits: MixtureLimits) -> EngineResult<()> {
        validate_limits(limits)?;
        if self.entries.len() > limits.entries as usize
            || self.receipts.len() > limits.receipts as usize
            || self
                .entries
                .iter()
                .any(|entry| entry.parts.len() > limits.components as usize)
        {
            return Err(invalid("既存の混合状態を下回る容量"));
        }
        self.limits = limits;
        Ok(())
    }
    /// 保存順の成分変換witness。
    pub fn receipts(&self) -> impl Iterator<Item = &MixtureReceiptWitness> {
        self.receipts.iter()
    }
    /// 有効な組成参照を解決します。純物質はNoneです。
    pub fn resolve(&self, reference: MixtureRef) -> EngineResult<Option<&MixtureState>> {
        reference.validate()?;
        if reference.is_none() {
            return Ok(None);
        }
        if reference.world != Some(self.world) {
            return Err(EngineError::new(ErrorCode::WrongScope, "混合参照のWorld"));
        }
        self.entries
            .get(reference.index as usize - 1)
            .map(Some)
            .ok_or_else(|| invalid("混合参照のindex"))
    }
    /// 固定キーへの書戻しでも既存indexと履歴を保持し、専用取引以外からの履歴変更を拒否します。
    pub(super) fn validate_replacement(&self, world: &World) -> EngineResult<()> {
        self.scope(world)?;
        if world
            .state
            .records()
            .any(|record| record.key == Self::key())
        {
            let current = world.state::<Self>(Self::key())?;
            current.scope(world)?;
            if self.entries != current.entries || self.receipts != current.receipts {
                return Err(conflict("混合storeの専用取引外での定義または履歴変更"));
            }
        } else if !self.entries.is_empty() || !self.receipts.is_empty() {
            return Err(conflict("混合storeの非空状態での初期化"));
        }
        Ok(())
    }
    fn scope(&self, world: &World) -> EngineResult<()> {
        if self.world != world.id() {
            return Err(EngineError::new(ErrorCode::WrongScope, "混合storeのWorld"));
        }
        if self.content != world.content().fingerprint() {
            return Err(conflict("混合storeのContent"));
        }
        for entry in &self.entries {
            entry.validate_content(world.content())?;
        }
        Ok(())
    }
    /// carrier一cellの量を純物質別へ正確に展開します。空cellと組成参照の併用を拒否します。
    pub fn quantities(
        &self,
        content: &Content,
        material: MaterialId,
        reference: MixtureRef,
    ) -> EngineResult<Vec<MaterialComponentQuantity>> {
        if self.content != content.fingerprint() {
            return Err(conflict("混合数量のContent"));
        }
        let mut output = Vec::new();
        self.append_quantities(content, material, reference, &mut output)?;
        Ok(output)
    }
    pub(super) fn append_quantities(
        &self,
        content: &Content,
        material: MaterialId,
        reference: MixtureRef,
        output: &mut Vec<MaterialComponentQuantity>,
    ) -> EngineResult<()> {
        let composition = self.resolve(reference)?;
        let Some(carrier) = content.material(material)? else {
            return if composition.is_none() {
                Ok(())
            } else {
                Err(invalid("空cellに混合参照"))
            };
        };
        if let Some(composition) = composition {
            let mut denominator = 0u128;
            for part in &composition.parts {
                let id = content.material_id(super::MaterialHandle(part.material))?;
                let mass = content
                    .material(id)?
                    .ok_or_else(|| invalid("混合成分が空"))?
                    .mass_per_cell();
                denominator = denominator
                    .checked_add(u128::from(part.ratio) * u128::from(mass))
                    .ok_or_else(|| invalid("混合換算の上限"))?;
            }
            output
                .try_reserve(composition.parts.len())
                .map_err(allocation)?;
            for part in &composition.parts {
                let id = content.material_id(super::MaterialHandle(part.material))?;
                let mass = content
                    .material(id)?
                    .ok_or_else(|| invalid("混合成分が空"))?
                    .mass_per_cell();
                let tokens = MaterialQuantity::new(
                    u128::from(carrier.mass_per_cell()) * u128::from(part.ratio),
                    denominator,
                )?;
                output.push(MaterialComponentQuantity {
                    material: part.material,
                    tokens,
                    mass: tokens.checked_mul(mass.into())?,
                });
            }
        } else {
            output.try_reserve(1).map_err(allocation)?;
            output.push(MaterialComponentQuantity {
                material: carrier.key(),
                tokens: MaterialQuantity::ONE,
                mass: MaterialQuantity::new(carrier.mass_per_cell().into(), 1)?,
            });
        }
        Ok(())
    }
    fn aggregate(
        &self,
        content: &Content,
        changes: &[MixtureCellChange],
        output: bool,
    ) -> EngineResult<Vec<MaterialComponentQuantity>> {
        let mut amounts = Vec::new();
        for change in changes {
            self.append_quantities(
                content,
                if output {
                    change.cell.output.material
                } else {
                    change.cell.expected.material
                },
                if output {
                    change.output
                } else {
                    change.expected
                },
                &mut amounts,
            )?;
        }
        amounts.sort_unstable_by_key(|amount| amount.material);
        let mut length = 0;
        for index in 0..amounts.len() {
            if length != 0 && amounts[length - 1].material == amounts[index].material {
                amounts[length - 1].tokens = amounts[length - 1]
                    .tokens
                    .checked_add(amounts[index].tokens)?;
                amounts[length - 1].mass =
                    amounts[length - 1].mass.checked_add(amounts[index].mass)?;
            } else {
                amounts[length] = amounts[index];
                length += 1;
            }
        }
        amounts.truncate(length);
        Ok(amounts)
    }
    /// 履歴とjournalの参照、過去の変更列と成分保存を監査します。現在の全表現の総量とは別です。
    /// journalを種類ごとに一度読み、全履歴を監査します。各cellのhot pathでは呼びません。
    pub fn validate_world(&self, world: &World) -> EngineResult<()> {
        self.scope(world)?;
        let mut journals: Vec<(StableKey, MaterialReceiptLog)> = Vec::new();
        for witness in &self.receipts {
            let index = match journals.binary_search_by_key(&witness.journal, |entry| entry.0) {
                Ok(index) => index,
                Err(index) => {
                    let log: MaterialReceiptLog = world.state(witness.journal)?;
                    if log.key() != witness.journal {
                        return Err(conflict("混合witnessとjournalのキーが不一致"));
                    }
                    log.validate_world(world)?;
                    journals.try_reserve(1).map_err(allocation)?;
                    journals.insert(index, (witness.journal, log));
                    index
                }
            };
            let log = &journals[index].1;
            let receipt = log
                .receipts()
                .nth(usize::try_from(witness.sequence).map_err(|_| invalid("混合receipt番号"))?)
                .ok_or_else(|| invalid("混合receiptがjournalにない"))?;
            let layer = world.content().layer_id(receipt.layer())?;
            validate_channel(world, layer, witness.channel)?;
            if receipt.changes().len() != witness.changes.len() {
                return Err(invalid("混合receiptの変更数"));
            }
            let mut changes = Vec::new();
            changes
                .try_reserve_exact(witness.changes.len())
                .map_err(allocation)?;
            for (cell, refs) in receipt.changes().iter().zip(&witness.changes) {
                if cell.point != refs.point {
                    return Err(invalid("混合receiptの座標"));
                }
                changes.push(MixtureCellChange {
                    cell: *cell,
                    expected: refs.expected,
                    output: refs.output,
                });
            }
            let before = self.aggregate(world.content(), &changes, false)?;
            let after = self.aggregate(world.content(), &changes, true)?;
            if witness.conservation == MixtureConservation::PreserveComponents
                && (receipt.reason().kind != MaterialReceiptKind::Conversion || before != after)
            {
                return Err(invalid("混合receiptの成分収支"));
            }
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    /// 正規化した組成を共有storeへ登録し、既存の同じ組成には同じ参照を返します。
    pub fn register_mixture(
        &mut self,
        schema: StableKey,
        state: &MixtureState,
    ) -> EngineResult<MixtureRef> {
        self.run(|editor| {
            validate_store_schema(editor.world, schema)?;
            let mut store: MixtureStore = editor.world.state(MixtureStore::key())?;
            store.scope(editor.world)?;
            state.validate_content(editor.world.content())?;
            if state.parts.len() > store.limits.components as usize {
                return Err(EngineError::capacity(
                    "混合の素材種類数",
                    state.parts.len() as u64,
                    store.limits.components.into(),
                ));
            }
            if let Some(index) = store.entries.iter().position(|entry| entry == state) {
                return Ok(MixtureRef {
                    world: Some(store.world),
                    index: index as u32 + 1,
                });
            }
            if store.entries.len() == store.limits.entries as usize {
                return Err(EngineError::capacity(
                    "混合の定義数",
                    store.entries.len() as u64 + 1,
                    store.limits.entries.into(),
                ));
            }
            store.entries.try_reserve(1).map_err(allocation)?;
            store.entries.push(state.try_copy()?);
            let reference = MixtureRef {
                world: Some(store.world),
                index: store.entries.len() as u32,
            };
            editor.set_material_state(MixtureStore::key(), schema, &store)?;
            Ok(reference)
        })
    }
    /// 混合・分離等の全入出力、組成参照、cell receiptとwitnessを同じ取引で公開します。
    /// 保持する各payloadの意味は作者が選び、外部storeの個体を暗黙に消費しません。
    /// 戻り値は外側のWorld::edit成功までは候補であり、確定証明ではありません。
    pub fn apply_mixture_changes(
        &mut self,
        layer: LayerId,
        config: MixtureEditConfig,
        reason: MaterialChangeReason,
        conservation: MixtureConservation,
        changes: &[MixtureCellChange],
    ) -> EngineResult<MaterialReceipt> {
        self.run(|editor| {
            validate_channel(editor.world, layer, config.channel)?;
            validate_store_schema(editor.world, config.schema)?;
            let mut store: MixtureStore = editor.world.state(MixtureStore::key())?;
            store.scope(editor.world)?;
            if store.receipts.len() == store.limits.receipts as usize {
                return Err(EngineError::capacity(
                    "混合receipt件数",
                    store.receipts.len() as u64 + 1,
                    store.limits.receipts.into(),
                ));
            }
            let origin = editor
                .world
                .channel_layout::<MixtureRef>(config.channel)?
                .origin;
            let mut ordered = super::terrain::copy_slice(changes)?;
            ordered.sort_unstable_by_key(|change| (change.cell.point.x, change.cell.point.y));
            if ordered
                .windows(2)
                .any(|pair| pair[0].cell.point == pair[1].cell.point)
            {
                return Err(conflict("混合変更の座標重複"));
            }
            let mut cores = Vec::new();
            cores.try_reserve_exact(ordered.len()).map_err(allocation)?;
            let mut refs = Vec::new();
            refs.try_reserve_exact(ordered.len()).map_err(allocation)?;
            for change in &ordered {
                let cell = editor.loaded(layer, change.cell.point)?;
                if cell.block.is_some() || cell.core != change.cell.expected {
                    return Err(conflict("混合変更のcell期待値が失効"));
                }
                match editor.world.channel_value::<MixtureRef>(
                    config.channel,
                    channel_point(change.cell.point, origin)?,
                )? {
                    ChannelQuery::Loaded(value) if value == change.expected => (),
                    _ => return Err(conflict("混合変更の参照期待値が失効")),
                }
                cores.push(change.cell);
                refs.push(MixtureReferenceChange {
                    point: change.cell.point,
                    expected: change.expected,
                    output: change.output,
                });
            }
            let before = store.aggregate(editor.world.content(), &ordered, false)?;
            let after = store.aggregate(editor.world.content(), &ordered, true)?;
            if conservation == MixtureConservation::PreserveComponents
                && (reason.kind != MaterialReceiptKind::Conversion || before != after)
            {
                return Err(conflict("混合・分離の成分収支が不一致"));
            }
            let receipt = editor.apply_material_changes(layer, config.journal, reason, &cores)?;
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for change in &ordered {
                updates.push((channel_point(change.cell.point, origin)?, change.output));
            }
            let default = editor
                .world
                .validate_channel_updates(config.channel, &updates)?;
            for clearing in [true, false] {
                for &(point, value) in &updates {
                    if (value == default) == clearing {
                        editor.set_channel(config.channel, point, value)?;
                    }
                }
            }
            store.receipts.try_reserve(1).map_err(allocation)?;
            store.receipts.push(MixtureReceiptWitness {
                journal: config.journal.key,
                sequence: receipt.sequence(),
                channel: config.channel,
                conservation,
                changes: refs,
            });
            editor.set_material_state(MixtureStore::key(), config.schema, &store)?;
            Ok(receipt)
        })
    }
}
fn validate_store_schema(world: &World, schema: StableKey) -> EngineResult<()> {
    let record = world
        .state
        .records()
        .find(|record| record.key == MixtureStore::key())
        .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "混合store"))?;
    if record.schema != schema {
        return Err(EngineError::new(
            ErrorCode::SchemaMismatch,
            "混合storeのschema",
        ));
    }
    Ok(())
}
fn validate_channel(world: &World, layer: LayerId, key: StableKey) -> EngineResult<()> {
    if world.channel_material_layer::<MixtureRef>(key)? != Some(world.content().layer(layer)?.key)
        || world.validate_channel_updates::<MixtureRef>(key, &[])? != MixtureRef::NONE
    {
        return Err(invalid(
            "混合参照は対象layerの既定値NONEの物質channelが必要",
        ));
    }
    Ok(())
}
fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("混合channelのx座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("混合channelのy座標"))?,
    })
}
fn validate_limits(limits: MixtureLimits) -> EngineResult<()> {
    if limits.entries == 0 || limits.components < 2 || limits.receipts == 0 {
        Err(invalid("混合storeの容量"))
    } else {
        Ok(())
    }
}
fn bounded_count(input: &mut Decoder<'_>, minimum: usize) -> EngineResult<usize> {
    let count = input.u32()? as usize;
    if count > input.remaining() / minimum {
        Err(invalid("混合recordの件数"))
    } else {
        Ok(count)
    }
}
impl StateCodec for MixtureStore {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHMIX001")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u32(self.limits.entries)?;
        out.u32(self.limits.components)?;
        out.u32(self.limits.receipts)?;
        out.u32(self.entries.len() as u32)?;
        for entry in &self.entries {
            entry.encode(out)?;
        }
        out.u32(self.receipts.len() as u32)?;
        for receipt in &self.receipts {
            receipt.journal.encode(out)?;
            out.u64(receipt.sequence)?;
            receipt.channel.encode(out)?;
            out.u8(match receipt.conservation {
                MixtureConservation::PreserveComponents => 0,
                MixtureConservation::RecordConversion => 1,
            })?;
            out.u32(
                u32::try_from(receipt.changes.len())
                    .map_err(|_| invalid("混合receiptの変更数上限"))?,
            )?;
            for change in &receipt.changes {
                change.point.encode(out)?;
                change.expected.encode(out)?;
                change.output.encode(out)?;
            }
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHMIX001" {
            return Err(invalid("混合storeのmagic"));
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("混合Content"))?;
        let limits = MixtureLimits {
            entries: input.u32()?,
            components: input.u32()?,
            receipts: input.u32()?,
        };
        validate_limits(limits)?;
        let count = bounded_count(input, 12)?;
        if count > limits.entries as usize {
            return Err(invalid("混合定義の保存上限"));
        }
        let mut entries = Vec::new();
        entries.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            let entry = MixtureState::decode_limited(input, limits.components as usize)?;
            if entry.parts.len() > limits.components as usize {
                return Err(invalid("混合成分の保存上限"));
            }
            entries.push(entry);
        }
        // 参照indexはappend順なので並べ替えず、索引だけを一時的に整列して重複を検査します。
        let mut sorted = Vec::new();
        sorted
            .try_reserve_exact(entries.len())
            .map_err(allocation)?;
        sorted.extend(entries.iter());
        sorted.sort_unstable_by(|a, b| a.parts.cmp(&b.parts));
        if sorted.windows(2).any(|pair| pair[0].parts == pair[1].parts) {
            return Err(invalid("共有混合定義の重複"));
        }
        let count = bounded_count(input, 24)?;
        if count > limits.receipts as usize {
            return Err(invalid("混合receiptの保存上限"));
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            let journal = StableKey::decode(input)?;
            let sequence = input.u64()?;
            let channel = StableKey::decode(input)?;
            let conservation = match input.u8()? {
                0 => MixtureConservation::PreserveComponents,
                1 => MixtureConservation::RecordConversion,
                _ => return Err(invalid("混合収支方針")),
            };
            let count = bounded_count(input, 18)?;
            if count == 0 {
                return Err(invalid("空の混合receipt"));
            }
            let mut changes = Vec::new();
            changes.try_reserve_exact(count).map_err(allocation)?;
            for _ in 0..count {
                changes.push(MixtureReferenceChange {
                    point: CellPoint::decode(input)?,
                    expected: MixtureRef::decode(input)?,
                    output: MixtureRef::decode(input)?,
                });
            }
            if changes.windows(2).any(|pair| {
                (pair[0].point.x, pair[0].point.y) >= (pair[1].point.x, pair[1].point.y)
            }) {
                return Err(invalid("混合receiptの座標順"));
            }
            receipts.push(MixtureReceiptWitness {
                journal,
                sequence,
                channel,
                conservation,
                changes,
            });
        }
        let store = Self {
            world,
            content,
            limits,
            entries,
            receipts,
        };
        for receipt in &store.receipts {
            for change in &receipt.changes {
                store.resolve(change.expected)?;
                store.resolve(change.output)?;
            }
        }
        // 同一journalの番号は増加させ、過去のwitnessを別の変換へ付け替えません。
        let mut order = Vec::new();
        order
            .try_reserve_exact(store.receipts.len())
            .map_err(allocation)?;
        for (index, receipt) in store.receipts.iter().enumerate() {
            order.push((receipt.journal, index, receipt.sequence));
        }
        order.sort_unstable_by_key(|value| (value.0, value.1));
        if order
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0 && pair[0].2 >= pair[1].2)
        {
            return Err(invalid("混合receiptの番号順"));
        }
        Ok(store)
    }
}
