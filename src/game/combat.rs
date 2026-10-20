//! 作者が登録する属性、攻撃hit、命中免疫とdamage候補を接続します。
//!
//! collision queryと対象選別はcallerが所有し、このmoduleは確定候補を原子的に解決します。
//! 既定の免疫はAttackId・target・hit windowを使いますが、作者providerで発射者やprojectile
//! group等へ置換できます。death、drop、World編集はresolverが返す候補をcaller transactionへ接続します。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::state::{StateStore, Transaction};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey, id::BodyId};
use crate::world::{
    BodyPose, ColliderShape, CollisionQueriesProvider, CollisionQueryGeometry, CollisionQueryHit,
    CollisionQueryRequest, CollisionQueryStatus, LayerId, World, WorldEdit,
};
use std::cmp::Ordering;

/// 登録済みAttributeKeyの範囲と初期値です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttributeDefinition {
    /// 作者が登録する安定keyです。
    pub key: StableKey,
    /// 作成時の値です。
    pub initial: f32,
    /// 有効な下限です。
    pub minimum: f32,
    /// 有効な上限です。
    pub maximum: f32,
}

impl AttributeDefinition {
    /// 有限な初期値と上下限を確認します。
    pub fn new(
        key: StableKey,
        initial: f32,
        minimum: f32,
        maximum: f32,
    ) -> Result<Self, CombatError> {
        if !initial.is_finite()
            || !minimum.is_finite()
            || !maximum.is_finite()
            || minimum > maximum
            || initial < minimum
            || initial > maximum
        {
            return Err(CombatError::InvalidAttribute);
        }
        Ok(Self {
            key,
            initial,
            minimum,
            maximum,
        })
    }
}

/// key順のAttribute定義catalogです。
#[derive(Clone, Debug, Default)]
pub struct AttributeCatalog {
    definitions: Vec<AttributeDefinition>,
}

impl AttributeCatalog {
    /// 重複keyを拒否して定義を整列します。
    pub fn new(mut definitions: Vec<AttributeDefinition>) -> Result<Self, CombatError> {
        if definitions.iter().any(|definition| {
            !definition.initial.is_finite()
                || !definition.minimum.is_finite()
                || !definition.maximum.is_finite()
                || definition.minimum > definition.maximum
                || definition.initial < definition.minimum
                || definition.initial > definition.maximum
        }) {
            return Err(CombatError::InvalidAttribute);
        }
        definitions.sort_unstable_by_key(|definition| definition.key);
        if definitions
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(CombatError::DuplicateAttribute);
        }
        Ok(Self { definitions })
    }

    /// 登録定義をkeyで取得します。
    pub fn get(&self, key: StableKey) -> Result<&AttributeDefinition, CombatError> {
        self.definitions
            .binary_search_by_key(&key, |definition| definition.key)
            .map(|index| &self.definitions[index])
            .map_err(|_| CombatError::UnknownAttribute(Box::new(key)))
    }

    /// Entity用の初期attribute stateを作ります。
    pub fn initial_state(&self, keys: &[StableKey]) -> Result<AttributeState, CombatError> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(keys.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        values.extend(keys.iter().map(|key| AttributeValue {
            key: *key,
            value: 0.0,
        }));
        values.sort_unstable_by_key(|value| value.key);
        if values.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(CombatError::DuplicateAttribute);
        }
        for value in &mut values {
            value.value = self.get(value.key)?.initial;
        }
        Ok(AttributeState { values })
    }

    /// 復元または登録境界で全valueが定義済みkeyと範囲内にあるか検証します。
    pub fn validate_state(&self, state: &AttributeState) -> Result<(), CombatError> {
        if state
            .values
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(CombatError::InvalidAttribute);
        }
        for value in &state.values {
            let definition = self.get(value.key)?;
            if !value.value.is_finite()
                || value.value < definition.minimum
                || value.value > definition.maximum
            {
                return Err(CombatError::InvalidAttribute);
            }
        }
        Ok(())
    }

    /// 作者resource transaction用に、範囲検査済みのattribute候補を作ります。
    ///
    /// 入力変更はkey順へ正規化し、同一keyの複数変更を拒否します。候補はStateStore等の
    /// 外側transactionが成功するまで正規stateへ公開されません。作業領域はattribute数と
    /// `changes`数に比例し、`maximum_changes`で作者が一回の更新量を制限します。
    pub fn apply_changes(
        &self,
        state: &AttributeState,
        changes: &[AttributeChange],
        maximum_changes: u32,
    ) -> Result<AttributeState, CombatError> {
        self.validate_state(state)?;
        if changes.len() > maximum_changes as usize {
            return Err(CombatError::CapacityExceeded);
        }
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(changes.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        ordered.extend_from_slice(changes);
        ordered.sort_unstable_by_key(|change| change.key);
        if ordered.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(CombatError::DuplicateAttribute);
        }
        let mut candidate = state.try_clone()?;
        for change in &ordered {
            let definition = self.get(change.key)?;
            if !change.value.is_finite()
                || change.value < definition.minimum
                || change.value > definition.maximum
            {
                return Err(CombatError::InvalidAttribute);
            }
            candidate.set(change.key, change.value)?;
        }
        self.validate_state(&candidate)?;
        Ok(candidate)
    }
}

/// 一つのEntityまたは共有HP poolが所有する属性値です。
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeState {
    values: Vec<AttributeValue>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AttributeValue {
    key: StableKey,
    value: f32,
}

impl AttributeState {
    /// 値を読み取ります。登録はあるがこのstateに存在しない属性は欠損として返します。
    pub fn get(&self, key: StableKey) -> Option<f32> {
        self.values
            .binary_search_by_key(&key, |value| value.key)
            .ok()
            .map(|index| self.values[index].value)
    }

    /// stable key順で読み取り専用の値を列挙します。
    pub fn iter(&self) -> impl Iterator<Item = (StableKey, f32)> + '_ {
        self.values.iter().map(|value| (value.key, value.value))
    }

    fn try_clone(&self) -> Result<Self, CombatError> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(self.values.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        values.extend_from_slice(&self.values);
        Ok(Self { values })
    }

    fn set(&mut self, key: StableKey, value: f32) -> Result<(), CombatError> {
        let index = self
            .values
            .binary_search_by_key(&key, |entry| entry.key)
            .map_err(|_| CombatError::UnknownAttribute(Box::new(key)))?;
        self.values[index].value = value;
        Ok(())
    }
}

