//! 配置用の素材・layer定義。登録順を実行時IDへ漏らしません。
//!
//! builderは作者の入力を保持し、registerで寸法・物性・形状を検証します。
//! freeze後の定義は不変であり、複数Worldと保存候補が共有できます。

use super::invalid;
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
};
use std::sync::Arc;

/// 素材定義に対する永続参照。freeze前にも使え、連番には依存しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaterialHandle(pub StableKey);

/// freezeした同じContent内でだけ使う実行時ID。0は空で、不明な参照とは別です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaterialId(pub(crate) u32);
impl MaterialId {
    /// 空cellの予約値です。
    pub const AIR: Self = Self(0);
    /// 診断・batch向けの値。別Contentへの参照変換は安定キーを使います。
    pub fn index(self) -> u32 {
        self.0
    }
}

/// 標準solverへ接続する基礎分類。ゲーム内の具体的な素材名ではありません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialBase {
    /// 固体。
    Solid,
    /// 粉体。
    Powder,
    /// 液体。
    Liquid,
    /// 気体。
    Gas,
}
/// 一回の破壊で除去する単位です。壊せるかどうかのゲーム規則は作者が判断します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakMode {
    /// 指定cellだけ除去し、個体の残存maskを更新します。
    Cell,
    /// 同じ個体の残存cellを一括除去します。
    Block,
}

/// footprint内の占有と、後続の衝突providerへ渡す形状の定義です。
// 定義はfreeze時に一度作る不変データです。安定キーをinlineで保持し、
// 小さい標準形状のために失敗不能なBox確保を追加しない方針を選びます。
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockShape {
    /// 全cellを占有。
    Full,
    /// 下側の半分。奇数高さでは中央のrowも下側に含めます。
    Half,
    /// 左下が直角の斜面。各cell中心を三角形へ投影して占有を決めます。
    Triangle,
    /// 最上段だけを占有する、上向きの片方向床。
    Platform,
    /// row-majorの明示占有。長さはfootprintの面積と一致する必要があります。
    OccupancyMask(Vec<bool>),
    /// 作者の精密形状。地形の占有maskと精密query用recordを別々に保持します。
    Custom {
        /// 衝突・生成等のbridgeで解釈するproviderの安定キー。
        provider: StableKey,
        /// providerのrecord版。
        version: u32,
        /// 地形格子を占有するmask。描画textureから推測しません。
        occupancy: Vec<bool>,
        /// 精密形状の正規record。解釈と検証は登録時のvalidatorへ渡します。
        data: Vec<u8>,
    },
}
impl BlockShape {
    pub(crate) fn occupied(&self, x: u16, y: u16, size: [u16; 2]) -> bool {
        match self {
            Self::Full => true,
            Self::Half => y >= size[1] / 2,
            Self::Triangle => {
                // 中心(x+1/2,y+1/2)を整数の交差積で比較し、float誤差を避けます。
                (2 * u64::from(y) + 1) * u64::from(size[0])
                    >= (2 * u64::from(x) + 1) * u64::from(size[1])
            }
            Self::Platform => y == 0,
            Self::OccupancyMask(bits)
            | Self::Custom {
                occupancy: bits, ..
            } => bits[usize::from(y) * usize::from(size[0]) + usize::from(x)],
        }
    }
    fn validate(&self, size: [u16; 2], limits: ContentLimits) -> EngineResult<()> {
        let area = usize::from(size[0]) * usize::from(size[1]);
        if let Self::OccupancyMask(bits)
        | Self::Custom {
            occupancy: bits, ..
        } = self
        {
            if bits.len() != area || !bits.iter().any(|value| *value) {
                return Err(invalid("素材の占有mask寸法または空mask"));
            }
            if area > limits.shape_cells {
                return Err(EngineError::capacity(
                    "占有maskのcell数",
                    area as u64,
                    limits.shape_cells as u64,
                ));
            }
        }
        if let Self::Custom { version, data, .. } = self {
            if *version == 0 {
                return Err(invalid("独自形状の版"));
            }
            if data.len() > limits.record_bytes {
                return Err(EngineError::capacity(
                    "独自形状byte数",
                    data.len() as u64,
                    limits.record_bytes as u64,
                ));
            }
        }
        Ok(())
    }
}

