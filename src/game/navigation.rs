//! 作者が提供する精密な移動queryへ接続する、決定的で再開可能なA*探索。
//!
//! 探索jobはfrontier、確定cost、親node、入力revisionを所有し、codecで保存できます。
//! Worldやcollision実装を複製せず、各展開時に作者queryを呼び出します。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use crate::world::CollisionQueryShapeRef;
use std::collections::{BTreeMap, BTreeSet};

/// 1回のpollで返された移動辺です。costは正の整数tick相当値です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationEdge {
    /// 到達先の安定node key。
    pub target: u64,
    /// この辺を移動する整数cost。0は不正です。
    pub cost: u32,
    /// walk/jump/swim等、作者が実行する移動actionの安定キー。
    pub action: StableKey,
}

/// jobが保持できる固有node数と、1 nodeから返す移動辺数の作者指定上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationLimits {
    /// 保存される探索nodeの最大数。
    pub nodes: u32,
    /// 1 nodeの展開で受け付ける移動辺の最大数。
    pub edges_per_node: u32,
}

impl NavigationLimits {
    /// ゼロの容量を拒否する探索上限を構築します。
    pub fn new(nodes: u32, edges_per_node: u32) -> Result<Self, NavigationError> {
        if nodes == 0 || edges_per_node == 0 {
            return Err(NavigationError::InvalidInput);
        }
        Ok(Self {
            nodes,
            edges_per_node,
        })
    }
}

/// profile別の通行可能条件に加算する危険地形costです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationHazardCost {
    /// 地形またはauthor-defined hazardの安定key。
    pub hazard: StableKey,
    /// そのhazardへ入るaction edgeへ加算するtick数。
    pub additional_ticks: u32,
}

/// profileと一緒に保存する移動能力・費用です。
///
/// edge生成と精密geometry判定はauthor `NavigationQuery`が担当します。この値はその判定へ
/// 必要なfootprint、移動能力、collision group、整数costを提供し、game固有の遷移規則を固定しません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationAgentConfig {
    /// 1 nodeが占めるgrid cell数。細かな形状はprofileのshape providerが判定します。
    pub footprint_cells: [u32; 2],
    /// 1 cell相当の水平移動に必要な整数tick。queryは距離に応じてedge costへ反映します。
    pub travel_ticks_per_cell: u32,
    /// 地形上へstepで上がれる最大cell数。
    pub max_step_cells: u32,
    /// jumpの最高点と水平range。両方0ならjump edgeを出しません。
    pub max_jump_height_cells: u32,
    pub max_jump_range_cells: u32,
    /// 落下actionで移動できる最大の垂直距離。
    pub max_fall_cells: u32,
    /// 液体または水中領域を移動できるprofileです。
    pub can_swim: bool,
    /// 飛行経路と飛行用collision responseを使えるprofileです。
    pub can_fly: bool,
    /// collision providerへ渡す通過group maskです。
    pub collision_groups: u32,
    /// 各hazard keyを通過する際に加算する整数tickです。
    pub hazard_costs: Vec<NavigationHazardCost>,
}

impl Default for NavigationAgentConfig {
    fn default() -> Self {
        Self {
            footprint_cells: [1, 1],
            travel_ticks_per_cell: 1,
            max_step_cells: 0,
            max_jump_height_cells: 0,
            max_jump_range_cells: 0,
            max_fall_cells: 1,
            can_swim: false,
            can_fly: false,
            collision_groups: u32::MAX,
            hazard_costs: Vec::new(),
        }
    }
}

/// 安定key/versionと作者shape参照を持つ保存可能なagent navigation profileです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationAgentProfile {
    key: StableKey,
    version: u32,
    shape: CollisionQueryShapeRef,
    config: NavigationAgentConfig,
}

impl NavigationAgentProfile {
    /// author geometry参照とsettingsを検証し、hazard cost件数をcaller上限内に制限します。
    pub fn new(
        key: StableKey,
        version: u32,
        shape: CollisionQueryShapeRef,
        mut config: NavigationAgentConfig,
        hazard_limit: u32,
    ) -> Result<Self, NavigationError> {
        config.hazard_costs.sort_unstable_by_key(|cost| cost.hazard);
        let profile = Self {
            key,
            version,
            shape,
            config,
        };
        profile.validate(hazard_limit)?;
        Ok(profile)
    }

    /// content/provider登録keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }

    /// settingsとshape参照のschema versionです。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// exact overlap/sweepで使うprovider-owned geometry参照です。
    pub fn shape(&self) -> CollisionQueryShapeRef {
        self.shape
    }

    /// movement capability、footprint、group、hazard costです。
    pub fn config(&self) -> &NavigationAgentConfig {
        &self.config
    }

    /// callerが指定するhazard上限と全profile invariantを再検査します。
    pub fn validate(&self, hazard_limit: u32) -> Result<(), NavigationError> {
        let config = &self.config;
        if self.version == 0
            || self.shape.version == 0
            || config.footprint_cells.contains(&0)
            || config.travel_ticks_per_cell == 0
            || config.collision_groups == 0
            || (config.max_jump_height_cells == 0) != (config.max_jump_range_cells == 0)
            || config.hazard_costs.len() > hazard_limit as usize
            || config
                .hazard_costs
                .iter()
                .any(|cost| cost.additional_ticks == 0)
            || config
                .hazard_costs
                .windows(2)
                .any(|pair| pair[0].hazard >= pair[1].hazard)
        {
            return Err(NavigationError::InvalidInput);
        }
        Ok(())
    }
}

