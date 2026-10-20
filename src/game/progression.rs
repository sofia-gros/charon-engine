//! 作者が登録する進行resourceと、world eventの一度だけ適用を保存します。
//!
//! 固定のstory flagや勝利条件は持たず、stable key・値域・初期値を作者が登録します。
//! event receiptはStateStore checkpointへ含まれ、復元後の再配送で同じ変更を二重適用しません。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use crate::game::events::WorldEvent;

/// 作者登録resourceの値形式です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressionKind {
    /// on/offのstory、quest、world状態です。
    Flag,
    /// 作者が用途と単位を定義する整数counterです。
    Counter,
}

/// 起動時に登録する進行値のkey・型・値域です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressionDefinition {
    /// resourceを識別する安定keyです。
    pub key: StableKey,
    /// flagかinteger counterかを固定します。
    pub kind: ProgressionKind,
    /// World作成時の初期値です。
    pub initial: i64,
    /// counterのinclusive最小値です。flagでは0でなければなりません。
    pub minimum: i64,
    /// counterのinclusive最大値です。flagでは1でなければなりません。
    pub maximum: i64,
}

/// 登録済み進行定義をstable key順で検索するcatalogです。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgressionCatalog {
    definitions: Vec<ProgressionDefinition>,
}

impl ProgressionCatalog {
    /// key重複、不正な初期値、逆転した値域を拒否してcatalogを作ります。
    pub fn new(mut definitions: Vec<ProgressionDefinition>) -> Result<Self, ProgressionError> {
        definitions.sort_unstable_by_key(|definition| definition.key);
        if definitions
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(ProgressionError::DuplicateResource);
        }
        if definitions.iter().any(|definition| {
            definition.minimum > definition.maximum
                || definition.initial < definition.minimum
                || definition.initial > definition.maximum
                || match definition.kind {
                    ProgressionKind::Flag => {
                        definition.minimum != 0
                            || definition.maximum != 1
                            || !(0..=1).contains(&definition.initial)
                    }
                    ProgressionKind::Counter => false,
                }
        }) {
            return Err(ProgressionError::InvalidDefinition);
        }
        Ok(Self { definitions })
    }

    /// 登録定義をkeyで取得します。
    pub fn get(&self, key: StableKey) -> Result<&ProgressionDefinition, ProgressionError> {
        self.definitions
            .binary_search_by_key(&key, |definition| definition.key)
            .map(|index| &self.definitions[index])
            .map_err(|_| ProgressionError::UnknownResource(Box::new(key)))
    }

    /// stable key順の全定義を返します。
    pub fn iter(&self) -> impl Iterator<Item = &ProgressionDefinition> {
        self.definitions.iter()
    }

    /// 選択したresourceだけを持つscope付きWorld stateを作ります。
    pub fn create_state(
        &self,
        scope: StableKey,
        keys: &[StableKey],
        receipt_capacity: u32,
    ) -> Result<ProgressionState, ProgressionError> {
        if keys.is_empty() || receipt_capacity == 0 {
            return Err(ProgressionError::InvalidDefinition);
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(keys.len())
            .map_err(|_| ProgressionError::AllocationFailed)?;
        values.extend(keys.iter().map(|key| (*key, 0_i64)));
        values.sort_unstable_by_key(|(key, _)| *key);
        if values.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(ProgressionError::DuplicateResource);
        }
        for (key, value) in &mut values {
            *value = self.get(*key)?.initial;
        }
        Ok(ProgressionState {
            scope,
            revision: 0,
            receipt_capacity,
            values,
            receipts: Vec::new(),
        })
    }
}

/// 一つのeventから進行stateへ適用する変更です。同一eventはresourceごとに一変更です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressionChange {
    /// flagを指定値へ設定します。
    SetFlag { key: StableKey, value: bool },
    /// counterへ符号付き差分を加えます。
    AddCounter { key: StableKey, delta: i64 },
}

