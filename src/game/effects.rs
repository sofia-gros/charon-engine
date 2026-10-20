//! 作者定義Effect evaluatorと、保存可能な1命令単位job実行基盤です。
//!
//! evaluatorはWorldを書き換えずcommand候補を返します。呼出側はworld/state候補のcommitを
//! 成功させた後だけ`EffectCandidate::commit`で命令位置を進め、失敗時はcandidateを破棄します。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::state::{StateStore, Transaction};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use crate::world::{World, WorldEdit};

/// 静的manifestに登録する、版付きeffect命令列です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectProgram {
    key: StableKey,
    version: u32,
    nodes: Vec<StableKey>,
}

impl EffectProgram {
    /// 空命令列とversion 0を拒否してprogramを構築します。
    pub fn new(key: StableKey, version: u32, nodes: Vec<StableKey>) -> Result<Self, EffectError> {
        if version == 0 || nodes.is_empty() || nodes.len() > u32::MAX as usize {
            return Err(EffectError::InvalidProgram);
        }
        Ok(Self {
            key,
            version,
            nodes,
        })
    }

    /// 保存・manifestで参照する安定keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 互換性検査に使うprogram版です。
    pub fn version(&self) -> u32 {
        self.version
    }
    /// 実行命令数です。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// 命令が空かを返します。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    fn node(&self, instruction: u32) -> Option<StableKey> {
        self.nodes.get(instruction as usize).copied()
    }
}

/// 1回のeffect node実行に適用する作者指定容量です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectLimits {
    /// node固有状態recordの最大byte数。
    pub state_bytes: usize,
    /// 一つの命令が返せるcommand数。
    pub commands_per_step: u32,
    /// 一つの命令が返せるcommand payload合計byte数。
    pub command_bytes_per_step: usize,
    /// 同tickで一つのjobが実行できる命令数。
    pub instructions_per_tick: u32,
    /// 同tickで一つのjobが要求できるspawn数。
    pub spawns_per_tick: u32,
    /// 同tickで一つのjobが書き込めるWorld cell数。
    pub cells_per_tick: u32,
    /// jobが重複なく保持できるreceipt数。
    pub receipts: u32,
    /// 同じEffectJobに積める別program呼出しの最大深度。
    pub max_call_depth: u32,
}

impl EffectLimits {
    /// instruction予算は1以上、各出力上限は0以上、codec byte上限はu32範囲に制限します。
    pub fn new(
        state_bytes: usize,
        commands_per_step: u32,
        command_bytes_per_step: usize,
        instructions_per_tick: u32,
        spawns_per_tick: u32,
        cells_per_tick: u32,
        receipts: u32,
    ) -> Result<Self, EffectError> {
        if state_bytes > u32::MAX as usize
            || command_bytes_per_step > u32::MAX as usize
            || instructions_per_tick == 0
        {
            return Err(EffectError::InvalidLimits);
        }
        Ok(Self {
            state_bytes,
            commands_per_step,
            command_bytes_per_step,
            instructions_per_tick,
            spawns_per_tick,
            cells_per_tick,
            receipts,
            max_call_depth: 16,
        })
    }

    /// 別program呼出しの最大深度を作者のjob容量へ合わせます。0なら呼出しを許可しません。
    pub fn with_max_call_depth(mut self, maximum: u32) -> Self {
        self.max_call_depth = maximum;
        self
    }
}

/// evaluatorへ渡す正規tickと決定的な乱数呼出位置です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectContext {
    /// この命令を実行するlogical tick。
    pub tick: u64,
    /// 命令を発生させたsource identity。
    pub source: u64,
    /// 命令の対象identity。対象なしは0。
    pub target: u64,
    /// 乱数providerの安定key。
    pub random_provider: StableKey,
    /// provider内の安定stream。
    pub random_stream: u64,
    /// providerが次に消費する位置。draw数は結果で返して進めます。
    pub random_cursor: u64,
}

/// 作者evaluatorが候補として発行する登録済みcommand payloadです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCommand {
    /// command handlerの安定key。
    pub key: StableKey,
    /// command固有codecで検証されるpayload。
    pub payload: Vec<u8>,
}

/// 1命令の制御結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectControl {
    /// 次の命令へ進みます。
    Next,
    /// 同じprogram内の指定命令へ分岐します。
    Jump(u32),
    /// jobを完了します。
    Complete,
    /// 指定した論理tick数だけ待ってから次の命令へ進みます。0は不正です。
    WaitTicks(u64),
    /// 別programの先頭を呼び、完了後は呼出元の次命令へ戻ります。
    CallProgram { key: StableKey, version: u32 },
    /// 現在programを呼出元へ返します。root programでのReturnは不正です。
    Return,
}

/// 次の論理tickへ処理を進めるかを指定します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectSchedule {
    /// Runtimeの同tick継続pollで次命令を実行します。
    SameTick,
    /// 次tickへjobを保存して送ります。
    Deferred,
}

/// evaluatorの検証済み候補出力です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectNodeResult {
    /// 次命令の移動先。
    pub control: EffectControl,
    /// 次回のnode固有codec状態。
    pub state: Vec<u8>,
    /// 次のbarrierで適用する順序付きcommand候補。
    pub commands: Vec<EffectCommand>,
    /// 同じjobへ一度だけ記録するreceipt ID。
    pub receipts: Vec<u64>,
    /// nodeが候補に要求するspawn数です。
    pub spawn_count: u32,
    /// nodeが候補に要求するcell書込数です。
    pub cell_count: u32,
    /// 指定乱数providerから消費した値の個数。
    pub random_draws: u64,
    /// 継続jobを同tickまたは次tickへ送ります。
    pub schedule: EffectSchedule,
}

/// standard evaluatorの代わりに登録する作者実装です。
///
/// callbackは同じ入力で同じ結果を返し、渡されたstate/乱数cursorを越えて正規状態を
/// 隠し持たないことが契約です。永続化する追加状態はStateStoreへ登録し、World commandは
/// `EffectNodeResult`で返してください。
pub trait EffectEvaluator {
    /// evaluator実装の安定key。
    fn key(&self) -> StableKey;
    /// 保存jobと照合するevaluator版。
    fn version(&self) -> u32;
    /// 1命令を読み取り専用入力から評価します。
    fn evaluate(
        &self,
        node: StableKey,
        context: EffectContext,
        state: &[u8],
    ) -> Result<EffectNodeResult, EffectError>;
}

/// effect実行で起きた失敗です。失敗時はjob候補をcommitしません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectError {
    InvalidProgram,
    InvalidLimits,
    WrongProvider,
    InvalidState,
    InvalidControl,
    CapacityExceeded,
    BudgetExceededSameTick,
    DuplicateReceipt,
    CounterOverflow,
    AllocationFailed,
    EvaluatorFailed,
}

/// 標準Effect graph内の決定的な条件です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectCondition {
    /// 常に成立します。
    Always,
    /// target identityが指定値と一致します。
    TargetEquals(u64),
    /// source identityが指定値と一致します。
    SourceEquals(u64),
    /// 現在のlogical tickが指定値以上です。
    TickAtLeast(u64),
}

/// 標準compilerが検証する静的Effect nodeです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectNodeOperation {
    /// 登録済みhandlerへ順序付きcommandと次の制御を出します。
    Emit {
        commands: Vec<EffectCommand>,
        state: Vec<u8>,
        receipts: Vec<u64>,
        control: EffectControl,
        schedule: EffectSchedule,
        spawn_count: u32,
        cell_count: u32,
        random_draws: u64,
    },
    /// 作者が明示した命令位置へ分岐します。
    Branch {
        condition: EffectCondition,
        if_true: u32,
        if_false: u32,
        schedule: EffectSchedule,
    },
    /// 指定tick数だけjobを停止して、次の命令から再開します。
    WaitTicks { ticks: u64 },
    /// 別programを呼び出します。program key/versionは登録済みcatalogで解決します。
    CallProgram {
        key: StableKey,
        version: u32,
        schedule: EffectSchedule,
    },
    /// 現在programから呼出元へ戻ります。
    Return { schedule: EffectSchedule },
}

/// 標準Effect graphで参照する安定node定義です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectNodeDefinition {
    /// graphから参照するstable keyです。
    pub key: StableKey,
    /// nodeの標準動作です。
    pub operation: EffectNodeOperation,
}

