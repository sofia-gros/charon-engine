//! 作者イベントを、保存可能なSameTick/Deferred queueへ接続します。
//!
//! 配送順、loop抑止、容量、checkpointは共有Signals queueが担当します。イベント型と
//! world eventから次のイベントを作る規則はproviderが所有します。通常providerはdeliveryを返し、
//! transactional providerはWorld commandとProgressionStateを共通の成功境界へ接続できます。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::state::{StateStore, Transaction};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use crate::game::progression::{
    ProgressionCatalog, ProgressionChange, ProgressionError, ProgressionState,
};
use crate::game::signals::{
    Signal, SignalDelivery, SignalError, SignalLimits, SignalProgress, SignalQueue, SignalTiming,
    SignalsProvider,
};
use crate::world::{World, WorldEdit};
use std::cell::RefCell;

/// World scope内の作者定義イベントです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEvent {
    /// 保存scopeとqueueを識別するWorld key。
    pub scope: StableKey,
    /// 作者が登録したイベント型。
    pub key: StableKey,
    /// イベントを発生させた安定source。
    pub source: StableKey,
    /// source内で再利用しない単調sequence。
    pub sequence: u64,
    /// event型のcodecで検証するpayload。
    pub payload: Vec<u8>,
}

/// handlerが次に配送するイベントと時刻方針です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEventDelivery {
    /// 次のイベント候補。
    pub event: WorldEvent,
    /// 同tick内か、次tickへ送るかを作者が選びます。
    pub timing: SignalTiming,
}

/// queueが所有するpayloadを複製せずに読むhandler viewです。
#[derive(Clone, Copy, Debug)]
pub struct WorldEventView<'a> {
    /// eventが所属するworld scope。
    pub scope: StableKey,
    /// 作者登録のevent型。
    pub key: StableKey,
    /// 発生元。
    pub source: StableKey,
    /// 発生元内のsequence。
    pub sequence: u64,
    /// event固有codecへ渡す不変payload。
    pub payload: &'a [u8],
}

/// WorldEvent handlerを標準queueへ差し替え可能に接続する作者providerです。
pub trait WorldEventProvider {
    /// provider実装の安定key。
    fn key(&self) -> StableKey;
    /// 保存queueと照合する版。
    fn version(&self) -> u32;
    /// イベントを読み取り、順序付きの配送候補を返します。
    fn handle(&self, event: WorldEventView<'_>) -> Result<Vec<WorldEventDelivery>, SignalError>;
}

/// 進行変更を含むイベントhandlerの候補です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEventTransactionResult {
    /// 次にqueueへ配送するイベントと時刻方針です。
    pub deliveries: Vec<WorldEventDelivery>,
    /// このevent receiptに一度だけ適用する作者進行値です。key順に指定します。
    pub progression_changes: Vec<ProgressionChange>,
    /// callbackでWorld commandも適用するeventかを示します。
    pub has_world_commands: bool,
}

/// queue配送時にWorldへ適用する、所有済みイベントと進行変更です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEventAction {
    /// 発生元・sequence・payloadを含む元イベントです。
    pub event: WorldEvent,
    /// receiptが新規の場合に適用する進行変更です。
    pub progression_changes: Vec<ProgressionChange>,
    /// providerがWorld編集を要求したeventです。
    pub has_world_commands: bool,
}

/// World commandとProgressionChangeを同じeventへ対応付ける作者handlerです。
pub trait TransactionalWorldEventProvider {
    /// provider実装の安定keyです。
    fn key(&self) -> StableKey;
    /// 保存queueと照合する版です。
    fn version(&self) -> u32;
    /// eventから次配送と進行候補を作ります。外部副作用を発生させません。
    fn handle(&self, event: WorldEventView<'_>)
    -> Result<WorldEventTransactionResult, SignalError>;
}

/// event queue、進行候補、World commandの連結で起きる失敗です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorldEventTransactionError {
    /// Signals queue/providerの失敗です。
    Signal(SignalError),
    /// 作者進行値・receipt検証の失敗です。
    Progression(ProgressionError),
    /// StateStore candidateの失敗です。
    State(EngineError),
    /// World編集候補の失敗です。
    World(EngineError),
}

impl std::fmt::Display for WorldEventTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for WorldEventTransactionError {}

/// 一つのWorldに属する保存可能イベントqueueです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEventQueue {
    scope: StableKey,
    queue: SignalQueue,
}