impl ProgressionChange {
    fn key(self) -> StableKey {
        match self {
            Self::SetFlag { key, .. } | Self::AddCounter { key, .. } => key,
        }
    }
}

/// World eventの発生元と単調sequenceで構成するreceipt identityです。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgressionEventId {
    /// 発生元keyです。
    pub source: StableKey,
    /// 発生元内で再利用しないsequenceです。
    pub sequence: u64,
}

/// World scopeのcreator-owned進行値と、既に適用したevent IDを保持します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressionState {
    scope: StableKey,
    revision: u64,
    receipt_capacity: u32,
    values: Vec<(StableKey, i64)>,
    receipts: Vec<ProgressionEventId>,
}

impl ProgressionState {
    /// 所属World/save scopeです。
    pub fn scope(&self) -> StableKey {
        self.scope
    }

    /// 成功した変更eventごとに増えるrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 登録済みresourceの現在値です。
    pub fn get(&self, key: StableKey) -> Option<i64> {
        self.values
            .binary_search_by_key(&key, |(known, _)| *known)
            .ok()
            .map(|index| self.values[index].1)
    }

    /// stable key順のcreator-owned値を読み取ります。
    pub fn iter(&self) -> impl Iterator<Item = (StableKey, i64)> + '_ {
        self.values.iter().copied()
    }

    /// eventを一度だけ適用します。duplicate receiptはrevisionを変えず`false`を返します。
    ///
    /// resourceとreceipt双方を候補で検証し、どれかの値域・容量・revision検査が失敗した場合は
    /// 一部だけを公開しません。StateStore transaction内でこの候補を保存すると、eventと作者状態を
    /// 同じcheckpoint世代へ結び付けられます。
    pub fn apply_event(
        &mut self,
        expected_scope: StableKey,
        expected_revision: u64,
        event_id: ProgressionEventId,
        changes: &[ProgressionChange],
        catalog: &ProgressionCatalog,
    ) -> Result<bool, ProgressionError> {
        if self.scope != expected_scope {
            return Err(ProgressionError::WrongScope);
        }
        if self.receipts.binary_search(&event_id).is_ok() {
            return Ok(false);
        }
        if self.revision != expected_revision {
            return Err(ProgressionError::StaleRevision);
        }
        if self.receipts.len() >= self.receipt_capacity as usize {
            return Err(ProgressionError::ReceiptCapacity);
        }
        if changes
            .windows(2)
            .any(|pair| pair[0].key() >= pair[1].key())
        {
            return Err(ProgressionError::DuplicateResource);
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(ProgressionError::RevisionOverflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(self.values.len())
            .map_err(|_| ProgressionError::AllocationFailed)?;
        values.extend_from_slice(&self.values);
        for change in changes {
            let key = change.key();
            let definition = catalog.get(key)?;
            let index = values
                .binary_search_by_key(&key, |(known, _)| *known)
                .map_err(|_| ProgressionError::UnknownResource(Box::new(key)))?;
            let value = match (*change, definition.kind) {
                (ProgressionChange::SetFlag { value, .. }, ProgressionKind::Flag) => {
                    i64::from(value)
                }
                (ProgressionChange::AddCounter { delta, .. }, ProgressionKind::Counter) => values
                    [index]
                    .1
                    .checked_add(delta)
                    .ok_or(ProgressionError::ValueOutOfRange(Box::new(key)))?,
                _ => return Err(ProgressionError::WrongValueKind(Box::new(key))),
            };
            if value < definition.minimum || value > definition.maximum {
                return Err(ProgressionError::ValueOutOfRange(Box::new(key)));
            }
            values[index].1 = value;
        }
        let mut receipts = Vec::new();
        receipts
            .try_reserve_exact(self.receipts.len().saturating_add(1))
            .map_err(|_| ProgressionError::AllocationFailed)?;
        receipts.extend_from_slice(&self.receipts);
        let receipt_index = receipts.binary_search(&event_id).unwrap_err();
        receipts.insert(receipt_index, event_id);
        self.values = values;
        self.receipts = receipts;
        self.revision = next_revision;
        Ok(true)
    }

    /// scopeとsource+sequenceをWorldEventから導き、進行eventを一度だけ適用します。
    pub fn apply_world_event(
        &mut self,
        expected_scope: StableKey,
        expected_revision: u64,
        event: &WorldEvent,
        changes: &[ProgressionChange],
        catalog: &ProgressionCatalog,
    ) -> Result<bool, ProgressionError> {
        if event.scope != expected_scope {
            return Err(ProgressionError::WrongScope);
        }
        self.apply_event(
            expected_scope,
            expected_revision,
            ProgressionEventId {
                source: event.source,
                sequence: event.sequence,
            },
            changes,
            catalog,
        )
    }

    /// 復元時にscope、設定容量、定義、値域、receipt順を再検証します。
    pub fn validate(
        &self,
        expected_scope: StableKey,
        expected_receipt_capacity: u32,
        catalog: &ProgressionCatalog,
    ) -> Result<(), ProgressionError> {
        if self.scope != expected_scope
            || self.receipt_capacity != expected_receipt_capacity
            || self.values.is_empty()
            || self.values.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.receipts.len() > self.receipt_capacity as usize
            || self.receipts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(ProgressionError::InvalidState);
        }
        for (key, value) in &self.values {
            let definition = catalog.get(*key)?;
            if *value < definition.minimum || *value > definition.maximum {
                return Err(ProgressionError::ValueOutOfRange(Box::new(*key)));
            }
        }
        Ok(())
    }
}