impl StateCodec for AttributeState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(u32::try_from(self.values.len()).map_err(|_| invalid_state())?)?;
        let mut previous = None;
        for value in &self.values {
            if !value.value.is_finite() || previous.is_some_and(|key| key >= value.key) {
                return Err(invalid_state());
            }
            previous = Some(value.key);
            value.key.encode(out)?;
            out.u32(value.value.to_bits())?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let count = input.u32()? as usize;
        if count > input.remaining() / 11 {
            return Err(invalid_state());
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let key = StableKey::decode(input)?;
            let value = f32::from_bits(input.u32()?);
            if !value.is_finite()
                || values
                    .last()
                    .is_some_and(|last: &AttributeValue| last.key >= key)
            {
                return Err(invalid_state());
            }
            values.push(AttributeValue { key, value });
        }
        Ok(Self { values })
    }
}

/// 攻撃queryへ渡すlocal spaceのshapeです。
#[derive(Clone, Debug, PartialEq)]
pub enum AttackShape {
    /// 半径で指定する円です。
    Circle { radius: f32 },
    /// 中心からの半幅・半高さで指定する軸整列boxです。
    Aabb { half_extents: [f32; 2] },
    /// 半高の中心線と両端の半円で定義するcapsuleです。
    Capsule { half_height: f32, radius: f32 },
    /// 作者がcodecと厳密queryを提供する任意形状です。
    ///
    /// 稀な拡張shapeのprovider keyを間接格納し、標準shapeを含むenumの固定サイズを抑えます。
    Custom {
        provider: Box<StableKey>,
        version: u32,
        geometry: Vec<u8>,
    },
}

impl AttackShape {
    fn validate(&self, max_geometry_bytes: u32) -> Result<(), CombatError> {
        match self {
            Self::Circle { radius } if radius.is_finite() && *radius > 0.0 => Ok(()),
            Self::Aabb { half_extents }
                if half_extents
                    .iter()
                    .all(|extent| extent.is_finite() && *extent > 0.0) =>
            {
                Ok(())
            }
            Self::Capsule {
                half_height,
                radius,
            } if half_height.is_finite()
                && *half_height >= 0.0
                && radius.is_finite()
                && *radius > 0.0 =>
            {
                Ok(())
            }
            Self::Custom {
                provider,
                version,
                geometry,
            } if !geometry.is_empty() && geometry.len() <= max_geometry_bytes as usize => {
                validate_provider(**provider, *version)?;
                Ok(())
            }
            _ => Err(CombatError::InvalidAttack),
        }
    }
}

/// 作者が登録するattack shape、hit group、damage規則です。
#[derive(Clone, Debug, PartialEq)]
pub struct AttackDefinition {
    key: StableKey,
    version: u32,
    shape: AttackShape,
    query_provider: StableKey,
    query_version: u32,
    hit_group: StableKey,
    damage_kind: StableKey,
    raw_damage: f32,
    hit_window: u64,
}

impl AttackDefinition {
    /// shape・query provider・damage値を登録境界で検証します。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: StableKey,
        version: u32,
        shape: AttackShape,
        query_provider: StableKey,
        query_version: u32,
        hit_group: StableKey,
        damage_kind: StableKey,
        raw_damage: f32,
        hit_window: u64,
    ) -> Result<Self, CombatError> {
        shape.validate(u32::MAX)?;
        if version == 0 || query_version == 0 || !raw_damage.is_finite() || raw_damage < 0.0 {
            return Err(CombatError::InvalidAttack);
        }
        Ok(Self {
            key,
            version,
            shape,
            query_provider,
            query_version,
            hit_group,
            damage_kind,
            raw_damage,
            hit_window,
        })
    }

    /// 作者登録keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 保存されたactive attackと照合するdefinition版です。
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// 重複keyを拒否し、攻撃定義をstable順に引くcatalogです。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttackCatalogLimits {
    /// 登録可能な攻撃定義数です。
    pub definitions: u32,
    /// custom shape一件あたりの最大codec byte数です。
    pub shape_bytes: u32,
    /// 一回のqueryで要求できる最大contact件数です。
    pub contacts_per_query: u32,
}

