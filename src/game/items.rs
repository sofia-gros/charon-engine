//! 作者が定義するitemと、容量・保存状態を検証するcontainer取引です。
//!
//! itemの意味は安定keyと登録済み定義が所有します。containerはslotを正規状態として保存し、
//! 移送時は送受信双方の候補を検査してから同時に公開します。装備や使用効果は個別のgame
//! serviceとしてitem keyへ接続し、この層は暗黙の副作用を発生させません。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey, grid::CellPoint};
use crate::world::{World, WorldEdit};

const CONTAINER_CODEC_VERSION: u16 = 1;

/// 作者登録する一種類のitem定義です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemDefinition {
    key: StableKey,
    max_stack: u32,
    max_state_bytes: u32,
    unit_weight_micros: u64,
    appearance: Option<StableKey>,
    use_behavior: Option<StableKey>,
    equipment_slot: Option<StableKey>,
    tags: Vec<StableKey>,
    author_attributes: Vec<ItemAttribute>,
}

/// ItemDefへ登録するauthor-owned静的属性payloadです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemAttribute {
    /// 属性schema keyです。
    pub key: StableKey,
    /// schema providerが検証する値です。
    pub value: Vec<u8>,
}

impl ItemDefinition {
    /// 表示・使用・装備slot・tagを明示した定義を作ります。
    #[allow(clippy::too_many_arguments)] // 定義項目を型付き引数で明示し、呼出側の設定ミスを防ぎます。
    pub fn new(
        key: StableKey,
        max_stack: u32,
        max_state_bytes: u32,
        unit_weight_micros: u64,
        appearance: Option<StableKey>,
        use_behavior: Option<StableKey>,
        equipment_slot: Option<StableKey>,
        mut tags: Vec<StableKey>,
    ) -> Result<Self, ItemError> {
        if max_stack == 0 || (max_stack > 1 && max_state_bytes != 0) {
            return Err(ItemError::InvalidDefinition);
        }
        tags.sort_unstable();
        if tags.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ItemError::InvalidDefinition);
        }
        Ok(Self {
            key,
            max_stack,
            max_state_bytes,
            unit_weight_micros,
            appearance,
            use_behavior,
            equipment_slot,
            tags,
            author_attributes: Vec::new(),
        })
    }

    /// author固有schemaの静的属性を安定key順で設定します。
    pub fn with_author_attributes(
        mut self,
        mut attributes: Vec<ItemAttribute>,
    ) -> Result<Self, ItemError> {
        attributes.sort_unstable_by_key(|attribute| attribute.key);
        if attributes.windows(2).any(|pair| pair[0].key == pair[1].key)
            || attributes
                .iter()
                .try_fold(0_usize, |sum, attribute| {
                    sum.checked_add(attribute.value.len())
                })
                .is_none()
        {
            return Err(ItemError::InvalidDefinition);
        }
        self.author_attributes = attributes;
        Ok(self)
    }

    /// 定義の安定keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 一つのslotへ入る最大個数です。
    pub fn max_stack(&self) -> u32 {
        self.max_stack
    }
    /// 個体状態byteの上限です。状態を持つitemはmax_stackが1です。
    pub fn max_state_bytes(&self) -> u32 {
        self.max_state_bytes
    }
    /// 作者が選ぶ固定小数点重量です。
    pub fn unit_weight_micros(&self) -> u64 {
        self.unit_weight_micros
    }
    /// 外観参照keyです。
    pub fn appearance(&self) -> Option<StableKey> {
        self.appearance
    }
    /// 使用挙動provider keyです。
    pub fn use_behavior(&self) -> Option<StableKey> {
        self.use_behavior
    }
    /// 装備slot keyです。
    pub fn equipment_slot(&self) -> Option<StableKey> {
        self.equipment_slot
    }
    /// 安定key順の分類tagです。
    pub fn tags(&self) -> &[StableKey] {
        &self.tags
    }
    /// 安定key順の作者定義属性です。
    pub fn author_attributes(&self) -> &[ItemAttribute] {
        &self.author_attributes
    }
}

/// item定義の重複と不正なstackable個体設定を登録境界で検出します。
#[derive(Clone, Debug, Default)]
pub struct ItemCatalog {
    definitions: Vec<ItemDefinition>,
}

impl ItemCatalog {
    /// 定義をkey順へ整列し、重複を拒否します。
    pub fn new(mut definitions: Vec<ItemDefinition>) -> Result<Self, ItemError> {
        definitions.sort_unstable_by_key(ItemDefinition::key);
        if definitions
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(ItemError::DuplicateDefinition);
        }
        Ok(Self { definitions })
    }

    /// 登録済みitem定義を返します。
    pub fn get(&self, key: StableKey) -> Result<&ItemDefinition, ItemError> {
        self.definitions
            .binary_search_by_key(&key, ItemDefinition::key)
            .map(|index| &self.definitions[index])
            .map_err(|_| ItemError::UnknownItem(Box::new(key)))
    }

    /// key順の定義を列挙します。
    pub fn iter(&self) -> impl Iterator<Item = &ItemDefinition> {
        self.definitions.iter()
    }

    /// checkpoint復元後にcontainerの定義参照・容量・instance一意性を検証します。
    pub fn validate_container(&self, container: &ItemContainer) -> Result<(), ItemError> {
        container.validate(self)
    }
}

/// 個体差を持つitem一つ分のauthor stateです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInstance {
    /// セーブscope内で一意なinstance keyです。
    pub id: StableKey,
    /// catalog内のitem定義です。
    pub item: StableKey,
    /// 定義固有codecへ渡す保存状態です。
    pub state: Vec<u8>,
}

/// container slotの保存単位です。
///
/// 個体stateは間接格納します。通常のstack slotへinstance全体の固定領域を負担させず、
/// 作者が定めたslot数上限に比例するcontainer memoryを抑えます。unique itemを作る時は
/// `Instance(Box::new(value))`を使い、複製・保存では個体内容を含めて扱います。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemLot {
    /// 個体状態を持たない同種itemのstackです。
    Stack { item: StableKey, count: u32 },
    /// stateを持つ非stackable個体です。
    Instance(Box<ItemInstance>),
}

impl ItemLot {
    /// このlotのitem定義keyです。
    pub fn item_key(&self) -> StableKey {
        match self {
            Self::Stack { item, .. } => *item,
            Self::Instance(instance) => instance.item,
        }
    }
}

/// item key/tag条件を保管検索とcraft候補検索で共有する読み取りqueryです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemQuery {
    item: Option<StableKey>,
    tag: Option<StableKey>,
}

/// queryが見つけたcontainer slotの軽量位置情報です。item payloadは複製しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemMatch {
    /// 所持containerの安定keyです。
    pub container: StableKey,
    /// container内のslot indexです。
    pub slot: u32,
    /// item定義keyです。
    pub item: StableKey,
    /// stack個数または個体一つを表す1です。
    pub count: u32,
    /// 個体state付きlotだけにあるinstance keyです。
    pub instance: Option<StableKey>,
}

/// 上限付きquery結果と、後続一致が残っているかを返します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemQueryResult {
    /// container key、slot index順の一致位置です。
    pub matches: Vec<ItemMatch>,
    /// result limitの先に一致があった場合はtrueです。
    pub truncated: bool,
}

/// ItemInstanceを所有するslotの種類です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemInstanceOwnerSlot {
    Container(u32),
    Equipment(StableKey),
}

/// World scope内で一意なItemInstanceの現在位置です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemInstanceOwner {
    pub instance: StableKey,
    pub owner: StableKey,
    pub slot: ItemInstanceOwnerSlot,
}

/// World内registryの復元・更新で呼出側が適用する上限です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemInstanceRegistryLimits {
    pub owners: u32,
    pub instances: u32,
}

/// 複数player/container/equipmentをまたぐItemInstance位置索引です。
///
/// 個体の正規stateは各ItemContainer・EquipmentLoadoutが持ち、このregistryはownerとslotの
/// 一意索引を持ちます。container transactionや復元境界で全ownerを照合し、同じ個体IDが
/// player inventory、装備、world drop間で二重所有されていないことを確認できます。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInstanceRegistry {
    scope: StableKey,
    revision: u64,
    owners: Vec<ItemInstanceOwner>,
}

impl ItemInstanceRegistry {
    /// 保存scopeのregistryを空で作ります。
    pub fn new(scope: StableKey) -> Self {
        Self {
            scope,
            revision: 0,
            owners: Vec::new(),
        }
    }

    /// 所属World/save scopeです。
    pub fn scope(&self) -> StableKey {
        self.scope
    }

    /// registryを更新するたびに進むrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// ItemInstance key順の所有位置です。
    pub fn owners(&self) -> &[ItemInstanceOwner] {
        &self.owners
    }

    /// 現在の全owner stateからregistry候補を再構築し、一回で公開します。
    pub fn refresh(
        &mut self,
        expected_revision: u64,
        containers: &[&ItemContainer],
        equipment: &[&EquipmentLoadout],
        catalog: &ItemCatalog,
        limits: ItemInstanceRegistryLimits,
    ) -> Result<u64, ItemError> {
        if self.revision != expected_revision {
            return Err(ItemError::StaleRevision);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let owners = build_item_instance_owners(containers, equipment, catalog, limits)?;
        self.owners = owners;
        self.revision = revision;
        Ok(revision)
    }

    /// 保存索引と現在のplayer/world owner stateが完全一致するか検証します。
    pub fn validate_ownership(
        &self,
        expected_scope: StableKey,
        containers: &[&ItemContainer],
        equipment: &[&EquipmentLoadout],
        catalog: &ItemCatalog,
        limits: ItemInstanceRegistryLimits,
    ) -> Result<(), ItemError> {
        if self.scope != expected_scope {
            return Err(ItemError::InvalidContainer);
        }
        let owners = build_item_instance_owners(containers, equipment, catalog, limits)?;
        if owners == self.owners {
            Ok(())
        } else {
            Err(ItemError::InvalidContainer)
        }
    }

    /// 復元した索引が登録済み容量内にあることを確認します。
    pub fn validate_limits(&self, limits: ItemInstanceRegistryLimits) -> Result<(), ItemError> {
        if self.owners.len() > limits.instances as usize
            || (limits.owners == 0 && !self.owners.is_empty())
            || self
                .owners
                .windows(2)
                .any(|pair| pair[0].instance >= pair[1].instance)
        {
            Err(ItemError::CapacityExceeded)
        } else {
            Ok(())
        }
    }
}

impl ItemQuery {
    /// item key、tagまたは両条件の積を使うqueryです。
    pub fn new(item: Option<StableKey>, tag: Option<StableKey>) -> Result<Self, ItemError> {
        if item.is_none() && tag.is_none() {
            return Err(ItemError::InvalidQuery);
        }
        Ok(Self { item, tag })
    }

    /// 任意の定義を列挙する明示的なqueryです。
    pub fn all() -> Self {
        Self {
            item: None,
            tag: None,
        }
    }

    /// stable container keyとslot順で複数containerを検索します。
    ///
    /// containerは最初にcatalogと容量で検証します。結果上限へ達すると最初の超過一致で
    /// scanを終え、残りがあることだけを`truncated`で返します。payload bytesやitem状態は
    /// 複製せず、作者の保管UI・craft選択が同じ位置形式を使えます。
    pub fn execute(
        self,
        containers: &[&ItemContainer],
        catalog: &ItemCatalog,
        result_limit: usize,
    ) -> Result<ItemQueryResult, ItemError> {
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(containers.len())
            .map_err(|_| ItemError::AllocationFailed)?;
        ordered.extend_from_slice(containers);
        ordered.sort_unstable_by_key(|container| container.key);
        if ordered.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(ItemError::InvalidContainer);
        }
        for container in &ordered {
            container.validate(catalog)?;
        }

        let mut matches = Vec::new();
        let mut truncated = false;
        'containers: for container in ordered {
            for (slot_index, lot) in container.slots.iter().enumerate() {
                let Some(lot) = lot else {
                    continue;
                };
                let item = lot.item_key();
                if self.item.is_some_and(|required| required != item) {
                    continue;
                }
                let definition = catalog.get(item)?;
                if self
                    .tag
                    .is_some_and(|required| definition.tags.binary_search(&required).is_err())
                {
                    continue;
                }
                if matches.len() == result_limit {
                    truncated = true;
                    break 'containers;
                }
                matches
                    .try_reserve(1)
                    .map_err(|_| ItemError::AllocationFailed)?;
                let (count, instance) = match lot {
                    ItemLot::Stack { count, .. } => (*count, None),
                    ItemLot::Instance(value) => (1, Some(value.id)),
                };
                matches.push(ItemMatch {
                    container: container.key,
                    slot: u32::try_from(slot_index).map_err(|_| ItemError::InvalidContainer)?,
                    item,
                    count,
                    instance,
                });
            }
        }
        Ok(ItemQueryResult { matches, truncated })
    }
}

/// slot・保存byte・合計重量を制限するcontainer設定です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemContainerLimits {
    /// containerが保持するslot数です。
    pub slots: u32,
    /// 個体state payloadの合計byte上限です。
    pub payload_bytes: u64,
    /// 重量の合計上限です。単位は作者定義のmicro unitです。
    pub weight_micros: u64,
}

/// 装備slot groupごとの個体state byteと重量上限です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EquipmentLimits {
    /// 全装備個体が持つstateの合計byte上限です。
    pub payload_bytes: u64,
    /// 装備品自身の合計重量上限です。
    pub weight_micros: u64,
}

/// item取引で発生する検証・容量・競合失敗です。
///
/// 診断キーは`StableKey`本体が大きいため、失敗経路だけでBoxへ確保します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemError {
    InvalidDefinition,
    DuplicateDefinition,
    UnknownItem(Box<StableKey>),
    InvalidContainer,
    InvalidQuery,
    InvalidLot,
    MissingItem,
    CapacityExceeded,
    StaleRevision,
    RevisionOverflow,
    DuplicateInstance(Box<StableKey>),
    AllocationFailed,
    InvalidRecipe,
    UnknownRecipe(Box<StableKey>),
    CraftBusy,
    MissingTool(Box<StableKey>),
    InvalidCraftContext,
    InvalidCraftCapacity,
    StaleCraftTick,
    CraftNotReady,
    UnknownReceipt,
    DuplicateReceipt,
    ReceiptCapacity,
    /// mining ruleのitem種別、報酬個数またはcatalog登録上限が不正です。
    InvalidMiningRule,
    /// 対象World素材に採掘報酬ruleが登録されていません。
    MissingMiningRule(Box<StableKey>),
}

/// item更新とWorld候補の一方が失敗した取引結果です。
#[derive(Debug)]
pub enum ItemWorldTransactionError {
    /// item候補の検証・容量・revision失敗です。
    Item(ItemError),
    /// 原子的World編集のcallbackまたはvalidation失敗です。
    World(EngineError),
}

impl std::fmt::Display for ItemWorldTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for ItemWorldTransactionError {}

/// World素材の破壊量から、containerへ予約する作者定義の採掘報酬です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningDropRule {
    /// 対象Content素材のstable keyです。
    pub material: StableKey,
    /// inventoryまたはworld dropへ追加するstack itemです。
    pub item: StableKey,
    /// cell単位の破壊一回あたりに追加する個数です。
    pub per_cell: u32,
    /// block単位の破壊一回あたりに追加する個数です。0なら残存cell数×`per_cell`を使います。
    pub per_block: u32,
}