impl StateCodec for ProgressionState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self.values.is_empty()
            || self.values.len() > u32::MAX as usize
            || self.receipts.len() > self.receipt_capacity as usize
            || self.values.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.receipts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid_state());
        }
        out.raw(b"CHRPROG1")?;
        self.scope.encode(out)?;
        out.u64(self.revision)?;
        out.u32(self.receipt_capacity)?;
        out.u32(u32::try_from(self.values.len()).map_err(|_| invalid_state())?)?;
        for (key, value) in &self.values {
            key.encode(out)?;
            out.i64(*value)?;
        }
        out.u32(u32::try_from(self.receipts.len()).map_err(|_| invalid_state())?)?;
        for receipt in &self.receipts {
            receipt.source.encode(out)?;
            out.u64(receipt.sequence)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHRPROG1" {
            return Err(invalid_state());
        }
        let scope = StableKey::decode(input)?;
        let revision = input.u64()?;
        let receipt_capacity = input.u32()?;
        if receipt_capacity == 0 {
            return Err(invalid_state());
        }
        let count = input.u32()? as usize;
        if count == 0 || count > input.remaining() / 15 {
            return Err(invalid_state());
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let entry = (StableKey::decode(input)?, input.i64()?);
            if values
                .last()
                .is_some_and(|previous: &(StableKey, i64)| previous.0 >= entry.0)
            {
                return Err(invalid_state());
            }
            values.push(entry);
        }
        let receipt_count = input.u32()?;
        if receipt_count > receipt_capacity || receipt_count as usize > input.remaining() / 15 {
            return Err(invalid_state());
        }
        let mut receipts = Vec::new();
        receipts
            .try_reserve_exact(receipt_count as usize)
            .map_err(|_| allocation_error())?;
        for _ in 0..receipt_count {
            let receipt = ProgressionEventId {
                source: StableKey::decode(input)?,
                sequence: input.u64()?,
            };
            if receipts.last().is_some_and(|previous| *previous >= receipt) {
                return Err(invalid_state());
            }
            receipts.push(receipt);
        }
        Ok(Self {
            scope,
            revision,
            receipt_capacity,
            values,
            receipts,
        })
    }
}

/// catalog、scope、値域、revision、保存容量に関する失敗です。
///
/// 診断キーは`StableKey`本体が大きいため、失敗経路だけでBoxへ確保します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressionError {
    InvalidDefinition,
    DuplicateResource,
    UnknownResource(Box<StableKey>),
    WrongValueKind(Box<StableKey>),
    ValueOutOfRange(Box<StableKey>),
    WrongScope,
    StaleRevision,
    RevisionOverflow,
    ReceiptCapacity,
    AllocationFailed,
    InvalidState,
}