impl AttackCatalogLimits {
    /// 作者がcontent登録時に上限を固定します。
    pub fn new(definitions: u32, shape_bytes: u32, contacts_per_query: u32) -> Self {
        Self {
            definitions,
            shape_bytes,
            contacts_per_query,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AttackCatalog {
    definitions: Vec<AttackDefinition>,
    limits: AttackCatalogLimits,
}

impl AttackCatalog {
    /// author attackをkey順に登録します。
    pub fn new(
        mut definitions: Vec<AttackDefinition>,
        limits: AttackCatalogLimits,
    ) -> Result<Self, CombatError> {
        if definitions.len() > limits.definitions as usize {
            return Err(CombatError::CapacityExceeded);
        }
        for definition in &definitions {
            definition.shape.validate(limits.shape_bytes)?;
            if definition.version == 0
                || definition.query_version == 0
                || !definition.raw_damage.is_finite()
                || definition.raw_damage < 0.0
            {
                return Err(CombatError::InvalidAttack);
            }
        }
        definitions.sort_unstable_by_key(|definition| definition.key);
        if definitions
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(CombatError::DuplicateAttack);
        }
        Ok(Self {
            definitions,
            limits,
        })
    }

    /// 登録済みattackをkeyで取得します。
    pub fn get(&self, key: StableKey) -> Result<&AttackDefinition, CombatError> {
        self.definitions
            .binary_search_by_key(&key, |definition| definition.key)
            .map(|index| &self.definitions[index])
            .map_err(|_| CombatError::UnknownAttack(Box::new(key)))
    }

    /// 登録providerの完全性statusを保持し、正確な結果だけから安定順hitを作ります。
    ///
    /// providerはshape/cell精密判定、遮蔽、対象条件を適用し、`target_limit`以下のcontactを
    /// 返します。Hit結果はtarget/substep順へ正規化し、同じtarget/substepの多重接触を拒否します。
    #[allow(clippy::too_many_arguments)] // 攻撃queryの対象、発生元、時刻、上限を一つの呼び出し境界で固定します。
    pub fn query_hits(
        &self,
        attack: StableKey,
        attack_id: StableKey,
        source: StableKey,
        origin: [f32; 2],
        tick: u64,
        target_limit: u32,
        query: &impl AttackSpatialQuery,
    ) -> Result<CombatQueryResult, CombatError> {
        let definition = self.get(attack)?;
        if !origin[0].is_finite() || !origin[1].is_finite() {
            return Err(CombatError::InvalidHit);
        }
        if target_limit > self.limits.contacts_per_query {
            return Err(CombatError::CapacityExceeded);
        }
        if query.key() != definition.query_provider || query.version() != definition.query_version {
            return Err(CombatError::InvalidProvider);
        }
        let result = query.query(&definition.shape, source, origin, tick, target_limit)?;
        if result.contacts.len() > target_limit as usize {
            return Err(CombatError::CapacityExceeded);
        }
        if result.status != AttackQueryStatus::Hit {
            if matches!(
                result.status,
                AttackQueryStatus::NoHit | AttackQueryStatus::Unsupported
            ) && !result.contacts.is_empty()
            {
                return Err(CombatError::InvalidQueryResult);
            }
            return Ok(CombatQueryResult {
                status: result.status,
                hits: Vec::new(),
            });
        }
        if result.contacts.is_empty()
            || result
                .contacts
                .iter()
                .any(|contact| !contact.impulse[0].is_finite() || !contact.impulse[1].is_finite())
        {
            return Err(CombatError::InvalidQueryResult);
        }
        let mut contacts = result.contacts;
        contacts.sort_unstable_by_key(|contact| (contact.target, contact.substep));
        if contacts
            .windows(2)
            .any(|pair| pair[0].target == pair[1].target && pair[0].substep == pair[1].substep)
        {
            return Err(CombatError::DuplicateTarget);
        }
        let mut hits = Vec::new();
        hits.try_reserve_exact(contacts.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        hits.extend(contacts.into_iter().map(|contact| CombatHit {
            attack: attack_id,
            source,
            target: contact.target,
            hit_group: definition.hit_group,
            hit_window: definition.hit_window,
            damage_kind: definition.damage_kind,
            raw_damage: definition.raw_damage,
            impulse: contact.impulse,
            tick,
            substep: contact.substep,
        }));
        Ok(CombatQueryResult {
            status: AttackQueryStatus::Hit,
            hits,
        })
    }
}

/// 精密collision backendまたは作者target systemが返す一接触です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackContact {
    /// 命中対象の安定keyです。
    pub target: StableKey,
    /// query/solver由来の有限impulse候補です。
    pub impulse: [f32; 2],
    /// 発生したlogical tick内の物理substepです。
    pub substep: u32,
}

/// 攻撃shape queryが正確な結果を返せたかを表します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackQueryStatus {
    /// 少なくとも一件以上の確定contactが得られました。
    Hit,
    /// query範囲を完全に検査し、contactがありませんでした。
    NoHit,
    /// 一部候補を得ましたが、query全体の正確性を保証できません。
    Incomplete,
    /// 登録shapeまたはquery種別を処理できません。
    Unsupported,
}

/// 作者queryが返すstatus付きcontact列です。
#[derive(Clone, Debug, PartialEq)]
pub struct AttackQueryResult {
    /// 完全性を示すstatusです。
    pub status: AttackQueryStatus,
    /// Incompleteのcontactは診断専用であり、確定hitへ昇格しません。
    pub contacts: Vec<AttackContact>,
}

/// 正確なcombat hitまたはquery未完了状態です。
#[derive(Clone, Debug, PartialEq)]
pub struct CombatQueryResult {
    /// CollisionQueriesの完全性statusです。
    pub status: AttackQueryStatus,
    /// Hitの時だけstable順で格納されます。
    pub hits: Vec<CombatHit>,
}

/// Engineまたは作者が登録する厳密shape/cell collision queryです。
pub trait AttackSpatialQuery {
    /// 実装のstable keyです。
    fn key(&self) -> StableKey;
    /// 同じkey内の意味版です。
    fn version(&self) -> u32;
    /// 最大件数以内でshape接触と完全性statusを返します。
    fn query(
        &self,
        shape: &AttackShape,
        source: StableKey,
        origin: [f32; 2],
        tick: u64,
        target_limit: u32,
    ) -> Result<AttackQueryResult, CombatError>;
}

/// World CollisionQueriesを使う標準attack overlap queryの設定です。
#[derive(Clone, Debug)]
pub struct WorldAttackQueryOptions {
    /// query対象のWorld layerです。
    pub layer: LayerId,
    /// providerが解釈するcollision group maskです。
    pub collision_groups: u32,
    /// self geometryなど、query対象から外す剛体です。
    pub ignore_bodies: Vec<BodyId>,
    /// one-way surfaceの向きです。
    pub one_way_direction: Option<[f64; 2]>,
    /// sensorとの接触を含めます。
    pub include_sensors: bool,
    /// query開始時に既に重なっている対象を含めます。
    pub include_initial_overlap: bool,
    /// 境界接触を含めます。
    pub include_boundary: bool,
    /// providerへ渡すgeometry epsilonです。
    pub epsilon: f64,
    /// CombatHitへ記録する物理substepです。
    pub substep: u32,
}

impl WorldAttackQueryOptions {
    /// World layerを受け取り、overlap attackに使う既定設定を作ります。
    pub fn new(layer: LayerId) -> Self {
        Self {
            layer,
            collision_groups: u32::MAX,
            ignore_bodies: Vec::new(),
            one_way_direction: None,
            include_sensors: false,
            include_initial_overlap: true,
            include_boundary: true,
            epsilon: 0.0,
            substep: 0,
        }
    }
}

/// World collision hitを作者の安定Entity IDと衝撃候補へ解決します。
pub trait WorldAttackTargetResolver {
    /// 非攻撃対象なら`None`を返し、衝撃量はゲーム規則に沿って計算します。
    fn resolve(
        &self,
        source: StableKey,
        hit: &CollisionQueryHit,
    ) -> Result<Option<(StableKey, [f32; 2])>, CombatError>;
}

/// Worldの標準query contractと攻撃対象registryを接続するoverlap queryです。
///
/// Circle/Aabb/CapsuleをWorld CollisionQueriesへ渡します。任意shapeは作者の
/// `AttackSpatialQuery`で処理し、未対応の標準queryを確定hitとして扱いません。
pub struct WorldAttackQuery<'a> {
    world: &'a World,
    provider: &'a dyn CollisionQueriesProvider,
    target_resolver: &'a dyn WorldAttackTargetResolver,
    options: WorldAttackQueryOptions,
}

impl<'a> WorldAttackQuery<'a> {
    /// immutable World/providerと作者Entity resolverからquery adapterを作ります。
    pub fn new(
        world: &'a World,
        provider: &'a dyn CollisionQueriesProvider,
        target_resolver: &'a dyn WorldAttackTargetResolver,
        options: WorldAttackQueryOptions,
    ) -> Self {
        Self {
            world,
            provider,
            target_resolver,
            options,
        }
    }
}

fn attack_shape_to_collider(shape: &AttackShape) -> Option<ColliderShape> {
    match shape {
        AttackShape::Circle { radius } => Some(ColliderShape::Circle {
            radius: f64::from(*radius),
        }),
        AttackShape::Aabb { half_extents } => Some(ColliderShape::Cuboid {
            half_extents: [f64::from(half_extents[0]), f64::from(half_extents[1])],
        }),
        AttackShape::Capsule {
            half_height,
            radius,
        } => Some(ColliderShape::Capsule {
            start: [0.0, -f64::from(*half_height)],
            end: [0.0, f64::from(*half_height)],
            radius: f64::from(*radius),
        }),
        AttackShape::Custom { .. } => None,
    }
}

