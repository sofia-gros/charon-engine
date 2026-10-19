//! cellの発行・消費・変換と、理由付きreceiptの原子的な保存。
//!
//! receiptは変更前後のcellと物質量を保存し、実際の変更と同じWorld取引で公開します。
//! 全表現の総量を自動収集する台帳や、署名された外部監査証明とは区別します。

use super::{CellCore, LayerId, MaterialId, World, WorldEdit, conflict, invalid};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 物質の変化の区分。現実のmass保存を作者のゲーム規則へ強制しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialReceiptKind {
    /// 空cellだけを入力とし、物質を発行します。
    Source,
    /// 物質を消費し、全出力を空cellにします。
    Sink,
    /// 非空の入力・出力を持つ変換です。mass差もreceiptから検査できます。
    Conversion,
}
impl MaterialReceiptKind {
    fn tag(self) -> u8 {
        match self {
            Self::Source => 0,
            Self::Sink => 1,
            Self::Conversion => 2,
        }
    }
    fn from_tag(tag: u8) -> EngineResult<Self> {
        match tag {
            0 => Ok(Self::Source),
            1 => Ok(Self::Sink),
            2 => Ok(Self::Conversion),
            _ => Err(invalid("物質receiptの区分")),
        }
    }
}

/// 作者が版管理する発行・消費・変換の理由です。特定ゲームのrecipeを固定しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialChangeReason {
    /// ruleまたはイベントの安定キー。
    pub key: StableKey,
    /// 意味を変える場合に更新する、0以外の版。
    pub version: u32,
    /// 入出力と一致する変化の区分。
    pub kind: MaterialReceiptKind,
}

/// 変換先で、そのlayerの全物質channelをどう扱うかを明示します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialPayloadPolicy {
    /// 現在の値を保持します。作者の移行は同じ取引でset_channelしてから指定できます。
    KeepChannels,
    /// 全物質channelを各定義の既定値へ戻します。位置固定場には触れません。
    /// channelが参照する外部storeの個体までは破棄せず、その寿命管理は作者が行います。
    ResetChannels,
}

/// 一つのcellの期待値と出力。全点の期待値を検査してから一括変更します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialCellChange {
    /// 変更するWorld cell座標。
    pub point: CellPoint,
    /// flags・variationを含む、現在の値に対する期待値。
    pub expected: CellCore,
    /// 変更後の値。空はCellCore::AIRを使います。
    pub output: CellCore,
    /// 全物質channelの保持・初期化方針。
    pub payload: MaterialPayloadPolicy,
}

/// 一つの素材についてreceiptが記録する整数量です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialReceiptAmount {
    /// Content内の安定した素材キー。
    pub material: StableKey,
    /// 占有cellに対応するトークン数。空cellは数えません。
    pub tokens: u64,
    /// token数と登録済みmass_per_cellの積。overflowを避けてu128で保存します。
    pub mass: u128,
}