/// 素材stable keyから採掘報酬を解決する起動時catalogです。
#[derive(Clone, Debug, Default)]
pub struct MiningDropCatalog {
    rules: Vec<MiningDropRule>,
}

impl MiningDropCatalog {
    /// 素材重複、存在しないitem、stateを必要とする個体item、件数超過を拒否します。
    pub fn new(
        mut rules: Vec<MiningDropRule>,
        items: &ItemCatalog,
        max_rules: u32,
    ) -> Result<Self, ItemError> {
        if rules.len() > max_rules as usize {
            return Err(ItemError::CapacityExceeded);
        }
        for rule in &rules {
            let definition = items.get(rule.item)?;
            if (rule.per_cell == 0 && rule.per_block == 0) || definition.max_state_bytes() != 0 {
                return Err(ItemError::InvalidMiningRule);
            }
        }
        rules.sort_unstable_by_key(|rule| rule.material);
        if rules
            .windows(2)
            .any(|pair| pair[0].material == pair[1].material)
        {
            return Err(ItemError::InvalidMiningRule);
        }
        Ok(Self { rules })
    }

    /// 素材の報酬規則を検索します。
    pub fn get(&self, material: StableKey) -> Result<MiningDropRule, ItemError> {
        self.rules
            .binary_search_by_key(&material, |rule| rule.material)
            .map(|index| self.rules[index])
            .map_err(|_| ItemError::MissingMiningRule(Box::new(material)))
    }
}

/// 採掘成功後に確定した報酬先と破壊量です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningResult {
    /// 除去したWorld素材です。
    pub material: StableKey,
    /// 報酬itemです。
    pub item: StableKey,
    /// inventoryまたはdrop containerへ追加した個数です。
    pub count: u32,
    /// 報酬を受け取ったcontainer keyです。0個報酬ならNoneです。
    pub destination: Option<StableKey>,
    /// BreakMode::Block時に除去した残存cell数です。
    pub broken_cells: u32,
}

/// World破壊とitem報酬の取引失敗です。
#[derive(Debug)]
pub enum MiningTransactionError {
    /// 報酬、registry、container revisionの失敗です。
    Item(ItemError),
    /// cellの読み書き、World revision、World編集候補の失敗です。
    World(EngineError),
}

impl std::fmt::Display for MiningTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for MiningTransactionError {}

/// 採掘取引へ渡す、World・container・全owner照合のrevisionとscopeです。
pub struct MiningTransaction<'a> {
    /// 採掘対象を持つlayerです。
    pub layer: crate::world::LayerId,
    /// 採掘対象cellです。
    pub point: crate::foundation::grid::CellPoint,
    /// 取引開始時に読み取ったWorld revisionです。
    pub expected_world_revision: u64,
    /// player inventoryの期待revisionです。
    pub expected_inventory_revision: u64,
    /// World drop containerの期待revisionです。
    pub expected_drop_revision: u64,
    /// registryとcontainer群が属するstable World scopeです。
    pub registry_scope: StableKey,
    /// 取引開始時のinstance registry revisionです。
    pub expected_registry_revision: u64,
    /// inventory/drop以外に同じWorldでitemを所有するcontainer群です。
    pub other_containers: &'a [&'a ItemContainer],
    /// 同じWorldでitem instanceを所有する装備群です。
    pub equipment: &'a [&'a EquipmentLoadout],
    /// instance owner索引の作者上限です。
    pub registry_limits: ItemInstanceRegistryLimits,
}

/// material breakと報酬を、inventory優先・World drop fallbackで一括確定します。
///
/// 報酬stack候補、既存item全ownerのregistry、Worldのbreak候補を先に検証します。
/// inventory容量が不足するときだけdrop containerへ切り替えます。両方の容量が不足する場合や、
/// World編集が失敗する場合はWorld・両container・registryの正規状態を変更しません。tool条件、
/// damageの蓄積、採掘可能性は呼出側のゲーム規則として事前に判定してください。
pub fn mine_cell_with_drop(
    world: &mut World,
    inventory: &mut ItemContainer,
    drop_container: &mut ItemContainer,
    registry: &mut ItemInstanceRegistry,
    items: &ItemCatalog,
    drops: &MiningDropCatalog,
    transaction: MiningTransaction<'_>,
) -> Result<MiningResult, MiningTransactionError> {
    use crate::world::{BreakMode, CellQuery};

    if std::ptr::eq(inventory, drop_container) || inventory.key == drop_container.key {
        return Err(MiningTransactionError::Item(ItemError::InvalidContainer));
    }
    if inventory.revision != transaction.expected_inventory_revision
        || drop_container.revision != transaction.expected_drop_revision
        || registry.revision != transaction.expected_registry_revision
    {
        return Err(MiningTransactionError::Item(ItemError::StaleRevision));
    }
    if world.revision() != transaction.expected_world_revision {
        return Err(MiningTransactionError::World(EngineError::new(
            ErrorCode::Conflict,
            "採掘World revisionが古くなっています",
        )));
    }
    let (material_id, block_ref) = match world.cell(transaction.layer, transaction.point) {
        Ok(CellQuery::Loaded(cell)) if cell.core.material != crate::world::MaterialId::AIR => {
            (cell.core.material, cell.block)
        }
        Ok(CellQuery::Loaded(_)) => {
            return Err(MiningTransactionError::World(EngineError::new(
                ErrorCode::InvalidValue,
                "空cellは採掘できません",
            )));
        }
        Ok(CellQuery::Unavailable(_)) | Ok(CellQuery::WorldBoundary) => {
            return Err(MiningTransactionError::World(EngineError::new(
                ErrorCode::Conflict,
                "採掘対象cellが未読込またはWorld外です",
            )));
        }
        Err(error) => return Err(MiningTransactionError::World(error)),
    };
    let material = world
        .content()
        .material(material_id)
        .map_err(MiningTransactionError::World)?
        .ok_or_else(|| {
            MiningTransactionError::World(EngineError::new(
                ErrorCode::InvalidState,
                "採掘素材定義がありません",
            ))
        })?;
    let material_key = material.key();
    let break_mode = material.break_mode();
    let rule = drops
        .get(material_key)
        .map_err(MiningTransactionError::Item)?;
    let broken_cells = match (break_mode, block_ref) {
        (BreakMode::Block, Some(reference)) => world
            .block(reference)
            .map_err(MiningTransactionError::World)?
            .remaining_cells(),
        _ => 1,
    };
    let broken_cells = u32::try_from(broken_cells)
        .map_err(|_| MiningTransactionError::Item(ItemError::CapacityExceeded))?;
    let count = match break_mode {
        BreakMode::Cell => rule.per_cell,
        BreakMode::Block if rule.per_block != 0 => rule.per_block,
        BreakMode::Block => rule
            .per_cell
            .checked_mul(broken_cells)
            .ok_or(MiningTransactionError::Item(ItemError::CapacityExceeded))?,
    };

    let mut current_containers = Vec::new();
    current_containers
        .try_reserve_exact(transaction.other_containers.len().saturating_add(2))
        .map_err(|_| MiningTransactionError::Item(ItemError::AllocationFailed))?;
    current_containers.extend([&*inventory, &*drop_container]);
    current_containers.extend_from_slice(transaction.other_containers);
    registry
        .validate_ownership(
            transaction.registry_scope,
            &current_containers,
            transaction.equipment,
            items,
            transaction.registry_limits,
        )
        .map_err(MiningTransactionError::Item)?;

    let mut inventory_candidate = None;
    let mut drop_candidate = None;
    let destination = if count == 0 {
        None
    } else {
        match prepare_mining_deposit(
            inventory,
            transaction.expected_inventory_revision,
            rule.item,
            count,
            items,
        ) {
            Ok(candidate) => {
                inventory_candidate = Some(candidate);
                Some(inventory.key)
            }
            Err(ItemError::CapacityExceeded) => {
                let candidate = prepare_mining_deposit(
                    drop_container,
                    transaction.expected_drop_revision,
                    rule.item,
                    count,
                    items,
                )
                .map_err(MiningTransactionError::Item)?;
                drop_candidate = Some(candidate);
                Some(drop_container.key)
            }
            Err(error) => return Err(MiningTransactionError::Item(error)),
        }
    };

    let mut candidate_containers = Vec::new();
    candidate_containers
        .try_reserve_exact(transaction.other_containers.len().saturating_add(2))
        .map_err(|_| MiningTransactionError::Item(ItemError::AllocationFailed))?;
    candidate_containers.push(inventory_candidate.as_ref().unwrap_or(inventory));
    candidate_containers.push(drop_candidate.as_ref().unwrap_or(drop_container));
    candidate_containers.extend_from_slice(transaction.other_containers);
    let candidate_owners = build_item_instance_owners(
        &candidate_containers,
        transaction.equipment,
        items,
        transaction.registry_limits,
    )
    .map_err(MiningTransactionError::Item)?;
    let registry_candidate = if candidate_owners == registry.owners {
        None
    } else {
        Some(ItemInstanceRegistry {
            scope: registry.scope,
            revision: registry
                .revision
                .checked_add(1)
                .ok_or(MiningTransactionError::Item(ItemError::RevisionOverflow))?,
            owners: candidate_owners,
        })
    };

    world
        .edit(|editor| {
            match editor.cell(transaction.layer, transaction.point)? {
                CellQuery::Loaded(cell) if cell.core.material == material_id => {}
                _ => {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "採掘対象cellが取引前から変化しました",
                    ));
                }
            }
            editor.break_at(transaction.layer, transaction.point)
        })
        .map_err(MiningTransactionError::World)?;

    if let Some(candidate) = inventory_candidate {
        *inventory = candidate;
    }
    if let Some(candidate) = drop_candidate {
        *drop_container = candidate;
    }
    if let Some(candidate) = registry_candidate {
        *registry = candidate;
    }
    Ok(MiningResult {
        material: material_key,
        item: rule.item,
        count,
        destination,
        broken_cells,
    })
}

fn prepare_mining_deposit(
    container: &ItemContainer,
    expected_revision: u64,
    item: StableKey,
    count: u32,
    catalog: &ItemCatalog,
) -> Result<ItemContainer, ItemError> {
    if container.revision != expected_revision {
        return Err(ItemError::StaleRevision);
    }
    let revision = container
        .revision
        .checked_add(1)
        .ok_or(ItemError::RevisionOverflow)?;
    container.validate(catalog)?;
    let mut candidate = try_clone_container(container)?;
    insert_lot(&mut candidate, ItemLot::Stack { item, count }, catalog)?;
    candidate.validate(catalog)?;
    candidate.revision = revision;
    Ok(candidate)
}

impl std::fmt::Display for ItemError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for ItemError {}

/// key付きの保存可能containerです。slot更新は取引commitからのみ公開します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemContainer {
    key: StableKey,
    revision: u64,
    limits: ItemContainerLimits,
    slots: Vec<Option<ItemLot>>,
}

/// Worldに存在するitem drop数と保存可能な領域を制限します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldDropLimits {
    /// 一Worldに置けるdrop containerの最大数です。
    pub containers: u32,
}

/// 座標とcontainerを結び付けたWorld item dropです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldDrop {
    position: CellPoint,
    container: ItemContainer,
}

/// World scope内の位置付きdrop container一覧です。
///
/// item payloadの正規状態は内包する`ItemContainer`にあります。storeは安定key順にdropを
/// 保持し、位置・container・World scopeを同じStateCodec recordへ保存します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldDropStore {
    scope: StableKey,
    revision: u64,
    limits: WorldDropLimits,
    drops: Vec<WorldDrop>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EquipmentSlot {
    key: StableKey,
    lot: Option<ItemLot>,
}

/// 作者が選んだ装備slotと個体を所有する保存可能stateです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquipmentLoadout {
    key: StableKey,
    revision: u64,
    limits: EquipmentLimits,
    slots: Vec<EquipmentSlot>,
}

