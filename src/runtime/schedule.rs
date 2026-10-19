//! 明示した依存・読書集合・barrierで作者Systemを順序付けます。
//!
//! SameTickはRust callbackの再帰で処理せず、同じcursorを次pollで再開します。
//! 全phaseとqueueが完了するまで、EngineはWorld候補を公開しません。

use super::engine::{CommandBuffer, ServiceRegistry, WorldAccess, WorldSet};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};

/// X1からX6へ対応する標準phaseです。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// 入力解釈や作者の前処理。Scene更新の前に候補Worldへ反映できます。
    BeforeScenes,
    /// 作者・AIの意図。
    Intent,
    /// 配置やinventoryの取引。
    Transactions,
    /// controllerとbody。
    Physics,
    /// cellや支持。
    Materials,
    /// 熱・反応。
    Reactions,
    /// 接触後の進捗確定。
    Resolve,
}

/// 一回の件数予算内での進捗です。壁時計は判断に使いません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkStatus {
    /// このtickの仕事を終えた。
    Complete,
    /// 同じtickに続きが必要。
    ContinueSameTick,
    /// 続きを次tickに実行してよい。
    Deferred,
}

/// 作者Systemの順序・権限です。未宣言のWorld recordへアクセスできません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemDescriptor {
    /// 永続manifestで使う安定キー。
    pub key: StableKey,
    /// 実行phase。
    pub phase: Phase,
    /// 先に完了するSystem。
    pub after: Vec<StableKey>,
    /// 読取可能なrecord。
    pub reads: Vec<StableKey>,
    /// 書込可能なrecord。読取も認めます。
    pub writes: Vec<StableKey>,
    /// 必要なservice slot。
    pub services: Vec<StableKey>,
    /// このSceneがRunningの期間だけ動きます。NoneはWorld側の独立System。
    pub scene: Option<StableKey>,
    /// このWorldがpause中なら実行しません。NoneはWorld横断の独立System。
    pub world: Option<StableKey>,
    /// 終了・継続の各回でcommandを候補内に反映します。
    pub barrier_after: bool,
    /// 読書宣言が独立した他Systemとworker上で同時実行できるという作者保証です。
    pub parallel: bool,
}
impl SystemDescriptor {
    /// 空の権限から組み立てます。
    pub fn new(key: StableKey, phase: Phase) -> Self {
        Self {
            key,
            phase,
            after: Vec::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            services: Vec::new(),
            scene: None,
            world: None,
            barrier_after: false,
            parallel: false,
        }
    }
    /// 依存を追加します。
    pub fn after(mut self, key: StableKey) -> Self {
        self.after.push(key);
        self
    }
    /// 読取権限を追加します。
    pub fn read(mut self, key: StableKey) -> Self {
        self.reads.push(key);
        self
    }
    /// 書込権限を追加します。
    pub fn write(mut self, key: StableKey) -> Self {
        self.writes.push(key);
        self
    }
    /// service依存を追加します。
    pub fn requires(mut self, key: StableKey) -> Self {
        self.services.push(key);
        self
    }
    /// Scene寿命へ結び付けます。
    pub fn in_scene(mut self, key: StableKey) -> Self {
        self.scene = Some(key);
        self
    }
    /// 自動更新を一つのWorldの活動状態へ結び付けます。
    pub fn in_world(mut self, key: StableKey) -> Self {
        self.world = Some(key);
        self
    }
    /// 候補内のcommit barrierを有効にします。
    pub fn barrier(mut self) -> Self {
        self.barrier_after = true;
        self
    }
    /// 宣言外の共有状態や外部副作用、呼出順への依存がないことを保証してworker実行を許可します。
    pub fn parallel(mut self) -> Self {
        self.parallel = true;
        self
    }
    pub(crate) fn allows(&self, key: StableKey, write: bool) -> bool {
        self.writes.contains(&key) || (!write && self.reads.contains(&key))
    }
}

/// 正規の進捗はWorldのStateStoreに置き、selfには実装や再生成できるcacheを置きます。
///
/// `SystemDescriptor::parallel`を指定したSystemはworkerへ移されるため、実装はSendです。
pub trait System: Send + 'static {
    /// 一回分を進め、同tickでの継続が必要か返します。
    fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus>;
}

/// 作者Systemへ一時的に渡す権限です。World内部の可変配列は公開しません。
pub struct SystemContext<'a> {
    pub(crate) tick: u64,
    pub(crate) worlds: &'a WorldSet,
    pub(crate) commands: &'a mut CommandBuffer,
    pub(crate) services: &'a ServiceRegistry,
    pub(crate) descriptor: &'a SystemDescriptor,
    pub(crate) input: &'a super::input::TickInput,
}
impl SystemContext<'_> {
    /// 処理中のlogical tick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 先行barrierまで反映された候補の読取と、順序付きcommandの提出。
    pub fn worlds(&mut self) -> WorldAccess<'_> {
        WorldAccess {
            worlds: self.worlds,
            commands: self.commands,
            access: Some(self.descriptor),
        }
    }
    /// このtickへ正規化された入力です。再開のたびに同じ値を読み取ります。
    pub fn input(&self) -> &super::input::TickInput {
        self.input
    }
    /// 登録された実装を取得します。
    pub fn services(&self) -> &ServiceRegistry {
        self.services
    }
}