impl StateCodec for NavigationAgentProfile {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate(u32::MAX).map_err(|_| invalid_encoding())?;
        self.key.encode(out)?;
        out.u32(self.version)?;
        self.shape.provider.encode(out)?;
        out.u32(self.shape.version)?;
        out.u32(self.shape.owner)?;
        out.u64(self.shape.object)?;
        out.u32(self.shape.part)?;
        out.u32(self.config.footprint_cells[0])?;
        out.u32(self.config.footprint_cells[1])?;
        out.u32(self.config.travel_ticks_per_cell)?;
        out.u32(self.config.max_step_cells)?;
        out.u32(self.config.max_jump_height_cells)?;
        out.u32(self.config.max_jump_range_cells)?;
        out.u32(self.config.max_fall_cells)?;
        out.u8(u8::from(self.config.can_swim))?;
        out.u8(u8::from(self.config.can_fly))?;
        out.u32(self.config.collision_groups)?;
        out.u32(u32::try_from(self.config.hazard_costs.len()).map_err(|_| invalid_encoding())?)?;
        for cost in &self.config.hazard_costs {
            cost.hazard.encode(out)?;
            out.u32(cost.additional_ticks)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let key = StableKey::decode(input)?;
        let version = input.u32()?;
        let shape = CollisionQueryShapeRef {
            provider: StableKey::decode(input)?,
            version: input.u32()?,
            owner: input.u32()?,
            object: input.u64()?,
            part: input.u32()?,
        };
        let footprint_cells = [input.u32()?, input.u32()?];
        let travel_ticks_per_cell = input.u32()?;
        let max_step_cells = input.u32()?;
        let max_jump_height_cells = input.u32()?;
        let max_jump_range_cells = input.u32()?;
        let max_fall_cells = input.u32()?;
        let can_swim = decode_bool(input)?;
        let can_fly = decode_bool(input)?;
        let collision_groups = input.u32()?;
        let count = input.u32()? as usize;
        // StableKeyは長さheader + 最短3byte key、追加costは4byteです。
        if count > input.remaining() / 11 {
            return Err(invalid_encoding());
        }
        let mut hazard_costs = Vec::new();
        hazard_costs
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            hazard_costs.push(NavigationHazardCost {
                hazard: StableKey::decode(input)?,
                additional_ticks: input.u32()?,
            });
        }
        let profile = Self {
            key,
            version,
            shape,
            config: NavigationAgentConfig {
                footprint_cells,
                travel_ticks_per_cell,
                max_step_cells,
                max_jump_height_cells,
                max_jump_range_cells,
                max_fall_cells,
                can_swim,
                can_fly,
                collision_groups,
                hazard_costs,
            },
        };
        profile.validate(u32::MAX).map_err(|_| invalid_encoding())?;
        Ok(profile)
    }
}

/// 完成経路のnodeと、そこへ入るときに実行するactionです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationStep {
    /// 安定node key。
    pub node: u64,
    /// 開始nodeではNone、それ以降は直前からの移動actionです。
    pub action: Option<StableKey>,
}

impl NavigationStep {
    fn start(node: u64) -> Self {
        Self { node, action: None }
    }
}

/// 地形・body shape・streaming coverageを含む作者側のquery結果です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavigationExpansion {
    /// 展開できる移動辺。エンジン側で安定順へ正規化します。
    Ready(Vec<NavigationEdge>),
    /// 必要regionが未準備です。jobは同じnodeから安全に再開します。
    WaitingForRegion(u64),
}

/// 探索中に起きた契約違反または上限到達です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavigationError {
    InvalidInput,
    StaleInput,
    InvalidEdge,
    CostOverflow,
    NodeLimit,
    AllocationFailed,
    InvalidState,
}

impl std::fmt::Display for NavigationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for NavigationError {}

/// profileごとの形状、能力、移動規則を適用して隣接nodeと下界heuristicを返します。
///
/// `neighbors`は最終的なshape/cell queryを実施し、近似したcollision summaryだけで
/// 通過可能と判定してはいけません。`revision`は地形、body、profile規則など、探索結果へ
/// 影響する入力が変わるたびに更新します。heuristicはgoalまでの実costを超えない下界です。
pub trait NavigationQuery {
    /// 地形・動体・profile規則をまとめた入力revision。
    fn revision(&self) -> u64;

    /// 登録済みagent profileを返します。旧queryは`None`を返す既定動作で従来のkey連携を使えます。
    fn agent_profile(
        &self,
        _key: StableKey,
    ) -> Result<Option<&NavigationAgentProfile>, NavigationError> {
        Ok(None)
    }

    /// 古いrevisionで展開済みnodeの隣接結果が現在も有効か検証します。
    ///
    /// Regionの読込等でrevisionが変わったjobを引き継ぐ際、依存する区間を照合できない
    /// providerはfalseを返して再探索を要求します。既定実装は安全側に倒して再利用しません。
    fn can_reuse_expansion(
        &self,
        _profile: StableKey,
        _node: u64,
        _old_revision: u64,
    ) -> Result<bool, NavigationError> {
        Ok(false)
    }