impl EquipmentLoadout {
    /// 作者指定のslot keyを整列し、重複を拒否した装備stateを作ります。
    pub fn new(
        key: StableKey,
        mut slot_keys: Vec<StableKey>,
        limits: EquipmentLimits,
    ) -> Result<Self, ItemError> {
        if slot_keys.is_empty() || slot_keys.len() > u32::MAX as usize {
            return Err(ItemError::InvalidContainer);
        }
        slot_keys.sort_unstable();
        if slot_keys.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ItemError::InvalidContainer);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_keys.len())
            .map_err(|_| ItemError::AllocationFailed)?;
        slots.extend(
            slot_keys
                .into_iter()
                .map(|key| EquipmentSlot { key, lot: None }),
        );
        Ok(Self {
            key,
            revision: 0,
            limits,
            slots,
        })
    }

    /// 装備set安定keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 成功した装備取引ごとに進むrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// callerが設定したpayload/重量上限です。
    pub fn limits(&self) -> EquipmentLimits {
        self.limits
    }
    /// stable key順にslot keyと装備lotを読みます。
    pub fn slots(&self) -> impl Iterator<Item = (StableKey, Option<&ItemLot>)> + '_ {
        self.slots.iter().map(|slot| (slot.key, slot.lot.as_ref()))
    }

    /// item catalogに対してslot所有・重量・個体stateを検証します。
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<(), ItemError> {
        self.validate_intrinsic_state()?;
        let mut total_bytes = 0_u64;
        let mut total_weight = 0_u64;
        for slot in &self.slots {
            let Some(lot) = &slot.lot else {
                continue;
            };
            let definition = catalog.get(lot.item_key())?;
            if definition.equipment_slot != Some(slot.key) {
                return Err(ItemError::InvalidLot);
            }
            let count = match lot {
                ItemLot::Stack { count: 1, .. } if definition.max_state_bytes == 0 => 1,
                ItemLot::Instance(instance)
                    if definition.max_stack == 1
                        && instance.state.len() <= definition.max_state_bytes as usize =>
                {
                    total_bytes = total_bytes
                        .checked_add(instance.state.len() as u64)
                        .ok_or(ItemError::CapacityExceeded)?;
                    1
                }
                _ => return Err(ItemError::InvalidLot),
            };
            total_weight = total_weight
                .checked_add(
                    definition
                        .unit_weight_micros
                        .checked_mul(count)
                        .ok_or(ItemError::CapacityExceeded)?,
                )
                .ok_or(ItemError::CapacityExceeded)?;
        }
        if total_bytes > self.limits.payload_bytes || total_weight > self.limits.weight_micros {
            return Err(ItemError::CapacityExceeded);
        }
        Ok(())
    }

    /// 復元後の上限が登録済み設定と一致するか確認します。
    pub fn validate_limits(&self, expected: EquipmentLimits) -> Result<(), ItemError> {
        if self.limits == expected {
            Ok(())
        } else {
            Err(ItemError::InvalidContainer)
        }
    }

    /// inventoryと装備を合わせた一意性・定義・容量を確認します。
    pub fn validate_with_inventory(
        &self,
        inventory: &ItemContainer,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        inventory.validate(catalog)?;
        self.validate(catalog)?;
        validate_unique_instances_with_equipment(inventory, self)
    }

    /// inventory・equipment・全owner索引を一つの装備取引として更新します。
    ///
    /// callbackは複製候補だけを変更します。全ownerの重複、定義、容量、registry revisionを
    /// 検証した後にinventory/equipment/registryをmove公開するため、装備移動で個体索引だけが
    /// 古くなる状態を作りません。外側containerとloadoutはWorld内の他ownerを表します。
    #[allow(clippy::too_many_arguments)] // 原子的な装備取引に必要なrevision・owner範囲・容量を明示します。
    pub fn transact_with_registry<R>(
        &mut self,
        inventory: &mut ItemContainer,
        expected_inventory_revision: u64,
        expected_equipment_revision: u64,
        catalog: &ItemCatalog,
        registry: &mut ItemInstanceRegistry,
        registry_scope: StableKey,
        expected_registry_revision: u64,
        other_containers: &[&ItemContainer],
        other_equipment: &[&EquipmentLoadout],
        registry_limits: ItemInstanceRegistryLimits,
        update: impl FnOnce(&mut ItemContainer, &mut EquipmentLoadout) -> Result<R, ItemError>,
    ) -> Result<R, ItemError> {
        if inventory.revision != expected_inventory_revision
            || self.revision != expected_equipment_revision
            || registry.revision != expected_registry_revision
        {
            return Err(ItemError::StaleRevision);
        }

        let mut current_containers = Vec::new();
        current_containers
            .try_reserve_exact(other_containers.len().saturating_add(1))
            .map_err(|_| ItemError::AllocationFailed)?;
        current_containers.push(&*inventory);
        current_containers.extend_from_slice(other_containers);
        let mut current_equipment = Vec::new();
        current_equipment
            .try_reserve_exact(other_equipment.len().saturating_add(1))
            .map_err(|_| ItemError::AllocationFailed)?;
        current_equipment.push(&*self);
        current_equipment.extend_from_slice(other_equipment);
        registry.validate_ownership(
            registry_scope,
            &current_containers,
            &current_equipment,
            catalog,
            registry_limits,
        )?;

        let mut inventory_candidate = try_clone_container(inventory)?;
        let mut equipment_candidate = try_clone_equipment(self)?;
        let result = update(&mut inventory_candidate, &mut equipment_candidate)?;
        inventory_candidate.validate(catalog)?;
        equipment_candidate.validate(catalog)?;

        let mut candidate_containers = Vec::new();
        candidate_containers
            .try_reserve_exact(other_containers.len().saturating_add(1))
            .map_err(|_| ItemError::AllocationFailed)?;
        candidate_containers.push(&inventory_candidate);
        candidate_containers.extend_from_slice(other_containers);
        let mut candidate_equipment = Vec::new();
        candidate_equipment
            .try_reserve_exact(other_equipment.len().saturating_add(1))
            .map_err(|_| ItemError::AllocationFailed)?;
        candidate_equipment.push(&equipment_candidate);
        candidate_equipment.extend_from_slice(other_equipment);
        let owners = build_item_instance_owners(
            &candidate_containers,
            &candidate_equipment,
            catalog,
            registry_limits,
        )?;
        let registry_candidate = if owners == registry.owners {
            None
        } else {
            Some(ItemInstanceRegistry {
                scope: registry.scope,
                revision: registry
                    .revision
                    .checked_add(1)
                    .ok_or(ItemError::RevisionOverflow)?,
                owners,
            })
        };

        *inventory = inventory_candidate;
        *self = equipment_candidate;
        if let Some(candidate) = registry_candidate {
            *registry = candidate;
        }
        Ok(result)
    }

    fn validate_intrinsic_state(&self) -> Result<(), ItemError> {
        if self.slots.is_empty()
            || self.slots.len() > u32::MAX as usize
            || self.slots.windows(2).any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(ItemError::InvalidContainer);
        }
        if self.slots.iter().any(|slot| {
            matches!(
                &slot.lot,
                Some(ItemLot::Stack { count, .. }) if *count != 1
            )
        }) {
            return Err(ItemError::InvalidLot);
        }
        let instance_count = self
            .slots
            .iter()
            .filter(|slot| matches!(&slot.lot, Some(ItemLot::Instance(_))))
            .count();
        let mut instance_ids = Vec::new();
        instance_ids
            .try_reserve_exact(instance_count)
            .map_err(|_| ItemError::AllocationFailed)?;
        instance_ids.extend(self.slots.iter().filter_map(|slot| match &slot.lot {
            Some(ItemLot::Instance(instance)) => Some(instance.id),
            _ => None,
        }));
        validate_unique_instance_ids(&mut instance_ids)?;
        Ok(())
    }

    /// inventoryの個体を装備し、既存slot個体は空いたinventory容量へ戻します。
    ///
    /// 両方のrevision、item定義、slot対応、container/loadout容量と個体ID一意性を候補で
    /// 確認します。どれかが失敗すれば二stateとも変更せず、成功時は双方のrevisionを一つ進めます。
    pub fn equip_from(
        &mut self,
        inventory: &mut ItemContainer,
        equipment_slot: StableKey,
        inventory_slot: usize,
        expected_inventory_revision: u64,
        expected_equipment_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        if inventory.revision != expected_inventory_revision
            || self.revision != expected_equipment_revision
        {
            return Err(ItemError::StaleRevision);
        }
        self.validate_with_inventory(inventory, catalog)?;
        let equipment_index = self
            .slots
            .binary_search_by_key(&equipment_slot, |slot| slot.key)
            .map_err(|_| ItemError::InvalidContainer)?;
        let source = inventory
            .slots
            .get(inventory_slot)
            .and_then(Option::as_ref)
            .ok_or(ItemError::MissingItem)?;
        let item_definition = catalog.get(source.item_key())?;
        if item_definition.equipment_slot != Some(equipment_slot) {
            return Err(ItemError::InvalidLot);
        }
        let inventory_revision = inventory
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let equipment_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let mut inventory_candidate = try_clone_container(inventory)?;
        let mut equipment_candidate = try_clone_equipment(self)?;
        let source_lot = inventory_candidate.slots[inventory_slot]
            .take()
            .ok_or(ItemError::MissingItem)?;
        let moving = match source_lot {
            ItemLot::Stack { item, count: 1 } => ItemLot::Stack { item, count: 1 },
            ItemLot::Stack { item, count } => {
                inventory_candidate.slots[inventory_slot] = Some(ItemLot::Stack {
                    item,
                    count: count - 1,
                });
                ItemLot::Stack { item, count: 1 }
            }
            ItemLot::Instance(instance) => ItemLot::Instance(instance),
        };
        if let Some(previous) = equipment_candidate.slots[equipment_index].lot.take() {
            insert_lot(&mut inventory_candidate, previous, catalog)?;
        }
        equipment_candidate.slots[equipment_index].lot = Some(moving);
        inventory_candidate.validate(catalog)?;
        equipment_candidate.validate(catalog)?;
        validate_unique_instances_with_equipment(&inventory_candidate, &equipment_candidate)?;
        inventory_candidate.revision = inventory_revision;
        equipment_candidate.revision = equipment_revision;
        *inventory = inventory_candidate;
        *self = equipment_candidate;
        Ok(())
    }

    /// 装備品をinventoryへ戻し、slot取引を一括確定します。
    pub fn unequip_to(
        &mut self,
        inventory: &mut ItemContainer,
        equipment_slot: StableKey,
        expected_inventory_revision: u64,
        expected_equipment_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        if inventory.revision != expected_inventory_revision
            || self.revision != expected_equipment_revision
        {
            return Err(ItemError::StaleRevision);
        }
        self.validate_with_inventory(inventory, catalog)?;
        let equipment_index = self
            .slots
            .binary_search_by_key(&equipment_slot, |slot| slot.key)
            .map_err(|_| ItemError::InvalidContainer)?;
        if self.slots[equipment_index].lot.is_none() {
            return Err(ItemError::MissingItem);
        }
        let inventory_revision = inventory
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let equipment_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let mut inventory_candidate = try_clone_container(inventory)?;
        let mut equipment_candidate = try_clone_equipment(self)?;
        let lot = equipment_candidate.slots[equipment_index]
            .lot
            .take()
            .ok_or(ItemError::MissingItem)?;
        insert_lot(&mut inventory_candidate, lot, catalog)?;
        inventory_candidate.validate(catalog)?;
        equipment_candidate.validate(catalog)?;
        validate_unique_instances_with_equipment(&inventory_candidate, &equipment_candidate)?;
        inventory_candidate.revision = inventory_revision;
        equipment_candidate.revision = equipment_revision;
        *inventory = inventory_candidate;
        *self = equipment_candidate;
        Ok(())
    }
}

impl ItemContainer {
    /// 設定済みslot数を持つ空containerを作ります。
    pub fn new(key: StableKey, limits: ItemContainerLimits) -> Result<Self, ItemError> {
        validate_limits(limits)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(limits.slots as usize)
            .map_err(|_| ItemError::AllocationFailed)?;
        slots.resize_with(limits.slots as usize, || None);
        Ok(Self {
            key,
            revision: 0,
            limits,
            slots,
        })
    }

    /// container安定keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 取引ごとに進むrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// 設定済みの許容量です。
    pub fn limits(&self) -> ItemContainerLimits {
        self.limits
    }
    /// 読み取り専用slot viewです。
    pub fn slots(&self) -> &[Option<ItemLot>] {
        &self.slots
    }

    /// item候補と原子的WorldEditの両方が検証できた後だけを公開します。
    ///
    /// item操作はWorld編集より先に複製候補へ適用します。容量やrevision失敗ならWorldを
    /// 開かず、World callback/validationが失敗すればitem候補を破棄します。Worldが確定した
    /// 後のcontainer移動は失敗不能な所有権移動です。この関数中は両方の`&mut` borrowを
    /// 保持するため、他のsafe Rust呼出側から中間状態を観測できません。callbackが触れる
    /// この二状態以外の外部副作用は本取引のrollback対象ではありません。
    pub fn transact_with_world<R>(
        &mut self,
        expected_revision: u64,
        catalog: &ItemCatalog,
        world: &mut World,
        item_update: impl FnOnce(&mut ItemContainer) -> Result<R, ItemError>,
        world_update: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<()>,
    ) -> Result<R, ItemWorldTransactionError> {
        if self.revision != expected_revision {
            return Err(ItemWorldTransactionError::Item(ItemError::StaleRevision));
        }
        self.validate(catalog)
            .map_err(ItemWorldTransactionError::Item)?;
        let mut candidate = try_clone_container(self).map_err(ItemWorldTransactionError::Item)?;
        let result = item_update(&mut candidate).map_err(ItemWorldTransactionError::Item)?;
        candidate
            .validate(catalog)
            .map_err(ItemWorldTransactionError::Item)?;
        world
            .edit(world_update)
            .map_err(ItemWorldTransactionError::World)?;
        *self = candidate;
        Ok(result)
    }

    /// World候補、container候補、World scope全体のinstance registryを一括更新します。
    ///
    /// `other_containers`は同一Worldに属する他player inventory・chest・drop containerを、
    /// `equipment`は登録済みloadoutを表します。開始時にregistryと全ownerを照合し、変更後は
    /// candidate全体から新しいowner索引を作ります。重複個体、容量、revision、World editの失敗は
    /// いずれも公開前に返り、成功時だけWorld、container、registryを順に失敗不能なmoveで確定します。
    /// registry検査は明示的なitem取引境界だけで行い、game tick毎の全instance走査にはしません。
    #[allow(clippy::too_many_arguments)] // World/container/registryを同時commitする境界条件を明示します。
    pub fn transact_with_world_and_registry<R>(
        &mut self,
        expected_revision: u64,
        catalog: &ItemCatalog,
        world: &mut World,
        registry: &mut ItemInstanceRegistry,
        registry_scope: StableKey,
        expected_registry_revision: u64,
        other_containers: &[&ItemContainer],
        equipment: &[&EquipmentLoadout],
        registry_limits: ItemInstanceRegistryLimits,
        item_update: impl FnOnce(&mut ItemContainer) -> Result<R, ItemError>,
        world_update: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<()>,
    ) -> Result<R, ItemWorldTransactionError> {
        if self.revision != expected_revision {
            return Err(ItemWorldTransactionError::Item(ItemError::StaleRevision));
        }
        if registry.revision != expected_registry_revision {
            return Err(ItemWorldTransactionError::Item(ItemError::StaleRevision));
        }
        self.validate(catalog)
            .map_err(ItemWorldTransactionError::Item)?;
        let owner_count = other_containers
            .len()
            .checked_add(1)
            .ok_or(ItemWorldTransactionError::Item(ItemError::CapacityExceeded))?;
        if owner_count.saturating_add(equipment.len()) > registry_limits.owners as usize {
            return Err(ItemWorldTransactionError::Item(ItemError::CapacityExceeded));
        }
        let mut current_containers = Vec::new();
        current_containers
            .try_reserve_exact(owner_count)
            .map_err(|_| ItemWorldTransactionError::Item(ItemError::AllocationFailed))?;
        current_containers.push(&*self);
        current_containers.extend_from_slice(other_containers);
        registry
            .validate_ownership(
                registry_scope,
                &current_containers,
                equipment,
                catalog,
                registry_limits,
            )
            .map_err(ItemWorldTransactionError::Item)?;

        let mut candidate = try_clone_container(self).map_err(ItemWorldTransactionError::Item)?;
        let result = item_update(&mut candidate).map_err(ItemWorldTransactionError::Item)?;
        candidate
            .validate(catalog)
            .map_err(ItemWorldTransactionError::Item)?;
        let mut candidate_containers = Vec::new();
        candidate_containers
            .try_reserve_exact(owner_count)
            .map_err(|_| ItemWorldTransactionError::Item(ItemError::AllocationFailed))?;
        candidate_containers.push(&candidate);
        candidate_containers.extend_from_slice(other_containers);
        let candidate_owners =
            build_item_instance_owners(&candidate_containers, equipment, catalog, registry_limits)
                .map_err(ItemWorldTransactionError::Item)?;
        let registry_candidate = if candidate_owners == registry.owners {
            None
        } else {
            let revision = registry
                .revision
                .checked_add(1)
                .ok_or(ItemWorldTransactionError::Item(ItemError::RevisionOverflow))?;
            Some(ItemInstanceRegistry {
                scope: registry.scope,
                revision,
                owners: candidate_owners,
            })
        };

        world
            .edit(world_update)
            .map_err(ItemWorldTransactionError::World)?;
        *self = candidate;
        if let Some(candidate) = registry_candidate {
            *registry = candidate;
        }
        Ok(result)
    }