impl WorldEventQueue {
    /// world scopeとhandler providerを固定して空queueを作ります。
    pub fn new(
        scope: StableKey,
        provider: &impl WorldEventProvider,
        tick: u64,
    ) -> Result<Self, SignalError> {
        Ok(Self {
            scope,
            queue: SignalQueue::new(scope, &EventAdapter { scope, provider }, tick)?,
        })
    }

    /// transactional handlerのstable key/versionを保存して空queueを作ります。
    pub fn new_transactional(
        scope: StableKey,
        provider: &impl TransactionalWorldEventProvider,
        tick: u64,
    ) -> Result<Self, SignalError> {
        let adapter = TransactionalEventAdapter {
            scope,
            provider,
            actions: RefCell::new(Vec::new()),
        };
        Ok(Self {
            scope,
            queue: SignalQueue::new(scope, &adapter, tick)?,
        })
    }

    /// 現在のlogical tickです。
    pub fn tick(&self) -> u64 {
        self.queue.tick()
    }
    /// 同tick・次tick queueの未処理件数です。
    pub fn pending(&self) -> usize {
        self.queue.pending()
    }

    /// 現在tickでpoll可能なevent数です。Deferred待ちは含みません。
    pub fn ready_pending(&self) -> usize {
        self.queue.ready_pending()
    }

    /// 作者入力イベントをSameTickまたはDeferredとして追加します。
    pub fn publish(
        &mut self,
        event: WorldEvent,
        timing: SignalTiming,
        limits: SignalLimits,
    ) -> Result<bool, SignalError> {
        if event.scope != self.scope {
            return Err(SignalError::InvalidSignal);
        }
        self.queue.enqueue(event_to_signal(event), timing, limits)
    }

    /// 固定event budget内でprovider handlerとqueueを進めます。
    pub fn poll(
        &mut self,
        provider: &impl WorldEventProvider,
        limits: SignalLimits,
        budget: u32,
    ) -> Result<SignalProgress, SignalError> {
        self.queue.poll(
            &EventAdapter {
                scope: self.scope,
                provider,
            },
            limits,
            budget,
        )
    }