/// World編集候補へ記録する、一つのreceiptです。
/// 外側のWorld::editが成功した場合だけ、変更とjournalがともに確定します。
/// callback内部で得た値を持ち出しても、失敗した取引の確定証明にはなりません。
pub struct MaterialReceipt {
    sequence: u64,
    tick: u64,
    epoch: u64,
    layer: StableKey,
    reason: MaterialChangeReason,
    changes: Vec<MaterialCellChange>,
    consumed: Vec<MaterialReceiptAmount>,
    produced: Vec<MaterialReceiptAmount>,
}
impl MaterialReceipt {
    /// journal内で0から増える番号。World・journalキーとの組で識別します。
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// 変換したWorld tick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 変換時の素材remap epoch。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// 対象layerの安定キー。
    pub fn layer(&self) -> StableKey {
        self.layer
    }
    /// 発行・消費・変換の理由。
    pub fn reason(&self) -> MaterialChangeReason {
        self.reason
    }
    /// 座標順の変更前後とpayload方針。
    pub fn changes(&self) -> &[MaterialCellChange] {
        &self.changes
    }
    /// 消費した素材別の量。安定キー順です。
    pub fn consumed(&self) -> &[MaterialReceiptAmount] {
        &self.consumed
    }
    /// 発行した素材別の量。安定キー順です。
    pub fn produced(&self) -> &[MaterialReceiptAmount] {
        &self.produced
    }
    /// 消費した換算massの合計。
    pub fn consumed_mass(&self) -> EngineResult<u128> {
        total_mass(&self.consumed)
    }
    /// 発行した換算massの合計。差を0へ強制しません。
    pub fn produced_mass(&self) -> EngineResult<u128> {
        total_mass(&self.produced)
    }
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            sequence: self.sequence,
            tick: self.tick,
            epoch: self.epoch,
            layer: self.layer,
            reason: self.reason,
            changes: super::terrain::copy_slice(&self.changes)?,
            consumed: super::terrain::copy_slice(&self.consumed)?,
            produced: super::terrain::copy_slice(&self.produced)?,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if self.reason.version == 0
            || self.changes.is_empty()
            || self.changes.windows(2).any(|pair| {
                (pair[0].point.x, pair[0].point.y) >= (pair[1].point.x, pair[1].point.y)
            })
        {
            return Err(invalid("物質receiptの版・変更列"));
        }
        for change in &self.changes {
            for core in [change.expected, change.output] {
                if core.flags & !7 != 0
                    || (core.material == MaterialId::AIR && core != CellCore::AIR)
                {
                    return Err(invalid("物質receiptのcell値"));
                }
            }
        }
        validate_amounts(&self.consumed)?;
        validate_amounts(&self.produced)?;
        let valid = match self.reason.kind {
            MaterialReceiptKind::Source => self.consumed.is_empty() && !self.produced.is_empty(),
            MaterialReceiptKind::Sink => !self.consumed.is_empty() && self.produced.is_empty(),
            MaterialReceiptKind::Conversion => {
                !self.consumed.is_empty() && !self.produced.is_empty()
            }
        };
        if !valid {
            return Err(invalid("物質receiptの区分と入出力量が不一致"));
        }
        Ok(())
    }
}

/// World StateStore上のjournal recordと、その登録済みschemaを指定します。
#[derive(Clone, Copy, Debug)]
pub struct MaterialReceiptJournal {
    /// MaterialReceiptLog::newへ渡したjournalキー。
    pub key: StableKey,
    /// MaterialReceiptLogの型schema。record上限も作者が設定します。
    pub schema: StableKey,
}