    /// 1 nodeを展開し、辺配列の所有権を呼出側へ返します。
    fn neighbors(
        &self,
        profile: StableKey,
        node: u64,
    ) -> Result<NavigationExpansion, NavigationError>;

    /// 登録profileを使うqueryです。profile対応後も既存queryはkey callbackへ委譲できます。
    fn neighbors_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
    ) -> Result<NavigationExpansion, NavigationError> {
        self.neighbors(profile.key(), node)
    }

    /// goalまでのadmissibleな整数cost下界を返します。
    fn heuristic(&self, profile: StableKey, node: u64, goal: u64) -> Result<u64, NavigationError>;

    /// 登録profileを使うheuristicです。結果は移動costの下界でなければなりません。
    fn heuristic_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
        goal: u64,
    ) -> Result<u64, NavigationError> {
        self.heuristic(profile.key(), node, goal)
    }
}

/// budget内での探索結果です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavigationStatus {
    /// 指定budgetを使い切ったため、同じjobを次回pollできます。
    Continue,
    /// 読込が必要です。region準備後に同じjobを再pollします。
    WaitingForRegion(u64),
    /// 経路が見つかりました。node列は開始点とgoalを含みます。
    Complete(Vec<NavigationStep>),
    /// 到達可能なnodeを全て調べましたが経路がありません。
    NoPath,
}

/// 保存・復元できるA*の途中状態です。
///
/// BTree集合を使い、同点を`f, h, node key`で固定します。node上限を超えたときは
/// 部分経路を公開せずエラーにし、呼出側がjobを破棄するか、より大きい上限で再作成します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationJob {
    profile: StableKey,
    profile_version: Option<u32>,
    start: u64,
    goal: u64,
    input_revision: u64,
    limits: NavigationLimits,
    open: BTreeSet<(u64, u64, u64)>,
    scores: BTreeMap<u64, u64>,
    estimates: BTreeMap<u64, u64>,
    parents: BTreeMap<u64, (u64, StableKey)>,
    closed: BTreeSet<u64>,
    finished: bool,
}

impl NavigationJob {
    /// queryの現在revisionを固定した探索jobを作ります。
    pub fn new(
        query: &impl NavigationQuery,
        profile: StableKey,
        start: u64,
        goal: u64,
        limits: NavigationLimits,
    ) -> Result<Self, NavigationError> {
        if limits.nodes == 0 || limits.edges_per_node == 0 {
            return Err(NavigationError::InvalidInput);
        }
        let input_revision = query.revision();
        let agent_profile = query.agent_profile(profile)?;
        let profile_version = agent_profile.map(NavigationAgentProfile::version);
        let h = match agent_profile {
            Some(profile) => query.heuristic_with_profile(profile, start, goal)?,
            None => query.heuristic(profile, start, goal)?,
        };
        if query.revision() != input_revision {
            return Err(NavigationError::StaleInput);
        }
        if query
            .agent_profile(profile)?
            .map(NavigationAgentProfile::version)
            != profile_version
        {
            return Err(NavigationError::StaleInput);
        }
        let mut open = BTreeSet::new();
        open.insert((h, h, start));
        let mut scores = BTreeMap::new();
        scores.insert(start, 0);
        let mut estimates = BTreeMap::new();
        estimates.insert(start, h);
        Ok(Self {
            profile,
            profile_version,
            start,
            goal,
            input_revision,
            limits,
            open,
            scores,
            estimates,
            parents: BTreeMap::new(),
            closed: BTreeSet::new(),
            finished: false,
        })
    }

    /// 対象World/profileと結び付いた入力revisionです。
    pub fn input_revision(&self) -> u64 {
        self.input_revision
    }

    /// 変更後のqueryで全ての確定済み展開が有効だとproviderが証明できる場合にrevisionを更新します。
    ///
    /// 途中で一つでも無効または未判定の区間があればjobは変更せず`StaleInput`を返します。
    /// 有効性が確認できた場合もfrontier各nodeのheuristicを再取得し、現在のqueryに合う
    /// 優先度へ組み直します。地形revision変更後に旧heuristicを使い続けると、旧cost下界が
    /// 新しい経路では下界でなくなり、最短でないgoalを確定する恐れがあるためです。
    pub fn revalidate(&mut self, query: &impl NavigationQuery) -> Result<(), NavigationError> {
        if self.finished {
            return Err(NavigationError::InvalidState);
        }
        if query
            .agent_profile(self.profile)?
            .map(NavigationAgentProfile::version)
            != self.profile_version
        {
            return Err(NavigationError::StaleInput);
        }
        if query.revision() == self.input_revision {
            return Ok(());
        }
        let new_revision = query.revision();
        for node in &self.closed {
            if !query.can_reuse_expansion(self.profile, *node, self.input_revision)? {
                return Err(NavigationError::StaleInput);
            }
        }
        let mut refreshed = Vec::new();
        refreshed
            .try_reserve_exact(self.open.len())
            .map_err(|_| NavigationError::AllocationFailed)?;
        let mut new_open = BTreeSet::new();
        for (_, _, node) in &self.open {
            let score = *self.scores.get(node).ok_or(NavigationError::InvalidState)?;
            let heuristic = match self.profile_version {
                Some(version) => {
                    let profile = query
                        .agent_profile(self.profile)?
                        .filter(|profile| profile.version() == version)
                        .ok_or(NavigationError::StaleInput)?;
                    query.heuristic_with_profile(profile, *node, self.goal)?
                }
                None => query.heuristic(self.profile, *node, self.goal)?,
            };
            if query.revision() != new_revision
                || query
                    .agent_profile(self.profile)?
                    .map(NavigationAgentProfile::version)
                    != self.profile_version
            {
                return Err(NavigationError::StaleInput);
            }
            let estimate = score
                .checked_add(heuristic)
                .ok_or(NavigationError::CostOverflow)?;
            new_open.insert((estimate, heuristic, *node));
            refreshed.push((*node, heuristic));
        }
        if query.revision() != new_revision {
            return Err(NavigationError::StaleInput);
        }
        for (node, heuristic) in refreshed {
            self.estimates.insert(node, heuristic);
        }
        self.open = new_open;
        self.input_revision = new_revision;
        Ok(())
    }