/// 重複node keyを拒否し、標準Effect nodeをstable順に保持します。
#[derive(Clone, Debug, Default)]
pub struct EffectNodeCatalog {
    nodes: Vec<EffectNodeDefinition>,
}

impl EffectNodeCatalog {
    /// command、状態、receiptとnode数をcreator上限で検証して登録します。
    pub fn new(
        mut nodes: Vec<EffectNodeDefinition>,
        node_limit: u32,
        limits: EffectLimits,
    ) -> Result<Self, EffectError> {
        if nodes.len() > node_limit as usize {
            return Err(EffectError::CapacityExceeded);
        }
        for node in &nodes {
            match &node.operation {
                EffectNodeOperation::Emit {
                    commands,
                    state,
                    receipts,
                    spawn_count,
                    cell_count,
                    control,
                    ..
                } => {
                    let command_bytes = commands.iter().try_fold(0usize, |sum, command| {
                        sum.checked_add(command.payload.len())
                            .ok_or(EffectError::CapacityExceeded)
                    })?;
                    if commands.len() > limits.commands_per_step as usize
                        || command_bytes > limits.command_bytes_per_step
                        || state.len() > limits.state_bytes
                        || receipts.len() > limits.receipts as usize
                        || *spawn_count > limits.spawns_per_tick
                        || *cell_count > limits.cells_per_tick
                    {
                        return Err(EffectError::CapacityExceeded);
                    }
                    if matches!(control, EffectControl::WaitTicks(0)) {
                        return Err(EffectError::InvalidControl);
                    }
                    if matches!(control, EffectControl::CallProgram { version: 0, .. }) {
                        return Err(EffectError::InvalidControl);
                    }
                }
                EffectNodeOperation::WaitTicks { ticks: 0 } => {
                    return Err(EffectError::InvalidControl);
                }
                EffectNodeOperation::CallProgram { version: 0, .. } => {
                    return Err(EffectError::InvalidControl);
                }
                _ => {}
            }
        }
        nodes.sort_unstable_by_key(|node| node.key);
        if nodes.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(EffectError::InvalidProgram);
        }
        Ok(Self { nodes })
    }

    /// 定義済みnodeだけを参照し、分岐先を検証して実行可能programをcompileします。
    pub fn compile(
        &self,
        key: StableKey,
        version: u32,
        nodes: Vec<StableKey>,
    ) -> Result<EffectProgram, EffectError> {
        let program = EffectProgram::new(key, version, nodes)?;
        validate_effect_program(&program, self)?;
        Ok(program)
    }

    fn get(&self, key: StableKey) -> Result<&EffectNodeDefinition, EffectError> {
        self.nodes
            .binary_search_by_key(&key, |node| node.key)
            .map(|index| &self.nodes[index])
            .map_err(|_| EffectError::InvalidProgram)
    }
}

/// compile済みprogramを参照解決して保持する不変catalogです。
#[derive(Clone, Debug, Default)]
pub struct EffectProgramCatalog {
    programs: Vec<EffectProgram>,
}

impl EffectProgramCatalog {
    /// 登録nodeとprogram呼出先を一度だけ検証し、key/version順に固定します。
    pub fn new(
        mut programs: Vec<EffectProgram>,
        nodes: &EffectNodeCatalog,
        program_limit: u32,
    ) -> Result<Self, EffectError> {
        if programs.len() > program_limit as usize {
            return Err(EffectError::CapacityExceeded);
        }
        programs.sort_unstable_by_key(|program| (program.key, program.version));
        if programs
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key && pair[0].version == pair[1].version)
        {
            return Err(EffectError::InvalidProgram);
        }
        for program in &programs {
            validate_effect_program(program, nodes)?;
            for instruction in 0..program.len() as u32 {
                let node_key = program
                    .node(instruction)
                    .ok_or(EffectError::InvalidProgram)?;
                let definition = nodes.get(node_key)?;
                let call = match &definition.operation {
                    EffectNodeOperation::CallProgram { key, version, .. } => Some((*key, *version)),
                    EffectNodeOperation::Emit {
                        control: EffectControl::CallProgram { key, version },
                        ..
                    } => Some((*key, *version)),
                    _ => None,
                };
                if let Some((key, version)) = call
                    && programs
                        .binary_search_by_key(&(key, version), |candidate| {
                            (candidate.key, candidate.version)
                        })
                        .is_err()
                {
                    return Err(EffectError::InvalidProgram);
                }
            }
        }
        Ok(Self { programs })
    }

    /// 指定したstable keyとversionを持つprogramを返します。
    pub fn get(&self, key: StableKey, version: u32) -> Result<&EffectProgram, EffectError> {
        self.programs
            .binary_search_by_key(&(key, version), |program| (program.key, program.version))
            .map(|index| &self.programs[index])
            .map_err(|_| EffectError::InvalidProgram)
    }

    /// 登録済みprogram数です。
    pub fn len(&self) -> usize {
        self.programs.len()
    }

    /// catalogにprogramがないかを返します。
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }
}

fn validate_effect_program(
    program: &EffectProgram,
    nodes: &EffectNodeCatalog,
) -> Result<(), EffectError> {
    let valid_target = |target: u32| target < program.len() as u32;
    for instruction in 0..program.len() as u32 {
        let node_key = program
            .node(instruction)
            .ok_or(EffectError::InvalidProgram)?;
        let definition = nodes.get(node_key)?;
        match &definition.operation {
            EffectNodeOperation::Emit {
                control: EffectControl::Jump(target),
                ..
            } if !valid_target(*target) => return Err(EffectError::InvalidControl),
            EffectNodeOperation::Emit {
                control: EffectControl::WaitTicks(ticks),
                ..
            } if *ticks == 0 || instruction + 1 >= program.len() as u32 => {
                return Err(EffectError::InvalidControl);
            }
            EffectNodeOperation::Emit {
                control: EffectControl::CallProgram { version: 0, .. },
                ..
            }
            | EffectNodeOperation::CallProgram { version: 0, .. } => {
                return Err(EffectError::InvalidControl);
            }
            EffectNodeOperation::Branch {
                if_true, if_false, ..
            } if !valid_target(*if_true) || !valid_target(*if_false) => {
                return Err(EffectError::InvalidControl);
            }
            EffectNodeOperation::WaitTicks { ticks }
                if *ticks == 0 || instruction + 1 >= program.len() as u32 =>
            {
                return Err(EffectError::InvalidControl);
            }
            _ => {}
        }
    }
    Ok(())
}

/// 静的Effect nodeを標準EffectProgram evaluatorへ接続します。
pub struct StandardEffectEvaluator<'a> {
    key: StableKey,
    version: u32,
    catalog: &'a EffectNodeCatalog,
}

impl<'a> StandardEffectEvaluator<'a> {
    /// provider identity/versionとcompile済みnode catalogを固定します。
    pub fn new(
        key: StableKey,
        version: u32,
        catalog: &'a EffectNodeCatalog,
    ) -> Result<Self, EffectError> {
        if version == 0 {
            return Err(EffectError::WrongProvider);
        }
        Ok(Self {
            key,
            version,
            catalog,
        })
    }
}