impl AttackSpatialQuery for WorldAttackQuery<'_> {
    fn key(&self) -> StableKey {
        self.provider.key()
    }

    fn version(&self) -> u32 {
        self.provider.version()
    }

    fn query(
        &self,
        shape: &AttackShape,
        source: StableKey,
        origin: [f32; 2],
        tick: u64,
        target_limit: u32,
    ) -> Result<AttackQueryResult, CombatError> {
        if target_limit == 0 || !origin[0].is_finite() || !origin[1].is_finite() {
            return Err(CombatError::InvalidQueryResult);
        }
        let collider = match attack_shape_to_collider(shape) {
            Some(collider) => collider,
            None => {
                return Ok(AttackQueryResult {
                    status: AttackQueryStatus::Unsupported,
                    contacts: Vec::new(),
                });
            }
        };
        let mut ignored = Vec::new();
        ignored
            .try_reserve_exact(self.options.ignore_bodies.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        ignored.extend_from_slice(&self.options.ignore_bodies);
        let request = CollisionQueryRequest {
            geometry: CollisionQueryGeometry::Overlap {
                shape: collider,
                pose: BodyPose {
                    x: f64::from(origin[0]),
                    y: f64::from(origin[1]),
                    angle: 0.0,
                },
            },
            layer: self.options.layer,
            collision_groups: self.options.collision_groups,
            owner: Some(source),
            ignore_bodies: ignored,
            one_way_direction: self.options.one_way_direction,
            include_sensors: self.options.include_sensors,
            include_initial_overlap: self.options.include_initial_overlap,
            include_boundary: self.options.include_boundary,
            epsilon: self.options.epsilon,
            tick,
            substep: self.options.substep,
            read_latest_candidate: false,
            first_hit_only: false,
            stable_order: true,
            max_hits: target_limit,
        };
        let report = self
            .world
            .query_body_collisions(&request, self.provider)
            .map_err(|error| CombatError::WorldQueryFailed(error.code))?;
        let status = match report.result.status {
            CollisionQueryStatus::Hit if report.result.truncated => AttackQueryStatus::Incomplete,
            CollisionQueryStatus::Hit => AttackQueryStatus::Hit,
            CollisionQueryStatus::NoHit => AttackQueryStatus::NoHit,
            CollisionQueryStatus::Incomplete => AttackQueryStatus::Incomplete,
            CollisionQueryStatus::Unsupported => AttackQueryStatus::Unsupported,
        };
        if status != AttackQueryStatus::Hit {
            return Ok(AttackQueryResult {
                status,
                contacts: Vec::new(),
            });
        }
        let mut contacts = Vec::new();
        contacts
            .try_reserve_exact(report.result.hits.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        for hit in &report.result.hits {
            if let Some((target, impulse)) = self.target_resolver.resolve(source, hit)? {
                contacts.push(AttackContact {
                    target,
                    impulse,
                    substep: self.options.substep,
                });
            }
        }
        Ok(AttackQueryResult {
            status: if contacts.is_empty() {
                AttackQueryStatus::NoHit
            } else {
                AttackQueryStatus::Hit
            },
            contacts,
        })
    }
}

/// Query/target systemから確定候補として渡す一回の攻撃接触です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatHit {
    /// 攻撃発生ごとに異なる安定IDです。
    pub attack: StableKey,
    /// owner filterと作者ルールに使うsource identityです。
    pub source: StableKey,
    /// 命中対象または共有poolの安定keyです。
    pub target: StableKey,
    /// authored hit groupです。
    pub hit_group: StableKey,
    /// 作者が定義する免疫windowです。tick固定である必要はありません。
    pub hit_window: u64,
    /// Attribute/効果耐性が参照するdamage種別です。
    pub damage_kind: StableKey,
    /// resolverへ渡す未処理damage量です。
    pub raw_damage: f32,
    /// query/solverから得た衝撃候補です。
    pub impulse: [f32; 2],
    /// hitが生成されたlogical tickです。
    pub tick: u64,
    /// 同一tick内のsubstepです。
    pub substep: u32,
}

impl CombatHit {
    fn validate(&self) -> Result<(), CombatError> {
        if !self.raw_damage.is_finite()
            || self.raw_damage < 0.0
            || !self.impulse[0].is_finite()
            || !self.impulse[1].is_finite()
        {
            return Err(CombatError::InvalidHit);
        }
        Ok(())
    }
}

/// 命中免疫providerが作るauthor policy固有のdedupe tokenです。
pub trait HitImmunityProvider {
    /// 保存済みtokenへ紐付けるprovider keyです。
    fn key(&self) -> StableKey;
    /// keyの意味を変えた際に進めるversionです。
    fn version(&self) -> u32;
    /// Hit履歴の免疫範囲を定義する安定tokenを返します。
    fn token(&self, hit: &CombatHit) -> Result<Vec<u8>, CombatError>;
}

/// AttackId × target × hit_windowで重複を抑止する標準免疫です。
pub struct AttackTargetWindowImmunity;

impl HitImmunityProvider for AttackTargetWindowImmunity {
    fn key(&self) -> StableKey {
        StableKey::parse("engine:attack-target-window").expect("定数keyは有効")
    }
    fn version(&self) -> u32 {
        1
    }
    fn token(&self, hit: &CombatHit) -> Result<Vec<u8>, CombatError> {
        let mut out = Encoder::new(512);
        hit.attack
            .encode(&mut out)
            .map_err(|_| CombatError::AllocationFailed)?;
        hit.target
            .encode(&mut out)
            .map_err(|_| CombatError::AllocationFailed)?;
        out.u64(hit.hit_window)
            .map_err(|_| CombatError::AllocationFailed)?;
        Ok(out.finish())
    }
}

/// HitResolutionが候補更新する属性です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttributeChange {
    /// 更新対象の登録済みAttributeKeyです。
    pub key: StableKey,
    /// 確定後の値です。範囲外はEngineが拒否します。
    pub value: f32,
}

/// resolverがWorld/effect runtimeへ渡す非正規副作用候補です。
#[derive(Clone, Debug, PartialEq)]
pub struct HitResolution {
    /// target/shared poolの候補attribute更新です。
    pub attributes: Vec<AttributeChange>,
    /// callerが同じtransactionでdispatchするeffect triggerです。
    pub effects: Vec<StableKey>,
    /// 移動serviceへ渡す追加knockbackです。
    pub knockback: Option<[f32; 2]>,
}

/// armor、resistance、部位共有HP、反射、貫通等を定義する置換可能なresolverです。
pub trait HitResolutionProvider {
    /// 保存/replay記録へ残すresolver keyです。
    fn key(&self) -> StableKey;
    /// resolver意味版です。
    fn version(&self) -> u32;
    /// 読取専用stateから全副作用を候補として返します。
    fn resolve(
        &self,
        hit: &CombatHit,
        attributes: &AttributeState,
        limits: HitLimits,
    ) -> Result<HitResolution, CombatError>;
}