    /// 次の固定node予算分だけ進めます。0予算は状態を変更しません。
    pub fn poll(
        &mut self,
        query: &impl NavigationQuery,
        node_budget: u32,
    ) -> Result<NavigationStatus, NavigationError> {
        if query.revision() != self.input_revision {
            return Err(NavigationError::StaleInput);
        }
        if query
            .agent_profile(self.profile)?
            .map(NavigationAgentProfile::version)
            != self.profile_version
        {
            return Err(NavigationError::StaleInput);
        }
        if self.finished {
            return Err(NavigationError::InvalidState);
        }
        if node_budget == 0 {
            return Ok(NavigationStatus::Continue);
        }

        let mut expanded = 0;
        while expanded < node_budget {
            let Some(priority) = self.open.pop_first() else {
                self.finished = true;
                return Ok(NavigationStatus::NoPath);
            };
            let node = priority.2;
            if self.closed.contains(&node) {
                continue;
            }
            if node == self.goal {
                let path = match self.reconstruct() {
                    Ok(path) => path,
                    Err(error) => {
                        self.open.insert(priority);
                        return Err(error);
                    }
                };
                self.finished = true;
                return Ok(NavigationStatus::Complete(path));
            }

            let expansion = match self.profile_version {
                Some(version) => match query.agent_profile(self.profile) {
                    Ok(Some(profile)) if profile.version() == version => {
                        query.neighbors_with_profile(profile, node)
                    }
                    Ok(_) => Err(NavigationError::StaleInput),
                    Err(error) => Err(error),
                },
                None => query.neighbors(self.profile, node),
            };
            let expansion = match expansion {
                Ok(expansion) => expansion,
                Err(error) => {
                    self.open.insert(priority);
                    return Err(error);
                }
            };
            let edges = match expansion {
                NavigationExpansion::WaitingForRegion(region) => {
                    self.open.insert(priority);
                    return Ok(NavigationStatus::WaitingForRegion(region));
                }
                NavigationExpansion::Ready(edges) => edges,
            };
            if query.revision() != self.input_revision {
                self.open.insert(priority);
                return Err(NavigationError::StaleInput);
            }
            let current_profile_version = match query.agent_profile(self.profile) {
                Ok(profile) => profile.map(NavigationAgentProfile::version),
                Err(error) => {
                    self.open.insert(priority);
                    return Err(error);
                }
            };
            if current_profile_version != self.profile_version {
                self.open.insert(priority);
                return Err(NavigationError::StaleInput);
            }
            let mut edges = edges;
            if edges.len() > self.limits.edges_per_node as usize {
                self.open.insert(priority);
                return Err(NavigationError::InvalidEdge);
            }
            edges.sort_unstable_by_key(|edge| (edge.target, edge.cost, edge.action));
            if edges.iter().any(|edge| edge.cost == 0) {
                self.open.insert(priority);
                return Err(NavigationError::InvalidEdge);
            }
            edges.dedup_by_key(|edge| edge.target);
            let prepared = (|| {
                let current = *self
                    .scores
                    .get(&node)
                    .ok_or(NavigationError::InvalidState)?;
                let mut updates = Vec::new();
                updates
                    .try_reserve_exact(edges.len())
                    .map_err(|_| NavigationError::AllocationFailed)?;
                let mut new_nodes = 0usize;
                for edge in edges {
                    let next = current
                        .checked_add(u64::from(edge.cost))
                        .ok_or(NavigationError::CostOverflow)?;
                    if self
                        .scores
                        .get(&edge.target)
                        .is_some_and(|old| *old <= next)
                    {
                        continue;
                    }
                    if !self.scores.contains_key(&edge.target) {
                        if self.scores.len().saturating_add(new_nodes) >= self.limits.nodes as usize
                        {
                            return Err(NavigationError::NodeLimit);
                        }
                        new_nodes += 1;
                    }
                    let h = match self.profile_version {
                        Some(version) => {
                            let profile = query
                                .agent_profile(self.profile)?
                                .filter(|profile| profile.version() == version)
                                .ok_or(NavigationError::StaleInput)?;
                            query.heuristic_with_profile(profile, edge.target, self.goal)?
                        }
                        None => query.heuristic(self.profile, edge.target, self.goal)?,
                    };
                    if query.revision() != self.input_revision {
                        return Err(NavigationError::StaleInput);
                    }
                    if query
                        .agent_profile(self.profile)?
                        .map(NavigationAgentProfile::version)
                        != self.profile_version
                    {
                        return Err(NavigationError::StaleInput);
                    }
                    let f = next.checked_add(h).ok_or(NavigationError::CostOverflow)?;
                    updates.push((edge.target, next, h, f, edge.action));
                }
                Ok::<_, NavigationError>(updates)
            })();
            let updates = match prepared {
                Ok(updates) => updates,
                Err(error) => {
                    self.open.insert(priority);
                    return Err(error);
                }
            };
            self.closed.insert(node);
            expanded += 1;
            for (target, next, h, f, action) in updates {
                let old_score = self.scores.insert(target, next);
                if let Some(old) = old_score
                    && let Some(old_h) = self.estimates.get(&target).copied()
                    && let Some(old_f) = old.checked_add(old_h)
                {
                    self.open.remove(&(old_f, old_h, target));
                }
                if old_score.is_some() {
                    self.closed.remove(&target);
                }
                self.estimates.insert(target, h);
                self.parents.insert(target, (node, action));
                self.open.insert((f, h, target));
            }
        }
        Ok(NavigationStatus::Continue)
    }