    /// event配送、World command、進行receipt、両resourceのcheckpointを一取引で確定します。
    ///
    /// queueと進行stateは上限付き候補として複製し、provider actionを決定的なqueue順で収集します。
    /// receiptが新しくWorld commandを持つeventだけをWorld callbackへ渡します。callbackまたはStateStore候補が失敗
    /// した場合はWorld、queue、進行値のいずれも公開しません。作者callback外のglobal/I/O副作用は
    /// rollback対象ではありません。候補準備量はpending payload、進行record/receipt、処理actionに比例します。
    #[allow(clippy::too_many_arguments)]
    pub fn poll_transactionally<R>(
        &mut self,
        progression: &mut ProgressionState,
        world: &mut World,
        state: &mut StateStore,
        queue_key: StableKey,
        queue_schema: StableKey,
        progression_key: StableKey,
        progression_schema: StableKey,
        provider: &impl TransactionalWorldEventProvider,
        catalog: &ProgressionCatalog,
        limits: SignalLimits,
        budget: u32,
        update_world: impl FnOnce(&[WorldEventAction], &mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> Result<(SignalProgress, Option<R>), WorldEventTransactionError> {
        self.poll_transactionally_with_state(
            progression,
            world,
            state,
            queue_key,
            queue_schema,
            progression_key,
            progression_schema,
            provider,
            catalog,
            limits,
            budget,
            |_, _| Ok(()),
            update_world,
        )
    }

    /// event消費・Progression・作者StateStore更新・World commandを一つの取引で確定します。
    ///
    /// `update_state`へ渡るeventは新しいreceiptとしてProgression候補に受理されたものだけです。
    /// 作者はここでEffectJobや独自trigger recordを同じStateStore候補へ追加できます。
    /// queue候補、進行候補、作者record候補、World編集のいずれかが失敗すれば、全正規stateを保持します。
    /// callbackから候補外のglobal stateやI/Oを変更した場合、その副作用はrollback対象ではありません。
    #[allow(clippy::too_many_arguments)]
    pub fn poll_transactionally_with_state<R>(
        &mut self,
        progression: &mut ProgressionState,
        world: &mut World,
        state: &mut StateStore,
        queue_key: StableKey,
        queue_schema: StableKey,
        progression_key: StableKey,
        progression_schema: StableKey,
        provider: &impl TransactionalWorldEventProvider,
        catalog: &ProgressionCatalog,
        limits: SignalLimits,
        budget: u32,
        update_state: impl FnOnce(&[WorldEventAction], &mut Transaction<'_>) -> EngineResult<()>,
        update_world: impl FnOnce(&[WorldEventAction], &mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> Result<(SignalProgress, Option<R>), WorldEventTransactionError> {
        if progression.scope() != self.scope {
            return Err(WorldEventTransactionError::Progression(
                ProgressionError::WrongScope,
            ));
        }
        if state.scope() != self.scope {
            return Err(WorldEventTransactionError::State(EngineError::new(
                ErrorCode::WrongScope,
                "World eventとStateStoreのscope不一致",
            )));
        }
        if self.ready_pending() == 0 {
            return Ok((SignalProgress::TickComplete, None));
        }

        let mut queue_candidate = self.clone();
        let mut progression_candidate = progression.clone();
        let adapter = TransactionalEventAdapter {
            scope: self.scope,
            provider,
            actions: RefCell::new(Vec::new()),
        };
        let progress = queue_candidate
            .queue
            .poll(&adapter, limits, budget)
            .map_err(WorldEventTransactionError::Signal)?;
        let actions = adapter.actions.into_inner();
        let mut accepted_actions = Vec::new();
        accepted_actions
            .try_reserve_exact(actions.len())
            .map_err(|_| WorldEventTransactionError::Signal(SignalError::AllocationFailed))?;
        for action in actions {
            let expected_revision = progression_candidate.revision();
            if progression_candidate
                .apply_world_event(
                    self.scope,
                    expected_revision,
                    &action.event,
                    &action.progression_changes,
                    catalog,
                )
                .map_err(WorldEventTransactionError::Progression)?
            {
                accepted_actions.push(action);
            }
        }

        let mut state_candidate = state
            .transaction()
            .map_err(WorldEventTransactionError::State)?;
        state_candidate
            .put(queue_key, queue_schema, &queue_candidate)
            .map_err(WorldEventTransactionError::State)?;
        state_candidate
            .put(progression_key, progression_schema, &progression_candidate)
            .map_err(WorldEventTransactionError::State)?;
        update_state(&accepted_actions, &mut state_candidate)
            .map_err(WorldEventTransactionError::State)?;
        state_candidate
            .validate_ready()
            .map_err(WorldEventTransactionError::State)?;

        let world_action_count = accepted_actions
            .iter()
            .filter(|action| action.has_world_commands)
            .count();
        let mut world_actions = Vec::new();
        world_actions
            .try_reserve_exact(world_action_count)
            .map_err(|_| WorldEventTransactionError::Signal(SignalError::AllocationFailed))?;
        world_actions.extend(
            accepted_actions
                .into_iter()
                .filter(|action| action.has_world_commands),
        );
        let result = if world_actions.is_empty() {
            None
        } else {
            Some(
                world
                    .edit(|editor| update_world(&world_actions, editor))
                    .map_err(WorldEventTransactionError::World)?,
            )
        };
        state_candidate
            .commit()
            .map_err(WorldEventTransactionError::State)?;
        *self = queue_candidate;
        *progression = progression_candidate;
        Ok((progress, result))
    }

    /// 現在queue完了後に次tickへ進み、Deferred eventを実行可能にします。
    pub fn begin_next_tick(&mut self) -> Result<(), SignalError> {
        self.queue.begin_next_tick()
    }

    /// 復元直後またはservice設定変更後に保存状態を作者指定上限で検証します。
    pub fn validate_limits(&self, limits: SignalLimits) -> Result<(), SignalError> {
        self.queue.validate_limits(limits)
    }

    /// 復元結果が呼出側のWorld scopeに属することを確認します。
    pub fn validate_scope(&self, expected: StableKey) -> Result<(), SignalError> {
        if self.scope == expected {
            Ok(())
        } else {
            Err(SignalError::InvalidSignal)
        }
    }
}

impl StateCodec for WorldEventQueue {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.queue.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let queue = SignalQueue::decode(input)?;
        let scope = queue.network_key();
        Ok(Self { scope, queue })
    }
}

struct EventAdapter<'a, P> {
    scope: StableKey,
    provider: &'a P,
}
impl<P: WorldEventProvider> SignalsProvider for EventAdapter<'_, P> {
    fn key(&self) -> StableKey {
        self.provider.key()
    }
    fn version(&self) -> u32 {
        self.provider.version()
    }
    fn propagate(&self, signal: &Signal) -> Result<Vec<SignalDelivery>, SignalError> {
        if signal.network != self.scope {
            return Err(SignalError::InvalidSignal);
        }
        let event = WorldEventView {
            scope: self.scope,
            key: signal.target,
            source: signal.source,
            sequence: signal.sequence,
            payload: &signal.payload,
        };
        let deliveries = self.provider.handle(event)?;
        let mut signals = Vec::new();
        signals
            .try_reserve_exact(deliveries.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        for delivery in deliveries {
            signals.push(SignalDelivery {
                signal: event_to_signal(delivery.event),
                timing: delivery.timing,
            });
        }
        Ok(signals)
    }
}

struct TransactionalEventAdapter<'a, P> {
    scope: StableKey,
    provider: &'a P,
    actions: RefCell<Vec<WorldEventAction>>,
}

impl<P: TransactionalWorldEventProvider> SignalsProvider for TransactionalEventAdapter<'_, P> {
    fn key(&self) -> StableKey {
        self.provider.key()
    }
    fn version(&self) -> u32 {
        self.provider.version()
    }
    fn propagate(&self, signal: &Signal) -> Result<Vec<SignalDelivery>, SignalError> {
        if signal.network != self.scope {
            return Err(SignalError::InvalidSignal);
        }
        let event = WorldEventView {
            scope: self.scope,
            key: signal.target,
            source: signal.source,
            sequence: signal.sequence,
            payload: &signal.payload,
        };
        let result = self.provider.handle(event)?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(signal.payload.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        payload.extend_from_slice(&signal.payload);
        let mut actions = self.actions.borrow_mut();
        actions
            .try_reserve(1)
            .map_err(|_| SignalError::AllocationFailed)?;
        actions.push(WorldEventAction {
            event: WorldEvent {
                scope: self.scope,
                key: signal.target,
                source: signal.source,
                sequence: signal.sequence,
                payload,
            },
            progression_changes: result.progression_changes,
            has_world_commands: result.has_world_commands,
        });
        drop(actions);

        let mut signals = Vec::new();
        signals
            .try_reserve_exact(result.deliveries.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        for delivery in result.deliveries {
            signals.push(SignalDelivery {
                signal: event_to_signal(delivery.event),
                timing: delivery.timing,
            });
        }
        Ok(signals)
    }
}

fn event_to_signal(event: WorldEvent) -> Signal {
    Signal {
        network: event.scope,
        source: event.source,
        sequence: event.sequence,
        target: event.key,
        payload: event.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::codec::StateCodec;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn limits() -> SignalLimits {
        SignalLimits {
            signals_per_tick: 16,
            queued_payload_bytes: 512,
            deliveries_per_signal: 4,
            payload_bytes: 64,
        }
    }

    struct Progression;
    impl WorldEventProvider for Progression {
        fn key(&self) -> StableKey {
            key("test:progression-provider")
        }
        fn version(&self) -> u32 {
            1
        }
        fn handle(
            &self,
            event: WorldEventView<'_>,
        ) -> Result<Vec<WorldEventDelivery>, SignalError> {
            if event.key == key("test:boss-defeated") {
                return Ok(vec![WorldEventDelivery {
                    event: WorldEvent {
                        scope: event.scope,
                        key: key("test:unlock-area"),
                        source: event.source,
                        sequence: event.sequence + 1,
                        payload: event.payload.to_vec(),
                    },
                    timing: SignalTiming::Deferred,
                }]);
            }
            Ok(Vec::new())
        }
    }

    #[test]
    fn custom_world_events_are_deferred_and_codec_preserves_scope_and_tick() {
        let provider = Progression;
        let scope = key("test:world");
        let mut events = WorldEventQueue::new(scope, &provider, 20).unwrap();
        events
            .publish(
                WorldEvent {
                    scope,
                    key: key("test:boss-defeated"),
                    source: key("test:player"),
                    sequence: 4,
                    payload: vec![1],
                },
                SignalTiming::SameTick,
                limits(),
            )
            .unwrap();
        assert_eq!(
            events.poll(&provider, limits(), 1).unwrap(),
            SignalProgress::TickComplete
        );
        assert_eq!(events.pending(), 1);
        let mut encoder = Encoder::new(4096);
        events.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let mut restored = WorldEventQueue::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(restored.tick(), 20);
        restored.begin_next_tick().unwrap();
        assert_eq!(
            restored.poll(&provider, limits(), 1).unwrap(),
            SignalProgress::TickComplete
        );
        assert_eq!(restored.pending(), 0);
    }
}