impl std::fmt::Display for ProgressionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for ProgressionError {}

fn invalid_state() -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, "進行resource保存状態")
}

fn allocation_error() -> EngineError {
    EngineError::new(ErrorCode::CapacityExceeded, "進行resource確保")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }

    fn catalog() -> ProgressionCatalog {
        ProgressionCatalog::new(vec![
            ProgressionDefinition {
                key: key("test:boss-defeated"),
                kind: ProgressionKind::Flag,
                initial: 0,
                minimum: 0,
                maximum: 1,
            },
            ProgressionDefinition {
                key: key("test:ore-mined"),
                kind: ProgressionKind::Counter,
                initial: 0,
                minimum: 0,
                maximum: 100,
            },
        ])
        .unwrap()
    }

    #[test]
    fn event_changes_are_atomic_idempotent_and_checkpointable() {
        let catalog = catalog();
        let scope = key("test:progress-world");
        let event = ProgressionEventId {
            source: key("test:event-source"),
            sequence: 1,
        };
        let mut state = catalog
            .create_state(
                scope,
                &[key("test:boss-defeated"), key("test:ore-mined")],
                4,
            )
            .unwrap();
        assert!(
            state
                .apply_event(
                    scope,
                    0,
                    event,
                    &[
                        ProgressionChange::SetFlag {
                            key: key("test:boss-defeated"),
                            value: true,
                        },
                        ProgressionChange::AddCounter {
                            key: key("test:ore-mined"),
                            delta: 3,
                        },
                    ],
                    &catalog,
                )
                .unwrap()
        );
        assert_eq!(state.get(key("test:boss-defeated")), Some(1));
        assert_eq!(state.get(key("test:ore-mined")), Some(3));
        let before = state.clone();
        assert_eq!(
            state.apply_event(
                scope,
                0,
                event,
                &[ProgressionChange::AddCounter {
                    key: key("test:ore-mined"),
                    delta: 98,
                }],
                &catalog,
            ),
            Ok(false)
        );
        assert_eq!(state, before);

        let overflow = state.apply_event(
            scope,
            1,
            ProgressionEventId {
                source: key("test:event-source"),
                sequence: 2,
            },
            &[
                ProgressionChange::SetFlag {
                    key: key("test:boss-defeated"),
                    value: false,
                },
                ProgressionChange::AddCounter {
                    key: key("test:ore-mined"),
                    delta: 98,
                },
            ],
            &catalog,
        );
        assert_eq!(
            overflow,
            Err(ProgressionError::ValueOutOfRange(Box::new(key(
                "test:ore-mined"
            ))))
        );
        assert_eq!(state, before);

        let mut encoder = Encoder::new(4096);
        state.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let mut restored = ProgressionState::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        restored
            .validate(scope, 4, &catalog)
            .expect("復元後のscopeとcreator定義が一致する");
        assert_eq!(restored, state);
        assert_eq!(
            restored.apply_event(scope, restored.revision(), event, &[], &catalog,),
            Ok(false)
        );
    }

    #[test]
    fn registration_rejects_invalid_flags_and_duplicate_keys() {
        let invalid_flag = ProgressionDefinition {
            key: key("test:invalid-flag"),
            kind: ProgressionKind::Flag,
            initial: 2,
            minimum: 0,
            maximum: 1,
        };
        assert_eq!(
            ProgressionCatalog::new(vec![invalid_flag]),
            Err(ProgressionError::InvalidDefinition)
        );
        let definition = ProgressionDefinition {
            key: key("test:duplicate-resource"),
            kind: ProgressionKind::Counter,
            initial: 0,
            minimum: 0,
            maximum: 2,
        };
        assert_eq!(
            ProgressionCatalog::new(vec![definition, definition]),
            Err(ProgressionError::DuplicateResource)
        );
    }
}