/// 標準気体圧力の素材別係数。温度Q16と整数ゲーム質量を使います。
/// 独自providerへこの式や数値形式を強制するものではありません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasPressureProperties {
    /// targetのmass×温度に掛ける非負Q16係数。
    pub coefficient_q16: u32,
    /// 各cellのtarget緩和の重みlambda。0なら直接のsource項を加えません。
    pub relaxation_weight: u32,
    /// 広域の気体profileが公開できる非負Q16圧力の上限。超過は丸めずエラーです。
    pub maximum_q16: i64,
}
/// 標準の速度・力積分に使う素材別の無次元Q16係数です。
///
/// 重力scaleと浮力scaleは元の加速度へ掛ける値です。抗力と摩擦は1tick
/// 当たりの速度減衰率で、0～65536のみを許します。これはエンジン内の
/// ゲーム単位であり、現実のSI物理や独自providerへ適用を強制しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialMotionProperties {
    /// 外部重力の倍率。65536が等倍、0が重力無効です。
    pub gravity_scale_q16: u32,
    /// 各tickの抗力による速度減衰割合。65536で全速度を除去します。
    pub drag_q16: u32,
    /// 接触する密度の高い液体・気体からの浮力加速度の倍率。
    pub buoyancy_scale_q16: u32,
    /// 重力方向に接触する固体での接線速度の減衰割合。
    pub friction_q16: u32,
}
impl MaterialMotionProperties {
    /// 分類別の参照初期値。独自providerはこの分類と式を上書きできます。
    pub fn for_base(base: MaterialBase) -> Self {
        Self {
            gravity_scale_q16: if base == MaterialBase::Solid {
                0
            } else {
                65536
            },
            drag_q16: 0,
            buoyancy_scale_q16: if matches!(base, MaterialBase::Liquid | MaterialBase::Gas) {
                65536
            } else {
                0
            },
            friction_q16: 0,
        }
    }
    /// エンジンの標準物性範囲を検査します。
    pub fn validate(self) -> EngineResult<()> {
        if self.drag_q16 > 65536 || self.friction_q16 > 65536 {
            return Err(invalid("物質の抗力・摩擦Q16上限"));
        }
        Ok(())
    }
}
impl Default for GasPressureProperties {
    fn default() -> Self {
        Self {
            coefficient_q16: 65536,
            relaxation_weight: 1,
            maximum_q16: i64::MAX,
        }
    }
}

/// 素材の配置に必要な不変定義。変更は別Contentの構築と明示移行で行います。
#[derive(Clone, Debug)]
pub struct MaterialDef {
    key: StableKey,
    base: MaterialBase,
    size: [u16; 2],
    shape: BlockShape,
    break_mode: BreakMode,
    mass: u32,
    gas_pressure: Option<GasPressureProperties>,
    motion: Option<MaterialMotionProperties>,
    texture: Option<StableKey>,
    anchor: super::AnchorRule,
}
impl MaterialDef {
    /// 素材が選んだ任意の配置条件。
    pub fn anchor_rule(&self) -> &super::AnchorRule {
        &self.anchor
    }
    /// 永続キー。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 標準の物質分類。
    pub fn base(&self) -> MaterialBase {
        self.base
    }
    /// ブロック配置時のcell寸法。格子一要素の寸法を変更しません。
    pub fn block_size(&self) -> [u16; 2] {
        self.size
    }
    /// 占有・精密形状。
    pub fn shape(&self) -> &BlockShape {
        &self.shape
    }
    /// 標準の破壊単位。
    pub fn break_mode(&self) -> BreakMode {
        self.break_mode
    }
    /// 1cellあたりの正のエンジン質量単位。
    pub fn mass_per_cell(&self) -> u32 {
        self.mass
    }
    /// 気体の標準圧力係数。気体は未指定時に標準値、他の分類はNoneを返します。
    pub fn gas_pressure(&self) -> Option<GasPressureProperties> {
        (self.base == MaterialBase::Gas).then(|| self.gas_pressure.unwrap_or_default())
    }
    /// 素材ごとの標準加速度と減衰係数。未設定では分類別の既定値を返します。
    pub fn motion_properties(&self) -> MaterialMotionProperties {
        self.motion
            .unwrap_or_else(|| MaterialMotionProperties::for_base(self.base))
    }
    /// 描画側で解決するtexture参照。物理形状とは独立しています。
    pub fn texture(&self) -> Option<StableKey> {
        self.texture
    }
}