impl EffectEvaluator for StandardEffectEvaluator<'_> {
    fn key(&self) -> StableKey {
        self.key
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn evaluate(
        &self,
        node: StableKey,
        context: EffectContext,
        state: &[u8],
    ) -> Result<EffectNodeResult, EffectError> {
        let definition = self.catalog.get(node)?;
        match &definition.operation {
            EffectNodeOperation::Emit {
                commands,
                state,
                receipts,
                control,
                schedule,
                spawn_count,
                cell_count,
                random_draws,
            } => Ok(EffectNodeResult {
                control: *control,
                state: copy_state(state).map_err(|_| EffectError::AllocationFailed)?,
                commands: clone_commands(commands)?,
                receipts: clone_values(receipts)?,
                spawn_count: *spawn_count,
                cell_count: *cell_count,
                random_draws: *random_draws,
                schedule: *schedule,
            }),
            EffectNodeOperation::Branch {
                condition,
                if_true,
                if_false,
                schedule,
            } => {
                let matches = match condition {
                    EffectCondition::Always => true,
                    EffectCondition::TargetEquals(target) => context.target == *target,
                    EffectCondition::SourceEquals(source) => context.source == *source,
                    EffectCondition::TickAtLeast(tick) => context.tick >= *tick,
                };
                Ok(EffectNodeResult {
                    control: EffectControl::Jump(if matches { *if_true } else { *if_false }),
                    state: Vec::new(),
                    commands: Vec::new(),
                    receipts: Vec::new(),
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                    schedule: *schedule,
                })
            }
            EffectNodeOperation::WaitTicks { ticks } => Ok(EffectNodeResult {
                control: EffectControl::WaitTicks(*ticks),
                state: Vec::new(),
                commands: Vec::new(),
                receipts: Vec::new(),
                spawn_count: 0,
                cell_count: 0,
                random_draws: 0,
                schedule: EffectSchedule::Deferred,
            }),
            EffectNodeOperation::CallProgram {
                key,
                version,
                schedule,
            } => Ok(EffectNodeResult {
                control: EffectControl::CallProgram {
                    key: *key,
                    version: *version,
                },
                state: copy_state(state).map_err(|_| EffectError::AllocationFailed)?,
                commands: Vec::new(),
                receipts: Vec::new(),
                spawn_count: 0,
                cell_count: 0,
                random_draws: 0,
                schedule: *schedule,
            }),
            EffectNodeOperation::Return { schedule } => Ok(EffectNodeResult {
                control: EffectControl::Return,
                state: copy_state(state).map_err(|_| EffectError::AllocationFailed)?,
                commands: Vec::new(),
                receipts: Vec::new(),
                spawn_count: 0,
                cell_count: 0,
                random_draws: 0,
                schedule: *schedule,
            }),
        }
    }
}

impl std::fmt::Display for EffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for EffectError {}

/// 呼出元へ戻るためにcheckpointへ保存するprogram実行位置です。
#[derive(Clone, Debug, PartialEq, Eq)]
struct EffectCallFrame {
    program: StableKey,
    program_version: u32,
    /// Noneは呼出元programの末尾で呼び出されたことを表します。
    return_instruction: Option<u32>,
    state: Vec<u8>,
}

/// Save/replayへ含める、途中で停止可能なeffect jobです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectJob {
    program: StableKey,
    program_version: u32,
    evaluator: StableKey,
    evaluator_version: u32,
    instruction: u32,
    source: u64,
    target: u64,
    random_provider: StableKey,
    random_stream: u64,
    random_cursor: u64,
    resume_tick: u64,
    same_tick_chain: bool,
    budget_tick: u64,
    instructions_this_tick: u32,
    spawns_this_tick: u32,
    cells_this_tick: u32,
    state: Vec<u8>,
    call_stack: Vec<EffectCallFrame>,
    receipts: Vec<u64>,
    revision: u64,
    complete: bool,
}

impl EffectJob {
    /// program/evaluatorの版と所有entity、乱数streamを固定したjobを作ります。
    pub fn new(
        program: &EffectProgram,
        evaluator: &impl EffectEvaluator,
        source: u64,
        target: u64,
        random_provider: StableKey,
        random_stream: u64,
    ) -> Result<Self, EffectError> {
        if evaluator.version() == 0 {
            return Err(EffectError::InvalidProgram);
        }
        Ok(Self {
            program: program.key,
            program_version: program.version,
            evaluator: evaluator.key(),
            evaluator_version: evaluator.version(),
            instruction: 0,
            source,
            target,
            random_provider,
            random_stream,
            random_cursor: 0,
            resume_tick: 0,
            same_tick_chain: false,
            budget_tick: 0,
            instructions_this_tick: 0,
            spawns_this_tick: 0,
            cells_this_tick: 0,
            state: Vec::new(),
            call_stack: Vec::new(),
            receipts: Vec::new(),
            revision: 0,
            complete: false,
        })
    }

    /// 次に実行するprogram命令番号です。
    pub fn instruction(&self) -> u32 {
        self.instruction
    }
    /// 現在実行中のprogram keyです。呼出し後に作者catalogからcalleeを解決する時に使います。
    pub fn program_key(&self) -> StableKey {
        self.program
    }
    /// 現在実行中のprogram versionです。
    pub fn program_version(&self) -> u32 {
        self.program_version
    }
    /// 保存済み呼出frame数です。
    pub fn call_depth(&self) -> usize {
        self.call_stack.len()
    }
    /// 保存済みreceiptを安定順ではなく確定順で返します。
    pub fn receipts(&self) -> &[u64] {
        &self.receipts
    }
    /// effectが完了したかを返します。
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    /// 次に評価可能となるlogical tickです。作者Systemはこの値より前のpollを省略できます。
    pub fn resume_tick(&self) -> u64 {
        self.resume_tick
    }