/// 破棄しない上限付きのreceipt列。型schemaを登録し、Worldの作者状態へ保存します。
/// 容量不足では変換も止めます。自動削除・外部保存の成功の捏造は行いません。
pub struct MaterialReceiptLog {
    world: WorldId,
    content: [u8; 32],
    key: StableKey,
    capacity: u32,
    receipts: Vec<MaterialReceipt>,
}
impl MaterialReceiptLog {
    /// 現在のWorldとContentへ所属する空journalを作ります。set_stateで保存してください。
    pub fn new(world: &World, key: StableKey, capacity: u32) -> EngineResult<Self> {
        if capacity == 0 {
            return Err(invalid("物質receiptの保存件数"));
        }
        Ok(Self {
            world: world.id(),
            content: world.content().fingerprint(),
            key,
            capacity,
            receipts: Vec::new(),
        })
    }
    /// journalの安定キー。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 保存されたreceipt数。
    pub fn len(&self) -> usize {
        self.receipts.len()
    }
    /// receiptがまだないか。
    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
    /// journalが保持できるreceipt件数の上限。
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
    /// 次のreceipt番号。過去のreceiptを消さないため件数と一致します。
    pub fn next_sequence(&self) -> u64 {
        self.receipts.len() as u64
    }
    /// 保存順のreceipt。内部列への可変参照は返しません。
    pub fn receipts(&self) -> impl Iterator<Item = &MaterialReceipt> {
        self.receipts.iter()
    }
    /// 既存recordを失わずに件数上限を変更します。schema・StateStore byte上限は独立です。
    pub fn set_capacity(&mut self, capacity: u32) -> EngineResult<()> {
        if capacity == 0 || (capacity as usize) < self.receipts.len() {
            return Err(invalid("既存receipt数を下回る保存件数"));
        }
        self.capacity = capacity;
        Ok(())
    }
    fn validate(&self) -> EngineResult<()> {
        if self.capacity == 0 || self.receipts.len() > self.capacity as usize {
            return Err(invalid("物質receipt logの容量"));
        }
        let mut tick = 0;
        let mut epoch = 0;
        for (index, receipt) in self.receipts.iter().enumerate() {
            receipt.validate()?;
            if receipt.sequence != index as u64 || receipt.tick < tick || receipt.epoch < epoch {
                return Err(invalid("物質receiptの番号・時刻・epoch順"));
            }
            tick = receipt.tick;
            epoch = receipt.epoch;
        }
        Ok(())
    }
    /// World・Content・過去の素材参照と変更列からの集計を照合します。
    /// 履歴と現在の全表現の総量が一致するかの検査や、変更の再実行ではありません。
    pub fn validate_world(&self, world: &World) -> EngineResult<()> {
        self.validate_for(world, self.key)
    }
    fn validate_for(&self, world: &World, key: StableKey) -> EngineResult<()> {
        self.validate()?;
        if self.world != world.id() {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "物質receiptのWorld",
            ));
        }
        if self.key != key || self.content != world.content().fingerprint() {
            return Err(conflict("物質receiptのjournalまたはContentが不一致"));
        }
        for receipt in &self.receipts {
            world.content.layer_id(receipt.layer)?;
            if receipt.tick > world.tick() || receipt.epoch > world.remaps.epoch() {
                return Err(conflict("物質receiptが現在より未来"));
            }
            for change in &receipt.changes {
                change.expected.validate(world.content())?;
                change.output.validate(world.content())?;
                if !world.bounds().contains(change.point) {
                    return Err(invalid("物質receiptの座標範囲"));
                }
            }
            if aggregate(world, &receipt.changes, false)? != receipt.consumed
                || aggregate(world, &receipt.changes, true)? != receipt.produced
            {
                return Err(invalid("物質receiptの変更列と集計が不一致"));
            }
        }
        Ok(())
    }
}

impl WorldEdit<'_> {
    /// 全cellの期待値と出力を検査し、理由付きreceiptと同じ取引で一括公開します。
    ///
    /// block・未読込・境界外・座標重複を拒否します。各点の全物質channelについて
    /// 保持か既定値への初期化を明示し、位置固定場を巻き込みません。独自のpayload
    /// 移行は同じedit内で準備してKeepChannelsを指定できます。外部storeの個体は
    /// 自動破棄しません。receiptにはその方針と変更前後のcellを記録します。
    ///
    /// journal件数、schema、StateStore、通常編集容量の不足ではreceiptも物質も
    /// 公開しません。callbackがエラーを無視しても先行編集を含めて取引を破棄します。
    /// これはcell表現の変換APIであり、rawなset_cellの全使用を監視する機能や、
    /// 退避領域・粒子・剛体・reservoirを集計済みの全World台帳ではありません。
    pub fn apply_material_changes(
        &mut self,
        layer: LayerId,
        journal: MaterialReceiptJournal,
        reason: MaterialChangeReason,
        changes: &[MaterialCellChange],
    ) -> EngineResult<MaterialReceipt> {
        self.run(|editor| {
            let layer_key = editor.world.content.layer(layer)?.key;
            let mut log = editor.world.state::<MaterialReceiptLog>(journal.key)?;
            log.validate_for(editor.world, journal.key)?;
            if log.receipts.len() >= log.capacity as usize {
                return Err(EngineError::capacity(
                    "物質receipt数",
                    log.receipts.len() as u64 + 1,
                    log.capacity.into(),
                ));
            }
            if changes.is_empty() || reason.version == 0 {
                return Err(invalid("物質変更の入力または理由の版"));
            }
            let channels = editor
                .world
                .channels
                .iter()
                .filter(|channel| channel.material_layer() == Some(layer_key))
                .count();
            let reset_count = changes
                .iter()
                .filter(|change| change.payload == MaterialPayloadPolicy::ResetChannels)
                .count();
            let writes = reset_count
                .checked_mul(channels)
                .and_then(|count| count.checked_add(changes.len()))
                .ok_or_else(|| invalid("物質変更の書込数overflow"))?;
            editor.reserve_writes(writes)?;
            let mut ordered = super::terrain::copy_slice(changes)?;
            ordered.sort_unstable_by_key(|change| (change.point.x, change.point.y));
            if ordered
                .windows(2)
                .any(|pair| pair[0].point == pair[1].point)
            {
                return Err(conflict("物質変更の座標重複"));
            }
            let mut reset = Vec::new();
            reset.try_reserve_exact(reset_count).map_err(allocation)?;
            for change in &ordered {
                change.expected.validate(editor.world.content())?;
                change.output.validate(editor.world.content())?;
                let cell = editor.loaded(layer, change.point)?;
                if cell.block.is_some() {
                    return Err(conflict("block個体は分解前に変換できない"));
                }
                if cell.core != change.expected {
                    return Err(conflict("物質変更の期待値が失効"));
                }
                if change.payload == MaterialPayloadPolicy::ResetChannels {
                    reset.push(change.point);
                }
            }
            let receipt = MaterialReceipt {
                sequence: log.next_sequence(),
                tick: editor.world.tick(),
                epoch: editor.world.remaps.epoch(),
                layer: layer_key,
                reason,
                consumed: aggregate(editor.world, &ordered, false)?,
                produced: aggregate(editor.world, &ordered, true)?,
                changes: ordered,
            };
            receipt.validate()?;
            log.receipts.try_reserve(1).map_err(allocation)?;
            log.receipts.push(receipt.try_copy()?);
            // codecと保存容量を先に検査します。後の地形編集が失敗した場合も、
            // この候補recordだけが残らないよう外側のWorld取引へ含めます。
            editor.set_state(journal.key, journal.schema, &log)?;
            for channel in &mut editor.world.channels {
                channel.reset_material(layer_key, &reset)?;
            }
            for change in &receipt.changes {
                editor.write(layer, change.point, change.output, None)?;
            }
            Ok(receipt)
        })
    }
}