/// 生成時だけ使う型付きbuilderです。最終的な検証はregisterでまとめて行います。
pub struct MaterialBuilder(MaterialDef);
impl MaterialBuilder {
    /// 任意の支持・独自配置条件を選びます。既定では条件を強制しません。
    pub fn anchor(mut self, rule: super::AnchorRule) -> Self {
        self.0.anchor = rule;
        self
    }
    /// 基本分類と完全修飾キーを指定します。既定footprintは1cellです。
    pub fn new(key: &str, base: MaterialBase) -> EngineResult<Self> {
        Ok(Self(MaterialDef {
            key: StableKey::parse(key)?,
            base,
            size: [1, 1],
            shape: BlockShape::Full,
            break_mode: BreakMode::Cell,
            mass: 1,
            gas_pressure: None,
            motion: None,
            texture: None,
            anchor: super::AnchorRule::Free,
        }))
    }
    /// ブロックとして配置するcell寸法。0はregister時に拒否します。
    pub fn block_size(mut self, width: u16, height: u16) -> Self {
        self.0.size = [width, height];
        self
    }
    /// 明示の占有形状。textureの透過には依存しません。
    pub fn shape(mut self, shape: BlockShape) -> Self {
        self.0.shape = shape;
        self
    }
    /// cell破壊か個体破壊かを選びます。
    pub fn break_mode(mut self, mode: BreakMode) -> Self {
        self.0.break_mode = mode;
        self
    }
    /// 各cellの質量。0はregister時に拒否します。
    pub fn mass_per_cell(mut self, mass: u32) -> Self {
        self.0.mass = mass;
        self
    }
    /// 気体の係数・lambda・圧力上限を明示します。気体以外や負の上限はregisterで拒否します。
    pub fn gas_pressure(mut self, properties: GasPressureProperties) -> Self {
        self.0.gas_pressure = Some(properties);
        self
    }
    /// 標準力積分の物性を明示します。範囲違反はregister時に拒否します。
    pub fn motion_properties(mut self, properties: MaterialMotionProperties) -> Self {
        self.0.motion = Some(properties);
        self
    }
    /// 資産キーを結びます。未読込でも物理地形は独立して扱えます。
    pub fn texture(mut self, texture: StableKey) -> Self {
        self.0.texture = Some(texture);
        self
    }
}

/// layerの意味。役割からゲームの配置・破壊規則を自動推測しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerRole {
    /// 前景地形。
    Terrain,
    /// 背景壁。
    Background,
    /// 装飾。
    Decoration,
    /// 配線や装置のanchor。
    Wiring,
    /// 作者独自の用途。
    Custom,
}
/// 不変のlayer定義。同じ座標へ異なるlayerの内容を重ねられます。
#[derive(Clone, Debug)]
pub struct LayerDef {
    /// 永続キー。
    pub key: StableKey,
    /// 用途の分類。
    pub role: LayerRole,
    /// 表示抽出時の順序。
    pub render_order: i32,
    /// 衝突providerが読むgroup bit。
    pub collision_group: u32,
    /// 物質solverへ参加するか。背景等はfalseにできます。
    pub simulated: bool,
}
/// Content内のlayer参照。別Contentへは安定キーで変換します。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LayerId(pub(crate) u32);

/// contentを確定するまでの容量。巨大なmaskの計算費用も制限します。
#[derive(Clone, Copy, Debug)]
pub struct ContentLimits {
    /// 作者素材の最大数。空cellの予約値は含めません。
    pub materials: u32,
    /// 独自channel定義数。
    pub channels: u32,
    /// 明示的に受け入れるschema移行元のcontent数。
    pub compatible_sources: u32,
    /// 最大layer数。
    pub layers: u32,
    /// 明示maskの最大要素数。
    pub shape_cells: usize,
    /// 独自recordの最大byte数。
    pub record_bytes: usize,
    /// 全定義の正規manifestのbyte上限。
    pub manifest_bytes: usize,
}
impl Default for ContentLimits {
    fn default() -> Self {
        Self {
            materials: 65536,
            channels: 64,
            compatible_sources: 64,
            layers: 256,
            shape_cells: 65536,
            record_bytes: 262144,
            manifest_bytes: 64 * 1024 * 1024,
        }
    }
}