/// callback結果と保存候補の上限です。設定はsaveから引き上げません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitLimits {
    /// 永続免疫recordの最大件数です。
    pub immunity_records: u32,
    /// providerが返すtoken一件の最大byteです。
    pub token_bytes: u32,
    /// 一hitで変更できるattribute数です。
    pub attribute_changes: u32,
    /// 一hitで発行できるeffect trigger数です。
    pub effects: u32,
}

/// provider key/versionごとに分離した保存済み免疫tokenです。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ImmunityRecord {
    provider: StableKey,
    version: u32,
    token: Vec<u8>,
}

impl Ord for ImmunityRecord {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.provider, self.version, &self.token).cmp(&(
            other.provider,
            other.version,
            &other.token,
        ))
    }
}
impl PartialOrd for ImmunityRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 命中済みtokenを所有するcheckpoint stateです。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitLedger {
    records: Vec<ImmunityRecord>,
}

impl HitLedger {
    /// 保存済みtoken件数です。
    pub fn len(&self) -> usize {
        self.records.len()
    }
    /// tokenが一件もないかを返します。
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// restore後/config更新後に呼出側の上限で全履歴を検証します。
    pub fn validate_limits(&self, limits: HitLimits) -> Result<(), CombatError> {
        if self.records.len() > limits.immunity_records as usize
            || self.records.windows(2).any(|pair| pair[0] >= pair[1])
            || self.records.iter().any(|record| {
                record.version == 0 || record.token.len() > limits.token_bytes as usize
            })
        {
            return Err(CombatError::CapacityExceeded);
        }
        Ok(())
    }

    fn contains(&self, provider: StableKey, version: u32, token: &[u8]) -> bool {
        self.records
            .binary_search_by(|record| {
                (record.provider, record.version, record.token.as_slice())
                    .cmp(&(provider, version, token))
            })
            .is_ok()
    }

    fn remove(&mut self, provider: StableKey, version: u32, token: &[u8]) -> bool {
        let index = self
            .records
            .binary_search_by(|record| {
                (record.provider, record.version, record.token.as_slice())
                    .cmp(&(provider, version, token))
            })
            .ok();
        if let Some(index) = index {
            self.records.remove(index);
            true
        } else {
            false
        }
    }

    fn insert(&mut self, record: ImmunityRecord) -> Result<(), CombatError> {
        let index = match self.records.binary_search(&record) {
            Ok(_) => return Ok(()),
            Err(index) => index,
        };
        self.records
            .try_reserve(1)
            .map_err(|_| CombatError::AllocationFailed)?;
        self.records.insert(index, record);
        Ok(())
    }

    fn try_clone(&self) -> Result<Self, CombatError> {
        let mut records = Vec::new();
        records
            .try_reserve_exact(self.records.len())
            .map_err(|_| CombatError::AllocationFailed)?;
        for record in &self.records {
            let mut token = Vec::new();
            token
                .try_reserve_exact(record.token.len())
                .map_err(|_| CombatError::AllocationFailed)?;
            token.extend_from_slice(&record.token);
            records.push(ImmunityRecord {
                provider: record.provider,
                version: record.version,
                token,
            });
        }
        Ok(Self { records })
    }
}

impl StateCodec for HitLedger {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(u32::try_from(self.records.len()).map_err(|_| invalid_state())?)?;
        let mut previous: Option<&ImmunityRecord> = None;
        for record in &self.records {
            if record.version == 0 || previous.is_some_and(|value| value >= record) {
                return Err(invalid_state());
            }
            previous = Some(record);
            record.provider.encode(out)?;
            out.u32(record.version)?;
            out.bytes(&record.token)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let count = input.u32()? as usize;
        if count > input.remaining() / 15 {
            return Err(invalid_state());
        }
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        for _ in 0..count {
            let provider = StableKey::decode(input)?;
            let version = input.u32()?;
            let token = copy_bytes(input.bytes(u32::MAX as usize)?)?;
            if version == 0 {
                return Err(invalid_state());
            }
            let record = ImmunityRecord {
                provider,
                version,
                token,
            };
            if records.last().is_some_and(|last| last >= &record) {
                return Err(invalid_state());
            }
            records.push(record);
        }
        Ok(Self { records })
    }
}

/// 正規attribute、命中免疫履歴、更新revisionを一つの保存stateとして所有します。
#[derive(Clone, Debug, PartialEq)]
pub struct CombatState {
    attributes: AttributeState,
    ledger: HitLedger,
    limits: HitLimits,
    revision: u64,
}

impl CombatState {
    /// 作者指定attributeと履歴上限から空のcombat stateを作ります。
    pub fn new(attributes: AttributeState, limits: HitLimits) -> Result<Self, CombatError> {
        Ok(Self {
            attributes,
            ledger: HitLedger::default(),
            limits,
            revision: 0,
        })
    }

    /// 現在のstate revisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// 読み取り専用attribute viewです。
    pub fn attributes(&self) -> &AttributeState {
        &self.attributes
    }
    /// 読み取り専用hit ledgerです。
    pub fn ledger(&self) -> &HitLedger {
        &self.ledger
    }

    /// restore後にattribute定義とcaller-owned履歴上限を照合します。
    pub fn validate(
        &self,
        catalog: &AttributeCatalog,
        limits: HitLimits,
    ) -> Result<(), CombatError> {
        catalog.validate_state(&self.attributes)?;
        if self.limits != limits {
            return Err(CombatError::InvalidLimits);
        }
        self.ledger.validate_limits(limits)
    }