    /// catalog定義と保存状態に対し、container全体の不変条件を確認します。
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<(), ItemError> {
        self.validate_intrinsic_state()?;
        let mut total_weight = 0_u64;
        for lot in self.slots.iter().flatten() {
            let definition = catalog.get(lot.item_key())?;
            let count = match lot {
                ItemLot::Stack { count, .. }
                    if definition.max_state_bytes == 0
                        && *count > 0
                        && *count <= definition.max_stack =>
                {
                    *count
                }
                ItemLot::Instance(instance)
                    if definition.max_stack == 1
                        && instance.state.len() <= definition.max_state_bytes as usize =>
                {
                    1
                }
                _ => return Err(ItemError::InvalidLot),
            };
            total_weight = total_weight
                .checked_add(
                    definition
                        .unit_weight_micros
                        .checked_mul(u64::from(count))
                        .ok_or(ItemError::CapacityExceeded)?,
                )
                .ok_or(ItemError::CapacityExceeded)?;
        }
        if total_weight > self.limits.weight_micros {
            return Err(ItemError::CapacityExceeded);
        }
        Ok(())
    }

    /// restore後のcontainer容量が呼出側で登録した設定と一致することを検証します。
    pub fn validate_limits(&self, expected: ItemContainerLimits) -> Result<(), ItemError> {
        validate_limits(expected)?;
        if self.limits == expected {
            Ok(())
        } else {
            Err(ItemError::InvalidContainer)
        }
    }

    fn validate_intrinsic_state(&self) -> Result<(), ItemError> {
        validate_limits(self.limits)?;
        if self.slots.len() != self.limits.slots as usize {
            return Err(ItemError::InvalidContainer);
        }
        let mut total_bytes = 0_u64;
        for lot in self.slots.iter().flatten() {
            match lot {
                ItemLot::Stack { count: 0, .. } => return Err(ItemError::InvalidLot),
                ItemLot::Stack { .. } => {}
                ItemLot::Instance(instance) => {
                    total_bytes = total_bytes
                        .checked_add(instance.state.len() as u64)
                        .ok_or(ItemError::CapacityExceeded)?;
                }
            }
        }
        if total_bytes > self.limits.payload_bytes {
            return Err(ItemError::CapacityExceeded);
        }
        validate_unique_instances(&[self])
    }

    /// 外部報酬や生成itemを候補へ追加し、定義・重量・byte・slotを一括検証して公開します。
    pub fn deposit(
        &mut self,
        lot: ItemLot,
        expected_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        if self.revision != expected_revision {
            return Err(ItemError::StaleRevision);
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        self.validate(catalog)?;
        let mut candidate = try_clone_container(self)?;
        insert_lot(&mut candidate, lot, catalog)?;
        candidate.validate(catalog)?;
        candidate.revision = next_revision;
        *self = candidate;
        Ok(())
    }

    /// 二containerの間でstack個数または一つのinstanceを原子的に移します。
    ///
    /// 期待revisionを両側へ指定します。容量・定義・instance重複・revision overflowのどれかが
    /// 起きた場合は両containerを変更しません。成功時はsourceとdestinationのrevisionを各1進めます。
    pub fn transfer_to(
        &mut self,
        destination: &mut Self,
        source_slot: usize,
        count: u32,
        expected_source_revision: u64,
        expected_destination_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        if std::ptr::eq(self, destination) {
            return Err(ItemError::InvalidContainer);
        }
        if self.key == destination.key {
            return Err(ItemError::InvalidContainer);
        }
        if self.revision != expected_source_revision
            || destination.revision != expected_destination_revision
        {
            return Err(ItemError::StaleRevision);
        }
        let source_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let destination_revision = destination
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        self.validate(catalog)?;
        destination.validate(catalog)?;

        let mut source_candidate = try_clone_container(self)?;
        let mut destination_candidate = try_clone_container(destination)?;
        let source_lot = source_candidate
            .slots
            .get_mut(source_slot)
            .and_then(Option::as_mut)
            .ok_or(ItemError::MissingItem)?;
        let moved = match source_lot {
            ItemLot::Stack {
                item,
                count: available,
            } => {
                if count == 0 || count > *available {
                    return Err(ItemError::MissingItem);
                }
                let moved = ItemLot::Stack { item: *item, count };
                *available -= count;
                if *available == 0 {
                    source_candidate.slots[source_slot] = None;
                }
                moved
            }
            ItemLot::Instance(instance) if count == 1 => {
                let moved = try_clone_instance(instance)?;
                source_candidate.slots[source_slot] = None;
                ItemLot::Instance(Box::new(moved))
            }
            ItemLot::Instance(_) => return Err(ItemError::InvalidLot),
        };
        insert_lot(&mut destination_candidate, moved, catalog)?;
        source_candidate.validate(catalog)?;
        destination_candidate.validate(catalog)?;
        validate_unique_instances(&[&source_candidate, &destination_candidate])?;
        source_candidate.revision = source_revision;
        destination_candidate.revision = destination_revision;
        *self = source_candidate;
        *destination = destination_candidate;
        Ok(())
    }
}

impl WorldDrop {
    /// dropのWorld cell座標です。
    pub fn position(&self) -> CellPoint {
        self.position
    }

    /// drop itemを所有する保存可能containerです。
    pub fn container(&self) -> &ItemContainer {
        &self.container
    }
}

impl WorldDropStore {
    /// World scopeと作者設定上限を固定して空storeを作ります。
    pub fn new(scope: StableKey, limits: WorldDropLimits) -> Self {
        Self {
            scope,
            revision: 0,
            limits,
            drops: Vec::new(),
        }
    }

    /// 所属World/save scopeです。
    pub fn scope(&self) -> StableKey {
        self.scope
    }

    /// dropの追加・回収取引ごとに進むrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// stable container key順の位置付きdropを読み取ります。
    pub fn iter(&self) -> impl Iterator<Item = &WorldDrop> {
        self.drops.iter()
    }

    /// container keyで位置付きdropを取得します。
    pub fn get(&self, key: StableKey) -> Option<&WorldDrop> {
        self.drops
            .binary_search_by_key(&key, |drop| drop.container.key)
            .ok()
            .map(|index| &self.drops[index])
    }

    /// 既存drop containerを採掘報酬取引へ渡します。
    pub fn get_mut(&mut self, key: StableKey) -> Option<&mut ItemContainer> {
        self.drops
            .binary_search_by_key(&key, |drop| drop.container.key)
            .ok()
            .map(|index| &mut self.drops[index].container)
    }

    /// 位置付きcontainerを追加します。空containerを先に登録してから採掘報酬を入れられます。
    pub fn insert(
        &mut self,
        position: CellPoint,
        container: ItemContainer,
        expected_revision: u64,
    ) -> Result<(), ItemError> {
        if self.revision != expected_revision {
            return Err(ItemError::StaleRevision);
        }
        if self.drops.len() >= self.limits.containers as usize {
            return Err(ItemError::CapacityExceeded);
        }
        if self
            .drops
            .binary_search_by_key(&container.key, |drop| drop.container.key)
            .is_ok()
        {
            return Err(ItemError::InvalidContainer);
        }
        container.validate_intrinsic_state()?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        self.drops
            .try_reserve(1)
            .map_err(|_| ItemError::AllocationFailed)?;
        let index = self
            .drops
            .binary_search_by_key(&container.key, |drop| drop.container.key)
            .unwrap_err();
        self.drops.insert(
            index,
            WorldDrop {
                position,
                container,
            },
        );
        self.revision = revision;
        Ok(())
    }

    /// 死亡時などにinventoryの全itemを、作者が指定した位置のdropへ一括移送します。
    ///
    /// inventory・drop store・World scopeのinstance registryを全て候補上で検査します。
    /// itemが空なら何も変更せず`Ok(false)`を返すため、呼出側は死亡状態だけを別途確定できます。
    /// drop容量、owner重複、保存量、revisionなどの失敗では3つの正規stateを変更しません。
    #[allow(clippy::too_many_arguments)]
    pub fn drop_inventory_at(
        &mut self,
        inventory: &mut ItemContainer,
        position: CellPoint,
        drop_key: StableKey,
        drop_limits: ItemContainerLimits,
        expected_store_revision: u64,
        expected_inventory_revision: u64,
        expected_registry_revision: u64,
        catalog: &ItemCatalog,
        registry: &mut ItemInstanceRegistry,
        registry_scope: StableKey,
        other_containers: &[&ItemContainer],
        equipment: &[&EquipmentLoadout],
        registry_limits: ItemInstanceRegistryLimits,
    ) -> Result<bool, ItemError> {
        if self.scope != registry_scope
            || inventory.key == drop_key
            || self.get(drop_key).is_some()
            || other_containers
                .iter()
                .any(|container| container.key == drop_key)
            || equipment.iter().any(|loadout| loadout.key == drop_key)
        {
            return Err(ItemError::InvalidContainer);
        }
        if self.revision != expected_store_revision
            || inventory.revision != expected_inventory_revision
            || registry.revision != expected_registry_revision
        {
            return Err(ItemError::StaleRevision);
        }
        inventory.validate(catalog)?;
        if inventory.slots.iter().all(Option::is_none) {
            return Ok(false);
        }
        let next_inventory_revision = inventory
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let next_store_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let owner_count = self
            .drops
            .len()
            .checked_add(other_containers.len())
            .and_then(|count| count.checked_add(1))
            .and_then(|count| count.checked_add(equipment.len()))
            .ok_or(ItemError::CapacityExceeded)?;
        if owner_count >= registry_limits.owners as usize
            || self.drops.len() >= self.limits.containers as usize
        {
            return Err(ItemError::CapacityExceeded);
        }

        let mut current_containers = Vec::new();
        current_containers
            .try_reserve_exact(self.drops.len() + other_containers.len() + 1)
            .map_err(|_| ItemError::AllocationFailed)?;
        current_containers.push(&*inventory);
        current_containers.extend(self.drops.iter().map(|drop| &drop.container));
        current_containers.extend_from_slice(other_containers);
        registry.validate_ownership(
            registry_scope,
            &current_containers,
            equipment,
            catalog,
            registry_limits,
        )?;

        let mut inventory_candidate = try_clone_container(inventory)?;
        let mut drop_candidate = ItemContainer::new(drop_key, drop_limits)?;
        for lot in inventory_candidate
            .slots
            .iter_mut()
            .filter_map(Option::take)
        {
            insert_lot(&mut drop_candidate, lot, catalog)?;
        }
        inventory_candidate.revision = next_inventory_revision;
        drop_candidate.revision = 1;
        inventory_candidate.validate(catalog)?;
        drop_candidate.validate(catalog)?;

        let mut drops_candidate = Vec::new();
        drops_candidate
            .try_reserve_exact(self.drops.len() + 1)
            .map_err(|_| ItemError::AllocationFailed)?;
        for drop in &self.drops {
            drops_candidate.push(WorldDrop {
                position: drop.position,
                container: try_clone_container(&drop.container)?,
            });
        }
        let drop_index = drops_candidate.partition_point(|drop| drop.container.key < drop_key);
        drops_candidate.insert(
            drop_index,
            WorldDrop {
                position,
                container: drop_candidate,
            },
        );

        let mut candidate_containers = Vec::new();
        candidate_containers
            .try_reserve_exact(drops_candidate.len() + other_containers.len() + 1)
            .map_err(|_| ItemError::AllocationFailed)?;
        candidate_containers.push(&inventory_candidate);
        candidate_containers.extend(drops_candidate.iter().map(|drop| &drop.container));
        candidate_containers.extend_from_slice(other_containers);
        let owners =
            build_item_instance_owners(&candidate_containers, equipment, catalog, registry_limits)?;
        let registry_candidate = if owners == registry.owners {
            None
        } else {
            Some(ItemInstanceRegistry {
                scope: registry.scope,
                revision: registry
                    .revision
                    .checked_add(1)
                    .ok_or(ItemError::RevisionOverflow)?,
                owners,
            })
        };
        let store_candidate = WorldDropStore {
            scope: self.scope,
            revision: next_store_revision,
            limits: self.limits,
            drops: drops_candidate,
        };

        *inventory = inventory_candidate;
        *self = store_candidate;
        if let Some(candidate) = registry_candidate {
            *registry = candidate;
        }
        Ok(true)
    }

    /// dropの一部をinventoryへ移し、個体owner索引と空drop削除も同じ候補で確定します。
    #[allow(clippy::too_many_arguments)]
    pub fn pickup(
        &mut self,
        key: StableKey,
        inventory: &mut ItemContainer,
        source_slot: usize,
        count: u32,
        expected_store_revision: u64,
        expected_drop_revision: u64,
        expected_inventory_revision: u64,
        catalog: &ItemCatalog,
        registry: &mut ItemInstanceRegistry,
        registry_scope: StableKey,
        expected_registry_revision: u64,
        other_containers: &[&ItemContainer],
        equipment: &[&EquipmentLoadout],
        registry_limits: ItemInstanceRegistryLimits,
    ) -> Result<(), ItemError> {
        if self.revision != expected_store_revision
            || inventory.revision != expected_inventory_revision
            || registry.revision != expected_registry_revision
        {
            return Err(ItemError::StaleRevision);
        }
        if self.scope != registry_scope || inventory.key == key {
            return Err(ItemError::InvalidContainer);
        }
        let drop_index = self
            .drops
            .binary_search_by_key(&key, |drop| drop.container.key)
            .map_err(|_| ItemError::MissingItem)?;
        let source = &self.drops[drop_index].container;
        if source.revision != expected_drop_revision {
            return Err(ItemError::StaleRevision);
        }
        let owner_count = self
            .drops
            .len()
            .checked_add(other_containers.len())
            .and_then(|count| count.checked_add(1))
            .ok_or(ItemError::CapacityExceeded)?;
        if owner_count.saturating_add(equipment.len()) > registry_limits.owners as usize {
            return Err(ItemError::CapacityExceeded);
        }
        let mut current_containers = Vec::new();
        current_containers
            .try_reserve_exact(owner_count)
            .map_err(|_| ItemError::AllocationFailed)?;
        current_containers.push(&*inventory);
        current_containers.extend(self.drops.iter().map(|drop| &drop.container));
        current_containers.extend_from_slice(other_containers);
        registry.validate_ownership(
            registry_scope,
            &current_containers,
            equipment,
            catalog,
            registry_limits,
        )?;

        let mut source_candidate = try_clone_container(source)?;
        let mut inventory_candidate = try_clone_container(inventory)?;
        source_candidate.transfer_to(
            &mut inventory_candidate,
            source_slot,
            count,
            expected_drop_revision,
            expected_inventory_revision,
            catalog,
        )?;
        let remove_drop = source_candidate.slots.iter().all(Option::is_none);
        let mut candidate_containers = Vec::new();
        candidate_containers
            .try_reserve_exact(owner_count)
            .map_err(|_| ItemError::AllocationFailed)?;
        candidate_containers.push(&inventory_candidate);
        candidate_containers.extend(self.drops.iter().enumerate().map(|(index, drop)| {
            if index == drop_index {
                &source_candidate
            } else {
                &drop.container
            }
        }));
        candidate_containers.extend_from_slice(other_containers);
        if remove_drop {
            candidate_containers.remove(drop_index + 1);
        }
        let owners =
            build_item_instance_owners(&candidate_containers, equipment, catalog, registry_limits)?;
        let registry_candidate = if owners == registry.owners {
            None
        } else {
            Some(ItemInstanceRegistry {
                scope: registry.scope,
                revision: registry
                    .revision
                    .checked_add(1)
                    .ok_or(ItemError::RevisionOverflow)?,
                owners,
            })
        };
        let store_revision = self
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;

        *inventory = inventory_candidate;
        if remove_drop {
            self.drops.remove(drop_index);
        } else {
            self.drops[drop_index].container = source_candidate;
        }
        self.revision = store_revision;
        if let Some(candidate) = registry_candidate {
            *registry = candidate;
        }
        Ok(())
    }