    fn reconstruct(&self) -> Result<Vec<NavigationStep>, NavigationError> {
        let mut path = Vec::new();
        path.try_reserve_exact(self.parents.len().saturating_add(1))
            .map_err(|_| NavigationError::AllocationFailed)?;
        let mut node = self.goal;
        path.push(NavigationStep { node, action: None });
        while node != self.start {
            let (parent, action) = *self
                .parents
                .get(&node)
                .ok_or(NavigationError::InvalidState)?;
            path.last_mut().ok_or(NavigationError::InvalidState)?.action = Some(action);
            node = parent;
            path.push(NavigationStep::start(node));
            if path.len() > self.limits.nodes as usize {
                return Err(NavigationError::InvalidState);
            }
        }
        path.reverse();
        Ok(path)
    }

    /// job状態をサイズ上限付きcodecで保存します。
    pub fn encode(&self, byte_limit: usize) -> Result<Vec<u8>, NavigationError> {
        self.validate()?;
        let mut out = Encoder::new(byte_limit);
        self.encode_state(&mut out)
            .map_err(|_| NavigationError::InvalidState)?;
        Ok(out.finish())
    }

    /// 保存stateを検証し、独立した再開jobとして復元します。
    pub fn decode(bytes: &[u8], byte_limit: usize) -> Result<Self, NavigationError> {
        let mut input =
            Decoder::new(bytes, byte_limit).map_err(|_| NavigationError::InvalidState)?;
        let job = Self::decode_state(&mut input).map_err(|_| NavigationError::InvalidState)?;
        input.finish().map_err(|_| NavigationError::InvalidState)?;
        job.validate()?;
        Ok(job)
    }

    fn validate(&self) -> Result<(), NavigationError> {
        let limit = self.limits.nodes as usize;
        if self.limits.nodes == 0
            || self.limits.edges_per_node == 0
            || self.profile_version == Some(0)
            || self.scores.is_empty()
            || self.scores.len() > limit
            || self.estimates.len() != self.scores.len()
            || self.parents.len() > limit
            || self.closed.len() > limit
            || self
                .closed
                .iter()
                .any(|node| !self.scores.contains_key(node))
            || self.open.len() > limit
            || !self.scores.contains_key(&self.start)
            || self.parents.len() != self.scores.len().saturating_sub(1)
            || self.parents.contains_key(&self.start)
            || self.scores.iter().any(|(node, score)| {
                let Some(h) = self.estimates.get(node) else {
                    return true;
                };
                let is_open = score
                    .checked_add(*h)
                    .is_some_and(|f| self.open.contains(&(f, *h, *node)));
                !self.closed.contains(node) && !is_open && !(self.finished && *node == self.goal)
            })
            || self.open.iter().any(|(f, h, node)| {
                !self.scores.contains_key(node)
                    || self.estimates.get(node) != Some(h)
                    || self.scores.get(node).and_then(|g| g.checked_add(*h)) != Some(*f)
                    || self.closed.contains(node)
            })
            || self.parents.iter().any(|(node, parent)| {
                node == &parent.0
                    || !self.scores.contains_key(node)
                    || !self.scores.contains_key(&parent.0)
            })
        {
            return Err(NavigationError::InvalidState);
        }
        Ok(())
    }
}

impl StateCodec for NavigationJob {
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

impl NavigationJob {
    fn encode_state(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHRNAV02")?;
        self.profile.encode(out)?;
        match self.profile_version {
            Some(version) => {
                out.u8(1)?;
                out.u32(version)?;
            }
            None => out.u8(0)?,
        }
        out.u64(self.start)?;
        out.u64(self.goal)?;
        out.u64(self.input_revision)?;
        out.u32(self.limits.nodes)?;
        out.u32(self.limits.edges_per_node)?;
        out.u8(u8::from(self.finished))?;
        encode_scores(&self.scores, out)?;
        encode_scores(&self.estimates, out)?;
        encode_pairs(&self.parents, out)?;
        encode_set(&self.closed, out)?;
        out.u32(u32::try_from(self.open.len()).map_err(|_| invalid_encoding())?)?;
        for (f, h, node) in &self.open {
            out.u64(*f)?;
            out.u64(*h)?;
            out.u64(*node)?;
        }
        Ok(())
    }