    /// 作者がhit windowの終了を確定したとき、その免疫tokenを解放します。
    ///
    /// tokenの範囲と寿命はproviderと呼出側が所有します。長寿命Worldで履歴が無制限に
    /// 蓄積しないよう、攻撃instanceの終了後に同じhit情報で明示的に解放します。未登録の
    /// tokenを解放した場合は`false`で状態revisionも変わりません。
    pub fn release_hit_window(
        &mut self,
        hit: &CombatHit,
        immunity: &impl HitImmunityProvider,
        catalog: &AttributeCatalog,
        limits: HitLimits,
    ) -> Result<bool, CombatError> {
        self.validate(catalog, limits)?;
        hit.validate()?;
        validate_provider(immunity.key(), immunity.version())?;
        let token = immunity.token(hit)?;
        if token.len() > limits.token_bytes as usize {
            return Err(CombatError::CapacityExceeded);
        }
        if !self
            .ledger
            .contains(immunity.key(), immunity.version(), &token)
        {
            return Ok(false);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(CombatError::RevisionOverflow)?;
        let removed = self
            .ledger
            .remove(immunity.key(), immunity.version(), &token);
        debug_assert!(removed);
        self.revision = revision;
        Ok(true)
    }

    /// Hitを解決候補へ変換します。免疫duplicateは`Ok(None)`で副作用なく返します。
    pub fn prepare_hit(
        &self,
        hit: CombatHit,
        immunity: &impl HitImmunityProvider,
        resolver: &impl HitResolutionProvider,
        catalog: &AttributeCatalog,
        limits: HitLimits,
    ) -> Result<Option<CombatCandidate>, CombatError> {
        self.validate(catalog, limits)?;
        hit.validate()?;
        validate_provider(immunity.key(), immunity.version())?;
        validate_provider(resolver.key(), resolver.version())?;
        let token = immunity.token(&hit)?;
        if token.len() > limits.token_bytes as usize {
            return Err(CombatError::CapacityExceeded);
        }
        if self
            .ledger
            .contains(immunity.key(), immunity.version(), &token)
        {
            return Ok(None);
        }
        if self.ledger.len() >= limits.immunity_records as usize {
            return Err(CombatError::CapacityExceeded);
        }
        let resolution = resolver.resolve(&hit, &self.attributes, limits)?;
        if resolution.attributes.len() > limits.attribute_changes as usize
            || resolution.effects.len() > limits.effects as usize
            || resolution
                .knockback
                .is_some_and(|value| !value[0].is_finite() || !value[1].is_finite())
        {
            return Err(CombatError::CapacityExceeded);
        }
        let mut changes = resolution.attributes;
        changes.sort_unstable_by_key(|change| change.key);
        if changes.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(CombatError::DuplicateAttribute);
        }
        let mut attributes = self.attributes.try_clone()?;
        for change in &changes {
            let definition = catalog.get(change.key)?;
            if !change.value.is_finite()
                || change.value < definition.minimum
                || change.value > definition.maximum
            {
                return Err(CombatError::InvalidAttribute);
            }
            attributes.set(change.key, change.value)?;
        }
        let mut ledger = self.ledger.try_clone()?;
        ledger.insert(ImmunityRecord {
            provider: immunity.key(),
            version: immunity.version(),
            token,
        })?;
        ledger.validate_limits(limits)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(CombatError::RevisionOverflow)?;
        Ok(Some(CombatCandidate {
            base_revision: self.revision,
            candidate: Self {
                attributes,
                ledger,
                limits,
                revision,
            },
            report: CombatHitReport {
                hit,
                resolver: resolver.key(),
                resolver_version: resolver.version(),
                resolution: HitResolution {
                    attributes: changes,
                    effects: resolution.effects,
                    knockback: resolution.knockback,
                },
            },
        }))
    }
}

impl StateCodec for CombatState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u16(1)?;
        out.u64(self.revision)?;
        out.u32(self.limits.immunity_records)?;
        out.u32(self.limits.token_bytes)?;
        out.u32(self.limits.attribute_changes)?;
        out.u32(self.limits.effects)?;
        self.attributes.encode(out)?;
        self.ledger.encode(out)
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.u16()? != 1 {
            return Err(invalid_state());
        }
        Ok(Self {
            revision: input.u64()?,
            limits: HitLimits {
                immunity_records: input.u32()?,
                token_bytes: input.u32()?,
                attribute_changes: input.u32()?,
                effects: input.u32()?,
            },
            attributes: AttributeState::decode(input)?,
            ledger: HitLedger::decode(input)?,
        })
    }
}

/// External transactionが受け取るcandidate stateと副作用reportです。
pub struct CombatCandidate {
    base_revision: u64,
    candidate: CombatState,
    report: CombatHitReport,
}

impl CombatCandidate {
    /// candidateのattribute・ledger・revisionを読み取ります。
    pub fn state(&self) -> &CombatState {
        &self.candidate
    }
    /// resolverが返したeffect/knockback候補です。
    pub fn report(&self) -> &CombatHitReport {
        &self.report
    }

    /// callerのWorld/StateStore transactionが成功した後だけcombat stateを公開します。
    pub fn commit_with<E>(
        self,
        target: &mut CombatState,
        commit: impl FnOnce(&CombatState, &CombatHitReport) -> Result<(), E>,
    ) -> Result<CombatHitReport, CombatCommitError<E>> {
        if target.revision != self.base_revision {
            return Err(CombatCommitError::StaleCandidate);
        }
        commit(&self.candidate, &self.report).map_err(CombatCommitError::External)?;
        *target = self.candidate;
        Ok(self.report)
    }

    /// combat state、作者resource、World commandを同じ成功境界で公開します。
    ///
    /// 先にcombat候補をStateStore transactionへ符号化し、作者resourceの更新後にtransactionが
    /// commit可能なことを検査します。続いて独立したWorld候補を編集し、Worldが成功した場合だけ
    /// StateStoreとcombat stateを公開します。失敗したcandidateは全て破棄され、callerが同じhitを
    /// 再度prepareして再試行できます。callbackのglobal state変更や外部I/Oはrollback対象外です。
    #[allow(clippy::too_many_arguments)]
    pub fn commit_with_world_and_state<R>(
        self,
        target: &mut CombatState,
        world: &mut World,
        state: &mut StateStore,
        combat_key: StableKey,
        combat_schema: StableKey,
        update_state: impl FnOnce(
            &CombatState,
            &CombatHitReport,
            &mut Transaction<'_>,
        ) -> EngineResult<()>,
        update_world: impl FnOnce(&CombatHitReport, &mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> Result<(R, CombatHitReport), CombatTransactionError> {
        if target.revision != self.base_revision {
            return Err(CombatTransactionError::StaleCandidate);
        }

        let mut state_candidate = state.transaction().map_err(CombatTransactionError::State)?;
        state_candidate
            .put(combat_key, combat_schema, &self.candidate)
            .map_err(CombatTransactionError::State)?;
        update_state(&self.candidate, &self.report, &mut state_candidate)
            .map_err(CombatTransactionError::State)?;
        state_candidate
            .validate_ready()
            .map_err(CombatTransactionError::State)?;

        let result = world
            .edit(|editor| update_world(&self.report, editor))
            .map_err(CombatTransactionError::World)?;

        // readiness検査済みかつexclusive borrow中なので、ここでは外部callbackや追加encodeを行わない。
        state_candidate
            .commit()
            .map_err(CombatTransactionError::State)?;
        let report = self.report;
        *target = self.candidate;
        Ok((result, report))
    }
}

/// Combat、作者state、Worldを束ねる取引で起きた失敗境界です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatTransactionError {
    /// 同じrevisionから作成したcandidateが既にcommitされています。
    StaleCandidate,
    /// combat/resource recordの候補作成または保存に失敗しました。
    State(EngineError),
    /// World command候補の検証・commitに失敗しました。
    World(EngineError),
}

impl std::fmt::Display for CombatTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "combat取引に失敗しました: {self:?}")
    }
}

impl std::error::Error for CombatTransactionError {}

/// combat candidate commitで起きるstaleまたはcaller transaction失敗です。
#[derive(Debug, PartialEq, Eq)]
pub enum CombatCommitError<E> {
    StaleCandidate,
    External(E),
}