    /// 復元先のowner上限が登録設定と一致するか確認します。
    pub fn validate_limits(&self, expected: WorldDropLimits) -> Result<(), ItemError> {
        if self.limits == expected && self.drops.len() <= expected.containers as usize {
            Ok(())
        } else {
            Err(ItemError::InvalidContainer)
        }
    }
}

impl StateCodec for WorldDropStore {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self.drops.len() > self.limits.containers as usize
            || self
                .drops
                .windows(2)
                .any(|pair| pair[0].container.key >= pair[1].container.key)
        {
            return Err(invalid_state());
        }
        out.raw(b"CHWDROP1")?;
        self.scope.encode(out)?;
        out.u64(self.revision)?;
        out.u32(self.limits.containers)?;
        out.u32(u32::try_from(self.drops.len()).map_err(|_| invalid_state())?)?;
        for drop in &self.drops {
            out.i32(drop.position.x)?;
            out.i32(drop.position.y)?;
            drop.container.encode(out)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHWDROP1" {
            return Err(invalid_state());
        }
        let scope = StableKey::decode(input)?;
        let revision = input.u64()?;
        let limits = WorldDropLimits {
            containers: input.u32()?,
        };
        let count = input.u32()? as usize;
        if count > limits.containers as usize || count > input.remaining() / 24 {
            return Err(invalid_state());
        }
        let mut drops = Vec::new();
        drops
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let position = CellPoint {
                x: input.i32()?,
                y: input.i32()?,
            };
            let container = ItemContainer::decode(input)?;
            if drops
                .last()
                .is_some_and(|previous: &WorldDrop| previous.container.key >= container.key)
            {
                return Err(invalid_state());
            }
            drops.push(WorldDrop {
                position,
                container,
            });
        }
        Ok(Self {
            scope,
            revision,
            limits,
            drops,
        })
    }
}

impl StateCodec for ItemContainer {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_intrinsic_state()
            .map_err(|_| invalid_state())?;
        out.u16(CONTAINER_CODEC_VERSION)?;
        self.key.encode(out)?;
        out.u64(self.revision)?;
        out.u32(self.limits.slots)?;
        out.u64(self.limits.payload_bytes)?;
        out.u64(self.limits.weight_micros)?;
        out.u32(u32::try_from(self.slots.len()).map_err(|_| invalid_state())?)?;
        for slot in &self.slots {
            match slot {
                None => out.u8(0)?,
                Some(ItemLot::Stack { item, count }) => {
                    out.u8(1)?;
                    item.encode(out)?;
                    out.u32(*count)?;
                }
                Some(ItemLot::Instance(instance)) => {
                    out.u8(2)?;
                    instance.item.encode(out)?;
                    instance.id.encode(out)?;
                    out.bytes(&instance.state)?;
                }
            }
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.u16()? != CONTAINER_CODEC_VERSION {
            return Err(invalid_state());
        }
        let key = StableKey::decode(input)?;
        let revision = input.u64()?;
        let limits = ItemContainerLimits {
            slots: input.u32()?,
            payload_bytes: input.u64()?,
            weight_micros: input.u64()?,
        };
        validate_limits(limits).map_err(|_| invalid_state())?;
        let count = input.u32()? as usize;
        if count != limits.slots as usize || count > input.remaining() {
            return Err(invalid_state());
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            slots.push(match input.u8()? {
                0 => None,
                1 => Some(ItemLot::Stack {
                    item: StableKey::decode(input)?,
                    count: input.u32()?,
                }),
                2 => Some(ItemLot::Instance(Box::new(ItemInstance {
                    item: StableKey::decode(input)?,
                    id: StableKey::decode(input)?,
                    state: copy_bytes(input.bytes(u32::MAX as usize)?)?,
                }))),
                _ => return Err(invalid_state()),
            });
        }
        let container = Self {
            key,
            revision,
            limits,
            slots,
        };
        container
            .validate_intrinsic_state()
            .map_err(|_| invalid_state())?;
        Ok(container)
    }
}

impl StateCodec for ItemInstanceRegistry {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self
            .owners
            .windows(2)
            .any(|pair| pair[0].instance >= pair[1].instance)
        {
            return Err(invalid_state());
        }
        out.raw(b"CHITEMR1")?;
        self.scope.encode(out)?;
        out.u64(self.revision)?;
        out.u32(u32::try_from(self.owners.len()).map_err(|_| invalid_state())?)?;
        for owner in &self.owners {
            owner.instance.encode(out)?;
            owner.owner.encode(out)?;
            match owner.slot {
                ItemInstanceOwnerSlot::Container(index) => {
                    out.u8(0)?;
                    out.u32(index)?;
                }
                ItemInstanceOwnerSlot::Equipment(slot) => {
                    out.u8(1)?;
                    slot.encode(out)?;
                }
            }
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHITEMR1" {
            return Err(invalid_state());
        }
        let scope = StableKey::decode(input)?;
        let revision = input.u64()?;
        let count = input.u32()? as usize;
        if count > input.remaining() / 19 {
            return Err(invalid_state());
        }
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let instance = StableKey::decode(input)?;
            let owner = StableKey::decode(input)?;
            let slot = match input.u8()? {
                0 => ItemInstanceOwnerSlot::Container(input.u32()?),
                1 => ItemInstanceOwnerSlot::Equipment(StableKey::decode(input)?),
                _ => return Err(invalid_state()),
            };
            if owners
                .last()
                .is_some_and(|previous: &ItemInstanceOwner| previous.instance >= instance)
            {
                return Err(invalid_state());
            }
            owners.push(ItemInstanceOwner {
                instance,
                owner,
                slot,
            });
        }
        Ok(Self {
            scope,
            revision,
            owners,
        })
    }
}

impl StateCodec for EquipmentLoadout {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_intrinsic_state()
            .map_err(|_| invalid_state())?;
        out.u16(1)?;
        self.key.encode(out)?;
        out.u64(self.revision)?;
        out.u64(self.limits.payload_bytes)?;
        out.u64(self.limits.weight_micros)?;
        out.u32(u32::try_from(self.slots.len()).map_err(|_| invalid_state())?)?;
        for slot in &self.slots {
            slot.key.encode(out)?;
            match &slot.lot {
                None => out.u8(0)?,
                Some(ItemLot::Stack { item, count }) => {
                    out.u8(1)?;
                    item.encode(out)?;
                    out.u32(*count)?;
                }
                Some(ItemLot::Instance(instance)) => {
                    out.u8(2)?;
                    instance.item.encode(out)?;
                    instance.id.encode(out)?;
                    out.bytes(&instance.state)?;
                }
            }
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.u16()? != 1 {
            return Err(invalid_state());
        }
        let key = StableKey::decode(input)?;
        let revision = input.u64()?;
        let limits = EquipmentLimits {
            payload_bytes: input.u64()?,
            weight_micros: input.u64()?,
        };
        let count = input.u32()? as usize;
        if count == 0 || count > input.remaining() / 8 {
            return Err(invalid_state());
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let slot_key = StableKey::decode(input)?;
            let lot = match input.u8()? {
                0 => None,
                1 => Some(ItemLot::Stack {
                    item: StableKey::decode(input)?,
                    count: input.u32()?,
                }),
                2 => Some(ItemLot::Instance(Box::new(ItemInstance {
                    item: StableKey::decode(input)?,
                    id: StableKey::decode(input)?,
                    state: copy_bytes(input.bytes(u32::MAX as usize)?)?,
                }))),
                _ => return Err(invalid_state()),
            };
            slots.push(EquipmentSlot { key: slot_key, lot });
        }
        let loadout = Self {
            key,
            revision,
            limits,
            slots,
        };
        loadout
            .validate_intrinsic_state()
            .map_err(|_| invalid_state())?;
        Ok(loadout)
    }
}

fn insert_lot(
    container: &mut ItemContainer,
    lot: ItemLot,
    catalog: &ItemCatalog,
) -> Result<(), ItemError> {
    let definition = catalog.get(lot.item_key())?;
    match lot {
        ItemLot::Stack { item, count } => {
            if definition.max_state_bytes != 0 || count == 0 || count > definition.max_stack {
                return Err(ItemError::InvalidLot);
            }
            insert_stack_batch(container, item, count, catalog)
        }
        instance @ ItemLot::Instance(_) => {
            if definition.max_stack != 1 {
                return Err(ItemError::InvalidLot);
            }
            let slot = container
                .slots
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(ItemError::CapacityExceeded)?;
            *slot = Some(instance);
            Ok(())
        }
    }
}

fn insert_stack_batch(
    container: &mut ItemContainer,
    item: StableKey,
    mut count: u32,
    catalog: &ItemCatalog,
) -> Result<(), ItemError> {
    let definition = catalog.get(item)?;
    if definition.max_state_bytes != 0 || count == 0 {
        return Err(ItemError::InvalidLot);
    }
    for slot in &mut container.slots {
        if let Some(ItemLot::Stack {
            item: existing,
            count: existing_count,
        }) = slot
            && *existing == item
            && *existing_count < definition.max_stack
        {
            let added = count.min(definition.max_stack - *existing_count);
            *existing_count += added;
            count -= added;
            if count == 0 {
                return Ok(());
            }
        }
    }
    for slot in &mut container.slots {
        if slot.is_none() {
            let added = count.min(definition.max_stack);
            *slot = Some(ItemLot::Stack { item, count: added });
            count -= added;
            if count == 0 {
                return Ok(());
            }
        }
    }
    Err(ItemError::CapacityExceeded)
}

fn try_clone_container(container: &ItemContainer) -> Result<ItemContainer, ItemError> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(container.slots.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    for slot in &container.slots {
        slots.push(match slot {
            None => None,
            Some(ItemLot::Stack { item, count }) => Some(ItemLot::Stack {
                item: *item,
                count: *count,
            }),
            Some(ItemLot::Instance(instance)) => {
                Some(ItemLot::Instance(Box::new(try_clone_instance(instance)?)))
            }
        });
    }
    Ok(ItemContainer {
        key: container.key,
        revision: container.revision,
        limits: container.limits,
        slots,
    })
}

fn try_clone_equipment(equipment: &EquipmentLoadout) -> Result<EquipmentLoadout, ItemError> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(equipment.slots.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    for slot in &equipment.slots {
        let lot = match &slot.lot {
            None => None,
            Some(ItemLot::Stack { item, count }) => Some(ItemLot::Stack {
                item: *item,
                count: *count,
            }),
            Some(ItemLot::Instance(instance)) => {
                Some(ItemLot::Instance(Box::new(try_clone_instance(instance)?)))
            }
        };
        slots.push(EquipmentSlot { key: slot.key, lot });
    }
    Ok(EquipmentLoadout {
        key: equipment.key,
        revision: equipment.revision,
        limits: equipment.limits,
        slots,
    })
}

fn try_clone_instance(instance: &ItemInstance) -> Result<ItemInstance, ItemError> {
    let mut state = Vec::new();
    state
        .try_reserve_exact(instance.state.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    state.extend_from_slice(&instance.state);
    Ok(ItemInstance {
        id: instance.id,
        item: instance.item,
        state,
    })
}

fn validate_unique_instances(containers: &[&ItemContainer]) -> Result<(), ItemError> {
    let count = containers
        .iter()
        .flat_map(|container| &container.slots)
        .filter(|slot| matches!(slot, Some(ItemLot::Instance(_))))
        .count();
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_| ItemError::AllocationFailed)?;
    for instance in containers
        .iter()
        .flat_map(|container| &container.slots)
        .filter_map(|slot| match slot {
            Some(ItemLot::Instance(instance)) => Some(instance),
            _ => None,
        })
    {
        ids.push(instance.id);
    }
    ids.sort_unstable();
    if let Some(duplicate) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(ItemError::DuplicateInstance(Box::new(duplicate[0])));
    }
    Ok(())
}

fn validate_unique_instances_with_equipment(
    inventory: &ItemContainer,
    equipment: &EquipmentLoadout,
) -> Result<(), ItemError> {
    let count = inventory
        .slots
        .iter()
        .chain(equipment.slots.iter().map(|slot| &slot.lot))
        .filter(|slot| matches!(slot, Some(ItemLot::Instance(_))))
        .count();
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_| ItemError::AllocationFailed)?;
    for instance in inventory
        .slots
        .iter()
        .filter_map(|slot| match slot {
            Some(ItemLot::Instance(instance)) => Some(instance),
            _ => None,
        })
        .chain(equipment.slots.iter().filter_map(|slot| match &slot.lot {
            Some(ItemLot::Instance(instance)) => Some(instance),
            _ => None,
        }))
    {
        ids.push(instance.id);
    }
    validate_unique_instance_ids(&mut ids)
}

fn build_item_instance_owners(
    containers: &[&ItemContainer],
    equipment: &[&EquipmentLoadout],
    catalog: &ItemCatalog,
    limits: ItemInstanceRegistryLimits,
) -> Result<Vec<ItemInstanceOwner>, ItemError> {
    let owner_count = containers
        .len()
        .checked_add(equipment.len())
        .ok_or(ItemError::CapacityExceeded)?;
    if owner_count > limits.owners as usize {
        return Err(ItemError::CapacityExceeded);
    }
    let mut owner_keys = Vec::new();
    owner_keys
        .try_reserve_exact(owner_count)
        .map_err(|_| ItemError::AllocationFailed)?;
    owner_keys.extend(containers.iter().map(|container| container.key));
    owner_keys.extend(equipment.iter().map(|loadout| loadout.key));
    owner_keys.sort_unstable();
    if owner_keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ItemError::InvalidContainer);
    }

    for container in containers {
        catalog.validate_container(container)?;
    }
    for loadout in equipment {
        loadout.validate(catalog)?;
    }
    let instance_count = containers
        .iter()
        .flat_map(|container| container.slots.iter())
        .chain(
            equipment
                .iter()
                .flat_map(|loadout| loadout.slots.iter().map(|slot| &slot.lot)),
        )
        .filter(|lot| matches!(lot, Some(ItemLot::Instance(_))))
        .count();
    if instance_count > limits.instances as usize {
        return Err(ItemError::CapacityExceeded);
    }
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(instance_count)
        .map_err(|_| ItemError::AllocationFailed)?;
    for container in containers {
        for (index, lot) in container.slots.iter().enumerate() {
            let Some(ItemLot::Instance(instance)) = lot else {
                continue;
            };
            owners.push(ItemInstanceOwner {
                instance: instance.id,
                owner: container.key,
                slot: ItemInstanceOwnerSlot::Container(
                    u32::try_from(index).map_err(|_| ItemError::InvalidContainer)?,
                ),
            });
        }
    }
    for loadout in equipment {
        for slot in &loadout.slots {
            let Some(ItemLot::Instance(instance)) = &slot.lot else {
                continue;
            };
            owners.push(ItemInstanceOwner {
                instance: instance.id,
                owner: loadout.key,
                slot: ItemInstanceOwnerSlot::Equipment(slot.key),
            });
        }
    }
    owners.sort_unstable_by_key(|owner| owner.instance);
    if let Some(pair) = owners
        .windows(2)
        .find(|pair| pair[0].instance == pair[1].instance)
    {
        return Err(ItemError::DuplicateInstance(Box::new(pair[0].instance)));
    }
    Ok(owners)
}