    fn decode_state(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let format = input.raw(8)?;
        let versioned_profile = match format {
            b"CHRNAV01" => false,
            b"CHRNAV02" => true,
            _ => return Err(invalid_encoding()),
        };
        let profile = StableKey::decode(input)?;
        let profile_version = if versioned_profile {
            match input.u8()? {
                0 => None,
                1 => {
                    let version = input.u32()?;
                    if version == 0 {
                        return Err(invalid_encoding());
                    }
                    Some(version)
                }
                _ => return Err(invalid_encoding()),
            }
        } else {
            None
        };
        let start = input.u64()?;
        let goal = input.u64()?;
        let input_revision = input.u64()?;
        let limits =
            NavigationLimits::new(input.u32()?, input.u32()?).map_err(|_| invalid_encoding())?;
        let finished = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid_encoding()),
        };
        let scores = decode_scores(input, limits.nodes)?;
        let estimates = decode_scores(input, limits.nodes)?;
        let parents = decode_pairs(input, limits.nodes)?;
        let closed = decode_set(input, limits.nodes)?;
        let open_count = input.u32()?;
        if open_count > limits.nodes {
            return Err(invalid_encoding());
        }
        let mut open = BTreeSet::new();
        let mut previous_priority = None;
        for _ in 0..open_count {
            let priority = (input.u64()?, input.u64()?, input.u64()?);
            if previous_priority.is_some_and(|previous| previous >= priority) {
                return Err(invalid_encoding());
            }
            previous_priority = Some(priority);
            open.insert(priority);
        }
        Ok(Self {
            profile,
            profile_version,
            start,
            goal,
            input_revision,
            limits,
            open,
            scores,
            estimates,
            parents,
            closed,
            finished,
        })
    }
}