/// HitResolverの確定後に記録する安定reportです。
#[derive(Clone, Debug, PartialEq)]
pub struct CombatHitReport {
    /// 解決対象hitです。
    pub hit: CombatHit,
    /// 実行したresolver key/versionです。
    pub resolver: StableKey,
    /// 実行したresolver版です。
    pub resolver_version: u32,
    /// callerへ渡す属性・effect・knockback候補です。
    pub resolution: HitResolution,
}

/// combat処理での定義・容量・競合失敗です。
///
/// 診断キーは`StableKey`本体が大きいため、失敗経路だけでBoxへ確保します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatError {
    InvalidLimits,
    InvalidAttack,
    DuplicateAttack,
    UnknownAttack(Box<StableKey>),
    DuplicateTarget,
    InvalidAttribute,
    DuplicateAttribute,
    UnknownAttribute(Box<StableKey>),
    InvalidHit,
    InvalidProvider,
    InvalidQueryResult,
    CapacityExceeded,
    AllocationFailed,
    RevisionOverflow,
    WorldQueryFailed(ErrorCode),
}

impl std::fmt::Display for CombatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for CombatError {}

fn validate_provider(key: StableKey, version: u32) -> Result<(), CombatError> {
    if key.as_str().is_empty() || version == 0 {
        Err(CombatError::InvalidProvider)
    } else {
        Ok(())
    }
}

fn invalid_state() -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, "combat保存状態")
}

fn allocation_error() -> EngineError {
    EngineError::new(ErrorCode::CapacityExceeded, "combat state確保")
}