fn validate_unique_instance_ids(ids: &mut [StableKey]) -> Result<(), ItemError> {
    ids.sort_unstable();
    if let Some(duplicate) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
        Err(ItemError::DuplicateInstance(Box::new(duplicate[0])))
    } else {
        Ok(())
    }
}

fn copy_bytes(bytes: &[u8]) -> EngineResult<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| allocation_error())?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn validate_limits(limits: ItemContainerLimits) -> Result<(), ItemError> {
    if limits.slots == 0 {
        Err(ItemError::InvalidContainer)
    } else {
        Ok(())
    }
}

fn invalid_state() -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, "item container保存状態")
}

fn allocation_error() -> EngineError {
    EngineError::new(ErrorCode::CapacityExceeded, "item container確保")
}

/// craft材料を個数で指定します。Recipe登録時に同じitem keyを集約します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CraftRequirement {
    /// 消費またはtoolとして必要なitem定義keyです。
    pub item: StableKey,
    /// 必要個数です。
    pub count: u32,
}

/// Recipeに必要なstation、world revision、入力、tool、出力、作業tickを持ちます。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftRecipe {
    key: StableKey,
    version: u32,
    inputs: Vec<CraftRequirement>,
    tools: Vec<CraftRequirement>,
    output_item: StableKey,
    output_count: u32,
    work_ticks: u64,
    station: Option<StableKey>,
    world_revision: Option<u64>,
}

impl CraftRecipe {
    /// 重複材料を安定順に集約し、0個・overflow・未指定版を登録時に拒否します。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: StableKey,
        version: u32,
        inputs: Vec<CraftRequirement>,
        tools: Vec<CraftRequirement>,
        output_item: StableKey,
        output_count: u32,
        work_ticks: u64,
        station: Option<StableKey>,
        world_revision: Option<u64>,
    ) -> Result<Self, ItemError> {
        if version == 0
            || inputs.is_empty()
            || inputs.len() > 64
            || tools.len() > 16
            || output_count == 0
        {
            return Err(ItemError::InvalidRecipe);
        }
        let inputs = aggregate_requirements(inputs)?;
        let mut tools = tools;
        tools.sort_unstable_by_key(|tool| tool.item);
        if tools.iter().any(|tool| tool.count == 0)
            || tools.windows(2).any(|pair| pair[0].item == pair[1].item)
        {
            return Err(ItemError::InvalidRecipe);
        }
        Ok(Self {
            key,
            version,
            inputs,
            tools,
            output_item,
            output_count,
            work_ticks,
            station,
            world_revision,
        })
    }

    /// Recipe定義をcontent登録/起動境界でitem catalogと照合します。
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<(), ItemError> {
        if self.version == 0 || self.inputs.is_empty() || self.output_count == 0 {
            return Err(ItemError::InvalidRecipe);
        }
        for requirement in &self.inputs {
            let definition = catalog.get(requirement.item)?;
            if requirement.count == 0 || definition.max_state_bytes != 0 {
                return Err(ItemError::InvalidRecipe);
            }
        }
        for tool in &self.tools {
            if tool.count == 0 {
                return Err(ItemError::InvalidRecipe);
            }
            catalog.get(tool.item)?;
        }
        let output = catalog.get(self.output_item)?;
        if output.max_state_bytes != 0 {
            return Err(ItemError::InvalidRecipe);
        }
        Ok(())
    }

    /// Recipe安定keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 保存中jobと照合するrecipe版です。
    pub fn version(&self) -> u32 {
        self.version
    }
    /// 消費材料の安定順リストです。
    pub fn inputs(&self) -> &[CraftRequirement] {
        &self.inputs
    }
    /// 消費しないtoolの安定順リストです。
    pub fn tools(&self) -> &[CraftRequirement] {
        &self.tools
    }
}

/// key/versionで検索する、作者登録済みcraft recipe集合です。
///
/// 登録時にitem定義と条件を照合し、saveから復元したjobのrecipe参照を実行時catalogへ
/// 解決できます。recipeの選択やstation規則をEngine側へ固定せず、作者が任意の集合を登録します。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CraftCatalog {
    recipes: Vec<CraftRecipe>,
}

impl CraftCatalog {
    /// recipe keyの重複とitem catalogとの不整合を拒否して構築します。
    pub fn new(mut recipes: Vec<CraftRecipe>, items: &ItemCatalog) -> Result<Self, ItemError> {
        recipes.sort_unstable_by_key(CraftRecipe::key);
        if recipes.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(ItemError::InvalidRecipe);
        }
        for recipe in &recipes {
            recipe.validate(items)?;
        }
        Ok(Self { recipes })
    }

    /// keyで登録recipeを取得します。
    pub fn get(&self, key: StableKey) -> Result<&CraftRecipe, ItemError> {
        self.recipes
            .binary_search_by_key(&key, CraftRecipe::key)
            .map(|index| &self.recipes[index])
            .map_err(|_| ItemError::UnknownRecipe(Box::new(key)))
    }

    /// stable key順でrecipeを列挙します。
    pub fn iter(&self) -> impl Iterator<Item = &CraftRecipe> {
        self.recipes.iter()
    }

    /// item定義が変化した起動・復元境界で全recipeを再検証します。
    pub fn validate(&self, items: &ItemCatalog) -> Result<(), ItemError> {
        for recipe in &self.recipes {
            recipe.validate(items)?;
        }
        Ok(())
    }
}

fn aggregate_requirements(
    requirements: Vec<CraftRequirement>,
) -> Result<Vec<CraftRequirement>, ItemError> {
    let mut aggregated = Vec::new();
    aggregated
        .try_reserve_exact(requirements.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    for requirement in requirements {
        if requirement.count == 0 {
            return Err(ItemError::InvalidRecipe);
        }
        if let Some(existing) = aggregated
            .iter_mut()
            .find(|existing: &&mut CraftRequirement| existing.item == requirement.item)
        {
            existing.count = existing
                .count
                .checked_add(requirement.count)
                .ok_or(ItemError::InvalidRecipe)?;
        } else {
            aggregated.push(requirement);
        }
    }
    aggregated.sort_unstable_by_key(|requirement| requirement.item);
    Ok(aggregated)
}

/// Craftのstationと、条件が成立しているworld revisionです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CraftContext {
    /// 実行中のstation keyです。
    pub station: Option<StableKey>,
    /// 予約を判定するWorld revisionです。
    pub world_revision: u64,
}

/// Craft jobのlogical tick予算状態です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CraftProgress {
    Running { remaining_ticks: u64 },
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CraftJob {
    recipe: StableKey,
    recipe_version: u32,
    receipt: StableKey,
    inputs: Vec<CraftRequirement>,
    output_item: StableKey,
    output_count: u32,
    work_ticks_total: u64,
    work_ticks_remaining: u64,
    last_tick: u64,
    station: Option<StableKey>,
    world_revision: Option<u64>,
}

impl CraftJob {
    fn progress(&self) -> CraftProgress {
        if self.work_ticks_remaining == 0 {
            CraftProgress::Ready
        } else {
            CraftProgress::Running {
                remaining_ticks: self.work_ticks_remaining,
            }
        }
    }
}

/// Inventoryと継続craft job/receiptを同じ保存recordで所有するsessionです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftSession {
    container: ItemContainer,
    job: Option<CraftJob>,
    completed_receipts: Vec<StableKey>,
    receipt_capacity: u32,
}

impl CraftSession {
    /// 検証済みcontainerと作者指定receipt容量をcraft sessionへ移します。
    pub fn new(
        container: ItemContainer,
        catalog: &ItemCatalog,
        receipt_capacity: u32,
    ) -> Result<Self, ItemError> {
        if receipt_capacity == 0 {
            return Err(ItemError::InvalidRecipe);
        }
        container.validate(catalog)?;
        Ok(Self {
            container,
            job: None,
            completed_receipts: Vec::new(),
            receipt_capacity,
        })
    }

    /// inventoryの読み取り専用viewです。
    pub fn container(&self) -> &ItemContainer {
        &self.container
    }
    /// 現在のjob進捗です。
    pub fn progress(&self) -> Option<CraftProgress> {
        self.job.as_ref().map(CraftJob::progress)
    }
    /// 過去に確定またはcancelしたreceiptかを確認します。
    pub fn contains_receipt(&self, receipt: StableKey) -> bool {
        self.completed_receipts.binary_search(&receipt).is_ok()
    }

    /// 未処理jobがないとき、通常のcapacity検証付きでitemを追加します。
    pub fn deposit(
        &mut self,
        lot: ItemLot,
        expected_container_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        if self.job.is_some() {
            return Err(ItemError::CraftBusy);
        }
        self.container
            .deposit(lot, expected_container_revision, catalog)
    }

    /// 材料と出力容量を候補で先に検査し、材料・work orderを同時に予約します。
    pub fn start(
        &mut self,
        recipe: &CraftRecipe,
        receipt: StableKey,
        tick: u64,
        context: CraftContext,
        expected_container_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<CraftProgress, ItemError> {
        if self.job.is_some() {
            return Err(ItemError::CraftBusy);
        }
        if self.contains_receipt(receipt) {
            return Err(ItemError::DuplicateReceipt);
        }
        if self.completed_receipts.len() >= self.receipt_capacity as usize {
            return Err(ItemError::ReceiptCapacity);
        }
        recipe.validate(catalog)?;
        if recipe
            .station
            .is_some_and(|station| context.station != Some(station))
            || recipe
                .world_revision
                .is_some_and(|revision| context.world_revision != revision)
        {
            return Err(ItemError::InvalidCraftContext);
        }
        if self.container.revision != expected_container_revision {
            return Err(ItemError::StaleRevision);
        }
        let mut candidate = try_clone_container(&self.container)?;
        let inputs = remove_craft_inputs(&mut candidate, &recipe.inputs)?;
        // 材料を消費した後の候補に対してtoolを検証し、同じ個体を入力とtoolの両方へ
        // 二重計上しません。失敗時は候補だけを破棄するため、sessionは変更されません。
        for tool in &recipe.tools {
            if query_item_count(&candidate, tool.item, catalog)? < u64::from(tool.count) {
                return Err(ItemError::MissingTool(Box::new(tool.item)));
            }
        }
        let mut output_preflight = try_clone_container(&candidate)?;
        insert_stack_batch(
            &mut output_preflight,
            recipe.output_item,
            recipe.output_count,
            catalog,
        )?;
        output_preflight.validate(catalog)?;
        candidate.validate(catalog)?;
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;

        let job = CraftJob {
            recipe: recipe.key,
            recipe_version: recipe.version,
            receipt,
            inputs,
            output_item: recipe.output_item,
            output_count: recipe.output_count,
            work_ticks_total: recipe.work_ticks,
            work_ticks_remaining: recipe.work_ticks,
            last_tick: tick,
            station: recipe.station,
            world_revision: recipe.world_revision,
        };
        let progress = job.progress();
        self.container = candidate;
        self.job = Some(job);
        Ok(progress)
    }

    /// 登録catalogのkey/versionを解決して継続craftを開始します。
    #[allow(clippy::too_many_arguments)]
    pub fn start_registered(
        &mut self,
        recipes: &CraftCatalog,
        recipe_key: StableKey,
        recipe_version: u32,
        receipt: StableKey,
        tick: u64,
        context: CraftContext,
        expected_container_revision: u64,
        items: &ItemCatalog,
    ) -> Result<CraftProgress, ItemError> {
        let recipe = recipes.get(recipe_key)?;
        if recipe.version != recipe_version {
            return Err(ItemError::InvalidRecipe);
        }
        self.start(
            recipe,
            receipt,
            tick,
            context,
            expected_container_revision,
            items,
        )
    }

    /// 同じjobをtick差分だけ進めます。station/World条件が外れたtickは進捗せず、
    /// `last_tick`をそのtickへ移して停止中の時間が再開時に加算されないようにします。
    pub fn advance_to(
        &mut self,
        recipe: &CraftRecipe,
        tick: u64,
        context: CraftContext,
        catalog: &ItemCatalog,
    ) -> Result<CraftProgress, ItemError> {
        let job = self.job.as_mut().ok_or(ItemError::CraftNotReady)?;
        validate_job_recipe(job, recipe, catalog)?;
        if tick < job.last_tick {
            return Err(ItemError::StaleCraftTick);
        }
        if job
            .station
            .is_some_and(|station| context.station != Some(station))
            || job
                .world_revision
                .is_some_and(|revision| context.world_revision != revision)
        {
            job.last_tick = tick;
            return Err(ItemError::InvalidCraftContext);
        }
        let elapsed = tick - job.last_tick;
        job.work_ticks_remaining = job.work_ticks_remaining.saturating_sub(elapsed);
        job.last_tick = tick;
        Ok(job.progress())
    }

    /// 保存jobのkey/versionをcatalogから解決してlogical tickを進めます。
    pub fn advance_registered(
        &mut self,
        recipes: &CraftCatalog,
        tick: u64,
        context: CraftContext,
        items: &ItemCatalog,
    ) -> Result<CraftProgress, ItemError> {
        let job = self.job.as_ref().ok_or(ItemError::CraftNotReady)?;
        let recipe = recipes.get(job.recipe)?;
        if recipe.version != job.recipe_version {
            return Err(ItemError::InvalidRecipe);
        }
        self.advance_to(recipe, tick, context, items)
    }

    /// Ready outputをcontainerへ確定しreceiptを記録します。容量不足ならjobはReadyのまま残ります。
    pub fn claim(
        &mut self,
        recipe: &CraftRecipe,
        receipt: StableKey,
        expected_container_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        let job = self.job.as_ref().ok_or(ItemError::UnknownReceipt)?;
        if job.receipt != receipt {
            return Err(ItemError::UnknownReceipt);
        }
        validate_job_recipe(job, recipe, catalog)?;
        if job.work_ticks_remaining != 0 {
            return Err(ItemError::CraftNotReady);
        }
        if self.container.revision != expected_container_revision {
            return Err(ItemError::StaleRevision);
        }
        let mut candidate = try_clone_container(&self.container)?;
        insert_stack_batch(&mut candidate, job.output_item, job.output_count, catalog)?;
        candidate.validate(catalog)?;
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let mut receipts = try_clone_keys(&self.completed_receipts)?;
        insert_receipt(&mut receipts, receipt, self.receipt_capacity)?;
        self.container = candidate;
        self.completed_receipts = receipts;
        self.job = None;
        Ok(())
    }

    /// Ready jobをcatalog版と照合してreceipt確定します。
    pub fn claim_registered(
        &mut self,
        recipes: &CraftCatalog,
        receipt: StableKey,
        expected_container_revision: u64,
        items: &ItemCatalog,
    ) -> Result<(), ItemError> {
        let job = self.job.as_ref().ok_or(ItemError::UnknownReceipt)?;
        if job.receipt != receipt {
            return Err(ItemError::UnknownReceipt);
        }
        let recipe = recipes.get(job.recipe)?;
        if recipe.version != job.recipe_version {
            return Err(ItemError::InvalidRecipe);
        }
        self.claim(recipe, receipt, expected_container_revision, items)
    }

    /// jobをcancelし、消費材料を同一候補containerへ返却してreceiptを閉じます。
    pub fn cancel(
        &mut self,
        receipt: StableKey,
        expected_container_revision: u64,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        let job = self.job.as_ref().ok_or(ItemError::UnknownReceipt)?;
        if job.receipt != receipt {
            return Err(ItemError::UnknownReceipt);
        }
        if self.container.revision != expected_container_revision {
            return Err(ItemError::StaleRevision);
        }
        let mut candidate = try_clone_container(&self.container)?;
        for input in &job.inputs {
            insert_stack_batch(&mut candidate, input.item, input.count, catalog)?;
        }
        candidate.validate(catalog)?;
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(ItemError::RevisionOverflow)?;
        let mut receipts = try_clone_keys(&self.completed_receipts)?;
        insert_receipt(&mut receipts, receipt, self.receipt_capacity)?;
        self.container = candidate;
        self.completed_receipts = receipts;
        self.job = None;
        Ok(())
    }

    /// restore後にcontainer、job item、receipt ledgerの内容をcatalogで検証します。
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<(), ItemError> {
        self.container.validate(catalog)?;
        self.validate_structure()?;
        if let Some(job) = &self.job {
            let output = catalog.get(job.output_item)?;
            if output.max_state_bytes != 0 {
                return Err(ItemError::InvalidRecipe);
            }
            for input in &job.inputs {
                if input.count == 0 || catalog.get(input.item)?.max_state_bytes != 0 {
                    return Err(ItemError::InvalidRecipe);
                }
            }
        }
        Ok(())
    }

    /// 保存復元後にactive jobのrecipe key/versionと全予約値を登録recipeへ照合します。
    pub fn validate_with_recipe(
        &self,
        recipe: &CraftRecipe,
        catalog: &ItemCatalog,
    ) -> Result<(), ItemError> {
        self.validate(catalog)?;
        if let Some(job) = &self.job {
            validate_job_recipe(job, recipe, catalog)?;
            for tool in &recipe.tools {
                if query_item_count(&self.container, tool.item, catalog)? < u64::from(tool.count) {
                    return Err(ItemError::MissingTool(Box::new(tool.item)));
                }
            }
        }
        Ok(())
    }

    /// restore後のreceipt ledger上限が呼出側のruntime設定と一致することを検証します。
    pub fn validate_receipt_capacity(&self, expected: u32) -> Result<(), ItemError> {
        if expected != 0
            && self.receipt_capacity == expected
            && self.completed_receipts.len() <= expected as usize
        {
            Ok(())
        } else {
            Err(ItemError::InvalidCraftCapacity)
        }
    }

    fn validate_structure(&self) -> Result<(), ItemError> {
        if self.completed_receipts.len() > self.receipt_capacity as usize
            || self.receipt_capacity == 0
            || self
                .completed_receipts
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(ItemError::InvalidRecipe);
        }
        if let Some(job) = &self.job
            && (self.contains_receipt(job.receipt)
                || job.recipe_version == 0
                || job.output_count == 0
                || job.work_ticks_remaining > job.work_ticks_total
                || !requirements_are_canonical(&job.inputs))
        {
            return Err(ItemError::InvalidRecipe);
        }
        Ok(())
    }
}