fn aggregate(
    world: &World,
    changes: &[MaterialCellChange],
    output: bool,
) -> EngineResult<Vec<MaterialReceiptAmount>> {
    let mut ids = Vec::new();
    ids.try_reserve_exact(changes.len()).map_err(allocation)?;
    for change in changes {
        let material = if output {
            change.output.material
        } else {
            change.expected.material
        };
        if material != MaterialId::AIR {
            ids.push(material);
        }
    }
    ids.sort_unstable();
    let mut amounts = Vec::new();
    let mut index = 0;
    while index < ids.len() {
        let material = ids[index];
        let start = index;
        while index < ids.len() && ids[index] == material {
            index += 1;
        }
        let definition = world
            .content
            .material(material)?
            .ok_or_else(|| invalid("receiptの空素材"))?;
        let tokens =
            u64::try_from(index - start).map_err(|_| invalid("receiptのtoken数overflow"))?;
        amounts.try_reserve(1).map_err(allocation)?;
        amounts.push(MaterialReceiptAmount {
            material: definition.key(),
            tokens,
            mass: u128::from(tokens) * u128::from(definition.mass_per_cell()),
        });
    }
    amounts.sort_unstable_by_key(|amount| amount.material);
    Ok(amounts)
}
fn validate_amounts(amounts: &[MaterialReceiptAmount]) -> EngineResult<()> {
    if amounts
        .windows(2)
        .any(|pair| pair[0].material >= pair[1].material)
    {
        return Err(invalid("receiptの素材集計順"));
    }
    let mut tokens = 0u64;
    for amount in amounts {
        if amount.tokens == 0
            || amount.mass % u128::from(amount.tokens) != 0
            || !(1..=u128::from(u32::MAX)).contains(&(amount.mass / u128::from(amount.tokens)))
        {
            return Err(invalid("receiptのtokenとmass"));
        }
        tokens = tokens
            .checked_add(amount.tokens)
            .ok_or_else(|| invalid("receiptのtoken合計overflow"))?;
    }
    total_mass(amounts)?;
    Ok(())
}
fn total_mass(amounts: &[MaterialReceiptAmount]) -> EngineResult<u128> {
    amounts.iter().try_fold(0u128, |sum, amount| {
        sum.checked_add(amount.mass)
            .ok_or_else(|| invalid("receiptのmass合計overflow"))
    })
}
fn encode_amounts(out: &mut Encoder, amounts: &[MaterialReceiptAmount]) -> EngineResult<()> {
    out.u32(u32::try_from(amounts.len()).map_err(|_| invalid("receiptの素材数"))?)?;
    for amount in amounts {
        amount.material.encode(out)?;
        out.u64(amount.tokens)?;
        out.raw(&amount.mass.to_le_bytes())?;
    }
    Ok(())
}
fn decode_amounts(input: &mut Decoder<'_>) -> EngineResult<Vec<MaterialReceiptAmount>> {
    let count = input.u32()? as usize;
    if count > input.remaining() / 24 {
        return Err(invalid("receipt素材列の切断"));
    }
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(allocation)?;
    for _ in 0..count {
        result.push(MaterialReceiptAmount {
            material: StableKey::decode(input)?,
            tokens: input.u64()?,
            mass: u128::from_le_bytes(input.raw(16)?.try_into().expect("16byteのmass")),
        });
    }
    Ok(result)
}
impl StateCodec for MaterialReceiptLog {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.raw(b"CHRCP001")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        self.key.encode(out)?;
        out.u32(self.capacity)?;
        out.u32(self.receipts.len() as u32)?;
        for receipt in &self.receipts {
            out.u64(receipt.sequence)?;
            out.u64(receipt.tick)?;
            out.u64(receipt.epoch)?;
            receipt.layer.encode(out)?;
            receipt.reason.key.encode(out)?;
            out.u32(receipt.reason.version)?;
            out.u8(receipt.reason.kind.tag())?;
            out.u32(u32::try_from(receipt.changes.len()).map_err(|_| invalid("receiptの変更数"))?)?;
            for change in &receipt.changes {
                change.point.encode(out)?;
                change.expected.encode(out)?;
                change.output.encode(out)?;
                out.u8(match change.payload {
                    MaterialPayloadPolicy::KeepChannels => 0,
                    MaterialPayloadPolicy::ResetChannels => 1,
                })?;
            }
            encode_amounts(out, &receipt.consumed)?;
            encode_amounts(out, &receipt.produced)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHRCP001" {
            return Err(invalid("物質receipt logの形式"));
        }
        let world = WorldId::decode(input)?;
        let mut content = [0; 32];
        content.copy_from_slice(input.raw(32)?);
        let key = StableKey::decode(input)?;
        let capacity = input.u32()?;
        let count = input.u32()? as usize;
        if count > capacity as usize || count > input.remaining() / 42 {
            return Err(invalid("物質receipt列の容量または切断"));
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            let sequence = input.u64()?;
            let tick = input.u64()?;
            let epoch = input.u64()?;
            let layer = StableKey::decode(input)?;
            let reason = MaterialChangeReason {
                key: StableKey::decode(input)?,
                version: input.u32()?,
                kind: MaterialReceiptKind::from_tag(input.u8()?)?,
            };
            let count = input.u32()? as usize;
            if count > input.remaining() / 25 {
                return Err(invalid("receipt変更列の切断"));
            }
            let mut changes = Vec::new();
            changes.try_reserve_exact(count).map_err(allocation)?;
            for _ in 0..count {
                let point = CellPoint::decode(input)?;
                let expected = CellCore::decode(input)?;
                let output = CellCore::decode(input)?;
                let payload = match input.u8()? {
                    0 => MaterialPayloadPolicy::KeepChannels,
                    1 => MaterialPayloadPolicy::ResetChannels,
                    _ => return Err(invalid("receiptのpayload方針")),
                };
                changes.push(MaterialCellChange {
                    point,
                    expected,
                    output,
                    payload,
                });
            }
            let consumed = decode_amounts(input)?;
            let produced = decode_amounts(input)?;
            receipts.push(MaterialReceipt {
                sequence,
                tick,
                epoch,
                layer,
                reason,
                changes,
                consumed,
                produced,
            });
        }
        let result = Self {
            world,
            content,
            key,
            capacity,
            receipts,
        };
        result.validate()?;
        Ok(result)
    }
}