pub(crate) struct SystemEntry {
    pub descriptor: SystemDescriptor,
    pub system: Box<dyn System>,
}

/// 同じpollで独立して開始できる連続System数を求めます。
///
/// 順序依存、読書競合、phase境界、明示barrierの手前でwaveを止めます。
pub(crate) fn independent_wave_len(entries: &[SystemEntry], start: usize, limit: usize) -> usize {
    if limit <= 1 || start >= entries.len() {
        return 1;
    }
    let first = &entries[start].descriptor;
    if !first.parallel || first.barrier_after {
        return 1;
    }
    let mut end = start + 1;
    while end < entries.len() && end - start < limit {
        let current = &entries[end].descriptor;
        if current.phase != first.phase {
            break;
        }
        if !current.parallel {
            break;
        }
        let independent = entries[start..end].iter().all(|prior| {
            let prior = &prior.descriptor;
            !current.after.contains(&prior.key)
                && !prior
                    .writes
                    .iter()
                    .any(|key| current.reads.contains(key) || current.writes.contains(key))
                && !current.writes.iter().any(|key| prior.reads.contains(key))
                && !prior.barrier_after
        });
        if !independent {
            break;
        }
        end += 1;
        if current.barrier_after {
            break;
        }
    }
    end - start
}

pub(crate) fn plan(
    mut entries: Vec<SystemEntry>,
    services: &ServiceRegistry,
) -> EngineResult<Vec<SystemEntry>> {
    entries.sort_by_key(|entry| (entry.descriptor.phase, entry.descriptor.key));
    for (index, entry) in entries.iter().enumerate() {
        if entries[..index]
            .iter()
            .any(|previous| previous.descriptor.key == entry.descriptor.key)
        {
            return Err(error(ErrorCode::DuplicateKey));
        }
        for slot in &entry.descriptor.services {
            if !services.contains(*slot) {
                return Err(error(ErrorCode::MissingDependency));
            }
        }
        for dependency in &entry.descriptor.after {
            let prior = entries
                .iter()
                .find(|other| other.descriptor.key == *dependency)
                .ok_or_else(|| error(ErrorCode::MissingDependency))?;
            if prior.descriptor.phase > entry.descriptor.phase {
                return Err(error(ErrorCode::DependencyCycle));
            }
        }
    }
    let mut result: Vec<SystemEntry> = Vec::new();
    while !entries.is_empty() {
        let phase = entries[0].descriptor.phase;
        let index = entries
            .iter()
            .position(|entry| {
                entry.descriptor.phase == phase
                    && entry
                        .descriptor
                        .after
                        .iter()
                        .all(|key| result.iter().any(|done| done.descriptor.key == *key))
            })
            .ok_or_else(|| error(ErrorCode::DependencyCycle))?;
        result.push(entries.remove(index));
    }
    // 同phaseの競合を単なるキー順で隠しません。作者が依存を明示した場合だけ許可します。
    for index in 0..result.len() {
        let mut ancestors = Vec::new();
        ancestors
            .try_reserve_exact(index)
            .map_err(crate::foundation::error::allocation)?;
        ancestors.resize(index, false);
        for earlier in (0..index).rev() {
            ancestors[earlier] = result[index]
                .descriptor
                .after
                .contains(&result[earlier].descriptor.key)
                || (earlier + 1..index).any(|later| {
                    ancestors[later]
                        && result[later]
                            .descriptor
                            .after
                            .contains(&result[earlier].descriptor.key)
                });
        }
        let current = &result[index].descriptor;
        for earlier in 0..index {
            let prior = &result[earlier].descriptor;
            // worldは活動状態への所属であり、実際の読書先を限定しません。
            // 別World所属でも同じrecordへ到達できるため、競合は宣言キーで検査します。
            if prior.phase != current.phase || ancestors[earlier] {
                continue;
            }
            let conflict = prior
                .writes
                .iter()
                .any(|key| current.reads.contains(key) || current.writes.contains(key))
                || current.writes.iter().any(|key| prior.reads.contains(key));
            if conflict {
                return Err(error(ErrorCode::Conflict));
            }
        }
    }
    Ok(result)
}

/// 解決済みの順序と権限です。実装の関数pointerやOS資源は含めません。
///
/// 起動後に確認・記録するための値で、実行中の順序を暗黙に変更するものではありません。
/// providerの実装キーはServiceRegistry::manifestと組み合わせて記録してください。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleProfile {
    /// 同じSystemのSameTick継続回数の診断上限。poll予算とは別に記録します。
    pub same_tick_limit: u64,
    /// 依存解決後の実行順。phase境界とbarrier_afterが候補の反映箇所です。
    pub systems: Vec<SystemDescriptor>,
}
fn error(code: ErrorCode) -> EngineError {
    EngineError::new(code, "作者Systemの依存と権限")
}