    /// 1命令を候補評価します。候補をcommitせずdropするとjobは変更されません。
    pub fn prepare<'a>(
        &'a mut self,
        program: &EffectProgram,
        evaluator: &impl EffectEvaluator,
        tick: u64,
        limits: EffectLimits,
    ) -> Result<EffectCandidate<'a>, EffectError> {
        self.prepare_with_context(program, evaluator, tick, limits, None)
    }

    /// catalogから現在programを解決し、nested call/returnを検証する候補を作ります。
    pub fn prepare_with_catalog<'a>(
        &'a mut self,
        catalog: &EffectProgramCatalog,
        evaluator: &impl EffectEvaluator,
        tick: u64,
        limits: EffectLimits,
    ) -> Result<EffectCandidate<'a>, EffectError> {
        let program = catalog.get(self.program, self.program_version)?;
        self.prepare_with_context(program, evaluator, tick, limits, Some(catalog))
    }

    fn prepare_with_context<'a>(
        &'a mut self,
        program: &EffectProgram,
        evaluator: &impl EffectEvaluator,
        tick: u64,
        limits: EffectLimits,
        catalog: Option<&EffectProgramCatalog>,
    ) -> Result<EffectCandidate<'a>, EffectError> {
        if self.complete {
            return Err(EffectError::InvalidState);
        }
        if self.program != program.key
            || self.program_version != program.version
            || self.evaluator != evaluator.key()
            || self.evaluator_version != evaluator.version()
        {
            return Err(EffectError::WrongProvider);
        }
        if !self.call_stack.is_empty() && catalog.is_none() {
            return Err(EffectError::InvalidProgram);
        }
        if let Some(catalog) = catalog {
            for frame in &self.call_stack {
                catalog.get(frame.program, frame.program_version)?;
            }
        }
        if self.state.len() > limits.state_bytes
            || self
                .call_stack
                .iter()
                .any(|frame| frame.state.len() > limits.state_bytes)
            || self.call_stack.len() > limits.max_call_depth as usize
            || self.receipts.len() > limits.receipts as usize
        {
            return Err(EffectError::CapacityExceeded);
        }
        if tick < self.resume_tick {
            return Err(EffectError::InvalidState);
        }
        let node = program
            .node(self.instruction)
            .ok_or(EffectError::InvalidState)?;
        let (used_instructions, used_spawns, used_cells) = if self.budget_tick == tick {
            (
                self.instructions_this_tick,
                self.spawns_this_tick,
                self.cells_this_tick,
            )
        } else {
            (0, 0, 0)
        };
        if limits.instructions_per_tick == 0 || used_instructions >= limits.instructions_per_tick {
            return Err(if self.same_tick_chain {
                EffectError::BudgetExceededSameTick
            } else {
                EffectError::CapacityExceeded
            });
        }
        let result = evaluator.evaluate(
            node,
            EffectContext {
                tick,
                source: self.source,
                target: self.target,
                random_provider: self.random_provider,
                random_stream: self.random_stream,
                random_cursor: self.random_cursor,
            },
            &self.state,
        )?;
        if result.spawn_count > limits.spawns_per_tick || result.cell_count > limits.cells_per_tick
        {
            return Err(EffectError::CapacityExceeded);
        }
        let spawn_over_budget =
            used_spawns.saturating_add(result.spawn_count) > limits.spawns_per_tick;
        let cell_over_budget = used_cells.saturating_add(result.cell_count) > limits.cells_per_tick;
        if spawn_over_budget || cell_over_budget {
            return Err(if self.same_tick_chain {
                EffectError::BudgetExceededSameTick
            } else {
                EffectError::CapacityExceeded
            });
        }
        if result.state.len() > limits.state_bytes
            || result.commands.len() > limits.commands_per_step as usize
            || result.receipts.len().saturating_add(self.receipts.len()) > limits.receipts as usize
        {
            return Err(EffectError::CapacityExceeded);
        }
        let command_bytes = result.commands.iter().try_fold(0usize, |sum, command| {
            sum.checked_add(command.payload.len())
                .ok_or(EffectError::CapacityExceeded)
        })?;
        if command_bytes > limits.command_bytes_per_step {
            return Err(EffectError::CapacityExceeded);
        }
        let next_random_cursor = self
            .random_cursor
            .checked_add(result.random_draws)
            .ok_or(EffectError::CounterOverflow)?;
        let mut candidate = try_clone_job(self)?;
        candidate.budget_tick = tick;
        candidate.instructions_this_tick = used_instructions + 1;
        candidate.spawns_this_tick = used_spawns + result.spawn_count;
        candidate.cells_this_tick = used_cells + result.cell_count;
        candidate.random_cursor = next_random_cursor;
        candidate.state = result.state;
        let mut new_receipts = result.receipts;
        new_receipts.sort_unstable();
        if new_receipts.windows(2).any(|pair| pair[0] == pair[1])
            || new_receipts
                .iter()
                .any(|receipt| self.receipts.binary_search(receipt).is_ok())
        {
            return Err(EffectError::DuplicateReceipt);
        }
        candidate
            .receipts
            .try_reserve(new_receipts.len())
            .map_err(|_| EffectError::AllocationFailed)?;
        candidate.receipts.extend(new_receipts.iter().copied());
        candidate.receipts.sort_unstable();
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(EffectError::CounterOverflow)?;
        let progress = match result.control {
            EffectControl::Next if self.instruction + 1 < program.len() as u32 => {
                candidate.instruction += 1;
                schedule_effect_progress(&mut candidate, result.schedule, tick)?
            }
            EffectControl::Next => {
                if candidate.call_stack.is_empty() {
                    candidate.complete = true;
                    EffectProgress::Complete
                } else {
                    return_from_effect_program(&mut candidate, result.schedule, tick)?
                }
            }
            EffectControl::Jump(instruction) if instruction < program.len() as u32 => {
                candidate.instruction = instruction;
                schedule_effect_progress(&mut candidate, result.schedule, tick)?
            }
            EffectControl::Jump(_) => return Err(EffectError::InvalidControl),
            EffectControl::Complete => {
                candidate.complete = true;
                candidate.call_stack.clear();
                EffectProgress::Complete
            }
            EffectControl::WaitTicks(ticks)
                if ticks > 0 && self.instruction + 1 < program.len() as u32 =>
            {
                candidate.instruction += 1;
                candidate.resume_tick = tick
                    .checked_add(ticks)
                    .ok_or(EffectError::CounterOverflow)?;
                EffectProgress::Deferred
            }
            EffectControl::WaitTicks(_) => return Err(EffectError::InvalidControl),
            EffectControl::CallProgram { version: 0, .. } => {
                return Err(EffectError::InvalidControl);
            }
            EffectControl::CallProgram { key, version }
                if candidate.call_stack.len() < limits.max_call_depth as usize =>
            {
                catalog
                    .ok_or(EffectError::InvalidProgram)?
                    .get(key, version)?;
                candidate
                    .call_stack
                    .try_reserve(1)
                    .map_err(|_| EffectError::AllocationFailed)?;
                let return_instruction =
                    (self.instruction + 1 < program.len() as u32).then_some(self.instruction + 1);
                let caller_state = std::mem::take(&mut candidate.state);
                candidate.call_stack.push(EffectCallFrame {
                    program: candidate.program,
                    program_version: candidate.program_version,
                    return_instruction,
                    state: caller_state,
                });
                candidate.program = key;
                candidate.program_version = version;
                candidate.instruction = 0;
                candidate.state = Vec::new();
                schedule_effect_progress(&mut candidate, result.schedule, tick)?
            }
            EffectControl::CallProgram { .. } => return Err(EffectError::CapacityExceeded),
            EffectControl::Return if !candidate.call_stack.is_empty() => {
                return_from_effect_program(&mut candidate, result.schedule, tick)?
            }
            EffectControl::Return => return Err(EffectError::InvalidControl),
        };
        candidate.same_tick_chain = progress == EffectProgress::ContinueSameTick;
        Ok(EffectCandidate {
            target: self,
            candidate,
            commands: result.commands,
            new_receipts,
            progress,
        })
    }

    /// jobを保存します。呼出側のschema byte上限も別途適用してください。
    pub fn encode(&self, byte_limit: usize) -> Result<Vec<u8>, EffectError> {
        self.validate()?;
        let mut out = Encoder::new(byte_limit);
        self.encode_state(&mut out)
            .map_err(|_| EffectError::CapacityExceeded)?;
        Ok(out.finish())
    }

    /// 保存jobを検証して独立した継続状態として復元します。
    pub fn decode(bytes: &[u8], byte_limit: usize) -> Result<Self, EffectError> {
        let mut input = Decoder::new(bytes, byte_limit).map_err(|_| EffectError::InvalidState)?;
        let job = Self::decode_state(&mut input).map_err(|_| EffectError::InvalidState)?;
        input.finish().map_err(|_| EffectError::InvalidState)?;
        job.validate()?;
        Ok(job)
    }

    fn validate(&self) -> Result<(), EffectError> {
        if self.program_version == 0
            || self.evaluator_version == 0
            || self.state.len() > u32::MAX as usize
            || self.receipts.len() > u32::MAX as usize
            || self.call_stack.len() > u32::MAX as usize
            || self
                .call_stack
                .iter()
                .any(|frame| frame.program_version == 0 || frame.state.len() > u32::MAX as usize)
            || self.receipts.windows(2).any(|pair| pair[0] >= pair[1])
            || (self.same_tick_chain && (self.complete || self.instructions_this_tick == 0))
        {
            return Err(EffectError::InvalidState);
        }
        Ok(())
    }

    fn encode_state(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHREFF03")?;
        self.program.encode(out)?;
        out.u32(self.program_version)?;
        self.evaluator.encode(out)?;
        out.u32(self.evaluator_version)?;
        out.u32(self.instruction)?;
        out.u64(self.source)?;
        out.u64(self.target)?;
        self.random_provider.encode(out)?;
        out.u64(self.random_stream)?;
        out.u64(self.random_cursor)?;
        out.u64(self.resume_tick)?;
        out.u8(u8::from(self.same_tick_chain))?;
        out.u64(self.budget_tick)?;
        out.u32(self.instructions_this_tick)?;
        out.u32(self.spawns_this_tick)?;
        out.u32(self.cells_this_tick)?;
        out.bytes(&self.state)?;
        out.u32(u32::try_from(self.call_stack.len()).map_err(|_| invalid_encoding())?)?;
        for frame in &self.call_stack {
            frame.program.encode(out)?;
            out.u32(frame.program_version)?;
            match frame.return_instruction {
                Some(instruction) => {
                    out.u8(1)?;
                    out.u32(instruction)?;
                }
                None => out.u8(0)?,
            }
            out.bytes(&frame.state)?;
        }
        out.u32(u32::try_from(self.receipts.len()).map_err(|_| invalid_encoding())?)?;
        for receipt in &self.receipts {
            out.u64(*receipt)?;
        }
        out.u64(self.revision)?;
        out.u8(u8::from(self.complete))?;
        Ok(())
    }

    fn decode_state(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let magic = input.raw(8)?;
        let (has_budgets, has_call_stack) = match magic {
            b"CHREFF01" => (false, false),
            b"CHREFF02" => (true, false),
            b"CHREFF03" => (true, true),
            _ => return Err(invalid_encoding()),
        };
        let program = StableKey::decode(input)?;
        let program_version = input.u32()?;
        let evaluator = StableKey::decode(input)?;
        let evaluator_version = input.u32()?;
        let instruction = input.u32()?;
        let source = input.u64()?;
        let target = input.u64()?;
        let random_provider = StableKey::decode(input)?;
        let random_stream = input.u64()?;
        let random_cursor = input.u64()?;
        let resume_tick = input.u64()?;
        let same_tick_chain = if has_budgets {
            match input.u8()? {
                0 => false,
                1 => true,
                _ => return Err(invalid_encoding()),
            }
        } else {
            false
        };
        let (budget_tick, instructions_this_tick, spawns_this_tick, cells_this_tick) =
            if has_budgets {
                (input.u64()?, input.u32()?, input.u32()?, input.u32()?)
            } else {
                (resume_tick, 0, 0, 0)
            };
        let state = copy_state(input.bytes(u32::MAX as usize)?)?;
        let mut call_stack = Vec::new();
        if has_call_stack {
            let count = input.u32()?;
            if count as usize > input.remaining() / 16 {
                return Err(invalid_encoding());
            }
            call_stack
                .try_reserve_exact(count as usize)
                .map_err(allocation)?;
            for _ in 0..count {
                let program = StableKey::decode(input)?;
                let program_version = input.u32()?;
                let return_instruction = match input.u8()? {
                    0 => None,
                    1 => Some(input.u32()?),
                    _ => return Err(invalid_encoding()),
                };
                let state = copy_state(input.bytes(u32::MAX as usize)?)?;
                call_stack.push(EffectCallFrame {
                    program,
                    program_version,
                    return_instruction,
                    state,
                });
            }
        }
        let count = input.u32()?;
        if count as usize > input.remaining() / 8 {
            return Err(invalid_encoding());
        }
        let mut receipts = Vec::new();
        receipts
            .try_reserve_exact(count as usize)
            .map_err(allocation)?;
        for _ in 0..count {
            receipts.push(input.u64()?);
        }
        let revision = input.u64()?;
        let complete = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid_encoding()),
        };
        Ok(Self {
            program,
            program_version,
            evaluator,
            evaluator_version,
            instruction,
            source,
            target,
            random_provider,
            random_stream,
            random_cursor,
            resume_tick,
            same_tick_chain,
            budget_tick,
            instructions_this_tick,
            spawns_this_tick,
            cells_this_tick,
            state,
            call_stack,
            receipts,
            revision,
            complete,
        })
    }
}