impl StateCodec for CraftSession {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_structure().map_err(|_| invalid_state())?;
        out.u16(1)?;
        self.container.encode(out)?;
        out.u32(self.receipt_capacity)?;
        out.u32(u32::try_from(self.completed_receipts.len()).map_err(|_| invalid_state())?)?;
        for receipt in &self.completed_receipts {
            receipt.encode(out)?;
        }
        match &self.job {
            None => out.u8(0)?,
            Some(job) => {
                out.u8(1)?;
                encode_craft_job(job, out)?;
            }
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.u16()? != 1 {
            return Err(invalid_state());
        }
        let container = ItemContainer::decode(input)?;
        let receipt_capacity = input.u32()?;
        let receipt_count = input.u32()? as usize;
        if receipt_capacity == 0
            || receipt_count > receipt_capacity as usize
            || (receipt_count as u64).saturating_mul(7) > input.remaining() as u64
        {
            return Err(invalid_state());
        }
        let mut completed_receipts = Vec::new();
        completed_receipts
            .try_reserve_exact(receipt_count)
            .map_err(|_| allocation_error())?;
        for _ in 0..receipt_count {
            let receipt = StableKey::decode(input)?;
            if completed_receipts
                .last()
                .is_some_and(|last| *last >= receipt)
            {
                return Err(invalid_state());
            }
            completed_receipts.push(receipt);
        }
        let job = match input.u8()? {
            0 => None,
            1 => Some(decode_craft_job(input)?),
            _ => return Err(invalid_state()),
        };
        let session = Self {
            container,
            job,
            completed_receipts,
            receipt_capacity,
        };
        if session
            .job
            .as_ref()
            .is_some_and(|job| session.contains_receipt(job.receipt))
        {
            return Err(invalid_state());
        }
        Ok(session)
    }
}

fn validate_job_recipe(
    job: &CraftJob,
    recipe: &CraftRecipe,
    catalog: &ItemCatalog,
) -> Result<(), ItemError> {
    recipe.validate(catalog)?;
    if job.recipe != recipe.key
        || job.recipe_version != recipe.version
        || job.inputs != recipe.inputs
        || job.output_item != recipe.output_item
        || job.output_count != recipe.output_count
        || job.work_ticks_total != recipe.work_ticks
        || job.station != recipe.station
        || job.world_revision != recipe.world_revision
    {
        return Err(ItemError::InvalidRecipe);
    }
    Ok(())
}

fn remove_craft_inputs(
    container: &mut ItemContainer,
    inputs: &[CraftRequirement],
) -> Result<Vec<CraftRequirement>, ItemError> {
    let mut reserved = Vec::new();
    reserved
        .try_reserve_exact(inputs.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    for input in inputs {
        let mut remaining = input.count;
        for slot in &mut container.slots {
            if let Some(ItemLot::Stack { item, count }) = slot
                && *item == input.item
                && remaining > 0
            {
                let removed = remaining.min(*count);
                *count -= removed;
                remaining -= removed;
                if *count == 0 {
                    *slot = None;
                }
            }
        }
        if remaining > 0 {
            return Err(ItemError::MissingItem);
        }
        reserved.push(*input);
    }
    Ok(reserved)
}

#[cfg(test)]
fn count_item(container: &ItemContainer, item: StableKey) -> u64 {
    container
        .slots
        .iter()
        .flatten()
        .filter_map(|lot| match lot {
            ItemLot::Stack { item: key, count } if *key == item => Some(u64::from(*count)),
            ItemLot::Instance(instance) if instance.item == item => Some(1),
            _ => None,
        })
        .sum()
}

fn query_item_count(
    container: &ItemContainer,
    item: StableKey,
    catalog: &ItemCatalog,
) -> Result<u64, ItemError> {
    let result = ItemQuery::new(Some(item), None)?.execute(
        &[container],
        catalog,
        container.limits.slots as usize,
    )?;
    if result.truncated {
        return Err(ItemError::CapacityExceeded);
    }
    result.matches.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(u64::from(entry.count))
            .ok_or(ItemError::CapacityExceeded)
    })
}

fn requirements_are_canonical(requirements: &[CraftRequirement]) -> bool {
    !requirements.is_empty()
        && requirements.len() <= 64
        && requirements.iter().all(|item| item.count > 0)
        && requirements
            .windows(2)
            .all(|pair| pair[0].item < pair[1].item)
}

fn insert_receipt(
    receipts: &mut Vec<StableKey>,
    receipt: StableKey,
    receipt_capacity: u32,
) -> Result<(), ItemError> {
    if receipts.len() >= receipt_capacity as usize {
        return Err(ItemError::ReceiptCapacity);
    }
    let index = match receipts.binary_search(&receipt) {
        Ok(_) => return Err(ItemError::DuplicateReceipt),
        Err(index) => index,
    };
    receipts
        .try_reserve(1)
        .map_err(|_| ItemError::AllocationFailed)?;
    receipts.insert(index, receipt);
    Ok(())
}

fn try_clone_keys(keys: &[StableKey]) -> Result<Vec<StableKey>, ItemError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(keys.len())
        .map_err(|_| ItemError::AllocationFailed)?;
    copy.extend_from_slice(keys);
    Ok(copy)
}

fn encode_craft_job(job: &CraftJob, out: &mut Encoder) -> EngineResult<()> {
    job.recipe.encode(out)?;
    out.u32(job.recipe_version)?;
    job.receipt.encode(out)?;
    out.u32(u32::try_from(job.inputs.len()).map_err(|_| invalid_state())?)?;
    for input in &job.inputs {
        input.item.encode(out)?;
        out.u32(input.count)?;
    }
    job.output_item.encode(out)?;
    out.u32(job.output_count)?;
    out.u64(job.work_ticks_total)?;
    out.u64(job.work_ticks_remaining)?;
    out.u64(job.last_tick)?;
    match job.station {
        None => out.u8(0)?,
        Some(station) => {
            out.u8(1)?;
            station.encode(out)?;
        }
    }
    match job.world_revision {
        None => out.u8(0)?,
        Some(revision) => {
            out.u8(1)?;
            out.u64(revision)?;
        }
    }
    Ok(())
}