fn copy_bytes(bytes: &[u8]) -> EngineResult<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| allocation_error())?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn catalog() -> AttributeCatalog {
        AttributeCatalog::new(vec![
            AttributeDefinition::new(key("test:health"), 100.0, 0.0, 100.0).unwrap(),
            AttributeDefinition::new(key("test:shield"), 10.0, 0.0, 20.0).unwrap(),
        ])
        .unwrap()
    }
    fn limits() -> HitLimits {
        HitLimits {
            immunity_records: 32,
            token_bytes: 128,
            attribute_changes: 4,
            effects: 4,
        }
    }
    fn hit() -> CombatHit {
        CombatHit {
            attack: key("test:attack-1"),
            source: key("test:player"),
            target: key("test:enemy"),
            hit_group: key("test:projectile"),
            hit_window: 3,
            damage_kind: key("test:fire"),
            raw_damage: 12.0,
            impulse: [1.0, 0.5],
            tick: 9,
            substep: 1,
        }
    }

    struct AttackQuery;
    impl AttackSpatialQuery for AttackQuery {
        fn key(&self) -> StableKey {
            key("test:attack-shape-query")
        }
        fn version(&self) -> u32 {
            3
        }
        fn query(
            &self,
            shape: &AttackShape,
            _: StableKey,
            _: [f32; 2],
            _: u64,
            _: u32,
        ) -> Result<AttackQueryResult, CombatError> {
            assert!(matches!(shape, AttackShape::Circle { radius } if *radius == 1.5));
            Ok(AttackQueryResult {
                status: AttackQueryStatus::Hit,
                contacts: vec![
                    AttackContact {
                        target: key("test:z-enemy"),
                        impulse: [0.0, -1.0],
                        substep: 2,
                    },
                    AttackContact {
                        target: key("test:a-enemy"),
                        impulse: [1.0, 0.0],
                        substep: 1,
                    },
                ],
            })
        }
    }

    struct PartialAttackQuery;
    impl AttackSpatialQuery for PartialAttackQuery {
        fn key(&self) -> StableKey {
            key("test:attack-shape-query")
        }
        fn version(&self) -> u32 {
            3
        }
        fn query(
            &self,
            _: &AttackShape,
            _: StableKey,
            _: [f32; 2],
            _: u64,
            _: u32,
        ) -> Result<AttackQueryResult, CombatError> {
            Ok(AttackQueryResult {
                status: AttackQueryStatus::Incomplete,
                contacts: vec![AttackContact {
                    target: key("test:partial-target"),
                    impulse: [1.0, 0.0],
                    substep: 0,
                }],
            })
        }
    }

    fn attack_definition() -> AttackDefinition {
        AttackDefinition::new(
            key("test:slash"),
            2,
            AttackShape::Circle { radius: 1.5 },
            key("test:attack-shape-query"),
            3,
            key("test:blade-group"),
            key("test:physical"),
            7.5,
            4,
        )
        .unwrap()
    }

    #[test]
    fn registered_attack_uses_exact_query_and_stably_builds_hits() {
        let definition = attack_definition();
        let catalog = AttackCatalog::new(
            vec![definition.clone()],
            AttackCatalogLimits::new(8, 4096, 8),
        )
        .unwrap();
        let hits = catalog
            .query_hits(
                definition.key(),
                key("test:slash-instance"),
                key("test:player"),
                [3.0, 2.0],
                12,
                4,
                &AttackQuery,
            )
            .unwrap();
        assert_eq!(hits.status, AttackQueryStatus::Hit);
        assert_eq!(hits.hits.len(), 2);
        assert_eq!(hits.hits[0].target, key("test:a-enemy"));
        assert_eq!(hits.hits[1].target, key("test:z-enemy"));
        assert!(hits.hits.iter().all(|hit| {
            hit.attack == key("test:slash-instance")
                && hit.hit_group == key("test:blade-group")
                && hit.damage_kind == key("test:physical")
                && hit.raw_damage == 7.5
                && hit.hit_window == 4
                && hit.tick == 12
        }));
        assert_eq!(
            catalog.query_hits(
                definition.key(),
                key("test:slash-instance"),
                key("test:player"),
                [0.0, 0.0],
                12,
                1,
                &AttackQuery,
            ),
            Err(CombatError::CapacityExceeded)
        );
    }

    #[test]
    fn incomplete_attack_query_never_promotes_partial_contacts_to_hits() {
        let definition = attack_definition();
        let catalog = AttackCatalog::new(
            vec![definition.clone()],
            AttackCatalogLimits::new(8, 4096, 8),
        )
        .unwrap();
        let result = catalog
            .query_hits(
                definition.key(),
                key("test:slash-instance"),
                key("test:player"),
                [0.0, 0.0],
                12,
                4,
                &PartialAttackQuery,
            )
            .unwrap();
        assert_eq!(result.status, AttackQueryStatus::Incomplete);
        assert!(result.hits.is_empty());
    }

    #[test]
    fn attack_registration_rejects_invalid_geometry_and_duplicate_target_contact() {
        assert!(
            AttackDefinition::new(
                key("test:bad-attack"),
                1,
                AttackShape::Circle { radius: f32::NAN },
                key("test:attack-shape-query"),
                1,
                key("test:group"),
                key("test:physical"),
                1.0,
                0,
            )
            .is_err()
        );
        let definition = attack_definition();
        assert!(matches!(
            AttackCatalog::new(
                vec![definition.clone(), definition],
                AttackCatalogLimits::new(8, 4096, 8),
            ),
            Err(CombatError::DuplicateAttack)
        ));
    }

    struct Damage;
    impl HitResolutionProvider for Damage {
        fn key(&self) -> StableKey {
            key("test:damage-resolver")
        }
        fn version(&self) -> u32 {
            1
        }
        fn resolve(
            &self,
            hit: &CombatHit,
            attributes: &AttributeState,
            _: HitLimits,
        ) -> Result<HitResolution, CombatError> {
            let shield = attributes
                .get(key("test:shield"))
                .ok_or(CombatError::UnknownAttribute(Box::new(key("test:shield"))))?;
            let absorbed = shield.min(hit.raw_damage);
            Ok(HitResolution {
                attributes: vec![
                    AttributeChange {
                        key: key("test:health"),
                        value: attributes.get(key("test:health")).unwrap()
                            - (hit.raw_damage - absorbed),
                    },
                    AttributeChange {
                        key: key("test:shield"),
                        value: shield - absorbed,
                    },
                ],
                effects: vec![key("test:on-hit")],
                knockback: Some([2.0, 0.5]),
            })
        }
    }

    #[test]
    fn default_hit_window_suppresses_duplicate_and_custom_policy_separates_source() {
        let catalog = catalog();
        let attributes = catalog
            .initial_state(&[key("test:health"), key("test:shield")])
            .unwrap();
        let mut state = CombatState::new(attributes, limits()).unwrap();
        let immunity = AttackTargetWindowImmunity;
        let resolver = Damage;
        let candidate = state
            .prepare_hit(hit(), &immunity, &resolver, &catalog, limits())
            .unwrap()
            .unwrap();
        let report = candidate
            .commit_with(&mut state, |candidate, report| {
                assert_eq!(candidate.attributes().get(key("test:health")), Some(98.0));
                assert_eq!(candidate.attributes().get(key("test:shield")), Some(0.0));
                assert_eq!(report.resolution.effects, vec![key("test:on-hit")]);
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(report.resolver, key("test:damage-resolver"));
        assert!(
            state
                .prepare_hit(hit(), &immunity, &resolver, &catalog, limits())
                .unwrap()
                .is_none()
        );
        let revision = state.revision();
        assert!(
            state
                .release_hit_window(&hit(), &immunity, &catalog, limits())
                .unwrap()
        );
        assert_eq!(state.revision(), revision + 1);
        assert!(
            state
                .prepare_hit(hit(), &immunity, &resolver, &catalog, limits())
                .unwrap()
                .is_some()
        );

        struct PerSource;
        impl HitImmunityProvider for PerSource {
            fn key(&self) -> StableKey {
                key("test:per-source-immunity")
            }
            fn version(&self) -> u32 {
                1
            }
            fn token(&self, hit: &CombatHit) -> Result<Vec<u8>, CombatError> {
                let mut out = Encoder::new(512);
                hit.source
                    .encode(&mut out)
                    .map_err(|_| CombatError::AllocationFailed)?;
                hit.target
                    .encode(&mut out)
                    .map_err(|_| CombatError::AllocationFailed)?;
                hit.hit_group
                    .encode(&mut out)
                    .map_err(|_| CombatError::AllocationFailed)?;
                out.u64(hit.hit_window)
                    .map_err(|_| CombatError::AllocationFailed)?;
                Ok(out.finish())
            }
        }
        let per_source = PerSource;
        let mut other_attack = hit();
        other_attack.attack = key("test:attack-2");
        assert!(
            state
                .prepare_hit(other_attack, &per_source, &resolver, &catalog, limits())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn resolver_failure_or_external_rollback_keeps_attributes_and_immunity_atomic() {
        let catalog = catalog();
        let attributes = catalog
            .initial_state(&[key("test:health"), key("test:shield")])
            .unwrap();
        let mut state = CombatState::new(attributes, limits()).unwrap();
        let before = state.clone();
        let candidate = state
            .prepare_hit(
                hit(),
                &AttackTargetWindowImmunity,
                &Damage,
                &catalog,
                limits(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            candidate.commit_with(&mut state, |_, _| Err::<(), _>("transaction failed")),
            Err(CombatCommitError::External("transaction failed"))
        );
        assert_eq!(state, before);
    }

    #[test]
    fn world_attack_query_converts_standard_shapes_and_leaves_custom_geometry_to_authors() {
        assert_eq!(
            attack_shape_to_collider(&AttackShape::Circle { radius: 1.5 }),
            Some(ColliderShape::Circle { radius: 1.5 })
        );
        assert_eq!(
            attack_shape_to_collider(&AttackShape::Aabb {
                half_extents: [2.0, 3.0],
            }),
            Some(ColliderShape::Cuboid {
                half_extents: [2.0, 3.0],
            })
        );
        assert_eq!(
            attack_shape_to_collider(&AttackShape::Capsule {
                half_height: 4.0,
                radius: 0.5,
            }),
            Some(ColliderShape::Capsule {
                start: [0.0, -4.0],
                end: [0.0, 4.0],
                radius: 0.5,
            })
        );
        assert_eq!(
            attack_shape_to_collider(&AttackShape::Custom {
                provider: Box::new(key("test:custom-attack-shape")),
                version: 1,
                geometry: vec![1],
            }),
            None
        );
    }

    #[test]
    fn attribute_resource_changes_are_bounded_and_return_an_unpublished_candidate() {
        let catalog = catalog();
        let state = catalog
            .initial_state(&[key("test:health"), key("test:shield")])
            .unwrap();
        let candidate = catalog
            .apply_changes(
                &state,
                &[
                    AttributeChange {
                        key: key("test:shield"),
                        value: 14.0,
                    },
                    AttributeChange {
                        key: key("test:health"),
                        value: 82.0,
                    },
                ],
                2,
            )
            .unwrap();
        assert_eq!(state.get(key("test:health")), Some(100.0));
        assert_eq!(state.get(key("test:shield")), Some(10.0));
        assert_eq!(candidate.get(key("test:health")), Some(82.0));
        assert_eq!(candidate.get(key("test:shield")), Some(14.0));
        assert_eq!(
            catalog.apply_changes(
                &state,
                &[
                    AttributeChange {
                        key: key("test:health"),
                        value: 80.0,
                    },
                    AttributeChange {
                        key: key("test:health"),
                        value: 70.0,
                    },
                ],
                2,
            ),
            Err(CombatError::DuplicateAttribute)
        );
        assert_eq!(
            catalog.apply_changes(
                &state,
                &[AttributeChange {
                    key: key("test:health"),
                    value: 80.0,
                }],
                0,
            ),
            Err(CombatError::CapacityExceeded)
        );
    }
}