type ShapeValidator = fn(u32, &[u8]) -> EngineResult<()>;
/// 作者の素材とlayerを登録する入口。失敗した登録は既存定義を変更しません。
pub struct ContentRegistry {
    pub(super) limits: ContentLimits,
    materials: Vec<MaterialDef>,
    layers: Vec<LayerDef>,
    geometry: Vec<(StableKey, ShapeValidator)>,
    pub(super) channels: Vec<Arc<dyn super::channels::ChannelFactory>>,
    pub(super) schemas: crate::foundation::schema::SchemaRegistry,
    pub(super) anchors: Vec<super::placement::AnchorProvider>,
    previous: Vec<Arc<Content>>,
}
impl ContentRegistry {
    /// 素材・layer・channelの形式が同じ旧Contentから、作者StateStoreのschema移行を許可します。
    /// 大きさ・形状・格子・channel値のcodecが変わる内容を互換と宣言することはできません。
    pub fn schema_migration_from(&mut self, previous: Arc<Content>) -> EngineResult<()> {
        if self.previous.len() >= self.limits.compatible_sources as usize {
            return Err(EngineError::capacity(
                "content移行元数",
                self.previous.len() as u64 + 1,
                self.limits.compatible_sources.into(),
            ));
        }
        self.previous.try_reserve(1).map_err(allocation)?;
        self.previous.push(previous);
        Ok(())
    }