fn encode_scores(values: &BTreeMap<u64, u64>, out: &mut Encoder) -> EngineResult<()> {
    out.u32(u32::try_from(values.len()).map_err(|_| invalid_encoding())?)?;
    for (key, value) in values {
        out.u64(*key)?;
        out.u64(*value)?;
    }
    Ok(())
}
fn encode_pairs(values: &BTreeMap<u64, (u64, StableKey)>, out: &mut Encoder) -> EngineResult<()> {
    out.u32(u32::try_from(values.len()).map_err(|_| invalid_encoding())?)?;
    for (key, (parent, action)) in values {
        out.u64(*key)?;
        out.u64(*parent)?;
        action.encode(out)?;
    }
    Ok(())
}
fn encode_set(values: &BTreeSet<u64>, out: &mut Encoder) -> EngineResult<()> {
    out.u32(u32::try_from(values.len()).map_err(|_| invalid_encoding())?)?;
    for value in values {
        out.u64(*value)?;
    }
    Ok(())
}
fn decode_scores(input: &mut Decoder<'_>, limit: u32) -> EngineResult<BTreeMap<u64, u64>> {
    let count = input.u32()?;
    if count > limit {
        return Err(invalid_encoding());
    }
    let mut out = BTreeMap::new();
    let mut previous = None;
    for _ in 0..count {
        let key = input.u64()?;
        let value = input.u64()?;
        if previous.is_some_and(|old| old >= key) {
            return Err(invalid_encoding());
        }
        previous = Some(key);
        out.insert(key, value);
    }
    Ok(out)
}
fn decode_pairs(
    input: &mut Decoder<'_>,
    limit: u32,
) -> EngineResult<BTreeMap<u64, (u64, StableKey)>> {
    let count = input.u32()?;
    if count > limit {
        return Err(invalid_encoding());
    }
    let mut out = BTreeMap::new();
    let mut previous = None;
    for _ in 0..count {
        let node = input.u64()?;
        let parent = input.u64()?;
        let action = StableKey::decode(input)?;
        if previous.is_some_and(|old| old >= node) {
            return Err(invalid_encoding());
        }
        previous = Some(node);
        out.insert(node, (parent, action));
    }
    Ok(out)
}
fn decode_set(input: &mut Decoder<'_>, limit: u32) -> EngineResult<BTreeSet<u64>> {
    let count = input.u32()?;
    if count > limit {
        return Err(invalid_encoding());
    }
    let mut out = BTreeSet::new();
    let mut previous = None;
    for _ in 0..count {
        let value = input.u64()?;
        if previous.is_some_and(|old| old >= value) {
            return Err(invalid_encoding());
        }
        previous = Some(value);
        out.insert(value);
    }
    Ok(out)
}
fn decode_bool(input: &mut Decoder<'_>) -> EngineResult<bool> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_encoding()),
    }
}
fn allocation_error() -> EngineError {
    EngineError::new(ErrorCode::CapacityExceeded, "経路探索profile memory確保")
}
fn invalid_encoding() -> EngineError {
    EngineError::new(ErrorCode::InvalidEncoding, "経路探索jobの保存形式")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_profile(version: u32) -> NavigationAgentProfile {
        NavigationAgentProfile::new(
            StableKey::parse("test:walker-profile").unwrap(),
            version,
            CollisionQueryShapeRef {
                provider: StableKey::parse("test:shape-provider").unwrap(),
                version: 3,
                owner: 2,
                object: 19,
                part: 1,
            },
            NavigationAgentConfig {
                footprint_cells: [2, 3],
                travel_ticks_per_cell: 2,
                max_step_cells: 1,
                max_jump_height_cells: 4,
                max_jump_range_cells: 7,
                max_fall_cells: 12,
                can_swim: true,
                can_fly: false,
                collision_groups: 0b1010,
                hazard_costs: vec![NavigationHazardCost {
                    hazard: StableKey::parse("test:lava").unwrap(),
                    additional_ticks: 25,
                }],
            },
            4,
        )
        .unwrap()
    }

    #[test]
    fn agent_profile_codec_roundtrips_shape_movement_and_hazard_costs() {
        let profile = sample_profile(5);
        let mut encoded = Encoder::new(4096);
        profile.encode(&mut encoded).unwrap();
        let bytes = encoded.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let restored = NavigationAgentProfile::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(restored, profile);
    }

    #[test]
    fn agent_profile_rejects_unpaired_jump_limits_and_empty_collision_groups() {
        let mut invalid = sample_profile(1);
        invalid.config.max_jump_range_cells = 0;
        assert_eq!(invalid.validate(4), Err(NavigationError::InvalidInput));
        let mut invalid = sample_profile(1);
        invalid.config.collision_groups = 0;
        assert_eq!(invalid.validate(4), Err(NavigationError::InvalidInput));
    }

    fn edge(target: u64, cost: u32) -> NavigationEdge {
        NavigationEdge {
            target,
            cost,
            action: StableKey::parse("test:walk").unwrap(),
        }
    }

    fn limits(nodes: u32) -> NavigationLimits {
        NavigationLimits::new(nodes, nodes).unwrap()
    }

    fn path(nodes: &[u64]) -> Vec<NavigationStep> {
        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| NavigationStep {
                node: *node,
                action: (index > 0).then(|| StableKey::parse("test:walk").unwrap()),
            })
            .collect()
    }

    struct Graph {
        revision: u64,
        wait_on: Option<u64>,
    }
    impl NavigationQuery for Graph {
        fn revision(&self) -> u64 {
            self.revision
        }
        fn can_reuse_expansion(
            &self,
            _: StableKey,
            _: u64,
            _: u64,
        ) -> Result<bool, NavigationError> {
            Ok(true)
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            if self.wait_on == Some(node) {
                return Ok(NavigationExpansion::WaitingForRegion(9));
            }
            let edges = match node {
                1 => vec![edge(3, 4), edge(2, 1)],
                2 => vec![edge(4, 1)],
                3 => vec![edge(4, 1)],
                4 => vec![edge(5, 1)],
                _ => vec![],
            };
            Ok(NavigationExpansion::Ready(edges))
        }
        fn heuristic(&self, _: StableKey, node: u64, goal: u64) -> Result<u64, NavigationError> {
            Ok(goal.saturating_sub(node))
        }
    }

    struct ProfileGraph {
        profile: NavigationAgentProfile,
    }
    impl NavigationQuery for ProfileGraph {
        fn revision(&self) -> u64 {
            1
        }
        fn agent_profile(
            &self,
            key: StableKey,
        ) -> Result<Option<&NavigationAgentProfile>, NavigationError> {
            Ok((key == self.profile.key()).then_some(&self.profile))
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            Ok(NavigationExpansion::Ready(if node == 1 {
                vec![edge(2, 1)]
            } else {
                Vec::new()
            }))
        }
        fn heuristic(&self, _: StableKey, _: u64, _: u64) -> Result<u64, NavigationError> {
            Ok(0)
        }
    }

    #[test]
    fn profile_version_change_invalidates_a_saved_search_job() {
        let initial = ProfileGraph {
            profile: sample_profile(1),
        };
        let mut job = NavigationJob::new(&initial, initial.profile.key(), 1, 2, limits(8)).unwrap();
        assert_eq!(job.poll(&initial, 1).unwrap(), NavigationStatus::Continue);
        let changed = ProfileGraph {
            profile: sample_profile(2),
        };
        assert_eq!(job.revalidate(&changed), Err(NavigationError::StaleInput));
    }

    #[test]
    fn search_is_budgeted_deterministic_and_resumes_after_save() {
        let graph = Graph {
            revision: 7,
            wait_on: None,
        };
        let mut job = NavigationJob::new(
            &graph,
            StableKey::parse("test:walker").unwrap(),
            1,
            5,
            limits(32),
        )
        .unwrap();
        assert_eq!(job.poll(&graph, 1).unwrap(), NavigationStatus::Continue);
        let saved = job.encode(16_384).unwrap();
        let mut resumed = NavigationJob::decode(&saved, 16_384).unwrap();
        assert_eq!(
            resumed.poll(&graph, 32).unwrap(),
            NavigationStatus::Complete(path(&[1, 2, 4, 5]))
        );
    }

    #[test]
    fn unloaded_region_preserves_frontier_and_revision_change_is_rejected() {
        let waiting = Graph {
            revision: 3,
            wait_on: Some(1),
        };
        let mut job = NavigationJob::new(
            &waiting,
            StableKey::parse("test:walker").unwrap(),
            1,
            5,
            limits(32),
        )
        .unwrap();
        assert_eq!(
            job.poll(&waiting, 4).unwrap(),
            NavigationStatus::WaitingForRegion(9)
        );
        let loaded = Graph {
            revision: 4,
            wait_on: None,
        };
        assert_eq!(job.poll(&loaded, 16), Err(NavigationError::StaleInput));
        job.revalidate(&loaded).unwrap();
        assert_eq!(
            job.poll(&loaded, 16).unwrap(),
            NavigationStatus::Complete(path(&[1, 2, 4, 5]))
        );
        assert_eq!(
            job.poll(
                &Graph {
                    revision: 4,
                    wait_on: None
                },
                1
            ),
            Err(NavigationError::InvalidState)
        );
    }

    struct RevisionHeuristicGraph(u64);
    impl NavigationQuery for RevisionHeuristicGraph {
        fn revision(&self) -> u64 {
            self.0
        }
        fn can_reuse_expansion(
            &self,
            _: StableKey,
            _: u64,
            _: u64,
        ) -> Result<bool, NavigationError> {
            Ok(true)
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            let edges = match node {
                1 => vec![edge(2, 1), edge(3, 2)],
                2 => vec![edge(4, 8)],
                3 => vec![edge(4, if self.0 == 1 { 10 } else { 1 })],
                _ => Vec::new(),
            };
            Ok(NavigationExpansion::Ready(edges))
        }
        fn heuristic(&self, _: StableKey, node: u64, _: u64) -> Result<u64, NavigationError> {
            Ok(match (self.0, node) {
                (1, 3) => 8,
                _ => 0,
            })
        }
    }

    #[test]
    fn revalidation_rebuilds_frontier_priority_from_current_heuristics() {
        let old = RevisionHeuristicGraph(1);
        let profile = StableKey::parse("test:walker").unwrap();
        let mut job = NavigationJob::new(&old, profile, 1, 4, limits(16)).unwrap();
        assert_eq!(job.poll(&old, 1).unwrap(), NavigationStatus::Continue);

        let changed = RevisionHeuristicGraph(2);
        job.revalidate(&changed).unwrap();
        assert_eq!(
            job.poll(&changed, 16).unwrap(),
            NavigationStatus::Complete(path(&[1, 3, 4]))
        );
    }

    struct InconsistentGraph;
    impl NavigationQuery for InconsistentGraph {
        fn revision(&self) -> u64 {
            1
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            let edges = match node {
                1 => vec![edge(2, 4), edge(3, 1)],
                2 => vec![edge(4, 10)],
                3 => vec![edge(2, 1)],
                _ => vec![],
            };
            Ok(NavigationExpansion::Ready(edges))
        }
        fn heuristic(&self, _: StableKey, node: u64, _: u64) -> Result<u64, NavigationError> {
            Ok(if node == 3 { 5 } else { 0 })
        }
    }

    #[test]
    fn admissible_but_inconsistent_heuristic_reopens_a_closed_node() {
        let mut job = NavigationJob::new(
            &InconsistentGraph,
            StableKey::parse("test:walker").unwrap(),
            1,
            4,
            limits(16),
        )
        .unwrap();
        assert_eq!(
            job.poll(&InconsistentGraph, 16).unwrap(),
            NavigationStatus::Complete(path(&[1, 3, 2, 4]))
        );
    }

    struct InvalidThenReady(std::cell::Cell<bool>);
    impl NavigationQuery for InvalidThenReady {
        fn revision(&self) -> u64 {
            1
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            if node == 1 && !self.0.replace(true) {
                return Ok(NavigationExpansion::Ready(vec![edge(2, 0)]));
            }
            Ok(NavigationExpansion::Ready(vec![edge(2, 1)]))
        }
        fn heuristic(&self, _: StableKey, _: u64, _: u64) -> Result<u64, NavigationError> {
            Ok(0)
        }
    }

    #[test]
    fn provider_error_does_not_consume_the_frontier_node() {
        let query = InvalidThenReady(std::cell::Cell::new(false));
        let mut job = NavigationJob::new(
            &query,
            StableKey::parse("test:walker").unwrap(),
            1,
            2,
            limits(8),
        )
        .unwrap();
        assert_eq!(job.poll(&query, 1), Err(NavigationError::InvalidEdge));
        assert_eq!(
            job.poll(&query, 2).unwrap(),
            NavigationStatus::Complete(path(&[1, 2]))
        );
    }

    struct BranchBeyondNodeLimit;
    impl NavigationQuery for BranchBeyondNodeLimit {
        fn revision(&self) -> u64 {
            1
        }
        fn neighbors(
            &self,
            _: StableKey,
            node: u64,
        ) -> Result<NavigationExpansion, NavigationError> {
            Ok(NavigationExpansion::Ready(if node == 1 {
                vec![edge(2, 1), edge(3, 1)]
            } else {
                vec![]
            }))
        }
        fn heuristic(&self, _: StableKey, _: u64, _: u64) -> Result<u64, NavigationError> {
            Ok(0)
        }
    }

    #[test]
    fn expansion_cannot_exceed_node_limit_and_keeps_job_unchanged() {
        let query = BranchBeyondNodeLimit;
        let mut job = NavigationJob::new(
            &query,
            StableKey::parse("test:walker").unwrap(),
            1,
            3,
            NavigationLimits::new(2, 4).unwrap(),
        )
        .unwrap();
        let before = job.encode(16_384).unwrap();

        assert_eq!(job.poll(&query, 1), Err(NavigationError::NodeLimit));
        assert_eq!(job.encode(16_384).unwrap(), before);
    }
}