fn schedule_effect_progress(
    candidate: &mut EffectJob,
    schedule: EffectSchedule,
    tick: u64,
) -> Result<EffectProgress, EffectError> {
    match schedule {
        EffectSchedule::SameTick => Ok(EffectProgress::ContinueSameTick),
        EffectSchedule::Deferred => {
            candidate.resume_tick = tick.checked_add(1).ok_or(EffectError::CounterOverflow)?;
            Ok(EffectProgress::Deferred)
        }
    }
}

fn return_from_effect_program(
    candidate: &mut EffectJob,
    schedule: EffectSchedule,
    tick: u64,
) -> Result<EffectProgress, EffectError> {
    while let Some(frame) = candidate.call_stack.pop() {
        candidate.program = frame.program;
        candidate.program_version = frame.program_version;
        candidate.state = frame.state;
        if let Some(instruction) = frame.return_instruction {
            candidate.instruction = instruction;
            return schedule_effect_progress(candidate, schedule, tick);
        }
    }
    candidate.complete = true;
    candidate.state.clear();
    Ok(EffectProgress::Complete)
}

impl StateCodec for EffectJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate().map_err(|_| invalid_encoding())?;
        self.encode_state(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let job = Self::decode_state(input)?;
        job.validate().map_err(|_| invalid_encoding())?;
        Ok(job)
    }
}

/// evaluator commandとjob更新をまとめた候補です。
pub struct EffectCandidate<'a> {
    target: &'a mut EffectJob,
    candidate: EffectJob,
    commands: Vec<EffectCommand>,
    new_receipts: Vec<u64>,
    progress: EffectProgress,
}

impl EffectCandidate<'_> {
    /// World transactionへ加える順序付きcommand候補です。
    pub fn commands(&self) -> &[EffectCommand] {
        &self.commands
    }
    /// この命令で初めて追加するreceiptです。資源予約をreceipt単位で冪等化する時に使います。
    pub fn new_receipts(&self) -> &[u64] {
        &self.new_receipts
    }
    /// schedulerへ返す継続状態です。
    pub fn progress(&self) -> EffectProgress {
        self.progress
    }
    /// 外側の同一candidate transactionへ保存する進捗候補です。
    pub fn job_state(&self) -> &EffectJob {
        &self.candidate
    }
    /// 外部のWorld/state candidateを先にcommitし、成功後にだけjobを公開します。
    ///
    /// callbackがerrorを返した場合、このjob候補は破棄され進捗・receiptを変更しません。
    /// callback内でcommandと`job_state`を同じRuntime transactionへ含めてください。
    pub fn commit_with<R, E>(
        self,
        commit_external: impl FnOnce(&[EffectCommand], &EffectJob) -> Result<R, E>,
    ) -> Result<(R, EffectCommit), E> {
        let external = commit_external(&self.commands, &self.candidate)?;
        *self.target = self.candidate;
        Ok((
            external,
            EffectCommit {
                commands: self.commands,
                progress: self.progress,
            },
        ))
    }

    /// World編集、作者StateStore、effect jobを一つの成功境界で公開します。
    ///
    /// StateStore候補を先に作り、effect jobと作者resourceを検証・符号化します。その後Worldの
    /// 独立候補を編集します。どちらかの準備またはWorld検証が失敗すると、StateStore候補と
    /// effect候補は破棄されます。Worldがcommitした後のStateStore commitは、可変borrowを保持し、
    /// 事前にすべてのputを成功させているため追加確保や作者callbackを行わない確定操作です。
    ///
    /// `update_state`と`update_world`が変更できるのはそれぞれ渡された候補だけです。callbackが
    /// 独自globalや外部I/Oへ副作用を出した場合、その副作用はrollbackされません。
    pub fn commit_with_world_and_state<R>(
        self,
        world: &mut World,
        state: &mut StateStore,
        job_key: StableKey,
        job_schema: StableKey,
        update_state: impl FnOnce(
            &[EffectCommand],
            &[u64],
            &EffectJob,
            &mut Transaction<'_>,
        ) -> EngineResult<()>,
        update_world: impl FnOnce(&[EffectCommand], &mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> Result<(R, EffectCommit), EffectTransactionError> {
        let mut state_candidate = state.transaction().map_err(EffectTransactionError::State)?;
        state_candidate
            .put(job_key, job_schema, &self.candidate)
            .map_err(EffectTransactionError::State)?;
        update_state(
            &self.commands,
            &self.new_receipts,
            &self.candidate,
            &mut state_candidate,
        )
        .map_err(EffectTransactionError::State)?;
        state_candidate
            .validate_ready()
            .map_err(EffectTransactionError::State)?;

        let result = world
            .edit(|editor| update_world(&self.commands, editor))
            .map_err(EffectTransactionError::World)?;

        // ここまで全書込が成功済みであり、transaction開始時にrevision上限も検査済み。
        // commitの唯一のerror条件（poison済み候補）は伝播しているため、この境界では発生しない。
        state_candidate
            .commit()
            .map_err(EffectTransactionError::State)?;
        *self.target = self.candidate;
        Ok((
            result,
            EffectCommit {
                commands: self.commands,
                progress: self.progress,
            },
        ))
    }
}

/// World/state連結effect取引の失敗境界です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectTransactionError {
    State(EngineError),
    World(EngineError),
}

impl std::fmt::Display for EffectTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(error) => write!(f, "effectのStateStore取引に失敗しました: {error}"),
            Self::World(error) => write!(f, "effectのWorld取引に失敗しました: {error}"),
        }
    }
}

impl std::error::Error for EffectTransactionError {}

/// job更新を公開した後、Runtimeへ返すcommandと進捗です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCommit {
    pub commands: Vec<EffectCommand>,
    pub progress: EffectProgress,
}

/// RuntimeのSameTick/Deferred規則へ接続するeffect進捗です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectProgress {
    ContinueSameTick,
    Deferred,
    Complete,
}