fn decode_craft_job(input: &mut Decoder<'_>) -> EngineResult<CraftJob> {
    let recipe = StableKey::decode(input)?;
    let recipe_version = input.u32()?;
    let receipt = StableKey::decode(input)?;
    let input_count = input.u32()? as usize;
    if input_count == 0 || input_count > 64 {
        return Err(invalid_state());
    }
    let mut inputs = Vec::new();
    inputs
        .try_reserve_exact(input_count)
        .map_err(|_| allocation_error())?;
    for _ in 0..input_count {
        let requirement = CraftRequirement {
            item: StableKey::decode(input)?,
            count: input.u32()?,
        };
        if requirement.count == 0
            || inputs
                .last()
                .is_some_and(|last: &CraftRequirement| last.item >= requirement.item)
        {
            return Err(invalid_state());
        }
        inputs.push(requirement);
    }
    let output_item = StableKey::decode(input)?;
    let output_count = input.u32()?;
    let work_ticks_total = input.u64()?;
    let work_ticks_remaining = input.u64()?;
    let last_tick = input.u64()?;
    let station = match input.u8()? {
        0 => None,
        1 => Some(StableKey::decode(input)?),
        _ => return Err(invalid_state()),
    };
    let world_revision = match input.u8()? {
        0 => None,
        1 => Some(input.u64()?),
        _ => return Err(invalid_state()),
    };
    if recipe_version == 0 || output_count == 0 || work_ticks_remaining > work_ticks_total {
        return Err(invalid_state());
    }
    Ok(CraftJob {
        recipe,
        recipe_version,
        receipt,
        inputs,
        output_item,
        output_count,
        work_ticks_total,
        work_ticks_remaining,
        last_tick,
        station,
        world_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn catalog() -> ItemCatalog {
        ItemCatalog::new(vec![
            ItemDefinition::new(
                key("test:ore"),
                10,
                0,
                2,
                Some(key("test:ore-image")),
                None,
                None,
                vec![key("test:material"), key("test:craftable")],
            )
            .unwrap(),
            ItemDefinition::new(
                key("test:sword"),
                1,
                8,
                5,
                None,
                Some(key("test:sword-use")),
                Some(key("test:hand")),
                vec![],
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn item_instance_registry_indexes_and_rejects_duplicates_across_owners() {
        let catalog = catalog();
        let limits = ItemContainerLimits {
            slots: 4,
            payload_bytes: 64,
            weight_micros: 100,
        };
        let registry_limits = ItemInstanceRegistryLimits {
            owners: 4,
            instances: 4,
        };
        let mut player = ItemContainer::new(key("test:player"), limits).unwrap();
        let mut chest = ItemContainer::new(key("test:chest"), limits).unwrap();
        let equipment = EquipmentLoadout::new(
            key("test:equipment"),
            vec![key("test:hand")],
            EquipmentLimits {
                payload_bytes: 64,
                weight_micros: 100,
            },
        )
        .unwrap();
        let unique = ItemInstance {
            id: key("test:sword-001"),
            item: key("test:sword"),
            state: vec![3, 4],
        };
        player
            .deposit(ItemLot::Instance(Box::new(unique.clone())), 0, &catalog)
            .unwrap();
        let mut registry = ItemInstanceRegistry::new(key("test:world-scope"));
        assert_eq!(
            registry.refresh(
                0,
                &[&player, &chest],
                &[&equipment],
                &catalog,
                registry_limits,
            ),
            Ok(1)
        );
        assert_eq!(
            registry.owners(),
            &[ItemInstanceOwner {
                instance: unique.id,
                owner: player.key(),
                slot: ItemInstanceOwnerSlot::Container(0),
            }]
        );
        registry
            .validate_ownership(
                key("test:world-scope"),
                &[&player, &chest],
                &[&equipment],
                &catalog,
                registry_limits,
            )
            .unwrap();

        chest
            .deposit(ItemLot::Instance(Box::new(unique)), 0, &catalog)
            .unwrap();
        assert_eq!(
            registry.refresh(
                1,
                &[&player, &chest],
                &[&equipment],
                &catalog,
                registry_limits,
            ),
            Err(ItemError::DuplicateInstance(Box::new(key(
                "test:sword-001"
            ))))
        );
        assert_eq!(registry.revision(), 1);
        assert_eq!(registry.owners().len(), 1);
    }
    fn craft_recipe(work_ticks: u64) -> CraftRecipe {
        CraftRecipe::new(
            key("test:ore-ingot"),
            3,
            vec![
                CraftRequirement {
                    item: key("test:ore"),
                    count: 3,
                },
                CraftRequirement {
                    item: key("test:ore"),
                    count: 2,
                },
            ],
            vec![CraftRequirement {
                item: key("test:sword"),
                count: 1,
            }],
            key("test:ore"),
            2,
            work_ticks,
            Some(key("test:forge")),
            Some(77),
        )
        .unwrap()
    }
    fn context() -> CraftContext {
        CraftContext {
            station: Some(key("test:forge")),
            world_revision: 77,
        }
    }
    fn craft_session(catalog: &ItemCatalog) -> CraftSession {
        let mut container =
            ItemContainer::new(key("test:craft-inventory"), limits(4, 100)).unwrap();
        container.slots[0] = Some(ItemLot::Stack {
            item: key("test:ore"),
            count: 8,
        });
        container.slots[1] = Some(ItemLot::Instance(Box::new(ItemInstance {
            id: key("test:tool-sword"),
            item: key("test:sword"),
            state: vec![4],
        })));
        CraftSession::new(container, catalog, 128).unwrap()
    }
    fn limits(slots: u32, weight_micros: u64) -> ItemContainerLimits {
        ItemContainerLimits {
            slots,
            payload_bytes: 32,
            weight_micros,
        }
    }

    #[test]
    fn cross_container_split_is_atomic_and_advances_both_revisions() {
        let catalog = catalog();
        let mut source = ItemContainer::new(key("test:player-bag"), limits(2, 100)).unwrap();
        source.slots[0] = Some(ItemLot::Stack {
            item: key("test:ore"),
            count: 8,
        });
        let mut destination = ItemContainer::new(key("test:chest"), limits(2, 100)).unwrap();
        destination.slots[0] = Some(ItemLot::Stack {
            item: key("test:ore"),
            count: 3,
        });

        source
            .transfer_to(&mut destination, 0, 4, 0, 0, &catalog)
            .unwrap();
        assert_eq!(
            source.slots[0],
            Some(ItemLot::Stack {
                item: key("test:ore"),
                count: 4
            })
        );
        assert_eq!(
            destination.slots[0],
            Some(ItemLot::Stack {
                item: key("test:ore"),
                count: 7
            })
        );
        assert_eq!((source.revision(), destination.revision()), (1, 1));

        let before_source = source.clone();
        let before_destination = destination.clone();
        assert_eq!(
            source.transfer_to(&mut destination, 0, 1, 0, 1, &catalog),
            Err(ItemError::StaleRevision)
        );
        assert_eq!(source, before_source);
        assert_eq!(destination, before_destination);
    }

    #[test]
    fn weight_failure_keeps_both_containers_unchanged() {
        let catalog = catalog();
        let mut source = ItemContainer::new(key("test:player-bag"), limits(2, 100)).unwrap();
        source.slots[0] = Some(ItemLot::Stack {
            item: key("test:ore"),
            count: 4,
        });
        let mut destination = ItemContainer::new(key("test:chest"), limits(1, 5)).unwrap();
        let before_source = source.clone();
        let before_destination = destination.clone();

        assert_eq!(
            source.transfer_to(&mut destination, 0, 3, 0, 0, &catalog),
            Err(ItemError::CapacityExceeded)
        );
        assert_eq!(source, before_source);
        assert_eq!(destination, before_destination);
    }

    #[test]
    fn duplicate_instance_across_transfer_boundary_is_rejected_atomically() {
        let catalog = catalog();
        let mut source = ItemContainer::new(key("test:source-bag"), limits(2, 100)).unwrap();
        let instance = ItemInstance {
            id: key("test:sword-instance-1"),
            item: key("test:sword"),
            state: vec![1],
        };
        source.slots[0] = Some(ItemLot::Instance(Box::new(
            try_clone_instance(&instance).unwrap(),
        )));
        let mut destination =
            ItemContainer::new(key("test:destination-chest"), limits(2, 100)).unwrap();
        destination.slots[0] = Some(ItemLot::Instance(Box::new(instance)));
        let before_source = source.clone();
        let before_destination = destination.clone();

        assert_eq!(
            source.transfer_to(&mut destination, 0, 1, 0, 0, &catalog),
            Err(ItemError::DuplicateInstance(Box::new(key(
                "test:sword-instance-1"
            ))))
        );
        assert_eq!(source, before_source);
        assert_eq!(destination, before_destination);
    }

    #[test]
    fn unique_item_state_moves_and_roundtrips_through_codec() {
        let catalog = catalog();
        let mut source = ItemContainer::new(key("test:player-bag"), limits(2, 100)).unwrap();
        source.slots[0] = Some(ItemLot::Instance(Box::new(ItemInstance {
            id: key("test:sword-instance-1"),
            item: key("test:sword"),
            state: vec![9, 7],
        })));
        let mut destination = ItemContainer::new(key("test:chest"), limits(1, 100)).unwrap();
        source
            .transfer_to(&mut destination, 0, 1, 0, 0, &catalog)
            .unwrap();
        catalog.validate_container(&destination).unwrap();

        let mut encoder = Encoder::new(4096);
        destination.encode(&mut encoder).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        let restored = ItemContainer::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(restored, destination);
        catalog.validate_container(&restored).unwrap();
        assert_eq!(
            restored.validate_limits(limits(1, 101)),
            Err(ItemError::InvalidContainer)
        );
    }

    #[test]
    fn author_attributes_are_key_sorted_and_duplicate_keys_are_rejected() {
        let definition =
            ItemDefinition::new(key("test:custom-item"), 1, 0, 0, None, None, None, vec![])
                .unwrap()
                .with_author_attributes(vec![
                    ItemAttribute {
                        key: key("test:zeta"),
                        value: vec![2],
                    },
                    ItemAttribute {
                        key: key("test:alpha"),
                        value: vec![1],
                    },
                ])
                .unwrap();
        assert_eq!(definition.author_attributes()[0].key, key("test:alpha"));
        assert_eq!(definition.author_attributes()[1].key, key("test:zeta"));
        assert_eq!(
            ItemDefinition::new(
                key("test:duplicate-attributes"),
                1,
                0,
                0,
                None,
                None,
                None,
                vec![],
            )
            .unwrap()
            .with_author_attributes(vec![
                ItemAttribute {
                    key: key("test:same"),
                    value: vec![],
                },
                ItemAttribute {
                    key: key("test:same"),
                    value: vec![],
                },
            ]),
            Err(ItemError::InvalidDefinition)
        );
    }

    #[test]
    fn codec_rejects_invalid_stack_count_and_duplicate_instance_ids() {
        let mut encoder = Encoder::new(4096);
        encoder.u16(CONTAINER_CODEC_VERSION).unwrap();
        key("test:container").encode(&mut encoder).unwrap();
        encoder.u64(0).unwrap();
        encoder.u32(1).unwrap();
        encoder.u64(8).unwrap();
        encoder.u64(8).unwrap();
        encoder.u32(1).unwrap();
        encoder.u8(1).unwrap();
        key("test:ore").encode(&mut encoder).unwrap();
        encoder.u32(0).unwrap();
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        assert!(ItemContainer::decode(&mut decoder).is_err());

        let mut encoder = Encoder::new(4096);
        encoder.u16(CONTAINER_CODEC_VERSION).unwrap();
        key("test:container").encode(&mut encoder).unwrap();
        encoder.u64(0).unwrap();
        encoder.u32(2).unwrap();
        encoder.u64(8).unwrap();
        encoder.u64(8).unwrap();
        encoder.u32(2).unwrap();
        for _ in 0..2 {
            encoder.u8(2).unwrap();
            key("test:sword").encode(&mut encoder).unwrap();
            key("test:sword-instance-duplicate")
                .encode(&mut encoder)
                .unwrap();
            encoder.bytes(&[1]).unwrap();
        }
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes, 4096).unwrap();
        assert!(ItemContainer::decode(&mut decoder).is_err());
    }

    #[test]
    fn crafting_aggregates_inputs_preserves_tool_and_claims_exactly_once() {
        let catalog = catalog();
        let recipe = craft_recipe(3);
        let receipt = key("test:craft-receipt-1");
        let mut session = craft_session(&catalog);
        assert_eq!(
            session.validate_receipt_capacity(127),
            Err(ItemError::InvalidCraftCapacity)
        );
        assert_eq!(
            session
                .start(&recipe, receipt, 10, context(), 0, &catalog)
                .unwrap(),
            CraftProgress::Running { remaining_ticks: 3 }
        );
        assert_eq!(count_item(&session.container, key("test:ore")), 3);
        assert_eq!(count_item(&session.container, key("test:sword")), 1);
        assert_eq!(
            session
                .advance_to(&recipe, 11, context(), &catalog)
                .unwrap(),
            CraftProgress::Running { remaining_ticks: 2 }
        );
        assert_eq!(
            session.advance_to(&recipe, 10, context(), &catalog),
            Err(ItemError::StaleCraftTick)
        );
        let before = session.clone();
        assert_eq!(
            session.claim(&recipe, receipt, 1, &catalog),
            Err(ItemError::CraftNotReady)
        );
        assert_eq!(session, before);
        assert_eq!(
            session
                .advance_to(&recipe, 13, context(), &catalog)
                .unwrap(),
            CraftProgress::Ready
        );
        let wrong_recipe = CraftRecipe::new(
            key("test:ore-ingot"),
            4,
            vec![CraftRequirement {
                item: key("test:ore"),
                count: 5,
            }],
            vec![CraftRequirement {
                item: key("test:sword"),
                count: 1,
            }],
            key("test:ore"),
            2,
            3,
            Some(key("test:forge")),
            Some(77),
        )
        .unwrap();
        let before_wrong_claim = session.clone();
        assert_eq!(
            session.claim(&wrong_recipe, receipt, 1, &catalog),
            Err(ItemError::InvalidRecipe)
        );
        assert_eq!(session, before_wrong_claim);
        session.claim(&recipe, receipt, 1, &catalog).unwrap();
        assert_eq!(count_item(&session.container, key("test:ore")), 5);
        assert_eq!(count_item(&session.container, key("test:sword")), 1);
        assert!(session.contains_receipt(receipt));
        assert_eq!(
            session.claim(&recipe, receipt, 2, &catalog),
            Err(ItemError::UnknownReceipt)
        );
        assert_eq!(
            session.start(&recipe, receipt, 13, context(), 3, &catalog),
            Err(ItemError::DuplicateReceipt)
        );
        session.validate(&catalog).unwrap();
    }

    #[test]
    fn craft_start_does_not_double_count_consumed_items_as_tools() {
        let items = catalog();
        let recipe = CraftRecipe::new(
            key("test:ore-with-ore-tool"),
            1,
            vec![CraftRequirement {
                item: key("test:ore"),
                count: 2,
            }],
            vec![CraftRequirement {
                item: key("test:ore"),
                count: 1,
            }],
            key("test:ore"),
            1,
            2,
            None,
            None,
        )
        .unwrap();
        let mut container = ItemContainer::new(key("test:ore-only-bag"), limits(2, 100)).unwrap();
        container.slots[0] = Some(ItemLot::Stack {
            item: key("test:ore"),
            count: 2,
        });
        let mut session = CraftSession::new(container, &items, 8).unwrap();
        let before = session.clone();

        assert_eq!(
            session.start(
                &recipe,
                key("test:missing-ore-tool-receipt"),
                0,
                CraftContext {
                    station: None,
                    world_revision: 0,
                },
                0,
                &items,
            ),
            Err(ItemError::MissingTool(Box::new(key("test:ore"))))
        );
        assert_eq!(session, before);
    }

    #[test]
    fn registered_craft_catalog_resolves_versions_across_saved_work() {
        let items = catalog();
        let recipe = craft_recipe(2);
        let recipe_key = recipe.key();
        let recipes = CraftCatalog::new(vec![recipe], &items).unwrap();
        let mut session = craft_session(&items);
        let receipt = key("test:registered-craft-receipt");

        assert_eq!(
            session.start_registered(&recipes, recipe_key, 3, receipt, 8, context(), 0, &items,),
            Ok(CraftProgress::Running { remaining_ticks: 2 })
        );
        assert_eq!(
            session.start_registered(
                &recipes,
                recipe_key,
                2,
                key("test:wrong-recipe-version"),
                8,
                context(),
                1,
                &items,
            ),
            Err(ItemError::InvalidRecipe)
        );

        let mut encoded = Encoder::new(32_768);
        session.encode(&mut encoded).unwrap();
        let bytes = encoded.finish();
        let mut decoder = Decoder::new(&bytes, 32_768).unwrap();
        let mut restored = CraftSession::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(
            restored.advance_registered(&recipes, 10, context(), &items),
            Ok(CraftProgress::Ready)
        );
        restored
            .claim_registered(&recipes, receipt, 1, &items)
            .unwrap();
        assert!(restored.contains_receipt(receipt));
        assert_eq!(
            restored.claim_registered(&recipes, receipt, 2, &items),
            Err(ItemError::UnknownReceipt)
        );

        assert_eq!(
            CraftCatalog::new(vec![craft_recipe(1), craft_recipe(1),], &items,),
            Err(ItemError::InvalidRecipe)
        );
        assert_eq!(
            recipes.get(key("test:missing-recipe")),
            Err(ItemError::UnknownRecipe(Box::new(key(
                "test:missing-recipe"
            ))))
        );
    }

    #[test]
    fn failed_conditions_and_cancel_keep_material_receipt_atomic() {
        let catalog = catalog();
        let recipe = craft_recipe(5);
        let receipt = key("test:craft-receipt-cancel");
        let mut session = craft_session(&catalog);
        let before = session.clone();
        let wrong_context = CraftContext {
            station: Some(key("test:wrong-forge")),
            world_revision: 77,
        };
        assert_eq!(
            session.start(&recipe, receipt, 1, wrong_context, 0, &catalog),
            Err(ItemError::InvalidCraftContext)
        );
        assert_eq!(session, before);
        session
            .start(&recipe, receipt, 1, context(), 0, &catalog)
            .unwrap();
        assert_eq!(count_item(&session.container, key("test:ore")), 3);
        session.cancel(receipt, 1, &catalog).unwrap();
        assert_eq!(count_item(&session.container, key("test:ore")), 8);
        assert!(session.contains_receipt(receipt));
        assert_eq!(session.progress(), None);
        session.validate(&catalog).unwrap();
    }

    #[test]
    fn unavailable_craft_context_time_is_not_counted_after_resume() {
        let items = catalog();
        let recipe = craft_recipe(5);
        let mut session = craft_session(&items);
        let receipt = key("test:craft-paused-context");
        session
            .start(&recipe, receipt, 1, context(), 0, &items)
            .unwrap();

        let unavailable = CraftContext {
            station: Some(key("test:wrong-forge")),
            world_revision: 77,
        };
        assert_eq!(
            session.advance_to(&recipe, 8, unavailable, &items),
            Err(ItemError::InvalidCraftContext)
        );
        assert_eq!(
            session.progress(),
            Some(CraftProgress::Running { remaining_ticks: 5 })
        );
        let mut encoded = Encoder::new(32_768);
        session.encode(&mut encoded).unwrap();
        let bytes = encoded.finish();
        let mut decoder = Decoder::new(&bytes, 32_768).unwrap();
        let mut session = CraftSession::decode(&mut decoder).unwrap();
        decoder.finish().unwrap();
        assert_eq!(
            session.advance_to(&recipe, 9, context(), &items),
            Ok(CraftProgress::Running { remaining_ticks: 4 })
        );
        assert_eq!(
            session.advance_to(&recipe, 10, context(), &items),
            Ok(CraftProgress::Running { remaining_ticks: 3 })
        );
    }

    #[test]
    fn crafted_output_is_split_across_stack_limited_slots() {
        let catalog = catalog();
        let recipe = CraftRecipe::new(
            key("test:bulk-recipe"),
            1,
            vec![CraftRequirement {
                item: key("test:ore"),
                count: 5,
            }],
            vec![],
            key("test:ore"),
            25,
            0,
            None,
            None,
        )
        .unwrap();
        let receipt = key("test:bulk-receipt");
        let mut session = craft_session(&catalog);
        assert_eq!(
            session
                .start(
                    &recipe,
                    receipt,
                    0,
                    CraftContext {
                        station: None,
                        world_revision: 0,
                    },
                    0,
                    &catalog,
                )
                .unwrap(),
            CraftProgress::Ready
        );
        session.claim(&recipe, receipt, 1, &catalog).unwrap();
        assert_eq!(count_item(&session.container, key("test:ore")), 28);
        assert_eq!(session.container.slots().iter().flatten().count(), 4);
    }
}