    pub(super) fn has_layer(&self, key: StableKey) -> bool {
        self.layers.iter().any(|value| value.key == key)
    }
    /// 空のregistryを作ります。最低一つのlayerはfreezeまでに登録してください。
    pub fn new(limits: ContentLimits) -> EngineResult<Self> {
        if limits.materials == 0 || limits.materials == u32::MAX || limits.layers == 0 {
            return Err(invalid("content容量"));
        }
        Ok(Self {
            limits,
            materials: Vec::new(),
            layers: Vec::new(),
            geometry: Vec::new(),
            anchors: Vec::new(),
            previous: Vec::new(),
            channels: Vec::new(),
            schemas: crate::foundation::schema::SchemaRegistry::default(),
        })
    }
    /// 独自形状のcodec検証を登録します。実行時queryのbridgeはこの同じkeyを解決します。
    pub fn register_geometry(
        &mut self,
        key: StableKey,
        validate: ShapeValidator,
    ) -> EngineResult<()> {
        if self.geometry.iter().any(|(old, _)| *old == key) {
            return Err(EngineError::new(
                ErrorCode::DuplicateKey,
                "geometry provider",
            ));
        }
        self.geometry.try_reserve(1).map_err(allocation)?;
        self.geometry.push((key, validate));
        Ok(())
    }
    /// 素材を検証して登録します。予約packageのengineは作者定義へ使えません。
    pub fn register(&mut self, builder: MaterialBuilder) -> EngineResult<MaterialHandle> {
        let value = builder.0;
        if value.key.as_str().starts_with("engine:") || value.size.contains(&0) || value.mass == 0 {
            return Err(invalid("素材の予約名・寸法・質量"));
        }
        if let Some(properties) = value.gas_pressure
            && (value.base != MaterialBase::Gas || properties.maximum_q16 < 0)
        {
            return Err(invalid("気体圧力係数の対象分類または上限"));
        }
        if let Some(properties) = value.motion {
            properties.validate()?;
        }
        value.shape.validate(value.size, self.limits)?;
        if !matches!(value.anchor, super::AnchorRule::Below { .. }) {
            value.anchor.validate_definition(self)?;
        }
        if let BlockShape::Custom {
            provider,
            version,
            data,
            ..
        } = &value.shape
        {
            let (_, validate) = self
                .geometry
                .iter()
                .find(|(key, _)| key == provider)
                .ok_or_else(|| {
                    EngineError::new(ErrorCode::MissingDependency, "独自geometryのvalidator")
                })?;
            validate(*version, data)?;
        }
        if self.materials.len() >= self.limits.materials as usize {
            return Err(EngineError::capacity(
                "素材数",
                self.materials.len() as u64 + 1,
                self.limits.materials.into(),
            ));
        }
        let index = match self
            .materials
            .binary_search_by_key(&value.key, |item| item.key)
        {
            Ok(_) => return Err(EngineError::new(ErrorCode::DuplicateKey, "素材キー")),
            Err(index) => index,
        };
        let handle = MaterialHandle(value.key);
        self.materials.try_reserve(1).map_err(allocation)?;
        self.materials.insert(index, value);
        Ok(handle)
    }
    /// layerを登録します。素材登録の順序と独立して安定キー順に確定します。
    pub fn layer(&mut self, layer: LayerDef) -> EngineResult<()> {
        if self.layers.len() >= self.limits.layers as usize {
            return Err(EngineError::capacity(
                "layer数",
                self.layers.len() as u64 + 1,
                self.limits.layers.into(),
            ));
        }
        let index = match self
            .layers
            .binary_search_by_key(&layer.key, |item| item.key)
        {
            Ok(_) => return Err(EngineError::new(ErrorCode::DuplicateKey, "layerキー")),
            Err(index) => index,
        };
        self.layers.try_reserve(1).map_err(allocation)?;
        self.layers.insert(index, layer);
        Ok(())
    }
    /// 所有権を不変Contentへ移します。登録順に依存する仮IDは公開しません。
    pub fn freeze(self) -> EngineResult<Arc<Content>> {
        if self.layers.is_empty() {
            return Err(invalid("Worldにはlayerが必要"));
        }
        for material in &self.materials {
            material.anchor.validate_definition(&self)?;
        }
        let mut content = Content {
            materials: self.materials,
            layers: self.layers,
            fingerprint: [0; 32],
            channels: self.channels,
            schemas: self.schemas,
            anchors: self.anchors,
            compatible: Vec::new(),
            previous: Vec::new(),
        };
        let physical = content.encode_manifest(self.limits.manifest_bytes, false)?;
        let mut compatible = std::collections::BTreeSet::new();
        for previous in &self.previous {
            if previous.encode_manifest(self.limits.manifest_bytes, false)? != physical {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "schema移行では素材・layer・格子の変更を同一視できない",
                ));
            }
            for channel in &content.channels {
                let key = channel.value_schema();
                let old = previous.schemas.get(key)?;
                let new = content.schemas.get(key)?;
                if old.version() != new.version() || old.max_bytes() != new.max_bytes() {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "channel値の移行には専用の格子変換が必要",
                    ));
                }
            }
            for old in previous.schemas.iter() {
                let new = content.schemas.get(old.key())?;
                if new.version() < old.version() {
                    return Err(EngineError::new(
                        ErrorCode::SchemaMismatch,
                        "schema版の後退",
                    ));
                }
            }
            compatible.insert(previous.fingerprint());
            compatible.extend(previous.compatible.iter().copied());
        }
        if compatible.len() > self.limits.compatible_sources as usize {
            return Err(EngineError::capacity(
                "content移行元数",
                compatible.len() as u64,
                self.limits.compatible_sources.into(),
            ));
        }
        content
            .compatible
            .try_reserve_exact(compatible.len())
            .map_err(allocation)?;
        content.compatible.extend(compatible);
        content.previous = self.previous;
        content.fingerprint =
            *blake3::hash(&content.manifest(self.limits.manifest_bytes)?).as_bytes();
        Ok(Arc::new(content))
    }
}
/// 複数Worldで共有できる、確定済みの定義とID対応です。
pub struct Content {
    pub(crate) materials: Vec<MaterialDef>,
    pub(crate) layers: Vec<LayerDef>,
    fingerprint: [u8; 32],
    pub(crate) channels: Vec<Arc<dyn super::channels::ChannelFactory>>,
    pub(crate) schemas: crate::foundation::schema::SchemaRegistry,
    pub(crate) anchors: Vec<super::placement::AnchorProvider>,
    compatible: Vec<[u8; 32]>,
    previous: Vec<Arc<Content>>,
}
impl Content {
    /// 定義全体のBLAKE3。登録順、TypeId、callbackのアドレスは含みません。
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    /// 保存・診断へ渡す正規manifest。素材IDの辞書はこの安定キー順で定まります。
    pub fn manifest(&self, byte_limit: usize) -> EngineResult<Vec<u8>> {
        self.encode_manifest(byte_limit, true)
    }
    /// 同じ定義、または明示登録され形式照合済みのschema移行元か。
    pub fn accepts_content(&self, hash: [u8; 32]) -> bool {
        hash == self.fingerprint || self.compatible.contains(&hash)
    }
    fn encode_manifest(&self, byte_limit: usize, schemas: bool) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(byte_limit);
        out.raw(b"CHCONT01")?;
        out.u32(self.materials.len() as u32)?;
        for value in &self.materials {
            value.key.encode(&mut out)?;
            out.u8(match value.base {
                MaterialBase::Solid => 0,
                MaterialBase::Powder => 1,
                MaterialBase::Liquid => 2,
                MaterialBase::Gas => 3,
            })?;
            out.u16(value.size[0])?;
            out.u16(value.size[1])?;
            out.u8(match value.break_mode {
                BreakMode::Cell => 0,
                BreakMode::Block => 1,
            })?;
            out.u32(value.mass)?;
            value.anchor.encode_definition(&mut out)?;
            out.u8(u8::from(value.texture.is_some()))?;
            if let Some(texture) = value.texture {
                texture.encode(&mut out)?;
            }
            match &value.shape {
                BlockShape::Full => out.u8(0)?,
                BlockShape::Half => out.u8(1)?,
                BlockShape::Triangle => out.u8(2)?,
                BlockShape::Platform => out.u8(3)?,
                BlockShape::OccupancyMask(mask) => {
                    out.u8(4)?;
                    encode_mask(mask, &mut out)?;
                }
                BlockShape::Custom {
                    provider,
                    version,
                    occupancy,
                    data,
                } => {
                    out.u8(5)?;
                    provider.encode(&mut out)?;
                    out.u32(*version)?;
                    encode_mask(occupancy, &mut out)?;
                    out.bytes(data)?;
                }
            }
        }
        out.u32(self.layers.len() as u32)?;
        for value in &self.layers {
            value.key.encode(&mut out)?;
            out.u8(match value.role {
                LayerRole::Terrain => 0,
                LayerRole::Background => 1,
                LayerRole::Decoration => 2,
                LayerRole::Wiring => 3,
                LayerRole::Custom => 4,
            })?;
            out.i32(value.render_order)?;
            out.u32(value.collision_group)?;
            out.u8(u8::from(value.simulated))?;
        }
        out.u32(if schemas {
            self.schemas.iter().count() as u32
        } else {
            0
        })?;
        if schemas {
            for schema in self.schemas.iter() {
                schema.key().encode(&mut out)?;
                out.u32(schema.version())?;
                out.u64(schema.max_bytes() as u64)?;
            }
        }
        out.u32(self.channels.len() as u32)?;
        for channel in &self.channels {
            out.bytes(&channel.descriptor(byte_limit)?)?;
        }
        if schemas {
            out.u32(self.compatible.len() as u32)?;
            for hash in &self.compatible {
                out.raw(hash)?;
            }
        }
        // 明示設定がない旧Contentのmanifestは維持します。物性を変更した場合だけ
        // 物理manifestにも拡張表を含め、異なる係数を保存互換と誤認しません。
        let configured = self
            .materials
            .iter()
            .filter(|material| material.gas_pressure.is_some())
            .count();
        if configured != 0 {
            out.raw(b"GASPRESS")?;
            out.u32(configured as u32)?;
            for material in &self.materials {
                if let Some(properties) = material.gas_pressure {
                    material.key.encode(&mut out)?;
                    out.u32(properties.coefficient_q16)?;
                    out.u32(properties.relaxation_weight)?;
                    out.i64(properties.maximum_q16)?;
                }
            }
        }
        // 物理係数だけを変更した別Contentを同じ保存定義と誤認しません。
        // 未設定の旧manifestは変更せず、追加定義は型付きで可視化します。
        let configured = self
            .materials
            .iter()
            .filter(|material| material.motion.is_some())
            .count();
        if configured != 0 {
            out.raw(b"MOTNPX01")?;
            out.u32(configured as u32)?;
            for material in &self.materials {
                if let Some(properties) = material.motion {
                    material.key.encode(&mut out)?;
                    out.u32(properties.gravity_scale_q16)?;
                    out.u32(properties.drag_q16)?;
                    out.u32(properties.buoyancy_scale_q16)?;
                    out.u32(properties.friction_q16)?;
                }
            }
        }
        Ok(out.finish())
    }
    pub(crate) fn generation_source(self: &Arc<Self>, hash: [u8; 32]) -> Option<Arc<Self>> {
        if self.fingerprint == hash {
            return Some(Arc::clone(self));
        }
        self.previous
            .iter()
            .find_map(|previous| previous.generation_source(hash))
    }
    /// 素材の安定参照をこのContentのIDへ解決します。不明名を空にしません。
    pub fn material_id(&self, handle: MaterialHandle) -> EngineResult<MaterialId> {
        if handle.0 == StableKey::parse("engine:air")? {
            return Ok(MaterialId::AIR);
        }
        self.materials
            .binary_search_by_key(&handle.0, |value| value.key)
            .map(|index| MaterialId(index as u32 + 1))
            .map_err(|_| EngineError::new(ErrorCode::MissingKey, "素材の安定キー"))
    }
    /// 空以外の素材定義。空には正のmassを持つ定義がないためNoneを返します。
    pub fn material(&self, id: MaterialId) -> EngineResult<Option<&MaterialDef>> {
        if id == MaterialId::AIR {
            return Ok(None);
        }
        self.materials
            .get(id.0 as usize - 1)
            .map(Some)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "素材ID"))
    }
    /// layerの安定キーを解決します。
    pub fn layer_id(&self, key: StableKey) -> EngineResult<LayerId> {
        self.layers
            .binary_search_by_key(&key, |value| value.key)
            .map(|i| LayerId(i as u32))
            .map_err(|_| EngineError::new(ErrorCode::MissingKey, "layerキー"))
    }
    /// 同じContentのlayer定義を借ります。
    pub fn layer(&self, id: LayerId) -> EngineResult<&LayerDef> {
        self.layers
            .get(id.0 as usize)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "layer ID"))
    }
    pub(crate) fn validate_body_sidecar(
        &self,
        layer: StableKey,
        key: StableKey,
        version: u32,
        payload: &[u8],
    ) -> EngineResult<()> {
        let factory = self
            .channels
            .binary_search_by_key(&key, |factory| factory.key())
            .ok()
            .and_then(|index| self.channels.get(index))
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "body sidecar channel"))?;
        if factory.material_layer() != Some(layer) || factory.version() != version {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "body sidecar channel owner or version",
            ));
        }
        self.schemas
            .get(factory.value_schema())?
            .validate(payload)?;
        factory.validate_payload(payload)
    }
    /// 素材を安定キー順に走査します。空は暗黙の予約値です。
    pub fn materials(&self) -> impl Iterator<Item = (MaterialId, &MaterialDef)> {
        self.materials
            .iter()
            .enumerate()
            .map(|(i, value)| (MaterialId(i as u32 + 1), value))
    }
    /// layerを安定キー順に走査します。
    pub fn layers(&self) -> impl Iterator<Item = (LayerId, &LayerDef)> {
        self.layers
            .iter()
            .enumerate()
            .map(|(i, value)| (LayerId(i as u32), value))
    }
}

fn encode_mask(mask: &[bool], out: &mut Encoder) -> EngineResult<()> {
    out.u32(mask.len() as u32)?;
    for value in mask {
        out.u8(u8::from(*value))?;
    }
    Ok(())
}

impl StateCodec for MaterialId {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.0)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self(input.u32()?))
    }
}
impl StateCodec for LayerId {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.0)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self(input.u32()?))
    }
}