fn try_clone_job(job: &EffectJob) -> Result<EffectJob, EffectError> {
    let mut state = Vec::new();
    state
        .try_reserve_exact(job.state.len())
        .map_err(|_| EffectError::AllocationFailed)?;
    state.extend_from_slice(&job.state);
    let mut receipts = Vec::new();
    receipts
        .try_reserve_exact(job.receipts.len())
        .map_err(|_| EffectError::AllocationFailed)?;
    receipts.extend_from_slice(&job.receipts);
    let mut call_stack = Vec::new();
    call_stack
        .try_reserve_exact(job.call_stack.len())
        .map_err(|_| EffectError::AllocationFailed)?;
    for frame in &job.call_stack {
        let mut frame_state = Vec::new();
        frame_state
            .try_reserve_exact(frame.state.len())
            .map_err(|_| EffectError::AllocationFailed)?;
        frame_state.extend_from_slice(&frame.state);
        call_stack.push(EffectCallFrame {
            program: frame.program,
            program_version: frame.program_version,
            return_instruction: frame.return_instruction,
            state: frame_state,
        });
    }
    Ok(EffectJob {
        program: job.program,
        program_version: job.program_version,
        evaluator: job.evaluator,
        evaluator_version: job.evaluator_version,
        instruction: job.instruction,
        source: job.source,
        target: job.target,
        random_provider: job.random_provider,
        random_stream: job.random_stream,
        random_cursor: job.random_cursor,
        resume_tick: job.resume_tick,
        same_tick_chain: job.same_tick_chain,
        budget_tick: job.budget_tick,
        instructions_this_tick: job.instructions_this_tick,
        spawns_this_tick: job.spawns_this_tick,
        cells_this_tick: job.cells_this_tick,
        state,
        call_stack,
        receipts,
        revision: job.revision,
        complete: job.complete,
    })
}
fn copy_state(bytes: &[u8]) -> EngineResult<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len()).map_err(allocation)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}
fn clone_values<T: Copy>(values: &[T]) -> Result<Vec<T>, EffectError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(values.len())
        .map_err(|_| EffectError::AllocationFailed)?;
    cloned.extend_from_slice(values);
    Ok(cloned)
}
fn clone_commands(commands: &[EffectCommand]) -> Result<Vec<EffectCommand>, EffectError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(commands.len())
        .map_err(|_| EffectError::AllocationFailed)?;
    for command in commands {
        cloned.push(EffectCommand {
            key: command.key,
            payload: clone_values(&command.payload)?,
        });
    }
    Ok(cloned)
}
fn allocation(_: std::collections::TryReserveError) -> EngineError {
    EngineError::new(ErrorCode::CapacityExceeded, "effect jobの確保")
}
fn invalid_encoding() -> EngineError {
    EngineError::new(ErrorCode::InvalidEncoding, "effect jobの保存形式")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn limits() -> EffectLimits {
        EffectLimits::new(256, 8, 1024, 16, 64, 256, 16).unwrap()
    }
    fn program() -> EffectProgram {
        EffectProgram::new(
            key("test:program"),
            3,
            vec![key("test:first"), key("test:second")],
        )
        .unwrap()
    }

    fn node_catalog(nodes: Vec<EffectNodeDefinition>, limits: EffectLimits) -> EffectNodeCatalog {
        EffectNodeCatalog::new(nodes, 16, limits).unwrap()
    }

    fn emit_node(
        node_key: &str,
        command_key: &str,
        control: EffectControl,
        spawn_count: u32,
        cell_count: u32,
    ) -> EffectNodeDefinition {
        EffectNodeDefinition {
            key: key(node_key),
            operation: EffectNodeOperation::Emit {
                commands: vec![EffectCommand {
                    key: key(command_key),
                    payload: vec![1],
                }],
                state: vec![2],
                receipts: vec![],
                control,
                schedule: EffectSchedule::SameTick,
                spawn_count,
                cell_count,
                random_draws: 0,
            },
        }
    }

    struct Evaluator {
        fail: bool,
    }
    impl EffectEvaluator for Evaluator {
        fn key(&self) -> StableKey {
            key("test:evaluator")
        }
        fn version(&self) -> u32 {
            2
        }
        fn evaluate(
            &self,
            node: StableKey,
            context: EffectContext,
            state: &[u8],
        ) -> Result<EffectNodeResult, EffectError> {
            if self.fail {
                return Err(EffectError::EvaluatorFailed);
            }
            Ok(EffectNodeResult {
                control: EffectControl::Next,
                state: vec![node.as_str().len() as u8],
                commands: vec![EffectCommand {
                    key: key("test:command"),
                    payload: vec![context.tick as u8],
                }],
                receipts: if state.is_empty() { vec![41] } else { vec![42] },
                spawn_count: 0,
                cell_count: 0,
                random_draws: 2,
                schedule: EffectSchedule::Deferred,
            })
        }
    }

    #[test]
    fn deferred_effect_candidate_is_atomic_and_resumes_from_saved_state() {
        let evaluator = Evaluator { fail: false };
        let program = program();
        let mut job = EffectJob::new(&program, &evaluator, 7, 9, key("test:random"), 3).unwrap();
        {
            let candidate = job.prepare(&program, &evaluator, 10, limits()).unwrap();
            assert_eq!(candidate.progress(), EffectProgress::Deferred);
            assert_eq!(candidate.commands().len(), 1);
            assert_eq!(candidate.new_receipts(), &[41]);
        }
        assert_eq!(job.instruction(), 0);
        let (_, commit) = job
            .prepare(&program, &evaluator, 10, limits())
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap();
        assert_eq!(commit.progress, EffectProgress::Deferred);
        assert_eq!(job.instruction(), 1);
        assert_eq!(job.receipts(), &[41]);
        let mut restored = EffectJob::decode(&job.encode(4096).unwrap(), 4096).unwrap();
        assert_eq!(restored, job);
        assert!(matches!(
            restored.prepare(&program, &evaluator, 10, limits()),
            Err(EffectError::InvalidState)
        ));
        let next_candidate = restored
            .prepare(&program, &evaluator, 11, limits())
            .unwrap();
        assert_eq!(next_candidate.new_receipts(), &[42]);
        let next = next_candidate
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap()
            .1;
        assert_eq!(next.progress, EffectProgress::Complete);
        assert_eq!(next.commands.len(), 1);
    }

    #[test]
    fn evaluator_failure_leaves_job_and_commands_unpublished() {
        let program = program();
        let evaluator = Evaluator { fail: false };
        let mut job = EffectJob::new(&program, &evaluator, 1, 2, key("test:random"), 5).unwrap();
        let before = job.clone();
        let failing = Evaluator { fail: true };
        assert!(matches!(
            job.prepare(&program, &failing, 0, limits()),
            Err(EffectError::EvaluatorFailed)
        ));
        assert_eq!(job, before);
        let failed_commit = job
            .prepare(&program, &evaluator, 0, limits())
            .unwrap()
            .commit_with(|_, _| Err::<(), _>("transaction rejected"));
        assert_eq!(failed_commit, Err("transaction rejected"));
        assert_eq!(job, before);
        assert_eq!(
            job.prepare(&program, &evaluator, 0, limits())
                .unwrap()
                .commit_with(|_, _| Ok::<_, ()>(()))
                .unwrap()
                .1
                .progress,
            EffectProgress::Deferred
        );
    }

    #[test]
    fn standard_effect_compiler_validates_branch_and_emits_creator_commands() {
        let limits = limits();
        let catalog = node_catalog(
            vec![
                EffectNodeDefinition {
                    key: key("test:branch"),
                    operation: EffectNodeOperation::Branch {
                        condition: EffectCondition::TargetEquals(9),
                        if_true: 1,
                        if_false: 2,
                        schedule: EffectSchedule::SameTick,
                    },
                },
                emit_node(
                    "test:hit",
                    "test:damage-command",
                    EffectControl::Complete,
                    0,
                    0,
                ),
                emit_node(
                    "test:miss",
                    "test:miss-command",
                    EffectControl::Complete,
                    0,
                    0,
                ),
            ],
            limits,
        );
        let program = catalog
            .compile(
                key("test:compiled-effect"),
                2,
                vec![key("test:branch"), key("test:hit"), key("test:miss")],
            )
            .unwrap();
        assert!(matches!(
            catalog.compile(
                key("test:invalid-effect"),
                1,
                vec![key("test:hit"), key("test:missing")]
            ),
            Err(EffectError::InvalidProgram)
        ));

        let evaluator =
            StandardEffectEvaluator::new(key("test:standard-evaluator"), 1, &catalog).unwrap();
        let mut job =
            EffectJob::new(&program, &evaluator, 4, 9, key("test:counter-random"), 0).unwrap();
        let branch = job
            .prepare(&program, &evaluator, 5, limits)
            .unwrap()
            .commit_with(|commands, candidate| {
                assert!(commands.is_empty());
                assert_eq!(candidate.instruction(), 1);
                Ok::<_, ()>(())
            })
            .unwrap()
            .1;
        assert_eq!(branch.progress, EffectProgress::ContinueSameTick);
        let completed = job
            .prepare(&program, &evaluator, 5, limits)
            .unwrap()
            .commit_with(|commands, _| {
                assert_eq!(commands[0].key, key("test:damage-command"));
                Ok::<_, ()>(())
            })
            .unwrap()
            .1;
        assert_eq!(completed.progress, EffectProgress::Complete);
        assert!(job.is_complete());
    }

    #[test]
    fn timer_node_defers_to_exact_tick_without_advancing_early() {
        let limits = limits();
        let catalog = node_catalog(
            vec![
                EffectNodeDefinition {
                    key: key("test:timer"),
                    operation: EffectNodeOperation::WaitTicks { ticks: 3 },
                },
                emit_node(
                    "test:after-timer",
                    "test:timer-command",
                    EffectControl::Complete,
                    0,
                    0,
                ),
            ],
            limits,
        );
        let program = catalog
            .compile(
                key("test:timer-program"),
                1,
                vec![key("test:timer"), key("test:after-timer")],
            )
            .unwrap();
        let evaluator =
            StandardEffectEvaluator::new(key("test:timer-evaluator"), 1, &catalog).unwrap();
        let mut job = EffectJob::new(&program, &evaluator, 1, 2, key("test:random"), 0).unwrap();

        let commit = job
            .prepare(&program, &evaluator, 10, limits)
            .unwrap()
            .commit_with(|commands, _| {
                assert!(commands.is_empty());
                Ok::<_, ()>(())
            })
            .unwrap()
            .1;
        assert_eq!(commit.progress, EffectProgress::Deferred);
        assert_eq!(job.instruction(), 1);
        assert_eq!(job.resume_tick(), 13);

        let before = job.encode(4096).unwrap();
        assert!(matches!(
            job.prepare(&program, &evaluator, 12, limits),
            Err(EffectError::InvalidState)
        ));
        assert_eq!(job.encode(4096).unwrap(), before);

        let commit = job
            .prepare(&program, &evaluator, 13, limits)
            .unwrap()
            .commit_with(|commands, _| {
                assert_eq!(commands[0].key, key("test:timer-command"));
                Ok::<_, ()>(())
            })
            .unwrap()
            .1;
        assert_eq!(commit.progress, EffectProgress::Complete);
        assert!(job.is_complete());

        let mut overflow_job =
            EffectJob::new(&program, &evaluator, 1, 2, key("test:random"), 0).unwrap();
        let before = overflow_job.encode(4096).unwrap();
        assert!(matches!(
            overflow_job.prepare(&program, &evaluator, u64::MAX - 1, limits),
            Err(EffectError::CounterOverflow)
        ));
        assert_eq!(overflow_job.encode(4096).unwrap(), before);

        let zero_timer = EffectNodeCatalog::new(
            vec![EffectNodeDefinition {
                key: key("test:zero-timer"),
                operation: EffectNodeOperation::WaitTicks { ticks: 0 },
            }],
            16,
            limits,
        );
        assert!(matches!(zero_timer, Err(EffectError::InvalidControl)));
        assert!(matches!(
            catalog.compile(
                key("test:terminal-timer-program"),
                1,
                vec![key("test:timer")]
            ),
            Err(EffectError::InvalidControl)
        ));
        let emit_wait_catalog = node_catalog(
            vec![emit_node(
                "test:emit-wait",
                "test:unused-command",
                EffectControl::WaitTicks(3),
                0,
                0,
            )],
            limits,
        );
        assert!(matches!(
            emit_wait_catalog.compile(
                key("test:terminal-emit-wait-program"),
                1,
                vec![key("test:emit-wait")]
            ),
            Err(EffectError::InvalidControl)
        ));
    }

    #[test]
    fn effect_same_tick_spawn_and_cell_budget_stops_without_publishing_next_node() {
        let limits = EffectLimits::new(32, 4, 128, 8, 1, 1, 8).unwrap();
        let catalog = node_catalog(
            vec![
                emit_node("test:first-spawn", "test:spawn", EffectControl::Next, 1, 1),
                emit_node("test:second-spawn", "test:spawn", EffectControl::Next, 1, 1),
            ],
            limits,
        );
        let program = catalog
            .compile(
                key("test:budget-program"),
                1,
                vec![key("test:first-spawn"), key("test:second-spawn")],
            )
            .unwrap();
        let evaluator =
            StandardEffectEvaluator::new(key("test:standard-evaluator"), 1, &catalog).unwrap();
        let mut job =
            EffectJob::new(&program, &evaluator, 1, 2, key("test:counter-random"), 0).unwrap();
        assert_eq!(
            job.prepare(&program, &evaluator, 10, limits)
                .unwrap()
                .commit_with(|commands, _| {
                    assert_eq!(commands.len(), 1);
                    Ok::<_, ()>(())
                })
                .unwrap()
                .1
                .progress,
            EffectProgress::ContinueSameTick
        );
        let before = job.clone();
        assert!(matches!(
            job.prepare(&program, &evaluator, 10, limits),
            Err(EffectError::BudgetExceededSameTick)
        ));
        assert_eq!(job, before);
        let restored = EffectJob::decode(&job.encode(4096).unwrap(), 4096).unwrap();
        assert_eq!(restored, before);
        assert!(matches!(
            job.prepare(&program, &evaluator, 10, limits),
            Err(EffectError::BudgetExceededSameTick)
        ));
    }

    #[test]
    fn effect_same_tick_instruction_budget_does_not_defer_the_tick() {
        let limits = EffectLimits::new(32, 4, 128, 1, 4, 4, 8).unwrap();
        let catalog = node_catalog(
            vec![
                emit_node(
                    "test:first-step",
                    "test:first-command",
                    EffectControl::Next,
                    0,
                    0,
                ),
                emit_node(
                    "test:second-step",
                    "test:second-command",
                    EffectControl::Next,
                    0,
                    0,
                ),
            ],
            limits,
        );
        let program = catalog
            .compile(
                key("test:instruction-budget-program"),
                1,
                vec![key("test:first-step"), key("test:second-step")],
            )
            .unwrap();
        let evaluator =
            StandardEffectEvaluator::new(key("test:standard-evaluator"), 1, &catalog).unwrap();
        let mut job =
            EffectJob::new(&program, &evaluator, 1, 2, key("test:counter-random"), 0).unwrap();
        job.prepare(&program, &evaluator, 10, limits)
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap();
        let before = job.clone();
        assert!(matches!(
            job.prepare(&program, &evaluator, 10, limits),
            Err(EffectError::BudgetExceededSameTick)
        ));
        assert_eq!(job, before);
    }

    #[test]
    fn effect_program_call_preserves_parent_state_and_roundtrips_stack() {
        let limits = limits();
        let root_key = key("test:root-program");
        let child_key = key("test:child-program");
        let catalog = node_catalog(
            vec![
                EffectNodeDefinition {
                    key: key("test:call-child"),
                    operation: EffectNodeOperation::Emit {
                        commands: Vec::new(),
                        state: vec![7],
                        receipts: Vec::new(),
                        control: EffectControl::CallProgram {
                            key: child_key,
                            version: 2,
                        },
                        schedule: EffectSchedule::SameTick,
                        spawn_count: 0,
                        cell_count: 0,
                        random_draws: 0,
                    },
                },
                EffectNodeDefinition {
                    key: key("test:return-child"),
                    operation: EffectNodeOperation::Emit {
                        commands: Vec::new(),
                        state: vec![9],
                        receipts: Vec::new(),
                        control: EffectControl::Next,
                        schedule: EffectSchedule::Deferred,
                        spawn_count: 0,
                        cell_count: 0,
                        random_draws: 0,
                    },
                },
                EffectNodeDefinition {
                    key: key("test:root-after-call"),
                    operation: EffectNodeOperation::Emit {
                        commands: Vec::new(),
                        state: vec![8],
                        receipts: Vec::new(),
                        control: EffectControl::Complete,
                        schedule: EffectSchedule::SameTick,
                        spawn_count: 0,
                        cell_count: 0,
                        random_draws: 0,
                    },
                },
            ],
            limits,
        );
        let root = catalog
            .compile(
                root_key,
                1,
                vec![key("test:call-child"), key("test:root-after-call")],
            )
            .unwrap();
        let child = catalog
            .compile(child_key, 2, vec![key("test:return-child")])
            .unwrap();
        let program_catalog =
            EffectProgramCatalog::new(vec![root.clone(), child.clone()], &catalog, 4).unwrap();
        assert_eq!(program_catalog.get(child_key, 2).unwrap(), &child);
        assert!(matches!(
            EffectProgramCatalog::new(vec![root.clone()], &catalog, 4),
            Err(EffectError::InvalidProgram)
        ));

        let evaluator =
            StandardEffectEvaluator::new(key("test:standard-evaluator"), 1, &catalog).unwrap();
        let mut job = EffectJob::new(&root, &evaluator, 1, 2, key("test:random"), 3).unwrap();
        job.prepare_with_catalog(&program_catalog, &evaluator, 4, limits)
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap();
        assert_eq!(job.program_key(), child_key);
        assert_eq!(job.call_depth(), 1);
        let restored = EffectJob::decode(&job.encode(4096).unwrap(), 4096).unwrap();
        assert_eq!(restored, job);
        job = restored;

        assert_eq!(job.resume_tick(), 0);
        let resumed = job
            .prepare_with_catalog(&program_catalog, &evaluator, 4, limits)
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap()
            .1;
        assert_eq!(resumed.progress, EffectProgress::Deferred);
        assert_eq!(job.program_key(), root_key);
        assert_eq!(job.instruction(), 1);
        assert_eq!(job.state, [7]);
        assert_eq!(job.call_depth(), 0);
        assert_eq!(job.resume_tick(), 5);

        job.prepare_with_catalog(&program_catalog, &evaluator, 5, limits)
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap();
        assert!(job.is_complete());
    }

    #[test]
    fn effect_program_call_depth_is_bounded_before_candidate_publication() {
        let program_key = key("test:recursive-program");
        let limits = limits().with_max_call_depth(0);
        let catalog = node_catalog(
            vec![EffectNodeDefinition {
                key: key("test:recursive-call"),
                operation: EffectNodeOperation::Emit {
                    commands: Vec::new(),
                    state: Vec::new(),
                    receipts: Vec::new(),
                    control: EffectControl::CallProgram {
                        key: program_key,
                        version: 1,
                    },
                    schedule: EffectSchedule::SameTick,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                },
            }],
            limits,
        );
        let program = catalog
            .compile(program_key, 1, vec![key("test:recursive-call")])
            .unwrap();
        let programs = EffectProgramCatalog::new(vec![program.clone()], &catalog, 1).unwrap();
        let evaluator =
            StandardEffectEvaluator::new(key("test:standard-evaluator"), 1, &catalog).unwrap();
        let mut job = EffectJob::new(&program, &evaluator, 1, 2, key("test:random"), 0).unwrap();
        let before = job.clone();
        assert!(matches!(
            job.prepare_with_catalog(&programs, &evaluator, 0, limits),
            Err(EffectError::CapacityExceeded)
        ));
        assert_eq!(job, before);
    }

    #[test]
    fn effect_custom_evaluator_cannot_publish_an_unregistered_program_call() {
        struct DynamicCallEvaluator {
            target: StableKey,
        }
        impl EffectEvaluator for DynamicCallEvaluator {
            fn key(&self) -> StableKey {
                key("test:dynamic-call-evaluator")
            }
            fn version(&self) -> u32 {
                1
            }
            fn evaluate(
                &self,
                _: StableKey,
                _: EffectContext,
                state: &[u8],
            ) -> Result<EffectNodeResult, EffectError> {
                Ok(EffectNodeResult {
                    control: EffectControl::CallProgram {
                        key: self.target,
                        version: 1,
                    },
                    state: copy_state(state).unwrap(),
                    commands: Vec::new(),
                    receipts: Vec::new(),
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                    schedule: EffectSchedule::SameTick,
                })
            }
        }

        let limits = limits();
        let catalog = node_catalog(
            vec![EffectNodeDefinition {
                key: key("test:dynamic-call-node"),
                operation: EffectNodeOperation::Emit {
                    commands: Vec::new(),
                    state: Vec::new(),
                    receipts: Vec::new(),
                    control: EffectControl::Complete,
                    schedule: EffectSchedule::SameTick,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                },
            }],
            limits,
        );
        let root = catalog
            .compile(
                key("test:dynamic-call-root"),
                1,
                vec![key("test:dynamic-call-node")],
            )
            .unwrap();
        let programs = EffectProgramCatalog::new(vec![root.clone()], &catalog, 1).unwrap();
        let evaluator = DynamicCallEvaluator {
            target: key("test:unregistered-program"),
        };
        let mut job = EffectJob::new(&root, &evaluator, 1, 2, key("test:random"), 0).unwrap();
        let before = job.clone();
        assert!(matches!(
            job.prepare_with_catalog(&programs, &evaluator, 0, limits),
            Err(EffectError::InvalidProgram)
        ));
        assert_eq!(job, before);
    }

    #[test]
    fn older_effect_job_checkpoints_restore_without_call_frames() {
        fn legacy_checkpoint(job: &EffectJob, version: u8) -> Vec<u8> {
            let mut out = Encoder::new(4096);
            match version {
                1 => out.raw(b"CHREFF01").unwrap(),
                2 => out.raw(b"CHREFF02").unwrap(),
                _ => unreachable!(),
            }
            job.program.encode(&mut out).unwrap();
            out.u32(job.program_version).unwrap();
            job.evaluator.encode(&mut out).unwrap();
            out.u32(job.evaluator_version).unwrap();
            out.u32(job.instruction).unwrap();
            out.u64(job.source).unwrap();
            out.u64(job.target).unwrap();
            job.random_provider.encode(&mut out).unwrap();
            out.u64(job.random_stream).unwrap();
            out.u64(job.random_cursor).unwrap();
            out.u64(job.resume_tick).unwrap();
            if version == 2 {
                out.u8(u8::from(job.same_tick_chain)).unwrap();
                out.u64(job.budget_tick).unwrap();
                out.u32(job.instructions_this_tick).unwrap();
                out.u32(job.spawns_this_tick).unwrap();
                out.u32(job.cells_this_tick).unwrap();
            }
            out.bytes(&job.state).unwrap();
            out.u32(job.receipts.len() as u32).unwrap();
            for receipt in &job.receipts {
                out.u64(*receipt).unwrap();
            }
            out.u64(job.revision).unwrap();
            out.u8(u8::from(job.complete)).unwrap();
            out.finish()
        }

        let program = program();
        let evaluator = Evaluator { fail: false };
        let mut source = EffectJob::new(&program, &evaluator, 1, 2, key("test:random"), 3).unwrap();
        source.instruction = 1;
        source.random_cursor = 7;
        source.resume_tick = 10;
        source.same_tick_chain = true;
        source.budget_tick = 10;
        source.instructions_this_tick = 2;
        source.spawns_this_tick = 3;
        source.cells_this_tick = 4;
        source.state = vec![5, 6];
        source.receipts = vec![7, 9];
        source.revision = 11;

        let version_two = EffectJob::decode(&legacy_checkpoint(&source, 2), 4096).unwrap();
        assert_eq!(version_two, source);

        let version_one = EffectJob::decode(&legacy_checkpoint(&source, 1), 4096).unwrap();
        assert_eq!(version_one.program, source.program);
        assert_eq!(version_one.program_version, source.program_version);
        assert_eq!(version_one.instruction, source.instruction);
        assert_eq!(version_one.state, source.state);
        assert_eq!(version_one.receipts, source.receipts);
        assert_eq!(version_one.resume_tick, source.resume_tick);
        assert!(!version_one.same_tick_chain);
        assert_eq!(version_one.budget_tick, source.resume_tick);
        assert_eq!(version_one.instructions_this_tick, 0);
        assert_eq!(version_one.spawns_this_tick, 0);
        assert_eq!(version_one.cells_this_tick, 0);
        assert!(version_one.call_stack.is_empty());
    }
}
