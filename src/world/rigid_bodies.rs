//! Rapier f64を隠蔽し、Worldごとの剛体状態・固定step・保存境界を提供します。
//!
//! この層は標準backendの候補step、body API、BodyStencil形状、基本joint、resident solid terrainを担当します。
//! solid terrain colliderはWorldEditと固定stepをまとめる入口から更新できます。
//! そのstepはbody中心の移動とshape外接半径で作るswept envelopeがWorld境界・Unavailable chunkへ
//! 入る場合にrollbackします。角度変化も覆いますが、保守的なenvelopeなので空間の誤検出があります。
//! 境界接触を解決して移動を継続する機能ではありません。
//! World所有stepは設定した最大移動profileを満たすsubstepを使い、上限超過候補はpublishしません。
//! body local gridの基礎状態は保存され、World所有physicsへ公開するとき素材量から質量・重心・慣性を換算します。
//! World↔bodyのcell/batch移送、body↔particle変換、連結成分抽出は一部対応しています。
//! particle→bodyは粒子とdynamic bodyの線形・角運動量を保存して再固定します。dynamic bodyは蓄積済み外力を分配し、
//! disconnected local componentを別bodyへ分ける破砕に対応します。流体排除reservoirと、
//! 重なりcellと隣接表面fluid cellのQ16抗力・浮力を固定step候補内で剛体へ反作用させる連成を備えます。
//! body-local reactionは保存可能な部分経路、block attachmentのdamage粒子化は作者migration provider経由で接続します。
//! 固定step候補内でbody/world境界熱flux、body内部熱伝導、reaction scheduleを同じ順序で実行できます。
//! 独自solverはfixed-step providerで置換でき、作者定義熱境界をWorldEditの標準energy取引へ接続できます。
//! custom shapeは`CouplingProvider`からfluid接触coverage・作用点と占有cellを供給でき、
//! Engineは件数・値・重複占有・resident領域を検証して標準fluid responseへ接続します。
//! この接触coverageは作者providerの形状近似を入力とし、正確な幾何生成そのものはprovider責務です。
//! backendの全構造をstep前に複製する参照経路は
//! 正確性を優先する基準実装です。B03規模の時間・割当を測り、最適化前に基準を残します。

use super::{
    BlockInstance, BlockShape, CELL_COUNT, CellCore, CellQuery, ChannelQuery, ChunkCoord, Content,
    EnthalpyFamily, LayerId, MaterialBase, MaterialHandle, MaterialId, MaterialInventoryProvider,
    MaterialInventorySink, MixturePart, MixtureRef, MixtureState, MixtureStore, ReactionCandidate,
    ReactionCell, ReactionJob, ReactionRuleSet, ReactionSidecar, ReactionStateTransfer,
    ReactionStepReport, ThermalCellState, ThermalStepConfig, World, WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
    id::BodyId,
    transaction::CandidateState,
};
use rapier2d_f64::math::Pose;
use rapier2d_f64::parry::mass_properties::MassProperties;
use rapier2d_f64::parry::query::{NonlinearRigidMotion, cast_shapes_nonlinear, intersection_test};
use rapier2d_f64::parry::utils::PoseOpt;
use rapier2d_f64::prelude::{
    ColliderBuilder, FixedJointBuilder, ImpulseJointHandle, PhysicsWorld as RapierWorld,
    RevoluteJointBuilder, RigidBodyBuilder, RigidBodyHandle, RigidBodyType, SharedShape, Vector,
};
use serde::{Deserialize, Serialize, ser::SerializeSeq};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Write};
use std::sync::Arc;

const SNAPSHOT_VERSION: u32 = 12;
const SNAPSHOT_MAGIC: &[u8; 8] = b"CHBODY12";
const LEGACY_SNAPSHOT_V11_VERSION: u32 = 11;
const LEGACY_SNAPSHOT_V11_MAGIC: &[u8; 8] = b"CHBODY11";
const LEGACY_SNAPSHOT_V10_VERSION: u32 = 10;
const LEGACY_SNAPSHOT_V10_MAGIC: &[u8; 8] = b"CHBODY10";
const LEGACY_SNAPSHOT_V9_VERSION: u32 = 9;
const LEGACY_SNAPSHOT_V9_MAGIC: &[u8; 8] = b"CHBODY09";
const LEGACY_SNAPSHOT_V8_VERSION: u32 = 8;
const LEGACY_SNAPSHOT_V8_MAGIC: &[u8; 8] = b"CHBODY08";
const LEGACY_SNAPSHOT_V7_VERSION: u32 = 7;
const LEGACY_SNAPSHOT_V7_MAGIC: &[u8; 8] = b"CHBODY07";
const LEGACY_SNAPSHOT_V6_VERSION: u32 = 6;
const LEGACY_SNAPSHOT_V6_MAGIC: &[u8; 8] = b"CHBODY06";
const LEGACY_SNAPSHOT_V5_VERSION: u32 = 5;
const LEGACY_SNAPSHOT_V5_MAGIC: &[u8; 8] = b"CHBODY05";
const LEGACY_SNAPSHOT_V4_VERSION: u32 = 4;
const LEGACY_SNAPSHOT_V4_MAGIC: &[u8; 8] = b"CHBODY04";
const LEGACY_SNAPSHOT_V3_VERSION: u32 = 3;
const LEGACY_SNAPSHOT_V3_MAGIC: &[u8; 8] = b"CHBODY03";
const DEFAULT_RESTORE_BYTE_LIMIT: usize = 64 * 1024 * 1024;
const BODY_ID_TAG: u64 = 0x4348_4152_4f4e_0001;
const JOINT_ID_TAG: u64 = 0x4348_4152_4f4e_0002;
const TERRAIN_BODY_TAG: u64 = 0x4348_4152_4f4e_0003;
// Rapier snapshotは同一版・feature構成の間だけ互換であるため、依存更新時に明示変更します。
const LEGACY_BACKEND_COMPATIBILITY: &str =
    "rapier2d-f64:0.36.1:dim2,f64,serde-serialize,enhanced-determinism";
const BACKEND_COMPATIBILITY: &str =
    "rapier2d-f64:0.36.1:dim2,f64,serde-serialize,enhanced-determinism;bincode:1.3.3";

/// 剛体local gridのcellに追従する作者channelのcodec済み値です。
///
/// `key`と`version`はContentのchannel manifestへ照合します。保存済みpayloadはWorld側の
/// channel factoryで検証するまで不透明なbyte列として保持し、未知のsidecarを黙って
/// 捨てたり既定値へ置換したりしません。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyCellSidecar {
    key: String,
    version: u32,
    payload: Vec<u8>,
}

impl BodyCellSidecar {
    /// 型付きchannel key、channel版、StateCodec payloadからsidecarを作ります。
    pub fn new(key: StableKey, version: u32, payload: Vec<u8>) -> EngineResult<Self> {
        let value = Self {
            key: key.as_str().to_owned(),
            version,
            payload,
        };
        value.validate()?;
        Ok(value)
    }

    /// channel keyを返します。
    pub fn key(&self) -> EngineResult<StableKey> {
        StableKey::parse(&self.key)
    }

    /// 所属channelの定義版を返します。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// channel factoryで検証する保存payloadです。
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn validate(&self) -> EngineResult<()> {
        StableKey::parse(&self.key)?;
        if self.version == 0 {
            return Err(invalid("body sidecar version"));
        }
        Ok(())
    }
}

/// local cellが所有する混合成分の正確なtoken量です。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyMaterialComponent {
    material: String,
    tokens: super::MaterialQuantity,
}

impl BodyMaterialComponent {
    /// 素材keyと正の有理token量から成分を作ります。
    pub fn new(material: StableKey, tokens: super::MaterialQuantity) -> EngineResult<Self> {
        if tokens == super::MaterialQuantity::ZERO {
            return Err(invalid("body mixture component quantity"));
        }
        Ok(Self {
            material: material.as_str().to_owned(),
            tokens,
        })
    }

    /// 成分素材keyです。
    pub fn material(&self) -> EngineResult<StableKey> {
        StableKey::parse(&self.material)
    }

    /// 成分の正確なtoken量です。
    pub fn tokens(&self) -> super::MaterialQuantity {
        self.tokens
    }

    fn validate(&self) -> EngineResult<()> {
        StableKey::parse(&self.material)?;
        if self.tokens == super::MaterialQuantity::ZERO
            || super::MaterialQuantity::new(self.tokens.numerator(), self.tokens.denominator())?
                != self.tokens
        {
            return Err(invalid("body mixture component state"));
        }
        Ok(())
    }
}

/// 剛体local gridが所有する一つの物質cellです。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyLocalCell {
    layer: String,
    material: String,
    /// `engine:bodies` remapを最後に適用したepoch。古いsnapshotは0から移行します。
    #[serde(default)]
    remap_epoch: u64,
    flags: u16,
    variation: u16,
    energy: i64,
    tokens: super::MaterialQuantity,
    mixture: Vec<BodyMaterialComponent>,
    sidecars: Vec<BodyCellSidecar>,
    /// block個体移行recordのbody-local ID。通常cellはNoneです。
    #[serde(default)]
    block_attachment: Option<u64>,
    /// 元block footprint上のmask index。body内のcell位置とは別に保持します。
    #[serde(default)]
    block_mask_index: Option<u32>,
}

impl BodyLocalCell {
    /// 素材、基本状態、熱energy、全channel sidecarを持つcellを作ります。
    pub fn new(
        layer: StableKey,
        material: StableKey,
        flags: u16,
        variation: u16,
        energy: i64,
        mut sidecars: Vec<BodyCellSidecar>,
    ) -> EngineResult<Self> {
        sidecars.sort_by(|left, right| left.key.cmp(&right.key));
        let value = Self {
            layer: layer.as_str().to_owned(),
            material: material.as_str().to_owned(),
            remap_epoch: 0,
            flags,
            variation,
            energy,
            tokens: super::MaterialQuantity::ONE,
            mixture: Vec::new(),
            sidecars,
            block_attachment: None,
            block_mask_index: None,
        };
        value.validate()?;
        Ok(value)
    }

    /// carrier material cell相当の正確な量を設定します。混合時の質量一致はContent境界で検査します。
    pub fn with_tokens(mut self, tokens: super::MaterialQuantity) -> EngineResult<Self> {
        self.tokens = tokens;
        self.validate()?;
        Ok(self)
    }

    /// 成分別token量を持つ混合cellにします。
    pub fn with_mixture(mut self, mut mixture: Vec<BodyMaterialComponent>) -> EngineResult<Self> {
        mixture.sort_by(|left, right| left.material.cmp(&right.material));
        self.mixture = mixture;
        self.validate()?;
        Ok(self)
    }

    /// material layerの安定keyです。
    pub fn layer(&self) -> EngineResult<StableKey> {
        StableKey::parse(&self.layer)
    }

    /// materialの安定keyです。
    pub fn material(&self) -> EngineResult<StableKey> {
        StableKey::parse(&self.material)
    }

    /// このcellへ最後に適用した`engine:bodies` material remap epochです。
    pub fn applied_remap_epoch(&self) -> u64 {
        self.remap_epoch
    }

    /// remap tableのepochを明示します。World編集へ渡す前に材料keyがそのepochの値と一致する必要があります。
    pub fn with_applied_remap_epoch(mut self, epoch: u64) -> Self {
        self.remap_epoch = epoch;
        self
    }

    /// 正規cell flagです。
    pub fn flags(&self) -> u16 {
        self.flags
    }

    /// 作者が指定した外観variationです。
    pub fn variation(&self) -> u16 {
        self.variation
    }

    /// 正規cellと一緒に移動・保存されるenergyです。
    pub fn energy(&self) -> i64 {
        self.energy
    }

    /// carrier cell相当の正確なtoken量です。
    pub fn tokens(&self) -> super::MaterialQuantity {
        self.tokens
    }

    /// 混合成分を素材key順で返します。
    pub fn mixture(&self) -> &[BodyMaterialComponent] {
        &self.mixture
    }

    /// channel key順のsidecarを返します。
    pub fn sidecars(&self) -> &[BodyCellSidecar] {
        &self.sidecars
    }

    fn validate(&self) -> EngineResult<()> {
        StableKey::parse(&self.layer)?;
        StableKey::parse(&self.material)?;
        if self.flags & !7 != 0
            || self.tokens == super::MaterialQuantity::ZERO
            || super::MaterialQuantity::new(self.tokens.numerator(), self.tokens.denominator())?
                != self.tokens
            || self
                .mixture
                .iter()
                .any(|component| component.validate().is_err())
            || self
                .mixture
                .windows(2)
                .any(|pair| pair[0].material >= pair[1].material)
            || self
                .sidecars
                .iter()
                .any(|sidecar| sidecar.validate().is_err())
            || self
                .sidecars
                .windows(2)
                .any(|pair| pair[0].key >= pair[1].key)
            || self.block_attachment.is_some() != self.block_mask_index.is_some()
        {
            return Err(invalid("body local cell state"));
        }
        if !self.mixture.is_empty() && self.mixture.len() < 2 {
            return Err(invalid("body mixture requires multiple materials"));
        }
        Ok(())
    }
}

/// BodyLocalGridが一時所有するblock個体codecと、mask indexからbody-local cellへの対応です。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BodyLocalBlockAttachment {
    size: [u16; 2],
    /// 元block mask indexごとのbody-local row-major index。mask順で整列します。
    members: Vec<(u32, u32)>,
    /// BlockInstanceのStateCodec record。作者StateStore、向き、残存maskを含みます。
    codec: Vec<u8>,
}

/// 排除reservoirが一時保管するWorld物質cellです。
///
/// 元のcell座標は排出元を識別する安定keyとして保持し、物質・混合量・enthalpy・作者sidecarは
/// `BodyLocalCell`のcodecで保存します。reservoir内の物質はcolliderや剛体質量へ加えません。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyExclusionReservoirCell {
    origin: [i32; 2],
    cell: BodyLocalCell,
}

impl BodyExclusionReservoirCell {
    /// 物質を排除したときのWorld cell座標です。
    pub fn origin(&self) -> CellPoint {
        CellPoint {
            x: self.origin[0],
            y: self.origin[1],
        }
    }

    /// 保管中の物質、熱energy、作者sidecarです。
    pub fn cell(&self) -> &BodyLocalCell {
        &self.cell
    }
}

/// 物質、熱、作者sidecarを所有する疎なbody local gridです。
///
/// 容量は構築時に明示します。gridは占有cellだけを確保するため、大きな空領域は
/// 幅・高さに比例する配列を作りません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLocalGridLimits {
    /// local grid cellと排除reservoirが合わせて所有できる物質cellの上限。
    ///
    /// reservoirは衝突形状に含まれないため、この値はgeometryの面積より大きくできます。
    pub cells: u32,
    /// dense occupancy queryとcollider化で走査できる矩形面積の上限。
    pub geometry_cells: u32,
    /// sidecar key、版、payloadの合計byte上限。
    pub sidecar_bytes: u32,
}

/// 地形cellを剛体へ移すときに認識する標準material channelです。
///
/// 混合channelを指定した場合は標準`MixtureStore`から正確な成分量を展開します。
/// 熱channelは`ThermalCellState`としてenergyへ移し、その他のmaterial-following
/// channelはcodec済みsidecarとして保存します。位置固定channelは地形座標に残します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodyCellTransferConfig {
    /// `MixtureRef`型のmaterial channel。省略時、cellの混合参照は空でなければなりません。
    pub mixture_channel: Option<StableKey>,
    /// `ThermalCellState`型のenthalpy channel。省略時、非zero energyを持つcellは拒否します。
    pub thermal_channel: Option<StableKey>,
}

/// body local cellをWorldへ戻す時に必要なmaterial channelです。
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyCellReturnConfig {
    /// 混合参照のmaterial channel。`mixture_schema`と同時に指定します。
    pub mixture_channel: Option<StableKey>,
    /// 不変MixtureStoreのschema。`mixture_channel`と同時に指定します。
    pub mixture_schema: Option<StableKey>,
    /// `ThermalCellState`型のenthalpy channel。省略時、非zero energyを持つcellは拒否します。
    pub thermal_channel: Option<StableKey>,
    /// batch後にbody local gridが空になる場合、接続jointがないbodyを明示除去します。
    pub remove_empty_body: bool,
}

/// body→World batch移送の確定結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCellReturnReport {
    /// Worldへ移したcell数。
    pub cells: usize,
    /// local gridが空になり、明示policyでbodyも除去したか。
    pub body_removed: bool,
}

/// body local gridからWorldへ戻す一対一のcell対応です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCellReturn {
    /// body local grid内のsource座標。
    pub source: [u32; 2],
    /// World上のdestination座標。
    pub destination: CellPoint,
}

/// body排除reservoirからWorldへcellを戻すsource/destinationです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyExclusionReservoirReturn {
    /// reservoirへ入れたときのWorld source座標。
    pub origin: CellPoint,
    /// 再出力するWorld cell座標。
    pub destination: CellPoint,
}

/// 排除reservoirからWorldへ戻したcell数です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyExclusionReleaseReport {
    /// 原子的にWorldへ戻したcell数。
    pub cells: usize,
}

/// body local cellを正規物質粒子へ移す一対一の変換です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCellParticleTransfer {
    /// body local grid内のsource座標。
    pub source: [u32; 2],
    /// 粒子の初期位置に使う、読込済みWorld cell座標。
    pub destination: CellPoint,
    /// 符号付きQ16 cell/tickの初速度。
    pub velocity_q16: [i64; 2],
}

/// damage particle化で使うWorld単位換算とbody→World復帰設定です。
#[derive(Clone, Copy, Debug)]
pub struct BodyDamageConfig {
    /// World cellの物理寸法。Q16 particle速度を剛体の距離/秒へ変換します。
    pub world_units_per_cell: f64,
    /// material channel、混合、熱、空body除去の設定です。
    pub return_config: BodyCellReturnConfig,
}

/// body→particle変換の確定結果です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyCellParticleTransferReport {
    /// 粒子へ移したcell数。
    pub cells: usize,
    /// 全cell移動後に明示policyでbodyも除去したか。
    pub body_removed: bool,
    /// 入力順に対応する生成particle handle。
    pub particles: Vec<super::MaterialParticleHandle>,
}

/// damage ruleが選んだbody cellの粒子化と、残存gridの破砕結果です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyDamageReport {
    /// 正規粒子へ移し、元のbodyから消費したcellと生成粒子。
    pub particle_transfer: BodyCellParticleTransferReport,
    /// bodyが残る場合、残存local gridの連結成分分割結果。
    pub fracture: Option<BodyFractureReport>,
    /// block attachment作者状態を粒子へ移したproviderのkeyと版です。
    pub attachment_provider: Option<BodyAttachmentProviderStamp>,
}

/// 静止物質粒子を既存body local gridへ再固定する一対一の入力です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyParticleReattachment {
    /// World arena内の物質粒子。
    pub particle: super::MaterialParticleHandle,
    /// body local grid内の目的cell。
    pub destination: [u32; 2],
}

/// 成功した粒子再固定のcell数です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyParticleReattachmentReport {
    /// body local gridへ移した粒子数。
    pub cells: usize,
}

/// 切断された一つのlocal gridを複数bodyへ分けた確定結果です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyFractureReport {
    /// 元のBodyIdを先頭に、残りをcomponentの安定順に並べたfragment IDです。
    pub fragments: Vec<BodyId>,
    /// anchorが切断後に残るoccupied component上へ属さず、明示的に除去したjointです。
    pub removed_joints: Vec<JointId>,
    /// 作者attachment migration providerを指定した場合の安定keyと版です。
    pub attachment_provider: Option<BodyAttachmentProviderStamp>,
}

/// 破砕transactionが選択したblock attachment状態移行providerの識別情報です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyAttachmentProviderStamp {
    /// providerの安定key。
    pub key: StableKey,
    /// providerの意味版。
    pub version: u32,
}

/// local gridへ切り出す地形cell間の結合条件を提供します。
///
/// Jobはproviderのkey/versionを保存し、再開時に別実装への差替えを拒否します。
/// providerは対称な結合条件を使い、同じWorld stateと二つのcellに対して同じ値を返し、
/// 外部状態を変更しないでください。非対称な規則は連結成分の結果を探索順へ依存させます。
pub trait BodyBondProvider: Send + Sync {
    /// 保存・照合に使う安定key。
    fn key(&self) -> StableKey;
    /// 結合規則の版。意味を変えた場合は更新します。
    fn version(&self) -> u32;
    /// 4近傍の二cellが同じ破片へ属するかを返します。
    fn joins(
        &self,
        world: &World,
        layer: LayerId,
        from: CellPoint,
        to: CellPoint,
        from_cell: super::CellView,
        to_cell: super::CellView,
    ) -> EngineResult<bool>;
}

/// 同じ非空materialの4近傍cellを一つの破片として扱う標準結合規則です。
pub struct SameMaterialBodyBondProvider;

impl BodyBondProvider for SameMaterialBodyBondProvider {
    fn key(&self) -> StableKey {
        StableKey::parse("engine:body-same-material-bond").expect("固定の剛体結合provider key")
    }

    fn version(&self) -> u32 {
        1
    }

    fn joins(
        &self,
        _world: &World,
        _layer: LayerId,
        _from: CellPoint,
        _to: CellPoint,
        from_cell: super::CellView,
        to_cell: super::CellView,
    ) -> EngineResult<bool> {
        Ok(from_cell.block.is_none()
            && to_cell.block.is_none()
            && from_cell.core.material != MaterialId::AIR
            && from_cell.core.material == to_cell.core.material)
    }
}

/// 保存可能な4近傍連結成分探索の段階です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyComponentSearchStatus {
    /// frontierを予算内で処理中。
    Searching,
    /// frontier先のchunkが未読込で、lease/load後の再開を待っています。
    WaitingForRegion,
    /// 全境界を検査し連結成分が確定しました。
    Ready,
    /// 読込済み入力chunkまたはmaterial epochが変わりました。
    Invalidated,
    /// 設定した連結cell上限へ達しました。
    CellLimitReached,
    /// componentをbodyへ移送しました。
    Transferred,
    /// callerが明示取消しました。
    Cancelled,
}

/// Ready component jobをbodyへ適用した結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyComponentTransferOutcome {
    /// revisionが一致し、全cellがbodyへ移りました。
    Transferred { cells: usize },
    /// 探索入力が変わり、World・bodyには手を加えず再計画が必要です。
    Invalidated,
    /// 探索入力chunkが退避中のため、再読込後に探索を再開してから移送してください。
    WaitingForRegion,
}

/// body local gridの隣接cellが同じ破片へ残る条件を提供します。
///
/// 条件は対称で、同じ2cellに対して外部状態を変更せず同じ値を返してください。分断時の
/// 結果を探索順へ依存させず、外部状態を隠れた入力にしないための契約です。
pub trait BodyLocalBondProvider: Send + Sync {
    /// 4近傍にある二つの占有cellを同じ破片へ残すか返します。
    fn joins(
        &self,
        from: [u32; 2],
        to: [u32; 2],
        from_cell: &BodyLocalCell,
        to_cell: &BodyLocalCell,
    ) -> EngineResult<bool>;
}

/// body破砕で一つのblock個体maskが複数fragmentへ分かれる場合の作者状態移行を提供します。
///
/// 入力mask集合はcomponent index順、その中は元blockのmask index昇順です。実装は副作用を起こさず、
/// 同じ個体・mask集合に同じ変換結果を返してください。返す個体数は入力mask集合数と一致し、各個体の
/// `remaining_mask()`は対応する入力maskと完全一致する必要があります。作者stateの複製、分割、集約は
/// このproviderが明示的に決めます。`max_encoded_bytes`を超えない作者状態を返してください。返された個体は
/// 通常のContent検証とfragment全体のbody sidecar容量検査を受けます。
pub trait BodyBlockAttachmentSplitProvider: Send + Sync {
    /// 保存記録・診断に使用するprovider keyです。
    fn key(&self) -> StableKey;
    /// 変換意味を変更した際に進める版です。
    fn version(&self) -> u32;
    /// 各fragment用の新しいblock個体状態を返します。
    fn split(
        &self,
        original: &BlockInstance,
        fragment_masks: &[Vec<u32>],
        max_encoded_bytes: usize,
    ) -> EngineResult<Vec<BlockInstance>>;
}

/// block個体の一部を粒子化するとき、選択maskごとの作者channel状態と残存block状態を返します。
pub struct BodyBlockAttachmentParticleization {
    selected_sidecars: Vec<(u32, Vec<BodyCellSidecar>)>,
    remaining: Option<BlockInstance>,
}

impl BodyBlockAttachmentParticleization {
    /// 選択mask index順の粒子sidecarと、bodyへ残すblock個体を指定します。
    pub fn new(
        mut selected_sidecars: Vec<(u32, Vec<BodyCellSidecar>)>,
        remaining: Option<BlockInstance>,
    ) -> EngineResult<Self> {
        selected_sidecars.sort_unstable_by_key(|(mask, _)| *mask);
        if selected_sidecars
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0)
        {
            return Err(invalid("block particleization mask duplicate"));
        }
        for (_, sidecars) in &mut selected_sidecars {
            sidecars.sort_unstable_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
            if sidecars.iter().any(|sidecar| sidecar.validate().is_err())
                || sidecars.windows(2).any(|pair| pair[0].key == pair[1].key)
            {
                return Err(invalid("block particleization sidecar order or value"));
            }
        }
        Ok(Self {
            selected_sidecars,
            remaining,
        })
    }
}

/// block個体maskの一部を粒子のMaterialChannelへ移す作者状態変換を提供します。
///
/// 選択mask indexは昇順です。結果は各選択cellのsidecarと、残存mask全体を保持するblock個体を
/// 明示します。blockの作者状態を捨てる・分割する規則はproviderが決めます。返されたsidecarと
/// block状態は通常のContent・World channel・容量検証を受け、失敗すれば粒子化全体がrollbackします。
pub trait BodyBlockAttachmentParticleizationProvider: Send + Sync {
    /// 診断と再現記録に使う安定keyです。
    fn key(&self) -> StableKey;
    /// 意味を変更した際に進める版です。
    fn version(&self) -> u32;
    /// 選択cellの状態移行と残存blockの作者状態を構成します。
    fn particleize(
        &self,
        original: &BlockInstance,
        selected_mask_indices: &[u32],
        remaining_mask_indices: &[u32],
        max_encoded_bytes: usize,
    ) -> EngineResult<BodyBlockAttachmentParticleization>;
}

fn prepare_body_attachment_particleization(
    editor: &mut WorldEdit<'_>,
    layer: LayerId,
    body: BodyId,
    transfers: &[BodyCellParticleTransfer],
    return_config: BodyCellReturnConfig,
    provider: Option<&dyn BodyBlockAttachmentParticleizationProvider>,
) -> EngineResult<bool> {
    let content = Arc::clone(&editor.world.content);
    let mut grid = editor
        .world
        .body_physics
        .as_ref()
        .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
        .local_grid(body)?
        .cloned()
        .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body local grid未設定"))?;
    let initial_revision = grid.revision;

    let mut selected_local = BTreeSet::new();
    let mut selected_attachments = BTreeSet::new();
    for transfer in transfers {
        let local_index = transfer.source[1]
            .checked_mul(grid.width)
            .and_then(|row| row.checked_add(transfer.source[0]))
            .ok_or_else(|| invalid("block particleization local index overflow"))?;
        if !selected_local.insert(local_index) {
            return Err(conflict("damage particle source duplicate"));
        }
        let cell = grid
            .cells
            .get(&local_index)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "damage source body cell"))?;
        if let Some(id) = cell.block_attachment {
            selected_attachments.insert(id);
        }
    }
    if selected_attachments.is_empty() {
        return Ok(false);
    }
    let provider = provider.ok_or_else(|| {
        EngineError::new(
            ErrorCode::Conflict,
            "block attachmentの粒子化には作者状態migration providerが必要です",
        )
    })?;
    let layer_key = content.layer(layer)?.key;

    for attachment_id in selected_attachments {
        let attachment = grid
            .block_attachments
            .get(&attachment_id)
            .cloned()
            .ok_or_else(|| invalid("body block particleization attachment missing"))?;
        let mut original = {
            let mut decoder = Decoder::new(&attachment.codec, attachment.codec.len())?;
            let block = BlockInstance::decode(&mut decoder)?;
            decoder.finish()?;
            block
        };
        original.connect_state(&content, editor.world.limits)?;
        original.validate_state()?;
        if original.layer() != layer
            || original.remaining_mask().len() != attachment.members.len()
            || original
                .remaining_mask()
                .iter()
                .zip(&attachment.members)
                .any(|(mask, (stored_mask, _))| mask != stored_mask)
        {
            return Err(invalid("body block particleization attachment mask"));
        }

        let mut selected_masks = Vec::new();
        let mut remaining_masks = Vec::new();
        let mut selected_members = Vec::new();
        let mut remaining_members = Vec::new();
        selected_masks
            .try_reserve_exact(attachment.members.len())
            .map_err(allocation)?;
        remaining_masks
            .try_reserve_exact(attachment.members.len())
            .map_err(allocation)?;
        for (mask_index, local_index) in &attachment.members {
            if selected_local.contains(local_index) {
                selected_masks.push(*mask_index);
                selected_members.push((*mask_index, *local_index));
            } else {
                remaining_masks.push(*mask_index);
                remaining_members.push((*mask_index, *local_index));
            }
        }
        if selected_masks.is_empty() {
            continue;
        }
        let mut migration = provider.particleize(
            &original,
            &selected_masks,
            &remaining_masks,
            grid.sidecar_byte_capacity as usize,
        )?;
        if migration.selected_sidecars.len() != selected_masks.len()
            || migration
                .selected_sidecars
                .iter()
                .zip(&selected_masks)
                .any(|((mask, _), expected)| mask != expected)
        {
            return Err(invalid("block particleization provider mask result"));
        }
        if remaining_masks.is_empty() {
            if migration.remaining.is_some() {
                return Err(invalid(
                    "block particleization returned an unexpected remaining block",
                ));
            }
        } else {
            let fragment = migration
                .remaining
                .as_mut()
                .ok_or_else(|| invalid("block particleization omitted remaining block state"))?;
            if fragment.material() != original.material()
                || fragment.layer() != original.layer()
                || fragment.anchor() != original.anchor()
                || fragment.orientation() != original.orientation()
                || fragment.remaining_mask() != remaining_masks
            {
                return Err(invalid("block particleization remaining block state"));
            }
            fragment.connect_state(&content, editor.world.limits)?;
            fragment.validate_state()?;
        }

        for (_, sidecars) in &migration.selected_sidecars {
            for sidecar in sidecars {
                let key = sidecar.key()?;
                if Some(key) == return_config.thermal_channel
                    || Some(key) == return_config.mixture_channel
                {
                    return Err(invalid(
                        "予約channelをblock particleization sidecarへ指定できません",
                    ));
                }
                content.validate_body_sidecar(
                    layer_key,
                    key,
                    sidecar.version(),
                    sidecar.payload(),
                )?;
                if !editor.world.channels.iter().any(|channel| {
                    channel.key() == key && channel.material_layer() == Some(layer_key)
                }) {
                    return Err(EngineError::new(
                        ErrorCode::MissingKey,
                        "block particleization material channel",
                    ));
                }
            }
        }

        for (_, local_index) in &attachment.members {
            let cell = grid
                .cells
                .get_mut(local_index)
                .ok_or_else(|| invalid("block particleization member cell missing"))?;
            if cell.block_attachment != Some(attachment_id) {
                return Err(invalid("block particleization member mapping"));
            }
            cell.block_attachment = None;
            cell.block_mask_index = None;
        }
        grid.block_attachments.remove(&attachment_id);
        for ((_, sidecars), (_, local_index)) in
            migration.selected_sidecars.iter().zip(&selected_members)
        {
            let cell = grid
                .cells
                .get_mut(local_index)
                .ok_or_else(|| invalid("block particleization selected cell missing"))?;
            for sidecar in sidecars {
                let key = sidecar.key()?;
                match cell
                    .sidecars
                    .binary_search_by(|existing| existing.key.as_str().cmp(key.as_str()))
                {
                    Ok(_) => {
                        return Err(conflict(
                            "block particleization sidecar duplicates cell state",
                        ));
                    }
                    Err(index) => cell.sidecars.insert(index, sidecar.clone()),
                }
            }
        }
        if let Some(fragment) = migration.remaining {
            let mut encoder = Encoder::new(grid.sidecar_byte_capacity as usize);
            fragment.encode(&mut encoder)?;
            let codec = encoder.finish();
            grid.attach_block_state(attachment.size, remaining_members, codec)?;
        }
    }

    grid.validate()?;
    if grid.revision == initial_revision {
        grid.revision = initial_revision
            .checked_add(1)
            .ok_or_else(|| invalid("block particleization grid revision overflow"))?;
    }
    let physics = editor
        .world
        .body_physics
        .as_mut()
        .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?;
    physics.validate_local_grid_budget(Some(body), &grid)?;
    grid.validate()?;
    // attachmentとsidecarだけの変更はoccupancy shape・massを変えないので、ここではRapier colliderを
    // 再生成しません。直後のcell移送がshapeを一度だけ更新し、Content検査は通常のcommit境界で実行します。
    Arc::make_mut(&mut physics.local_grids).insert(body, grid);
    physics.local_grid_content_dirty.insert(body);
    Ok(true)
}

/// 同じlayer・carrier materialのbody local 4近傍を一つの破片として扱います。
pub struct SameMaterialBodyLocalBondProvider;

impl BodyLocalBondProvider for SameMaterialBodyLocalBondProvider {
    fn joins(
        &self,
        _from: [u32; 2],
        _to: [u32; 2],
        from_cell: &BodyLocalCell,
        to_cell: &BodyLocalCell,
    ) -> EngineResult<bool> {
        Ok(
            from_cell.layer()? == to_cell.layer()?
                && from_cell.material()? == to_cell.material()?,
        )
    }
}

/// frontierと入力revisionを保存する連結成分抽出Job。
///
/// この値はStateCodecとしてWorldのStateStoreへ保存できます。実際のWorld→body移送は
/// Ready後にWorldEdit::transfer_body_component_jobで実行し、revision照合と移送を同じ候補へまとめます。
pub struct BodyComponentSearchJob {
    world: crate::foundation::id::WorldId,
    layer: StableKey,
    seed: CellPoint,
    material: StableKey,
    provider: StableKey,
    provider_version: u32,
    epoch: u64,
    cell_limit: u32,
    members: BTreeSet<(i32, i32)>,
    frontier: Vec<(CellPoint, CellPoint)>,
    cursor: usize,
    revisions: BTreeMap<ChunkCoord, u64>,
    status: BodyComponentSearchStatus,
}

impl BodyComponentSearchJob {
    /// 読込済み物質cellから、providerを固定した連結成分Jobを作ります。
    pub fn new(
        world: &World,
        layer: LayerId,
        seed: CellPoint,
        cell_limit: u32,
        provider: &dyn BodyBondProvider,
    ) -> EngineResult<Self> {
        if cell_limit == 0 || cell_limit as usize > world.limits.job_cells {
            return Err(EngineError::capacity(
                "剛体component Job cell数",
                cell_limit.into(),
                world.limits.job_cells as u64,
            ));
        }
        if provider.version() == 0 {
            return Err(invalid("剛体結合provider version"));
        }
        let view = match world.cell(layer, seed)? {
            CellQuery::Loaded(view)
                if view.block.is_none() && view.core.material != MaterialId::AIR =>
            {
                view
            }
            CellQuery::Loaded(_) => return Err(invalid("component seedは非block物質cellが必要")),
            CellQuery::Unavailable(_) => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "component seed chunkが未読込",
                ));
            }
            CellQuery::WorldBoundary => return Err(invalid("component seedがWorld境界外")),
        };
        let layer_key = world.content.layer(layer)?.key;
        let material = world
            .content
            .material(view.core.material)?
            .ok_or_else(|| invalid("component seed material"))?
            .key();
        let mut members = BTreeSet::new();
        members.insert((seed.x, seed.y));
        let mut frontier = Vec::new();
        frontier.try_reserve_exact(4).map_err(allocation)?;
        for neighbor in body_neighbors(seed).into_iter().flatten() {
            frontier.push((seed, neighbor));
        }
        let coord = ChunkCoord::at(seed);
        let revision = world
            .chunks
            .get(&coord)
            .ok_or_else(|| invalid("component seed revision"))?
            .revision;
        Ok(Self {
            world: world.id(),
            layer: layer_key,
            seed,
            material,
            provider: provider.key(),
            provider_version: provider.version(),
            epoch: world.remaps.epoch(),
            cell_limit,
            members,
            frontier,
            cursor: 0,
            revisions: BTreeMap::from([(coord, revision)]),
            status: BodyComponentSearchStatus::Searching,
        })
    }

    /// 探索段階。
    pub fn status(&self) -> BodyComponentSearchStatus {
        self.status
    }

    /// 見つかった連結cell数。
    pub fn component_cells(&self) -> usize {
        self.members.len()
    }

    /// pending edge数です。処理済みedgeも保存payloadに残るため差分だけを返します。
    pub fn pending_edges(&self) -> usize {
        self.frontier.len().saturating_sub(self.cursor)
    }

    /// 最大budget件のfrontier edgeを検証して進めます。
    ///
    /// chunkが未読込なら現在のedge位置を保持してWaitingForRegionを返します。入力revisionや
    /// material epochが変化した場合はInvalidatedで終端し、呼出側はJobを再計画できます。
    pub fn step(
        &mut self,
        world: &World,
        budget: usize,
        provider: &dyn BodyBondProvider,
    ) -> EngineResult<BodyComponentSearchStatus> {
        if budget == 0 {
            return Err(invalid("component Job budgetは正値"));
        }
        if world.id() != self.world {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "component Jobの所属World",
            ));
        }
        let layer = world.content.layer_id(self.layer)?;
        if provider.key() != self.provider || provider.version() != self.provider_version {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "component Jobのbond providerが異なる",
            ));
        }
        if !matches!(
            self.status,
            BodyComponentSearchStatus::Searching | BodyComponentSearchStatus::WaitingForRegion
        ) {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "component Jobは再開できない状態",
            ));
        }
        if self.inputs_changed(world) {
            self.status = BodyComponentSearchStatus::Invalidated;
            return Ok(self.status);
        }
        if self.inputs_unavailable(world) {
            self.status = BodyComponentSearchStatus::WaitingForRegion;
            return Ok(self.status);
        }

        self.status = BodyComponentSearchStatus::Searching;
        let mut processed = 0;
        while self.cursor < self.frontier.len() && processed < budget {
            let (from, to) = self.frontier[self.cursor];
            let from_view = match world.cell(layer, from)? {
                CellQuery::Loaded(view) => view,
                CellQuery::Unavailable(_) => {
                    self.status = BodyComponentSearchStatus::WaitingForRegion;
                    return Ok(self.status);
                }
                CellQuery::WorldBoundary => {
                    self.status = BodyComponentSearchStatus::Invalidated;
                    return Ok(self.status);
                }
            };
            let to_view = match world.cell(layer, to)? {
                CellQuery::Loaded(view) => view,
                CellQuery::Unavailable(_) => {
                    self.status = BodyComponentSearchStatus::WaitingForRegion;
                    return Ok(self.status);
                }
                CellQuery::WorldBoundary => {
                    self.cursor += 1;
                    processed += 1;
                    continue;
                }
            };
            if !self.remember_revision(world, ChunkCoord::at(from))?
                || !self.remember_revision(world, ChunkCoord::at(to))?
            {
                self.status = BodyComponentSearchStatus::Invalidated;
                return Ok(self.status);
            }
            let to_key = (to.x, to.y);
            let can_be_body_cell =
                to_view.block.is_none() && to_view.core.material != MaterialId::AIR;
            if !self.members.contains(&to_key)
                && can_be_body_cell
                && provider.joins(world, layer, from, to, from_view, to_view)?
            {
                if self.members.len() == self.cell_limit as usize {
                    self.status = BodyComponentSearchStatus::CellLimitReached;
                    return Ok(self.status);
                }
                self.frontier.try_reserve(4).map_err(allocation)?;
                self.members.insert(to_key);
                for neighbor in body_neighbors(to).into_iter().flatten() {
                    self.frontier.push((to, neighbor));
                }
            }
            self.cursor += 1;
            processed += 1;
        }
        if self.cursor == self.frontier.len() {
            self.status = BodyComponentSearchStatus::Ready;
        }
        Ok(self.status)
    }

    /// Ready componentをdestination originへ移すbatch転送列へ変換します。
    pub fn transfers(&self, destination_origin: [u32; 2]) -> EngineResult<Vec<BodyCellTransfer>> {
        if self.status != BodyComponentSearchStatus::Ready {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "未完了のcomponent Jobは転送できない",
            ));
        }
        let min_x = self
            .members
            .iter()
            .map(|point| point.0)
            .min()
            .unwrap_or(self.seed.x);
        let min_y = self
            .members
            .iter()
            .map(|point| point.1)
            .min()
            .unwrap_or(self.seed.y);
        let mut transfers = Vec::new();
        transfers
            .try_reserve_exact(self.members.len())
            .map_err(allocation)?;
        for &(x, y) in &self.members {
            let local_x = u64::from(destination_origin[0])
                .checked_add((i64::from(x) - i64::from(min_x)) as u64)
                .ok_or_else(|| invalid("component destination x overflow"))?;
            let local_y = u64::from(destination_origin[1])
                .checked_add((i64::from(y) - i64::from(min_y)) as u64)
                .ok_or_else(|| invalid("component destination y overflow"))?;
            transfers.push(BodyCellTransfer {
                source: CellPoint { x, y },
                destination: [
                    u32::try_from(local_x).map_err(|_| invalid("component destination x range"))?,
                    u32::try_from(local_y).map_err(|_| invalid("component destination y range"))?,
                ],
            });
        }
        Ok(transfers)
    }

    /// 未完了Jobを取消し、frontierと候補成分を回収します。
    pub fn cancel(&mut self) -> EngineResult<()> {
        if matches!(
            self.status,
            BodyComponentSearchStatus::Ready | BodyComponentSearchStatus::Transferred
        ) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "Readyまたは移送済みcomponent Jobは取消できない",
            ));
        }
        self.status = BodyComponentSearchStatus::Cancelled;
        self.members.clear();
        self.frontier.clear();
        self.cursor = 0;
        self.revisions.clear();
        Ok(())
    }

    fn inputs_changed(&self, world: &World) -> bool {
        self.epoch != world.remaps.epoch()
            || self.revisions.iter().any(|(coord, revision)| {
                world
                    .chunks
                    .get(coord)
                    .is_some_and(|chunk| chunk.revision != *revision)
            })
    }

    fn inputs_unavailable(&self, world: &World) -> bool {
        self.revisions
            .keys()
            .any(|coord| !world.chunks.contains_key(coord))
    }

    fn remember_revision(&mut self, world: &World, coord: ChunkCoord) -> EngineResult<bool> {
        let revision = world
            .chunks
            .get(&coord)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "component input chunk"))?
            .revision;
        match self.revisions.get(&coord) {
            Some(previous) => Ok(*previous == revision),
            None => {
                self.revisions.insert(coord, revision);
                Ok(true)
            }
        }
    }

    fn mark_transferred(&mut self) {
        self.status = BodyComponentSearchStatus::Transferred;
        self.members.clear();
        self.frontier.clear();
        self.cursor = 0;
        self.revisions.clear();
    }
}

fn body_neighbors(point: CellPoint) -> [Option<CellPoint>; 4] {
    [
        point.x.checked_sub(1).map(|x| CellPoint { x, y: point.y }),
        point.x.checked_add(1).map(|x| CellPoint { x, y: point.y }),
        point.y.checked_sub(1).map(|y| CellPoint { x: point.x, y }),
        point.y.checked_add(1).map(|y| CellPoint { x: point.x, y }),
    ]
}

impl StateCodec for BodyComponentSearchJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        validate_body_component_job(self)?;
        self.world.encode(out)?;
        self.layer.encode(out)?;
        self.seed.encode(out)?;
        self.material.encode(out)?;
        self.provider.encode(out)?;
        out.u32(self.provider_version)?;
        out.u64(self.epoch)?;
        out.u32(self.cell_limit)?;
        out.u8(match self.status {
            BodyComponentSearchStatus::Searching => 0,
            BodyComponentSearchStatus::WaitingForRegion => 1,
            BodyComponentSearchStatus::Ready => 2,
            BodyComponentSearchStatus::Invalidated => 3,
            BodyComponentSearchStatus::CellLimitReached => 4,
            BodyComponentSearchStatus::Transferred => 5,
            BodyComponentSearchStatus::Cancelled => 6,
        })?;
        out.u32(u32::try_from(self.members.len()).map_err(|_| invalid("component member数"))?)?;
        for &(x, y) in &self.members {
            out.i32(x)?;
            out.i32(y)?;
        }
        out.u32(u32::try_from(self.frontier.len()).map_err(|_| invalid("component frontier数"))?)?;
        for (from, to) in &self.frontier {
            from.encode(out)?;
            to.encode(out)?;
        }
        out.u64(u64::try_from(self.cursor).map_err(|_| invalid("component cursor"))?)?;
        out.u32(u32::try_from(self.revisions.len()).map_err(|_| invalid("component revision数"))?)?;
        for (coord, revision) in &self.revisions {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let world = crate::foundation::id::WorldId::decode(input)?;
        let layer = StableKey::decode(input)?;
        let seed = CellPoint::decode(input)?;
        let material = StableKey::decode(input)?;
        let provider = StableKey::decode(input)?;
        let provider_version = input.u32()?;
        let epoch = input.u64()?;
        let cell_limit = input.u32()?;
        let status = match input.u8()? {
            0 => BodyComponentSearchStatus::Searching,
            1 => BodyComponentSearchStatus::WaitingForRegion,
            2 => BodyComponentSearchStatus::Ready,
            3 => BodyComponentSearchStatus::Invalidated,
            4 => BodyComponentSearchStatus::CellLimitReached,
            5 => BodyComponentSearchStatus::Transferred,
            6 => BodyComponentSearchStatus::Cancelled,
            _ => return Err(invalid("component Job status")),
        };
        let count = input.u32()? as usize;
        if count > input.remaining() / 8 {
            return Err(invalid("component member count"));
        }
        let mut members = BTreeSet::new();
        for _ in 0..count {
            let point = (input.i32()?, input.i32()?);
            if !members.insert(point) {
                return Err(invalid("duplicate component member"));
            }
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 16 {
            return Err(invalid("component frontier count"));
        }
        let mut frontier = Vec::new();
        frontier.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            frontier.push((CellPoint::decode(input)?, CellPoint::decode(input)?));
        }
        let cursor = usize::try_from(input.u64()?).map_err(|_| invalid("component cursor"))?;
        let count = input.u32()? as usize;
        if count > input.remaining() / 16 {
            return Err(invalid("component revision count"));
        }
        let mut revisions = BTreeMap::new();
        for _ in 0..count {
            let coord = ChunkCoord::decode(input)?;
            let revision = input.u64()?;
            if revisions.insert(coord, revision).is_some() {
                return Err(invalid("duplicate component revision"));
            }
        }
        let job = Self {
            world,
            layer,
            seed,
            material,
            provider,
            provider_version,
            epoch,
            cell_limit,
            members,
            frontier,
            cursor,
            revisions,
            status,
        };
        validate_body_component_job(&job)?;
        Ok(job)
    }
}

fn validate_body_component_job(job: &BodyComponentSearchJob) -> EngineResult<()> {
    let max_edges = u64::from(job.cell_limit)
        .checked_mul(4)
        .ok_or_else(|| invalid("component frontier limit"))?;
    let cleared = matches!(
        job.status,
        BodyComponentSearchStatus::Transferred | BodyComponentSearchStatus::Cancelled
    );
    let seed_chunk = ChunkCoord::at(job.seed);
    let has_seed_revision = job.revisions.contains_key(&seed_chunk);
    if job.provider_version == 0
        || job.cell_limit == 0
        || (!cleared && job.members.is_empty())
        || job.members.len() > job.cell_limit as usize
        || (!cleared && !job.members.contains(&(job.seed.x, job.seed.y)))
        || job.frontier.len() as u64 > max_edges
        || job.cursor > job.frontier.len()
        || job.frontier.iter().any(|(from, to)| {
            let dx = (i64::from(from.x) - i64::from(to.x)).abs();
            let dy = (i64::from(from.y) - i64::from(to.y)).abs();
            !job.members.contains(&(from.x, from.y)) || dx + dy != 1
        })
        || (!cleared && !has_seed_revision)
        || (job.status == BodyComponentSearchStatus::Ready && job.cursor != job.frontier.len())
        || (cleared
            && (!job.members.is_empty() || !job.frontier.is_empty() || !job.revisions.is_empty()))
    {
        return Err(invalid("component Job state"));
    }
    StableKey::parse(job.layer.as_str())?;
    StableKey::parse(job.material.as_str())?;
    StableKey::parse(job.provider.as_str())?;
    Ok(())
}

/// 地形cellと剛体local gridの対応。batch移送ではsource/destinationを一対一に指定します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCellTransfer {
    /// World上の物質cell座標。
    pub source: CellPoint,
    /// body local grid内の目的cell座標。
    pub destination: [u32; 2],
}

#[derive(Clone, Copy)]
pub(super) struct BodyFluidOverlap {
    pub(super) body: BodyId,
    pub(super) source: CellPoint,
    pub(super) core: CellCore,
}

pub(super) struct PreparedBodyFluidOccupancy {
    pub(super) occupancy: Vec<BodyFluidOccupancySample>,
    pub(super) overlaps: Vec<BodyFluidOverlap>,
}

/// custom occupancyを一度だけ照合し、実際にfluidが重なるcellを保持します。
///
/// step前のread-only検査でfluidが一つも重ならない場合は、後続WorldEdit候補を作らずに済みます。
/// 重なる場合はこの所有済みcell情報をtransactionへ渡し、candidate内の再走査を避けます。
pub(super) fn prepare_body_fluid_occupancy(
    physics: &BodyPhysics,
    world: &World,
    layer: LayerId,
    mut occupancy: Vec<BodyFluidOccupancySample>,
) -> EngineResult<PreparedBodyFluidOccupancy> {
    if occupancy.len() > world.limits.job_cells {
        return Err(EngineError::capacity(
            "CouplingProvider fluid occupancy",
            occupancy.len() as u64,
            world.limits.job_cells as u64,
        ));
    }
    occupancy.sort_by_key(|sample| (sample.world_cell.y, sample.world_cell.x, sample.body));
    if occupancy
        .windows(2)
        .any(|pair| pair[0].world_cell == pair[1].world_cell)
    {
        return Err(EngineError::new(
            ErrorCode::Conflict,
            "custom fluid occupancyのcell ownerが重複しています",
        ));
    }
    let content = world.content();
    let mut overlaps = Vec::new();
    for sample in &occupancy {
        physics.state(sample.body)?;
        let view = match world.cell(layer, sample.world_cell)? {
            CellQuery::Loaded(view) => view,
            CellQuery::Unavailable(_) => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "custom fluid occupancyに未読込cellがある",
                ));
            }
            CellQuery::WorldBoundary => {
                return Err(invalid("custom fluid occupancyがWorld境界外"));
            }
        };
        if view.block.is_some() || view.core.material == MaterialId::AIR {
            continue;
        }
        let material = content
            .material(view.core.material)?
            .ok_or_else(|| invalid("custom fluid occupancy material"))?;
        if matches!(
            material.base(),
            MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
        ) {
            if overlaps.len() == overlaps.capacity() {
                overlaps.try_reserve(1).map_err(allocation)?;
            }
            overlaps.push(BodyFluidOverlap {
                body: sample.body,
                source: sample.world_cell,
                core: view.core,
            });
        }
    }
    Ok(PreparedBodyFluidOccupancy {
        occupancy,
        overlaps,
    })
}

#[derive(Clone, Copy)]
struct BodyFluidMove {
    source: CellPoint,
    destination: CellPoint,
    core: CellCore,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyLocalGrid {
    width: u32,
    height: u32,
    cell_size_bits: u64,
    pivot_bits: [u64; 2],
    revision: u64,
    cell_capacity: u32,
    geometry_cell_capacity: u32,
    sidecar_byte_capacity: u32,
    cells: BTreeMap<u32, BodyLocalCell>,
    /// Body-local +x/+y辺が所有する整数熱流の端数。移動する剛体と一緒に保存します。
    #[serde(default)]
    thermal_edges: BTreeMap<u32, super::ThermalEdgeState>,
    /// Worldとの境界面が所有する熱flux端数。local cellと面方向に結び付き、bodyと一緒に保存します。
    #[serde(default)]
    thermal_boundary_edges: BTreeMap<u64, BodyThermalBoundaryEdgeState>,
    /// block作者状態をbodyへ一時移管した際のcodecとmask対応。
    #[serde(default)]
    block_attachments: BTreeMap<u64, BodyLocalBlockAttachment>,
    /// attachment IDはWorld内の過去recordを再利用しません。
    #[serde(default)]
    next_block_attachment: u64,
    /// bodyが占有するcellから一時排出した物質。shapeやmass propertyには含めません。
    #[serde(default)]
    exclusion_reservoir: Vec<BodyExclusionReservoirCell>,
    /// remap epochの最小・最大値。serde対象外の派生cacheで、復元後は次の境界で再走査します。
    #[serde(skip)]
    remap_epoch_bounds: Option<(u64, u64)>,
}

/// BodyLocalGridとWorld cellの熱接触面です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BodyThermalBoundaryFace {
    /// local cellの左向き面です。
    Left,
    /// local cellの右向き面です。
    Right,
    /// local cellの上向き面です。
    Top,
    /// local cellの下向き面です。
    Bottom,
}

impl BodyThermalBoundaryFace {
    fn index(self) -> u64 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
    }
}

fn thermal_boundary_edge_key(index: u32, face: BodyThermalBoundaryFace) -> u64 {
    u64::from(index) * 4 + face.index()
}

/// 保存可能なbody/world境界熱流の端数です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyThermalBoundaryEdgeState {
    /// 熱量の符号付きQ32端数です。積分stepが分母に依存しないため、係数変更後も保存値を使えます。
    pub residual: i64,
    /// 平衡移動量の8分の1未満の蓄積creditです。
    pub equilibrium_credit: u8,
}

impl BodyThermalBoundaryEdgeState {
    fn validate(self) -> EngineResult<()> {
        if self.equilibrium_credit >= 8 {
            return Err(invalid("body/world thermal equilibrium credit範囲"));
        }
        Ok(())
    }
}

/// 全表現収支へbody local gridの正規物質量を渡すproviderです。
///
/// profileへ登録すると、pure cellはcarrier素材とtoken量で、混合cellは純物質ごとの
/// 有理token量で監査します。共有MixtureStoreやbodyの個体数を物質として重複計上しません。
pub struct BodyLocalGridInventoryProvider {
    key: StableKey,
}

impl BodyLocalGridInventoryProvider {
    /// 保存profileに使う安定owner keyでproviderを作ります。
    pub fn new() -> EngineResult<Self> {
        Ok(Self {
            key: body_inventory_owner(),
        })
    }
}

impl MaterialInventoryProvider for BodyLocalGridInventoryProvider {
    fn key(&self) -> StableKey {
        self.key
    }

    fn version(&self) -> u32 {
        1
    }

    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        let Some(physics) = world.body_physics() else {
            return Ok(());
        };
        for grid in physics.local_grids.values() {
            for cell in grid
                .cells
                .values()
                .chain(grid.exclusion_reservoir.iter().map(|entry| &entry.cell))
            {
                let layer = world.content.layer_id(cell.layer()?)?;
                if cell.mixture.is_empty() {
                    output.add(
                        layer,
                        MaterialHandle(cell.material()?),
                        cell.remap_epoch,
                        MixtureRef::NONE,
                        cell.tokens(),
                    )?;
                } else {
                    for component in &cell.mixture {
                        output.add(
                            layer,
                            MaterialHandle(component.material()?),
                            cell.remap_epoch,
                            MixtureRef::NONE,
                            component.tokens(),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn body_inventory_owner() -> StableKey {
    StableKey::parse("engine:body-local-grid-inventory")
        .expect("固定の剛体material remap owner key")
}

fn body_local_cell_mass(cell: &BodyLocalCell, content: &Content) -> EngineResult<f64> {
    let mass = if cell.mixture.is_empty() {
        let material = content.material_id(MaterialHandle(cell.material()?))?;
        let definition = content
            .material(material)?
            .ok_or_else(|| invalid("body local material has no mass"))?;
        quantity_as_f64(
            cell.tokens
                .checked_mul(u128::from(definition.mass_per_cell()))?,
        )
    } else {
        let mut mass = 0.0;
        for component in &cell.mixture {
            let material = content.material_id(MaterialHandle(component.material()?))?;
            let definition = content
                .material(material)?
                .ok_or_else(|| invalid("body mixture material has no mass"))?;
            mass += quantity_as_f64(
                component
                    .tokens()
                    .checked_mul(u128::from(definition.mass_per_cell()))?,
            );
        }
        mass
    };
    if !mass.is_finite() || mass <= 0.0 {
        return Err(invalid("body local cell mass"));
    }
    Ok(mass)
}

fn validate_body_cell_content(cell: &BodyLocalCell, content: &Content) -> EngineResult<()> {
    cell.validate()?;
    let layer = cell.layer()?;
    content.layer_id(layer)?;
    let material = content.material_id(MaterialHandle(cell.material()?))?;
    if material == MaterialId::AIR {
        return Err(invalid("body-owned material cell cannot be air"));
    }
    if !cell.mixture.is_empty() {
        let carrier_mass = content
            .material(material)?
            .ok_or_else(|| invalid("body carrier material has no mass"))?
            .mass_per_cell();
        let expected_mass = cell.tokens.checked_mul(u128::from(carrier_mass))?;
        let mut mixture_mass = super::MaterialQuantity::ZERO;
        for component in &cell.mixture {
            let component_id = content.material_id(MaterialHandle(component.material()?))?;
            if component_id == MaterialId::AIR {
                return Err(invalid("body mixture cannot own air"));
            }
            let component_mass = content
                .material(component_id)?
                .ok_or_else(|| invalid("body mixture material has no mass"))?
                .mass_per_cell();
            mixture_mass = mixture_mass
                .checked_add(component.tokens().checked_mul(u128::from(component_mass))?)?;
        }
        if mixture_mass != expected_mass {
            return Err(invalid(
                "body mixture mass differs from carrier material quantity",
            ));
        }
    }
    for sidecar in &cell.sidecars {
        content.validate_body_sidecar(layer, sidecar.key()?, sidecar.version, &sidecar.payload)?;
    }
    Ok(())
}

fn rotate_local_point(point: [f64; 2], angle: f64) -> Vector {
    let (sin, cos) = angle.sin_cos();
    Vector::new(
        cos.mul_add(point[0], -sin * point[1]),
        sin.mul_add(point[0], cos * point[1]),
    )
}

impl BodyLocalGrid {
    /// 有限寸法、物理単位、pivot、occupied cell・geometry・sidecar容量を検査して空gridを作ります。
    pub fn new(
        width: u32,
        height: u32,
        cell_size: f64,
        pivot: [f64; 2],
        revision: u64,
        limits: BodyLocalGridLimits,
    ) -> EngineResult<Self> {
        let cell_count = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || cell_count > u64::from(u32::MAX)
            || !cell_size.is_finite()
            || cell_size <= 0.0
            || pivot.iter().any(|value| !value.is_finite())
            || limits.cells == 0
            || limits.geometry_cells == 0
            || cell_count > u64::from(limits.geometry_cells)
            || limits.sidecar_bytes == 0
        {
            return Err(invalid("body local grid geometry or limits"));
        }
        Ok(Self {
            width,
            height,
            cell_size_bits: cell_size.to_bits(),
            pivot_bits: pivot.map(f64::to_bits),
            revision,
            cell_capacity: limits.cells,
            geometry_cell_capacity: limits.geometry_cells,
            sidecar_byte_capacity: limits.sidecar_bytes,
            cells: BTreeMap::new(),
            thermal_edges: BTreeMap::new(),
            thermal_boundary_edges: BTreeMap::new(),
            block_attachments: BTreeMap::new(),
            next_block_attachment: 0,
            exclusion_reservoir: Vec::new(),
            remap_epoch_bounds: Some((0, 0)),
        })
    }

    /// gridの寸法を返します。
    pub fn dimensions(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    /// 局所cellの物理寸法です。
    pub fn cell_size(&self) -> f64 {
        f64::from_bits(self.cell_size_bits)
    }

    /// body originからの局所pivotです。
    pub fn pivot(&self) -> [f64; 2] {
        self.pivot_bits.map(f64::from_bits)
    }

    /// cellまたはshapeが変化した世代です。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// gridの物質cell数です。
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// local material gridが所有する物質cellと、Worldへ一時返せないreservoir cellの合計です。
    pub fn owned_cell_count(&self) -> usize {
        self.cells.len() + self.exclusion_reservoir.len()
    }

    /// bodyに保持する排除reservoirを元座標順で読みます。
    pub fn exclusion_reservoir(&self) -> &[BodyExclusionReservoirCell] {
        &self.exclusion_reservoir
    }

    fn store_exclusion_cell(&mut self, origin: CellPoint, cell: BodyLocalCell) -> EngineResult<()> {
        self.store_exclusion_cells(vec![(origin, cell)])
    }

    /// 排除物質を座標順にまとめて追加し、shiftを含む作業量をO(N + M log M)に保ちます。
    fn store_exclusion_cells(
        &mut self,
        cells: Vec<(CellPoint, BodyLocalCell)>,
    ) -> EngineResult<()> {
        if cells.is_empty() {
            return Err(invalid("空の排除reservoir batch"));
        }
        let mut incoming = Vec::new();
        incoming
            .try_reserve_exact(cells.len())
            .map_err(allocation)?;
        let mut incoming_bytes = 0usize;
        for (origin, cell) in cells {
            cell.validate()?;
            incoming_bytes = incoming_bytes
                .checked_add(sidecar_bytes(&cell)?)
                .ok_or_else(|| invalid("body reservoir sidecar byte overflow"))?;
            incoming.push(BodyExclusionReservoirCell {
                origin: [origin.x, origin.y],
                cell,
            });
        }
        incoming.sort_unstable_by_key(|entry| (entry.origin[1], entry.origin[0]));
        if incoming
            .windows(2)
            .any(|pair| pair[0].origin == pair[1].origin)
        {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "排除reservoir batch内origin重複",
            ));
        }
        let mut existing = self.exclusion_reservoir.iter().peekable();
        for entry in &incoming {
            while existing.peek().is_some_and(|current| {
                (current.origin[1], current.origin[0]) < (entry.origin[1], entry.origin[0])
            }) {
                existing.next();
            }
            if existing
                .peek()
                .is_some_and(|current| current.origin == entry.origin)
            {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "排除reservoirのorigin重複",
                ));
            }
        }
        let projected_count = self
            .owned_cell_count()
            .checked_add(incoming.len())
            .ok_or_else(|| invalid("body reservoir cell count overflow"))?;
        if projected_count > self.cell_capacity as usize {
            return Err(EngineError::capacity(
                "body local and reservoir cell capacity",
                projected_count as u64,
                u64::from(self.cell_capacity),
            ));
        }
        let projected_bytes = self
            .sidecar_bytes()?
            .checked_add(incoming_bytes)
            .ok_or_else(|| invalid("body reservoir sidecar byte overflow"))?;
        if projected_bytes > self.sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body local and reservoir sidecar bytes",
                projected_bytes as u64,
                u64::from(self.sidecar_byte_capacity),
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body reservoir revision overflow"))?;
        let total = self
            .exclusion_reservoir
            .len()
            .checked_add(incoming.len())
            .ok_or_else(|| invalid("body reservoir cell count overflow"))?;
        let mut merged = Vec::new();
        merged.try_reserve_exact(total).map_err(allocation)?;
        let mut old = self.exclusion_reservoir.drain(..).peekable();
        let mut new = incoming.into_iter().peekable();
        while old.peek().is_some() && new.peek().is_some() {
            let old_key = old.peek().expect("old item exists");
            let new_key = new.peek().expect("new item exists");
            if (old_key.origin[1], old_key.origin[0]) < (new_key.origin[1], new_key.origin[0]) {
                merged.push(old.next().expect("old item exists"));
            } else {
                merged.push(new.next().expect("new item exists"));
            }
        }
        merged.extend(old);
        merged.extend(new);
        self.exclusion_reservoir = merged;
        self.remap_epoch_bounds = None;
        self.revision = revision;
        Ok(())
    }

    /// 指定座標を一回の順次走査で除去します。
    fn take_exclusion_cells(&mut self, origins: &[CellPoint]) -> EngineResult<Vec<BodyLocalCell>> {
        if origins.is_empty() {
            return Err(invalid("空の排除reservoir release batch"));
        }
        let mut requested = Vec::new();
        requested
            .try_reserve_exact(origins.len())
            .map_err(allocation)?;
        requested.extend_from_slice(origins);
        requested.sort_unstable_by_key(|point| (point.y, point.x));
        if requested.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "排除reservoir release origin重複",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body reservoir revision overflow"))?;
        let mut removed = Vec::new();
        removed
            .try_reserve_exact(requested.len())
            .map_err(allocation)?;
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(
                self.exclusion_reservoir.len()
                    - requested.len().min(self.exclusion_reservoir.len()),
            )
            .map_err(allocation)?;
        let mut request_index = 0usize;
        for entry in self.exclusion_reservoir.drain(..) {
            let key = (entry.origin[1], entry.origin[0]);
            if request_index < requested.len()
                && (requested[request_index].y, requested[request_index].x) < key
            {
                return Err(EngineError::new(
                    ErrorCode::MissingKey,
                    "body exclusion reservoir cell",
                ));
            }
            if request_index < requested.len()
                && (requested[request_index].y, requested[request_index].x) == key
            {
                removed.push(entry.cell);
                request_index += 1;
            } else {
                retained.push(entry);
            }
        }
        if request_index != requested.len() {
            return Err(EngineError::new(
                ErrorCode::MissingKey,
                "body exclusion reservoir cell",
            ));
        }
        self.exclusion_reservoir = retained;
        self.remap_epoch_bounds = None;
        self.revision = revision;
        Ok(removed)
    }

    /// body local gridをprovider規則に従って決定的な4近傍成分へ分割します。
    ///
    /// 戻り値と各成分内の座標はy、x順です。順序集合で既訪問cellを照合するため時間は
    /// O(N log N)、作業memoryはO(N)です。providerは読み取り専用で呼び出し、失敗した
    /// 場合は部分的な成分リストを返しません。この結果は分断計画用で、Worldやbodyの状態を
    /// 変更しません。
    pub fn connected_components(
        &self,
        provider: &dyn BodyLocalBondProvider,
    ) -> EngineResult<Vec<Vec<[u32; 2]>>> {
        let mut unvisited = self.cells.keys().copied().collect::<BTreeSet<_>>();
        let mut components = Vec::new();
        let mut frontier = VecDeque::new();
        while let Some(seed) = unvisited.pop_first() {
            frontier.clear();
            frontier.try_reserve(1).map_err(allocation)?;
            frontier.push_back(seed);
            let mut members = Vec::new();
            while let Some(index) = frontier.pop_front() {
                members.try_reserve(1).map_err(allocation)?;
                members.push(index);
                let from = [index % self.width, index / self.width];
                let from_cell = self
                    .cells
                    .get(&index)
                    .ok_or_else(|| invalid("body local component seed"))?;
                let neighbors = [
                    (from[0].checked_sub(1), Some(from[1])),
                    (
                        from[0].checked_add(1).filter(|x| *x < self.width),
                        Some(from[1]),
                    ),
                    (Some(from[0]), from[1].checked_sub(1)),
                    (
                        Some(from[0]),
                        from[1].checked_add(1).filter(|y| *y < self.height),
                    ),
                ];
                for (x, y) in neighbors {
                    let (Some(x), Some(y)) = (x, y) else {
                        continue;
                    };
                    let neighbor = y
                        .checked_mul(self.width)
                        .and_then(|row| row.checked_add(x))
                        .ok_or_else(|| invalid("body local component index overflow"))?;
                    let Some(to_cell) = self.cells.get(&neighbor) else {
                        continue;
                    };
                    if !unvisited.contains(&neighbor) {
                        continue;
                    }
                    if provider.joins(from, [x, y], from_cell, to_cell)? {
                        unvisited.remove(&neighbor);
                        frontier.try_reserve(1).map_err(allocation)?;
                        frontier.push_back(neighbor);
                    }
                }
            }
            members.sort_unstable();
            let mut coordinates = Vec::new();
            coordinates
                .try_reserve_exact(members.len())
                .map_err(allocation)?;
            coordinates.extend(
                members
                    .into_iter()
                    .map(|index| [index % self.width, index / self.width]),
            );
            components.try_reserve(1).map_err(allocation)?;
            components.push(coordinates);
        }
        Ok(components)
    }

    fn geometry_cell_count(&self) -> usize {
        (u64::from(self.width) * u64::from(self.height)) as usize
    }

    /// exact occupancy maskを処理する最大geometry cell数です。
    pub fn geometry_cell_capacity(&self) -> u32 {
        self.geometry_cell_capacity
    }

    /// local coordinateのcellを読みます。
    pub fn cell(&self, x: u32, y: u32) -> Option<&BodyLocalCell> {
        self.index(x, y).and_then(|index| self.cells.get(&index))
    }

    /// local座標cellを起点とする+x/+y辺の熱流端数を読みます。
    pub fn thermal_edge_state(&self, x: u32, y: u32) -> Option<super::ThermalEdgeState> {
        self.index(x, y)
            .and_then(|index| self.thermal_edges.get(&index).copied())
    }

    /// local cellのWorld境界熱流端数を読みます。
    pub fn thermal_boundary_edge_state(
        &self,
        x: u32,
        y: u32,
        face: BodyThermalBoundaryFace,
    ) -> Option<BodyThermalBoundaryEdgeState> {
        let index = self.index(x, y)?;
        self.thermal_boundary_edges
            .get(&thermal_boundary_edge_key(index, face))
            .copied()
    }

    /// local cellのWorld境界熱流端数を設定します。
    ///
    /// cellがgridに存在しない場合やbyte上限を超える場合は変更しません。
    pub fn set_thermal_boundary_edge_state(
        &mut self,
        x: u32,
        y: u32,
        face: BodyThermalBoundaryFace,
        state: BodyThermalBoundaryEdgeState,
    ) -> EngineResult<()> {
        state.validate()?;
        let index = self
            .index(x, y)
            .ok_or_else(|| invalid("body thermal boundary coordinate"))?;
        if !self.cells.contains_key(&index) {
            return Err(invalid("body thermal boundary cell is unoccupied"));
        }
        let key = thermal_boundary_edge_key(index, face);
        let existed = self.thermal_boundary_edges.contains_key(&key);
        let adding = state != BodyThermalBoundaryEdgeState::default() && !existed;
        let removing = state == BodyThermalBoundaryEdgeState::default() && existed;
        let entry_bytes = std::mem::size_of::<(u64, BodyThermalBoundaryEdgeState)>();
        let current_bytes = self.sidecar_bytes()?;
        let next_bytes = if adding {
            current_bytes
                .checked_add(entry_bytes)
                .ok_or_else(|| invalid("body thermal boundary byte overflow"))?
        } else if removing {
            current_bytes
                .checked_sub(entry_bytes)
                .ok_or_else(|| invalid("body thermal boundary byte underflow"))?
        } else {
            current_bytes
        };
        if next_bytes > self.sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body thermal boundary bytes",
                next_bytes as u64,
                u64::from(self.sidecar_byte_capacity),
            ));
        }
        if state == BodyThermalBoundaryEdgeState::default() {
            self.thermal_boundary_edges.remove(&key);
        } else {
            self.thermal_boundary_edges.insert(key, state);
        }
        Ok(())
    }

    fn clear_thermal_edges_for_removed_cell(&mut self, index: u32) {
        self.thermal_edges.remove(&index);
        let x = index % self.width;
        let y = index / self.width;
        if x > 0
            && let Some(edge) = self.thermal_edges.get_mut(&(index - 1))
        {
            edge.residual_x = 0;
            edge.equilibrium_credit_x = 0;
        }
        if y > 0
            && let Some(edge) = self.thermal_edges.get_mut(&(index - self.width))
        {
            edge.residual_y = 0;
            edge.equilibrium_credit_y = 0;
        }
        self.thermal_edges
            .retain(|_, edge| *edge != super::ThermalEdgeState::default());
        let first_boundary_edge = u64::from(index) * 4;
        for face in 0..4 {
            self.thermal_boundary_edges
                .remove(&(first_boundary_edge + face));
        }
    }

    fn apply_thermal_step(
        &mut self,
        cell_updates: &[(u32, i64, StableKey, u64)],
        thermal_edges: BTreeMap<u32, super::ThermalEdgeState>,
    ) -> EngineResult<(bool, bool)> {
        let mut any_change = self.thermal_edges != thermal_edges;
        let mut cell_state_changed = false;
        let mut material_changed = false;
        for (index, energy, material, _) in cell_updates {
            let cell = self
                .cells
                .get(index)
                .ok_or_else(|| invalid("body thermal cell disappeared"))?;
            let changed_material = cell.material()? != *material;
            let changed_energy = cell.energy != *energy;
            any_change |= changed_material || changed_energy;
            cell_state_changed |= changed_material || changed_energy;
            material_changed |= changed_material;
        }
        if !any_change {
            return Ok((false, false));
        }
        let revision = if cell_state_changed {
            Some(
                self.revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("body thermal grid revision overflow"))?,
            )
        } else {
            None
        };
        for (index, energy, material, remap_epoch) in cell_updates {
            let cell = self
                .cells
                .get_mut(index)
                .ok_or_else(|| invalid("body thermal cell disappeared"))?;
            cell.energy = *energy;
            if cell.material()? != *material {
                cell.material = material.as_str().to_owned();
                cell.remap_epoch = *remap_epoch;
            }
        }
        self.thermal_edges = thermal_edges;
        if let Some(revision) = revision {
            self.revision = revision;
        }
        Ok((true, material_changed))
    }

    /// 正確なmaterial quantityをbackend用のf64質量特性へ換算します。
    ///
    /// tokenと成分量の正規状態は有理数のまま保持し、ここでのみRapierのf64単位へ変換します。
    /// cellは一様な正方形とみなし、重心と平行軸の定理からlocal grid全体の角慣性を求めます。
    fn material_mass_properties(&self, content: &Content) -> EngineResult<MassProperties> {
        let cell_size = self.cell_size();
        let pivot = self.pivot();
        let mut total_mass = 0.0;
        let mut com_x = 0.0;
        let mut com_y = 0.0;
        let mut inertia = 0.0;

        for (index, cell) in &self.cells {
            let cell_mass = body_local_cell_mass(cell, content)?;

            let x = (*index % self.width) as f64;
            let y = (*index / self.width) as f64;
            let cell_center_x = (x + 0.5) * cell_size - pivot[0];
            let cell_center_y = (y + 0.5) * cell_size - pivot[1];
            let next_mass = total_mass + cell_mass;
            if !next_mass.is_finite() || next_mass <= 0.0 {
                return Err(invalid("body local mass overflow"));
            }
            if total_mass == 0.0 {
                com_x = cell_center_x;
                com_y = cell_center_y;
                inertia = cell_mass * cell_size * cell_size / 6.0;
            } else {
                let delta_x = cell_center_x - com_x;
                let delta_y = cell_center_y - com_y;
                let shared_mass = (total_mass / next_mass) * cell_mass;
                let new_component_weight = cell_mass / next_mass;
                inertia += cell_mass * cell_size * cell_size / 6.0
                    + shared_mass * delta_x.mul_add(delta_x, delta_y * delta_y);
                com_x += delta_x * new_component_weight;
                com_y += delta_y * new_component_weight;
            }
            total_mass = next_mass;
        }

        if !total_mass.is_finite() || total_mass <= 0.0 {
            return Err(invalid("body local mass properties overflow"));
        }
        if !com_x.is_finite() || !com_y.is_finite() || !inertia.is_finite() || inertia <= 0.0 {
            return Err(invalid("body local center of mass or inertia"));
        }
        Ok(MassProperties::new(
            Vector::new(com_x, com_y),
            total_mass,
            inertia,
        ))
    }

    /// 物質cellを挿入・置換します。容量・revision失敗ではgridを変更しません。
    pub fn set_cell(&mut self, x: u32, y: u32, cell: BodyLocalCell) -> EngineResult<()> {
        cell.validate()?;
        let index = self
            .index(x, y)
            .ok_or_else(|| invalid("body local cell coordinate"))?;
        let before = self.cells.get(&index);
        if before.is_some_and(|existing| existing.block_attachment.is_some()) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "block作者状態を保持するcellは個体移行手順でのみ置換できます",
            ));
        }
        let replacing = before.is_some();
        let new_count = self
            .owned_cell_count()
            .checked_add(usize::from(before.is_none()))
            .ok_or_else(|| invalid("body local cell count overflow"))?;
        if new_count > self.cell_capacity as usize {
            return Err(EngineError::capacity(
                "body local cell capacity",
                new_count as u64,
                u64::from(self.cell_capacity),
            ));
        }
        let current_bytes = self.sidecar_bytes()?;
        let old_bytes = before.map_or(Ok(0), sidecar_bytes)?;
        let new_bytes = sidecar_bytes(&cell)?;
        let projected_bytes = current_bytes
            .checked_sub(old_bytes)
            .and_then(|bytes| bytes.checked_add(new_bytes))
            .ok_or_else(|| invalid("body local sidecar byte overflow"))?;
        if projected_bytes > self.sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body local sidecar bytes",
                projected_bytes as u64,
                u64::from(self.sidecar_byte_capacity),
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body local grid revision overflow"))?;
        self.cells.insert(index, cell);
        if replacing {
            // 置換前のepochが境界値だった可能性があるため、次のWorld境界で再集計します。
            self.remap_epoch_bounds = None;
        } else if let Some((minimum, maximum)) = self.remap_epoch_bounds {
            let epoch = self.cells.get(&index).expect("挿入済みcell").remap_epoch;
            self.remap_epoch_bounds = Some((minimum.min(epoch), maximum.max(epoch)));
        }
        self.revision = revision;
        Ok(())
    }

    /// 既存cellを置換せず、複数の空slotへ一括挿入します。
    ///
    /// occupied数・sidecar byte数・grid revisionを一度の集計で検証します。cell単位の
    /// set_cellを連続で呼ぶと既存sidecar全体を毎回再集計するため、抽出batchはこちらを
    /// 使い、容量不足時に一部だけ増えたgridを作らないようにします。
    pub fn insert_cells_batch(
        &mut self,
        cells: Vec<(u32, u32, BodyLocalCell)>,
    ) -> EngineResult<()> {
        if cells.is_empty() {
            return Err(invalid("空のbody local cell batch"));
        }
        let mut indexes = Vec::new();
        indexes.try_reserve_exact(cells.len()).map_err(allocation)?;
        let mut incoming_sidecar_bytes = 0usize;
        for (x, y, cell) in &cells {
            cell.validate()?;
            let index = self
                .index(*x, *y)
                .ok_or_else(|| invalid("body local cell coordinate"))?;
            if self.cells.contains_key(&index) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body local batch slot is occupied",
                ));
            }
            indexes.push(index);
            incoming_sidecar_bytes = incoming_sidecar_bytes
                .checked_add(sidecar_bytes(cell)?)
                .ok_or_else(|| invalid("body local sidecar byte overflow"))?;
        }
        indexes.sort_unstable();
        if indexes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "body local batch destination duplicate",
            ));
        }
        let new_count = self
            .owned_cell_count()
            .checked_add(cells.len())
            .ok_or_else(|| invalid("body local cell count overflow"))?;
        if new_count > self.cell_capacity as usize {
            return Err(EngineError::capacity(
                "body local cell capacity",
                new_count as u64,
                u64::from(self.cell_capacity),
            ));
        }
        let projected_bytes = self
            .sidecar_bytes()?
            .checked_add(incoming_sidecar_bytes)
            .ok_or_else(|| invalid("body local sidecar byte overflow"))?;
        if projected_bytes > self.sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body local sidecar bytes",
                projected_bytes as u64,
                u64::from(self.sidecar_byte_capacity),
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body local grid revision overflow"))?;
        let incoming_epoch_bounds =
            cells
                .iter()
                .fold(None::<(u64, u64)>, |bounds, (_, _, cell)| {
                    Some(match bounds {
                        None => (cell.remap_epoch, cell.remap_epoch),
                        Some((minimum, maximum)) => {
                            (minimum.min(cell.remap_epoch), maximum.max(cell.remap_epoch))
                        }
                    })
                });
        for (x, y, cell) in cells {
            let index = self.index(x, y).expect("batch座標を事前検証済み");
            let previous = self.cells.insert(index, cell);
            debug_assert!(previous.is_none(), "batch挿入先は未使用");
        }
        if let (Some((minimum, maximum)), Some((incoming_minimum, incoming_maximum))) =
            (self.remap_epoch_bounds, incoming_epoch_bounds)
        {
            self.remap_epoch_bounds =
                Some((minimum.min(incoming_minimum), maximum.max(incoming_maximum)));
        }
        self.revision = revision;
        Ok(())
    }

    fn attach_block_state(
        &mut self,
        size: [u16; 2],
        mut members: Vec<(u32, u32)>,
        codec: Vec<u8>,
    ) -> EngineResult<u64> {
        if size.contains(&0) || members.is_empty() || codec.is_empty() {
            return Err(invalid("body block attachment input"));
        }
        members.sort_unstable_by_key(|(mask_index, _)| *mask_index);
        if members.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(conflict("body block attachment mask duplicate"));
        }
        let block_area = u32::from(size[0])
            .checked_mul(u32::from(size[1]))
            .ok_or_else(|| invalid("body block attachment area overflow"))?;
        let mut decoder = Decoder::new(&codec, codec.len())?;
        let block = BlockInstance::decode(&mut decoder)?;
        decoder.finish()?;
        if block.dormant.is_some()
            || block.remaining.len() != members.len()
            || block
                .remaining
                .iter()
                .zip(&members)
                .any(|(mask, (stored_mask, _))| mask != stored_mask || *stored_mask >= block_area)
        {
            return Err(invalid("body block attachment codec/mask"));
        }
        let mut local_indexes = Vec::new();
        local_indexes
            .try_reserve_exact(members.len())
            .map_err(allocation)?;
        for (_, local_index) in &members {
            if u64::from(*local_index) >= u64::from(self.width) * u64::from(self.height)
                || self.cells.get(local_index).is_none_or(|cell| {
                    cell.block_attachment.is_some() || cell.block_mask_index.is_some()
                })
            {
                return Err(conflict("body block attachment local cell is unavailable"));
            }
            local_indexes.push(*local_index);
        }
        local_indexes.sort_unstable();
        if local_indexes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(conflict("body block attachment local cell duplicate"));
        }
        let id = self.next_block_attachment;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| invalid("body block attachment ID overflow"))?;
        let mapping_bytes = members
            .len()
            .checked_mul(std::mem::size_of::<(u32, u32)>())
            .ok_or_else(|| invalid("body block attachment mapping byte overflow"))?;
        let required = self
            .sidecar_bytes()?
            .checked_add(codec.len())
            .and_then(|bytes| bytes.checked_add(mapping_bytes))
            .ok_or_else(|| invalid("body block attachment sidecar byte overflow"))?;
        if required > self.sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body local sidecar bytes",
                required as u64,
                u64::from(self.sidecar_byte_capacity),
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body local grid revision overflow"))?;
        for (mask_index, local_index) in &members {
            let cell = self.cells.get_mut(local_index).expect("cellを事前検証済み");
            cell.block_attachment = Some(id);
            cell.block_mask_index = Some(*mask_index);
        }
        self.block_attachments.insert(
            id,
            BodyLocalBlockAttachment {
                size,
                members,
                codec,
            },
        );
        self.next_block_attachment = next_id;
        self.revision = revision;
        Ok(id)
    }

    /// local cellを除去します。成功した場合だけrevisionを進めます。
    pub fn remove_cell(&mut self, x: u32, y: u32) -> EngineResult<Option<BodyLocalCell>> {
        let index = self
            .index(x, y)
            .ok_or_else(|| invalid("body local cell coordinate"))?;
        if !self.cells.contains_key(&index) {
            return Ok(None);
        }
        if self.cells[&index].block_attachment.is_some() {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "block作者状態を保持するcellは個体移行手順でのみ除去できます",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body local grid revision overflow"))?;
        let removed = self.cells.remove(&index);
        self.clear_thermal_edges_for_removed_cell(index);
        self.remap_epoch_bounds = None;
        self.revision = revision;
        Ok(removed)
    }

    /// 既存cellを一括で除去し、一度だけshape revisionを進めます。
    pub fn remove_cells_batch(
        &mut self,
        coordinates: &[[u32; 2]],
    ) -> EngineResult<Vec<BodyLocalCell>> {
        if coordinates.is_empty() {
            return Err(invalid("空のbody local remove batch"));
        }
        let mut indexes = Vec::new();
        indexes
            .try_reserve_exact(coordinates.len())
            .map_err(allocation)?;
        for [x, y] in coordinates {
            let index = self
                .index(*x, *y)
                .ok_or_else(|| invalid("body local cell coordinate"))?;
            if !self.cells.contains_key(&index) {
                return Err(EngineError::new(
                    ErrorCode::MissingKey,
                    "body local batch source is empty",
                ));
            }
            if self.cells[&index].block_attachment.is_some() {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "block作者状態を保持するcellは個体移行手順でのみ除去できます",
                ));
            }
            indexes.push(index);
        }
        indexes.sort_unstable();
        if indexes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "body local batch source duplicate",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("body local grid revision overflow"))?;
        let mut removed = Vec::new();
        removed
            .try_reserve_exact(indexes.len())
            .map_err(allocation)?;
        for index in indexes {
            removed.push(self.cells.remove(&index).expect("sourceを事前検証済み"));
            self.clear_thermal_edges_for_removed_cell(index);
        }
        self.remap_epoch_bounds = None;
        self.revision = revision;
        Ok(removed)
    }

    fn index(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        y.checked_mul(self.width)?.checked_add(x)
    }

    fn sidecar_bytes(&self) -> EngineResult<usize> {
        let physical = self.cells.values().try_fold(0usize, |sum, cell| {
            sum.checked_add(sidecar_bytes(cell)?)
                .ok_or_else(|| invalid("body sidecar byte overflow"))
        })?;
        let with_reservoir = self
            .exclusion_reservoir
            .iter()
            .try_fold(physical, |sum, entry| {
                sum.checked_add(sidecar_bytes(&entry.cell)?)
                    .ok_or_else(|| invalid("body reservoir sidecar byte overflow"))
            })?;
        let with_attachments =
            self.block_attachments
                .values()
                .try_fold(with_reservoir, |sum, attachment| {
                    let mapping_bytes = attachment
                        .members
                        .len()
                        .checked_mul(std::mem::size_of::<(u32, u32)>())
                        .ok_or_else(|| invalid("body block mapping byte overflow"))?;
                    sum.checked_add(attachment.codec.len())
                        .and_then(|bytes| bytes.checked_add(mapping_bytes))
                        .ok_or_else(|| invalid("body block state byte overflow"))
                })?;
        with_attachments
            .checked_add(
                self.thermal_boundary_edges
                    .len()
                    .checked_mul(std::mem::size_of::<(u64, BodyThermalBoundaryEdgeState)>())
                    .ok_or_else(|| invalid("body thermal boundary byte overflow"))?,
            )
            .ok_or_else(|| invalid("body local state byte overflow"))
    }

    fn validate(&self) -> EngineResult<()> {
        let cell_count = u64::from(self.width) * u64::from(self.height);
        let cell_size = self.cell_size();
        let pivot = self.pivot();
        if self.width == 0
            || self.height == 0
            || cell_count > u64::from(u32::MAX)
            || !cell_size.is_finite()
            || cell_size <= 0.0
            || pivot.iter().any(|value| !value.is_finite())
            || self.cell_capacity == 0
            || self.geometry_cell_capacity == 0
            || cell_count > u64::from(self.geometry_cell_capacity)
            || self.sidecar_byte_capacity == 0
            || self.owned_cell_count() > self.cell_capacity as usize
            || self
                .cells
                .keys()
                .any(|index| u64::from(*index) >= cell_count)
        {
            return Err(invalid("body local grid snapshot"));
        }
        for cell in self.cells.values() {
            cell.validate()?;
        }
        let mut attached_cells = BTreeSet::new();
        for (id, attachment) in &self.block_attachments {
            let block_area = u32::from(attachment.size[0])
                .checked_mul(u32::from(attachment.size[1]))
                .ok_or_else(|| invalid("body block attachment area overflow"))?;
            if *id >= self.next_block_attachment
                || attachment.size.contains(&0)
                || attachment.members.is_empty()
                || attachment.codec.is_empty()
                || attachment
                    .members
                    .windows(2)
                    .any(|pair| pair[0].0 >= pair[1].0)
            {
                return Err(invalid("body block attachment header"));
            }
            let mut decoder = Decoder::new(&attachment.codec, attachment.codec.len())?;
            let block = BlockInstance::decode(&mut decoder)?;
            decoder.finish()?;
            if block.dormant.is_some()
                || block.remaining.len() != attachment.members.len()
                || block
                    .remaining
                    .iter()
                    .zip(&attachment.members)
                    .any(|(mask, (stored_mask, _))| mask != stored_mask)
            {
                return Err(invalid("body block attachment codec/mask mismatch"));
            }
            let mut local_indexes = BTreeSet::new();
            for (mask_index, local_index) in &attachment.members {
                if *mask_index >= block_area
                    || u64::from(*local_index) >= cell_count
                    || !local_indexes.insert(*local_index)
                    || !attached_cells.insert(*local_index)
                {
                    return Err(invalid("body block attachment member index"));
                }
                let cell = self
                    .cells
                    .get(local_index)
                    .ok_or_else(|| invalid("body block attachment cell missing"))?;
                if cell.block_attachment != Some(*id) || cell.block_mask_index != Some(*mask_index)
                {
                    return Err(invalid("body block attachment cell mapping"));
                }
            }
        }
        for (index, cell) in &self.cells {
            match (cell.block_attachment, cell.block_mask_index) {
                (None, None) => {}
                (Some(id), Some(mask))
                    if attached_cells.contains(index)
                        && self.block_attachments.contains_key(&id)
                        && self.block_attachments[&id]
                            .members
                            .binary_search_by_key(&mask, |(mask_index, _)| *mask_index)
                            .is_ok() => {}
                _ => return Err(invalid("body cell has orphan block attachment")),
            }
        }
        for (index, edge) in &self.thermal_edges {
            if u64::from(*index) >= cell_count
                || *edge == super::ThermalEdgeState::default()
                || edge.validate().is_err()
            {
                return Err(invalid("body local thermal edge state"));
            }
            let x = *index % self.width;
            let y = *index / self.width;
            let has_right = x + 1 < self.width && self.cells.contains_key(&(*index + 1));
            let has_below =
                y + 1 < self.height && self.cells.contains_key(&index.saturating_add(self.width));
            if (edge.residual_x != 0 || edge.equilibrium_credit_x != 0)
                && (!has_right || !self.cells.contains_key(index))
            {
                return Err(invalid("body local +x thermal edge endpoints"));
            }
            if (edge.residual_y != 0 || edge.equilibrium_credit_y != 0)
                && (!has_below || !self.cells.contains_key(index))
            {
                return Err(invalid("body local +y thermal edge endpoints"));
            }
        }
        let boundary_edge_limit = cell_count
            .checked_mul(4)
            .ok_or_else(|| invalid("body thermal boundary edge count overflow"))?;
        for (key, edge) in &self.thermal_boundary_edges {
            let index = key / 4;
            if *key >= boundary_edge_limit
                || !self.cells.contains_key(&(index as u32))
                || *edge == BodyThermalBoundaryEdgeState::default()
                || edge.validate().is_err()
            {
                return Err(invalid("body local/world thermal boundary edge state"));
            }
        }
        let mut previous = None;
        for entry in &self.exclusion_reservoir {
            let key = (entry.origin[1], entry.origin[0]);
            if previous.is_some_and(|value| value >= key) {
                return Err(invalid("body exclusion reservoir order or duplicate"));
            }
            entry.cell.validate()?;
            previous = Some(key);
        }
        if self.sidecar_bytes()? > self.sidecar_byte_capacity as usize {
            return Err(invalid("body local grid sidecar capacity"));
        }
        Ok(())
    }

    fn stencil(&self) -> EngineResult<BodyStencil> {
        let count = usize::try_from(u64::from(self.width) * u64::from(self.height))
            .map_err(|_| invalid("body local grid dimensions"))?;
        let mut occupied = Vec::new();
        occupied
            .try_reserve_exact(count)
            .map_err(|_| EngineError::new(ErrorCode::AllocationFailed, "body local stencil"))?;
        occupied.resize(count, false);
        for index in self.cells.keys() {
            occupied[*index as usize] = true;
        }
        BodyStencil::new(
            self.width,
            self.height,
            self.cell_size(),
            self.pivot(),
            self.revision,
            occupied,
        )
    }
}

/// 正規の有理token量をRapierが使うf64質量単位へ変換します。
fn quantity_as_f64(quantity: super::MaterialQuantity) -> f64 {
    quantity.numerator() as f64 / quantity.denominator() as f64
}

/// body-local anchorが属するoccupied cell componentを安定順で返します。
///
/// anchorはcellの閉じた矩形内にある必要があり、複数cellの境界に重なる場合は最小の
/// component indexを選びます。空隙またはgrid外のanchorは`None`となり、破砕時にjointを
/// 暗黙に別fragmentへ付け替えず、呼出結果で除去を報告します。
fn body_local_component_for_anchor(
    grid: &BodyLocalGrid,
    component_by_cell: &BTreeMap<u32, usize>,
    anchor: Vector,
) -> Option<usize> {
    let cell_size = grid.cell_size();
    let pivot = grid.pivot();
    component_by_cell
        .iter()
        .filter_map(|(index, component)| {
            let x = *index % grid.width;
            let y = *index / grid.width;
            let left = f64::from(x) * cell_size - pivot[0];
            let top = f64::from(y) * cell_size - pivot[1];
            let right = left + cell_size;
            let bottom = top + cell_size;
            (anchor.x >= left && anchor.x <= right && anchor.y >= top && anchor.y <= bottom)
                .then_some(*component)
        })
        .min()
}

fn sidecar_bytes(cell: &BodyLocalCell) -> EngineResult<usize> {
    cell.sidecars.iter().try_fold(0usize, |sum, sidecar| {
        sum.checked_add(sidecar.key.len())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<u32>()))
            .and_then(|bytes| bytes.checked_add(sidecar.payload.len()))
            .ok_or_else(|| invalid("body sidecar byte overflow"))
    })
}

fn local_grid_totals(
    grids: &BTreeMap<BodyId, BodyLocalGrid>,
) -> EngineResult<(usize, usize, usize)> {
    grids.values().try_fold(
        (0usize, 0usize, 0usize),
        |(cells, geometry, bytes), grid| {
            Ok((
                cells
                    .checked_add(grid.owned_cell_count())
                    .ok_or_else(|| invalid("body local cell total overflow"))?,
                geometry
                    .checked_add(grid.geometry_cell_count())
                    .ok_or_else(|| invalid("body local geometry total overflow"))?,
                bytes
                    .checked_add(grid.sidecar_bytes()?)
                    .ok_or_else(|| invalid("body local sidecar total overflow"))?,
            ))
        },
    )
}

/// 剛体内の占有セルをWorld cellへ写す、正規の局所maskです。
///
/// row-majorの占有maskと局所pivotを保持し、衝突shape生成とWorld側の排除queryで同じ
/// 定義を利用します。物質やsidecarの所有権は持たず、未接続の物質連成を表しません。
#[derive(Clone, Debug, PartialEq)]
pub struct BodyStencil {
    width: u32,
    height: u32,
    cell_size: f64,
    pivot: [f64; 2],
    revision: u64,
    occupied: Vec<bool>,
    rectangles: Vec<StencilRect>,
}

/// BodyStencil内の一つの露出境界面を、World座標の線分で表します。
///
/// `local_cell`と`face`は線分の所有元を示します。`start`から`end`はBodyStencilの局所
/// cell edgeをposeで変換した端点で、面の長さは回転角に依存せず`cell_size`です。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyBoundarySegment {
    /// 面を所有するlocal cell座標。
    pub local_cell: [u32; 2],
    /// local cell内の面方向。
    pub face: BodyThermalBoundaryFace,
    /// World座標の線分始点。
    pub start: [f64; 2],
    /// World座標の線分終点。
    pub end: [f64; 2],
}

/// 露出境界線分とWorld格子cellの交差です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyBoundaryContact {
    /// 接触面を所有するlocal cell座標。
    pub local_cell: [u32; 2],
    /// 接触面のlocal方向。
    pub face: BodyThermalBoundaryFace,
    /// 線分が交差するWorld格子座標。
    pub world_cell: CellPoint,
    /// 交差区間の中点。物理作用点の集約に使います。
    pub world_point: [f64; 2],
    /// そのWorld cellに含まれる線分長。単位はposeおよびgrid sizeと同じです。
    pub covered_length: f64,
}

/// 任意剛体形状がbody-local gridとWorld gridの間に作る熱境界を供給します。
///
/// providerは形状とposeから接触線分をWorld cellごとに分割し、各接触をbody-local cell面へ
/// 対応づけます。戻り値はEngine側でlocal cell、面長、件数を検証した後、標準の整数energy
/// 取引へ渡されます。providerはWorldを変更せず、形状状態が必要なら外部StateStoreから読む
/// 決定的な参照実装を提供してください。
pub trait BodyThermalBoundaryProvider: Send + Sync {
    /// body poseにおける露出境界とWorld cellの交差を返します。
    fn boundary_contacts(
        &self,
        body: BodyId,
        grid: &BodyLocalGrid,
        pose: BodyPose,
        world_units_per_cell: f64,
        max_contacts: usize,
    ) -> EngineResult<Vec<BodyBoundaryContact>>;
}

/// 剛体の占有形状とWorld cellが共有する正確な面積です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyVolumeContact {
    /// 交差するWorld cell。
    pub world_cell: CellPoint,
    /// 交差領域の面積重心。作用点やcoverage重み付けに使用できます。
    pub world_point: [f64; 2],
    /// 占有形状がWorld cell内に覆う面積。
    pub covered_area: f64,
}

/// 局所maskを正確に覆う、pivot基準の軸平行矩形です。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StencilRect {
    /// pivotからの左端。
    pub min_x: f64,
    /// pivotからの上端。
    pub min_y: f64,
    /// 矩形の幅。
    pub width: f64,
    /// 矩形の高さ。
    pub height: f64,
}

/// PhysicsWorld内で再利用しない接合IDです。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JointId(u32);

impl JointId {
    /// このPhysicsWorld内で再利用されない連番です。
    pub fn index(self) -> u32 {
        self.0
    }
}

/// 接合の拘束方式です。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum JointKind {
    /// 二物体間の相対poseを固定します。
    Fixed,
    /// 指定anchorを共有し、相対回転を許します。
    Revolute { limits: Option<[f64; 2]> },
}

/// 二物体を接続する局所anchorと拘束方式です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDef {
    /// 一方のbody ID。
    pub body1: BodyId,
    /// もう一方のbody ID。
    pub body2: BodyId,
    /// body1局所座標でのanchor。
    pub local_anchor1: [f64; 2],
    /// body2局所座標でのanchor。
    pub local_anchor2: [f64; 2],
    /// 拘束方式と任意の角度範囲。
    pub kind: JointKind,
}

impl BodyStencil {
    /// row-major占有maskを検査して作ります。
    pub fn new(
        width: u32,
        height: u32,
        cell_size: f64,
        pivot: [f64; 2],
        revision: u64,
        occupied: Vec<bool>,
    ) -> EngineResult<Self> {
        let count = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .ok_or_else(|| invalid("body stencil dimensions"))?;
        if width == 0
            || height == 0
            || count != occupied.len()
            || count > u32::MAX as usize
            || !cell_size.is_finite()
            || cell_size <= 0.0
            || !(f64::from(width) * cell_size).is_finite()
            || !(f64::from(height) * cell_size).is_finite()
            || pivot.iter().any(|value| !value.is_finite())
            || !(f64::from(width) * cell_size - pivot[0]).is_finite()
            || !(f64::from(height) * cell_size - pivot[1]).is_finite()
        {
            return Err(invalid("body stencil geometry"));
        }
        let mut stencil = Self {
            width,
            height,
            cell_size,
            pivot,
            revision,
            occupied,
            rectangles: Vec::new(),
        };
        stencil.rectangles = stencil.build_rectangle_decomposition();
        Ok(stencil)
    }

    /// maskの変更世代です。collider cacheはこの値を含めて更新します。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// exact occupancyをRapierへ渡すcompound collider定義へ変換します。
    pub fn collider_shape(&self) -> EngineResult<ColliderShape> {
        let rectangles = self.rectangle_decomposition();
        if rectangles.is_empty() {
            return Err(invalid("empty body stencil has no collider"));
        }
        Ok(ColliderShape::CompoundRects { rectangles })
    }

    /// 占有cell数です。
    pub fn occupied_count(&self) -> usize {
        self.occupied.iter().filter(|&&occupied| occupied).count()
    }

    /// 局所maskを覆う矩形を、上から下、左から右の決定順で返します。
    ///
    /// 同じ水平方向runを隣接row間でだけ結合するため、穴と分離領域を埋めません。
    pub fn rectangle_decomposition(&self) -> Vec<StencilRect> {
        self.rectangles.clone()
    }

    fn build_rectangle_decomposition(&self) -> Vec<StencilRect> {
        let mut finished = Vec::new();
        let mut active = BTreeMap::<(u32, u32), u32>::new();
        for y in 0..self.height {
            let mut runs = Vec::new();
            let mut x = 0;
            while x < self.width {
                if !self.occupied[y as usize * self.width as usize + x as usize] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.width && self.occupied[y as usize * self.width as usize + x as usize]
                {
                    x += 1;
                }
                runs.push((start, x));
            }
            let run_set = runs.iter().copied().collect::<BTreeSet<_>>();
            let stale = active
                .keys()
                .copied()
                .filter(|run| !run_set.contains(run))
                .collect::<Vec<_>>();
            for run in stale {
                let start_y = active.remove(&run).expect("active run exists");
                finished.push(self.rect_for(run, start_y, y));
            }
            for run in runs {
                active.entry(run).or_insert(y);
            }
        }
        for (run, start_y) in active {
            finished.push(self.rect_for(run, start_y, self.height));
        }
        finished
    }

    fn rect_for(&self, run: (u32, u32), start_y: u32, end_y: u32) -> StencilRect {
        StencilRect {
            min_x: f64::from(run.0) * self.cell_size - self.pivot[0],
            min_y: f64::from(start_y) * self.cell_size - self.pivot[1],
            width: f64::from(run.1 - run.0) * self.cell_size,
            height: f64::from(end_y - start_y) * self.cell_size,
        }
    }

    /// 指定poseで占有するWorld cellを決定順で返します。
    ///
    /// 回転後の局所矩形とWorld cellの面積交差をSATで調べます。矩形分割とcell単位の
    /// 走査量を見積もり、小さい方を選びます。全World走査や近似円queryは行いません。
    pub fn world_cells(&self, pose: BodyPose) -> EngineResult<Vec<CellPoint>> {
        let mut cells = Vec::new();
        self.world_cells_into(pose, &mut cells)?;
        Ok(cells)
    }

    /// 呼出側scratchへposeの占有cellを書き込みます。同じbufferを繰り返し使えます。
    pub fn world_cells_into(
        &self,
        pose: BodyPose,
        output: &mut Vec<CellPoint>,
    ) -> EngineResult<()> {
        validate_pose(pose)?;
        output.clear();
        let cell_bound = self
            .occupied_count()
            .checked_mul(9)
            .ok_or_else(|| EngineError::capacity("body stencil cells", u64::MAX, u64::MAX))?;
        let c = snap_cardinal(pose.angle.cos()).abs();
        let s = snap_cardinal(pose.angle.sin()).abs();
        let rectangle_bound = self.rectangles.iter().try_fold(0usize, |sum, rect| {
            let span_x = rect.width * c + rect.height * s;
            let span_y = rect.width * s + rect.height * c;
            let count_x = candidate_axis_cells(span_x, self.cell_size)?;
            let count_y = candidate_axis_cells(span_y, self.cell_size)?;
            sum.checked_add(count_x.checked_mul(count_y)?)
        });
        let use_rectangles = rectangle_bound.is_some_and(|bound| bound < cell_bound);
        let maximum_cells = rectangle_bound
            .filter(|_| use_rectangles)
            .unwrap_or(cell_bound);
        output
            .try_reserve(maximum_cells)
            .map_err(crate::foundation::error::allocation)?;
        if use_rectangles {
            for rect in &self.rectangles {
                append_polygon_cells(self.transformed_rect(*rect, pose), self.cell_size, output)?;
            }
        } else {
            for index in 0..self.occupied.len() {
                if !self.occupied[index] {
                    continue;
                }
                let x = (index as u32) % self.width;
                let y = (index as u32) / self.width;
                let rect = StencilRect {
                    min_x: f64::from(x) * self.cell_size - self.pivot[0],
                    min_y: f64::from(y) * self.cell_size - self.pivot[1],
                    width: self.cell_size,
                    height: self.cell_size,
                };
                append_polygon_cells(self.transformed_rect(rect, pose), self.cell_size, output)?;
            }
        }
        output.sort_unstable_by_key(|point| (point.y, point.x));
        output.dedup();
        Ok(())
    }

    /// pose後のBodyStencil外周を構成する露出面線分を決定順で返します。
    ///
    /// 共有するlocal cell edgeは出力しません。穴の内周は露出面として含めます。
    /// 返す線分は幾何そのものなので、後段のfluid/熱連成がWorld gridとの交差長を計算
    /// できます。出力数はoccupied cell数×4以下です。
    pub fn boundary_segments(&self, pose: BodyPose) -> EngineResult<Vec<BodyBoundarySegment>> {
        let mut segments = Vec::new();
        self.boundary_segments_into(pose, &mut segments)?;
        Ok(segments)
    }

    fn boundary_segments_into(
        &self,
        pose: BodyPose,
        segments: &mut Vec<BodyBoundarySegment>,
    ) -> EngineResult<()> {
        validate_pose(pose)?;
        let maximum = self
            .occupied_count()
            .checked_mul(4)
            .ok_or_else(|| EngineError::capacity("body boundary segments", u64::MAX, u64::MAX))?;
        segments.clear();
        if segments.capacity() < maximum.min(64) {
            segments
                .try_reserve_exact(maximum.min(64) - segments.len())
                .map_err(crate::foundation::error::allocation)?;
        }
        let cosine = snap_cardinal(pose.angle.cos());
        let sine = snap_cardinal(pose.angle.sin());
        let transform = |x: f64, y: f64| {
            [
                pose.x + cosine * x - sine * y,
                pose.y + sine * x + cosine * y,
            ]
        };
        for index in 0..self.occupied.len() {
            if !self.occupied[index] {
                continue;
            }
            let x = index as u32 % self.width;
            let y = index as u32 / self.width;
            let left = f64::from(x) * self.cell_size - self.pivot[0];
            let top = f64::from(y) * self.cell_size - self.pivot[1];
            let right = left + self.cell_size;
            let bottom = top + self.cell_size;
            for (face, neighbor, start, end) in [
                (
                    BodyThermalBoundaryFace::Left,
                    if x > 0 { Some((x - 1, y)) } else { None },
                    [left, top],
                    [left, bottom],
                ),
                (
                    BodyThermalBoundaryFace::Right,
                    (x + 1 < self.width).then_some((x + 1, y)),
                    [right, top],
                    [right, bottom],
                ),
                (
                    BodyThermalBoundaryFace::Top,
                    if y > 0 { Some((x, y - 1)) } else { None },
                    [left, top],
                    [right, top],
                ),
                (
                    BodyThermalBoundaryFace::Bottom,
                    (y + 1 < self.height).then_some((x, y + 1)),
                    [left, bottom],
                    [right, bottom],
                ),
            ] {
                let shared = neighbor.is_some_and(|(nx, ny)| {
                    self.occupied[ny as usize * self.width as usize + nx as usize]
                });
                if !shared {
                    let start = transform(start[0], start[1]);
                    let end = transform(end[0], end[1]);
                    let length = (end[0] - start[0]).hypot(end[1] - start[1]);
                    if start.iter().chain(&end).any(|value| !value.is_finite())
                        || !length.is_finite()
                        || length == 0.0
                    {
                        return Err(invalid("body boundary segment座標overflow"));
                    }
                    if segments.len() == segments.capacity() {
                        segments
                            .try_reserve(1)
                            .map_err(crate::foundation::error::allocation)?;
                    }
                    segments.push(BodyBoundarySegment {
                        local_cell: [x, y],
                        face,
                        start,
                        end,
                    });
                }
            }
        }
        Ok(())
    }

    /// 露出境界線分をWorld格子cellへ分割し、cellごとの正確な交差長を返します。
    ///
    /// `world_cell_size`は原点(0, 0)を基準とするWorld格子の一辺、`maximum_contacts`は
    /// 返却上限です。線分の始点・終点や格子交点がi32 cell範囲を越える場合、または
    /// 上限を越える場合は失敗します。World境界内か、chunkがresidentかの確認は呼出側が
    /// Worldのcoverage APIで行います。交差長の合計は露出面長の合計と一致します。
    pub fn boundary_contacts(
        &self,
        pose: BodyPose,
        world_cell_size: f64,
        maximum_contacts: usize,
    ) -> EngineResult<Vec<BodyBoundaryContact>> {
        let mut segments = Vec::new();
        let mut cuts = Vec::new();
        let mut contacts = Vec::new();
        self.boundary_contacts_into(
            pose,
            world_cell_size,
            maximum_contacts,
            &mut segments,
            &mut cuts,
            &mut contacts,
        )?;
        Ok(contacts)
    }

    /// 占有矩形とWorld格子cellの正確な面積交差を決定順で返します。
    ///
    /// 各occupied local cellをposeで回転した凸四角形としてWorld cellへclipします。
    /// 共有境界の面積は0として除外し、結果はWorld座標順に統合します。出力上限を超え
    /// た場合は全結果を返さずCapacityExceededにします。
    pub fn volume_contacts(
        &self,
        pose: BodyPose,
        world_cell_size: f64,
        maximum_contacts: usize,
    ) -> EngineResult<Vec<BodyVolumeContact>> {
        let mut contacts = Vec::new();
        let mut polygon = Vec::new();
        let mut clipped = Vec::new();
        let mut clip_scratch = Vec::new();
        self.volume_contacts_into(
            pose,
            world_cell_size,
            maximum_contacts,
            &mut contacts,
            &mut polygon,
            &mut clipped,
            &mut clip_scratch,
        )?;
        Ok(contacts)
    }

    #[allow(clippy::too_many_arguments)]
    fn volume_contacts_into(
        &self,
        pose: BodyPose,
        world_cell_size: f64,
        maximum_contacts: usize,
        contacts: &mut Vec<BodyVolumeContact>,
        polygon: &mut Vec<[f64; 2]>,
        clipped: &mut Vec<[f64; 2]>,
        clip_scratch: &mut Vec<[f64; 2]>,
    ) -> EngineResult<()> {
        validate_pose(pose)?;
        if !world_cell_size.is_finite() || world_cell_size <= 0.0 {
            return Err(invalid("World volume contact cell size"));
        }
        contacts.clear();
        polygon
            .try_reserve(12usize.saturating_sub(polygon.len()))
            .map_err(allocation)?;
        clipped
            .try_reserve(12usize.saturating_sub(clipped.len()))
            .map_err(allocation)?;
        clip_scratch
            .try_reserve(12usize.saturating_sub(clip_scratch.len()))
            .map_err(allocation)?;
        let mut visited = 0usize;
        let visit_limit = maximum_contacts.saturating_mul(16).max(maximum_contacts);
        for index in 0..self.occupied.len() {
            if !self.occupied[index] {
                continue;
            }
            let x = index as u32 % self.width;
            let y = index as u32 / self.width;
            let rect = StencilRect {
                min_x: f64::from(x) * self.cell_size - self.pivot[0],
                min_y: f64::from(y) * self.cell_size - self.pivot[1],
                width: self.cell_size,
                height: self.cell_size,
            };
            let quad = self.transformed_rect(rect, pose);
            if quad
                .iter()
                .flatten()
                .any(|coordinate| !coordinate.is_finite())
            {
                return Err(invalid("Body volume contact coordinate overflow"));
            }
            let min_x = quad
                .iter()
                .map(|point| point[0])
                .fold(f64::INFINITY, f64::min);
            let min_y = quad
                .iter()
                .map(|point| point[1])
                .fold(f64::INFINITY, f64::min);
            let max_x = quad
                .iter()
                .map(|point| point[0])
                .fold(f64::NEG_INFINITY, f64::max);
            let max_y = quad
                .iter()
                .map(|point| point[1])
                .fold(f64::NEG_INFINITY, f64::max);
            let x0 = checked_cell_floor(min_x / world_cell_size)?;
            let y0 = checked_cell_floor(min_y / world_cell_size)?;
            let x1 = checked_cell_floor(max_x / world_cell_size)?;
            let y1 = checked_cell_floor(max_y / world_cell_size)?;
            polygon.clear();
            polygon.extend_from_slice(&quad);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    visited = visited
                        .checked_add(1)
                        .ok_or_else(|| invalid("Body volume candidate count overflow"))?;
                    if visited > visit_limit {
                        return Err(EngineError::capacity(
                            "body volume candidate cells",
                            visited as u64,
                            visit_limit as u64,
                        ));
                    }
                    let left = f64::from(cx) * world_cell_size;
                    let top = f64::from(cy) * world_cell_size;
                    clip_polygon_to_cell(
                        polygon,
                        left,
                        top,
                        world_cell_size,
                        clipped,
                        clip_scratch,
                    )?;
                    let (area, centroid) = polygon_area_centroid(clipped)?;
                    if area == 0.0 {
                        continue;
                    }
                    if contacts.len() == contacts.capacity() {
                        contacts.try_reserve(1).map_err(allocation)?;
                    }
                    contacts.push(BodyVolumeContact {
                        world_cell: CellPoint { x: cx, y: cy },
                        world_point: centroid,
                        covered_area: area,
                    });
                }
            }
        }
        contacts.sort_unstable_by_key(|contact| (contact.world_cell.y, contact.world_cell.x));
        let mut unique_len = 0usize;
        for read in 0..contacts.len() {
            let contact = contacts[read];
            if unique_len != 0 && contacts[unique_len - 1].world_cell == contact.world_cell {
                let unique = &mut contacts[unique_len - 1];
                let area = unique.covered_area + contact.covered_area;
                unique.world_point = [
                    (unique.world_point[0] * unique.covered_area
                        + contact.world_point[0] * contact.covered_area)
                        / area,
                    (unique.world_point[1] * unique.covered_area
                        + contact.world_point[1] * contact.covered_area)
                        / area,
                ];
                unique.covered_area = area;
                if !area.is_finite() || unique.world_point.iter().any(|value| !value.is_finite()) {
                    return Err(invalid("Body volume contact area overflow"));
                }
            } else {
                contacts[unique_len] = contact;
                unique_len += 1;
            }
        }
        contacts.truncate(unique_len);
        let maximum_cell_area = world_cell_size * world_cell_size;
        if !maximum_cell_area.is_finite()
            || contacts.iter().any(|contact| {
                contact.covered_area > maximum_cell_area * (1.0 + 64.0 * f64::EPSILON)
            })
        {
            return Err(invalid("Body volume contact exceeds World cell area"));
        }
        if contacts.len() > maximum_contacts {
            return Err(EngineError::capacity(
                "body volume contacts",
                contacts.len() as u64,
                maximum_contacts as u64,
            ));
        }
        Ok(())
    }

    fn boundary_contacts_into(
        &self,
        pose: BodyPose,
        world_cell_size: f64,
        maximum_contacts: usize,
        segments: &mut Vec<BodyBoundarySegment>,
        cuts: &mut Vec<f64>,
        contacts: &mut Vec<BodyBoundaryContact>,
    ) -> EngineResult<()> {
        if !world_cell_size.is_finite() || world_cell_size <= 0.0 {
            return Err(invalid("World boundary contact cell size"));
        }
        self.boundary_segments_into(pose, segments)?;
        let segment_count = segments.len();
        contacts.clear();
        let initial_capacity = maximum_contacts.min(segments.len());
        if contacts.capacity() < initial_capacity {
            contacts
                .try_reserve_exact(initial_capacity - contacts.len())
                .map_err(crate::foundation::error::allocation)?;
        }
        if cuts.capacity() < 16 {
            cuts.try_reserve_exact(16 - cuts.len())
                .map_err(crate::foundation::error::allocation)?;
        }
        let cosine = snap_cardinal(pose.angle.cos());
        let sine = snap_cardinal(pose.angle.sin());
        for segment in segments.iter().copied() {
            cuts.clear();
            cuts.push(0.0);
            cuts.push(1.0);
            let delta = [
                segment.end[0] - segment.start[0],
                segment.end[1] - segment.start[1],
            ];
            let scaled_min = [
                segment.start[0].min(segment.end[0]) / world_cell_size,
                segment.start[1].min(segment.end[1]) / world_cell_size,
            ];
            let scaled_max = [
                segment.start[0].max(segment.end[0]) / world_cell_size,
                segment.start[1].max(segment.end[1]) / world_cell_size,
            ];
            for axis in 0..2 {
                if !scaled_min[axis].is_finite()
                    || !scaled_max[axis].is_finite()
                    || scaled_min[axis] < f64::from(i32::MIN) - 1.0
                    || scaled_max[axis] > f64::from(i32::MAX) + 1.0
                {
                    return Err(invalid("World boundary contact cell coordinate range"));
                }
                if delta[axis] == 0.0 {
                    continue;
                }
                let first_line = scaled_min[axis].floor() as i64 + 1;
                let last_line = scaled_max[axis].floor() as i64;
                if first_line > last_line {
                    continue;
                }
                let crossing_count = usize::try_from(last_line - first_line + 1)
                    .map_err(|_| invalid("World boundary contact crossing count"))?;
                let maximum_cuts = maximum_contacts
                    .checked_add(segment_count.saturating_mul(2))
                    .ok_or_else(|| invalid("World boundary contact capacity overflow"))?;
                if cuts.len().saturating_add(crossing_count) > maximum_cuts {
                    return Err(EngineError::capacity(
                        "body boundary contacts",
                        contacts
                            .len()
                            .saturating_add(cuts.len())
                            .saturating_add(crossing_count) as u64,
                        maximum_contacts as u64,
                    ));
                }
                cuts.try_reserve(crossing_count)
                    .map_err(crate::foundation::error::allocation)?;
                for line in first_line..=last_line {
                    let coordinate = line as f64 * world_cell_size;
                    let t = (coordinate - segment.start[axis]) / delta[axis];
                    if t > 0.0 && t < 1.0 {
                        cuts.push(t);
                    }
                }
            }
            cuts.sort_unstable_by(f64::total_cmp);
            let mut unique = 1usize;
            for index in 1..cuts.len() {
                let previous = cuts[unique - 1];
                let current = cuts[index];
                let tolerance = 16.0 * f64::EPSILON * previous.abs().max(current.abs()).max(1.0);
                if current - previous > tolerance {
                    cuts[unique] = current;
                    unique += 1;
                }
            }
            let outward = match segment.face {
                BodyThermalBoundaryFace::Left => [-cosine, -sine],
                BodyThermalBoundaryFace::Right => [cosine, sine],
                BodyThermalBoundaryFace::Top => [sine, -cosine],
                BodyThermalBoundaryFace::Bottom => [-sine, cosine],
            };
            let length = delta[0].hypot(delta[1]);
            for interval in cuts[..unique].windows(2) {
                let span = interval[1] - interval[0];
                if span <= 0.0 {
                    continue;
                }
                if contacts.len() >= maximum_contacts {
                    return Err(EngineError::capacity(
                        "body boundary contacts",
                        contacts.len().saturating_add(1) as u64,
                        maximum_contacts as u64,
                    ));
                }
                let t = interval[0] + span * 0.5;
                let point = [
                    segment.start[0] + delta[0] * t,
                    segment.start[1] + delta[1] * t,
                ];
                let epsilon = (world_cell_size * 1.0e-12).max(
                    8.0 * f64::EPSILON * point[0].abs().max(point[1].abs()).max(world_cell_size),
                );
                if epsilon >= world_cell_size * 0.25 {
                    return Err(invalid("World boundary contact coordinate precision"));
                }
                let sample = [
                    point[0] + outward[0] * epsilon,
                    point[1] + outward[1] * epsilon,
                ];
                let cell_x = (sample[0] / world_cell_size).floor();
                let cell_y = (sample[1] / world_cell_size).floor();
                if !cell_x.is_finite()
                    || !cell_y.is_finite()
                    || cell_x < f64::from(i32::MIN)
                    || cell_x > f64::from(i32::MAX)
                    || cell_y < f64::from(i32::MIN)
                    || cell_y > f64::from(i32::MAX)
                {
                    return Err(invalid("World boundary contact cell coordinate range"));
                }
                if contacts.len() == contacts.capacity() {
                    contacts
                        .try_reserve(1)
                        .map_err(crate::foundation::error::allocation)?;
                }
                contacts.push(BodyBoundaryContact {
                    local_cell: segment.local_cell,
                    face: segment.face,
                    world_cell: CellPoint {
                        x: cell_x as i32,
                        y: cell_y as i32,
                    },
                    world_point: point,
                    covered_length: length * span,
                });
            }
        }
        Ok(())
    }

    /// 二つのpose間の予約候補となるoccupancy sampleを返します。
    ///
    /// 並進量と外周の回転移動量から、sample位置間の移動が半cell以下になる数を選びます。
    /// これはWorld容量予約用であり、sample間のexact collision queryを代替しません。callerが
    /// 予約したsample上限を超える場合は結果を返さず、stepをrollbackできるようにします。
    pub fn sampled_sweep_world_cells(
        &self,
        start: BodyPose,
        end: BodyPose,
        max_samples: u32,
    ) -> EngineResult<Vec<CellPoint>> {
        let mut cells = Vec::new();
        self.sampled_sweep_world_cells_into(start, end, max_samples, &mut cells)?;
        Ok(cells)
    }

    /// 呼出側scratchへ掃引領域を出力します。最大sample数と同じ上限で探索を止めます。
    pub fn sampled_sweep_world_cells_into(
        &self,
        start: BodyPose,
        end: BodyPose,
        max_samples: u32,
        output: &mut Vec<CellPoint>,
    ) -> EngineResult<()> {
        let mut sample = Vec::new();
        self.sampled_sweep_world_cells_with_scratch(start, end, max_samples, output, &mut sample)
    }

    /// 掃引結果と一姿勢分のscratchを両方再利用してWorld容量候補を作ります。
    pub fn sampled_sweep_world_cells_with_scratch(
        &self,
        start: BodyPose,
        end: BodyPose,
        max_samples: u32,
        output: &mut Vec<CellPoint>,
        sample: &mut Vec<CellPoint>,
    ) -> EngineResult<()> {
        validate_pose(start)?;
        validate_pose(end)?;
        if max_samples == 0 {
            return Err(invalid("body stencil sweep sample limit"));
        }
        let radius = self.max_radius();
        let translation = (end.x - start.x).hypot(end.y - start.y);
        let angular = (end.angle - start.angle).abs() * radius;
        let distance = translation + angular;
        if !distance.is_finite() {
            return Err(invalid("body stencil sweep distance"));
        }
        let steps = (distance / (self.cell_size * 0.5)).ceil().max(1.0);
        if steps + 1.0 > f64::from(max_samples) {
            return Err(EngineError::new(
                ErrorCode::CapacityExceeded,
                "body stencil sweep samples",
            ));
        }
        let intervals = steps as u32;
        output.clear();
        let sample_count = (intervals as usize).checked_add(1).ok_or_else(|| {
            EngineError::capacity("body stencil sweep samples", u64::MAX, u64::MAX)
        })?;
        let maximum_cells = self
            .occupied_count()
            .checked_mul(9)
            .and_then(|cells| cells.checked_mul(sample_count))
            .ok_or_else(|| EngineError::capacity("body stencil sweep cells", u64::MAX, u64::MAX))?;
        output
            .try_reserve_exact(maximum_cells)
            .map_err(crate::foundation::error::allocation)?;
        sample
            .try_reserve(self.occupied_count().saturating_mul(9))
            .map_err(crate::foundation::error::allocation)?;
        for i in 0..=intervals {
            let t = f64::from(i) / f64::from(intervals);
            let pose = BodyPose {
                x: start.x + (end.x - start.x) * t,
                y: start.y + (end.y - start.y) * t,
                angle: start.angle + (end.angle - start.angle) * t,
            };
            self.world_cells_into(pose, &mut *sample)?;
            output
                .try_reserve(sample.len())
                .map_err(crate::foundation::error::allocation)?;
            output.extend_from_slice(sample);
        }
        output.sort_unstable_by_key(|point| (point.y, point.x));
        output.dedup();
        Ok(())
    }

    fn max_radius(&self) -> f64 {
        let mut radius = 0.0f64;
        for rect in &self.rectangles {
            for y in [rect.min_y, rect.min_y + rect.height] {
                for x in [rect.min_x, rect.min_x + rect.width] {
                    radius = radius.max(x.hypot(y));
                }
            }
        }
        radius
    }

    fn is_aligned_to_world_grid(&self, pose: BodyPose, world_cell_size: f64) -> bool {
        if self.cell_size != world_cell_size {
            return false;
        }
        let cosine = snap_cardinal(pose.angle.cos());
        let sine = snap_cardinal(pose.angle.sin());
        if cosine.abs() + sine.abs() != 1.0 {
            return false;
        }
        let origin_x = pose.x - cosine * self.pivot[0] + sine * self.pivot[1];
        let origin_y = pose.y - sine * self.pivot[0] - cosine * self.pivot[1];
        [origin_x / world_cell_size, origin_y / world_cell_size]
            .iter()
            .all(|coordinate| {
                let nearest = coordinate.round();
                (coordinate - nearest).abs() <= 8.0 * f64::EPSILON * coordinate.abs().max(1.0)
            })
    }

    fn transformed_rect(&self, rect: StencilRect, pose: BodyPose) -> [[f64; 2]; 4] {
        let c = snap_cardinal(pose.angle.cos());
        let s = snap_cardinal(pose.angle.sin());
        let transform = |x: f64, y: f64| [pose.x + c * x - s * y, pose.y + s * x + c * y];
        [
            transform(rect.min_x, rect.min_y),
            transform(rect.min_x + rect.width, rect.min_y),
            transform(rect.min_x + rect.width, rect.min_y + rect.height),
            transform(rect.min_x, rect.min_y + rect.height),
        ]
    }
}

fn candidate_axis_cells(span: f64, cell_size: f64) -> Option<usize> {
    let count = (span / cell_size).ceil() + 2.0;
    if !count.is_finite() || count > usize::MAX as f64 {
        None
    } else {
        Some(count as usize)
    }
}

fn append_polygon_cells(
    polygon: [[f64; 2]; 4],
    cell_size: f64,
    output: &mut Vec<CellPoint>,
) -> EngineResult<()> {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for point in polygon {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    let min_x = checked_cell_floor(min[0] / cell_size)?;
    let min_y = checked_cell_floor(min[1] / cell_size)?;
    let max_x = checked_cell_floor(max[0] / cell_size)?;
    let max_y = checked_cell_floor(max[1] / cell_size)?;
    for cy in min_y..=max_y {
        for cx in min_x..=max_x {
            let left = f64::from(cx) * cell_size;
            let top = f64::from(cy) * cell_size;
            if square_intersects_cell(polygon, left, top, cell_size) {
                output.push(CellPoint { x: cx, y: cy });
            }
        }
    }
    Ok(())
}

fn snap_cardinal(value: f64) -> f64 {
    const TOLERANCE: f64 = 8.0 * f64::EPSILON;
    if value.abs() <= TOLERANCE {
        0.0
    } else if (value.abs() - 1.0).abs() <= TOLERANCE {
        value.signum()
    } else {
        value
    }
}

fn validate_pose(pose: BodyPose) -> EngineResult<()> {
    if [pose.x, pose.y, pose.angle]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err(invalid("body stencil pose"));
    }
    Ok(())
}

fn checked_cell_floor(value: f64) -> EngineResult<i32> {
    let cell = value.floor();
    if !cell.is_finite() || cell < f64::from(i32::MIN) || cell > f64::from(i32::MAX) {
        return Err(invalid("body stencil cell coordinate"));
    }
    Ok(cell as i32)
}

fn square_intersects_cell(polygon: [[f64; 2]; 4], left: f64, top: f64, size: f64) -> bool {
    let square = [
        [left, top],
        [left + size, top],
        [left + size, top + size],
        [left, top + size],
    ];
    let axes = [
        [1.0, 0.0],
        [0.0, 1.0],
        [polygon[1][1] - polygon[0][1], polygon[0][0] - polygon[1][0]],
        [polygon[2][1] - polygon[1][1], polygon[1][0] - polygon[2][0]],
    ];
    axes.iter().all(|axis| {
        let (amin, amax) = projection_range(&polygon, *axis);
        let (bmin, bmax) = projection_range(&square, *axis);
        // 境界だけが接するcellは占有せず、面積を共有する場合だけ含めます。
        amax > bmin && bmax > amin
    })
}

fn projection_range(points: &[[f64; 2]; 4], axis: [f64; 2]) -> (f64, f64) {
    let mut min = points[0][0] * axis[0] + points[0][1] * axis[1];
    let mut max = min;
    for point in &points[1..] {
        let projection = point[0] * axis[0] + point[1] * axis[1];
        min = min.min(projection);
        max = max.max(projection);
    }
    (min, max)
}

fn clip_polygon_to_cell(
    polygon: &[[f64; 2]],
    left: f64,
    top: f64,
    size: f64,
    output: &mut Vec<[f64; 2]>,
    scratch: &mut Vec<[f64; 2]>,
) -> EngineResult<()> {
    output.clear();
    output
        .try_reserve(12usize.saturating_sub(output.len()))
        .map_err(allocation)?;
    output.extend_from_slice(polygon);
    for (axis, bound, keep_greater) in [
        (0usize, left, true),
        (0usize, left + size, false),
        (1usize, top, true),
        (1usize, top + size, false),
    ] {
        scratch.clear();
        scratch
            .try_reserve(output.len().saturating_add(2).saturating_sub(scratch.len()))
            .map_err(allocation)?;
        if output.is_empty() {
            return Ok(());
        }
        let inside = |point: [f64; 2]| {
            if keep_greater {
                point[axis] >= bound
            } else {
                point[axis] <= bound
            }
        };
        let mut previous = *output
            .last()
            .ok_or_else(|| invalid("empty clipping polygon"))?;
        let mut previous_inside = inside(previous);
        for current in output.iter().copied() {
            let current_inside = inside(current);
            if current_inside != previous_inside {
                let denominator = current[axis] - previous[axis];
                if denominator == 0.0 || !denominator.is_finite() {
                    return Err(invalid("Body volume clipping denominator"));
                }
                let t = (bound - previous[axis]) / denominator;
                let point = [
                    previous[0] + (current[0] - previous[0]) * t,
                    previous[1] + (current[1] - previous[1]) * t,
                ];
                if point.iter().any(|coordinate| !coordinate.is_finite()) {
                    return Err(invalid("Body volume clipping coordinate"));
                }
                scratch.push(point);
            }
            if current_inside {
                scratch.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
        std::mem::swap(output, scratch);
    }
    Ok(())
}

fn polygon_area_centroid(polygon: &[[f64; 2]]) -> EngineResult<(f64, [f64; 2])> {
    if polygon.len() < 3 {
        return Ok((0.0, [0.0; 2]));
    }
    let origin = polygon[0];
    let mut twice_area = 0.0;
    let mut centroid_x = 0.0;
    let mut centroid_y = 0.0;
    for index in 0..polygon.len() {
        let a = polygon[index];
        let b = polygon[(index + 1) % polygon.len()];
        let relative_a = [a[0] - origin[0], a[1] - origin[1]];
        let relative_b = [b[0] - origin[0], b[1] - origin[1]];
        let cross = relative_a[0] * relative_b[1] - relative_b[0] * relative_a[1];
        twice_area += cross;
        centroid_x += (relative_a[0] + relative_b[0]) * cross;
        centroid_y += (relative_a[1] + relative_b[1]) * cross;
    }
    if !twice_area.is_finite() || twice_area == 0.0 {
        return Ok((0.0, [0.0; 2]));
    }
    let centroid = [
        origin[0] + centroid_x / (3.0 * twice_area),
        origin[1] + centroid_y / (3.0 * twice_area),
    ];
    let area = twice_area.abs() * 0.5;
    if !area.is_finite() || centroid.iter().any(|coordinate| !coordinate.is_finite()) {
        return Err(invalid("Body volume polygon area or centroid overflow"));
    }
    Ok((area, centroid))
}

/// 物理stepへ適用するWorld単位と容量です。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyPhysicsSettings {
    /// 1秒あたりの固定step数。描画頻度から独立します。
    pub step_hz: u32,
    /// 重力加速度。正のYは下向きです。
    pub gravity: [f64; 2],
    /// Worldに同時に保持できる剛体数です。
    pub body_capacity: u32,
    /// 一論理stepで公開できる標準solver接触eventの上限です。
    pub contact_event_capacity: u32,
    /// Worldに同時に保持できるjoint数です。
    pub joint_capacity: u32,
    /// 全body local gridの占有cell総数です。
    pub body_local_cell_capacity: u32,
    /// 全body local gridで走査・collider化する矩形領域の総cell数です。
    pub body_local_geometry_cell_capacity: u32,
    /// 全body local gridのsidecar key・版・payload総byte数です。
    pub body_local_sidecar_byte_capacity: u32,
    /// 保存byte列の上限です。
    pub snapshot_byte_limit: u32,
    /// `max_substep_displacement_cells`で指定した移動条件を満たすWorld所有physics stepのsubstep上限です。
    ///
    /// 予測substep数が上限を超える場合、または実測移動が境界を超える場合はWorldへcommitしません。
    pub max_substeps: u32,
    /// 各substepで許す並進距離とshape外周の回転移動量の合計を表すcell比です。
    ///
    /// 標準profileは0.5です。作者はWorld構築時にゲームの衝突profileへ合わせて変更できます。
    pub max_substep_displacement_cells: f64,
    /// 一つのfluidセルとの連成で交換できる力積の絶対値上限です。単位はmass × world unit / secondです。
    pub max_fluid_coupling_impulse: f64,
}

impl Default for BodyPhysicsSettings {
    fn default() -> Self {
        Self {
            step_hz: 60,
            gravity: [0.0, 627.84],
            body_capacity: 2048,
            contact_event_capacity: 16_384,
            joint_capacity: 2048,
            body_local_cell_capacity: 262_144,
            body_local_geometry_cell_capacity: 262_144,
            body_local_sidecar_byte_capacity: 64 * 1024 * 1024,
            snapshot_byte_limit: 64 * 1024 * 1024,
            max_substeps: 64,
            max_substep_displacement_cells: 0.5,
            max_fluid_coupling_impulse: 4096.0,
        }
    }
}

impl BodyPhysicsSettings {
    fn validate(self) -> EngineResult<Self> {
        if self.step_hz == 0
            || self.body_capacity == 0
            || self.contact_event_capacity == 0
            || self.joint_capacity == 0
            || self.body_local_cell_capacity == 0
            || self.body_local_geometry_cell_capacity == 0
            || self.body_local_sidecar_byte_capacity == 0
            || self.snapshot_byte_limit == 0
            || self.max_substeps == 0
            || !self.max_substep_displacement_cells.is_finite()
            || self.max_substep_displacement_cells <= 0.0
            || !self.max_fluid_coupling_impulse.is_finite()
            || self.max_fluid_coupling_impulse <= 0.0
            || self.gravity.iter().any(|value| !value.is_finite())
        {
            return Err(invalid("剛体物理設定"));
        }
        Ok(self)
    }
}

/// backendへ渡すshape定義です。寸法はWorldの物理単位で指定します。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColliderShape {
    /// 半径を持つ円です。
    Circle { radius: f64 },
    /// 中心からの半幅と半高を持つ矩形です。
    Cuboid { half_extents: [f64; 2] },
    /// 両端点を半径で結ぶcapsuleです。
    Capsule {
        start: [f64; 2],
        end: [f64; 2],
        radius: f64,
    },
    /// BodyStencilを厳密に覆う、pivot基準の矩形compoundです。
    CompoundRects { rectangles: Vec<StencilRect> },
    /// 頂点を周回順に指定する、厳密な凸多角形です。
    ///
    /// 既存snapshotのenum tagを保つため、variantは既存shapeの後ろへ追加します。
    ConvexPolygon { vertices: Vec<[f64; 2]> },
}

impl ColliderShape {
    fn validate_query_shape(&self) -> EngineResult<()> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        let finite_point = |point: [f64; 2]| point.iter().all(|value| value.is_finite());
        match self {
            Self::Circle { radius } if positive(*radius) => Ok(()),
            Self::Cuboid { half_extents } if half_extents.iter().all(|value| positive(*value)) => {
                Ok(())
            }
            Self::Capsule { start, end, radius }
                if finite_point(*start)
                    && finite_point(*end)
                    && positive(*radius)
                    && start != end =>
            {
                Ok(())
            }
            Self::ConvexPolygon { vertices } if valid_convex_polygon(vertices) => Ok(()),
            Self::CompoundRects { rectangles }
                if !rectangles.is_empty()
                    && rectangles.len() <= 65_536
                    && rectangles.iter().all(|rect| {
                        [rect.min_x, rect.min_y, rect.width, rect.height]
                            .iter()
                            .all(|value| value.is_finite())
                            && rect.width > 0.0
                            && rect.height > 0.0
                    }) =>
            {
                Ok(())
            }
            _ => Err(invalid("CollisionQueries shape")),
        }
    }

    fn build(&self) -> EngineResult<ColliderBuilder> {
        Ok(ColliderBuilder::new(self.shared_shape()?))
    }

    fn shared_shape(&self) -> EngineResult<SharedShape> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        let finite_point = |point: [f64; 2]| point.iter().all(|value| value.is_finite());
        match self {
            Self::Circle { radius } if positive(*radius) => Ok(SharedShape::ball(*radius)),
            Self::Cuboid { half_extents } if half_extents.iter().all(|value| positive(*value)) => {
                Ok(SharedShape::cuboid(half_extents[0], half_extents[1]))
            }
            Self::Capsule { start, end, radius }
                if finite_point(*start)
                    && finite_point(*end)
                    && positive(*radius)
                    && start != end =>
            {
                let a = Vector::new(start[0], start[1]);
                let b = Vector::new(end[0], end[1]);
                Ok(SharedShape::capsule(a, b, *radius))
            }
            Self::ConvexPolygon { vertices } if valid_convex_polygon(vertices) => {
                let mut points = Vec::new();
                points
                    .try_reserve_exact(vertices.len())
                    .map_err(allocation)?;
                points.extend(vertices.iter().map(|point| Vector::new(point[0], point[1])));
                SharedShape::convex_hull(&points)
                    .ok_or_else(|| invalid("convex polygon could not form a Rapier shape"))
            }
            Self::CompoundRects { rectangles }
                if !rectangles.is_empty()
                    && rectangles.iter().all(|rect| {
                        [rect.min_x, rect.min_y, rect.width, rect.height]
                            .iter()
                            .all(|value| value.is_finite())
                            && rect.width > 0.0
                            && rect.height > 0.0
                    }) =>
            {
                let shapes = rectangles
                    .iter()
                    .map(|rect| {
                        (
                            Pose::translation(
                                rect.min_x + rect.width * 0.5,
                                rect.min_y + rect.height * 0.5,
                            ),
                            SharedShape::cuboid(rect.width * 0.5, rect.height * 0.5),
                        )
                    })
                    .collect();
                Ok(SharedShape::compound(shapes))
            }
            _ => Err(invalid("collider shape")),
        }
    }
}

fn valid_convex_polygon(vertices: &[[f64; 2]]) -> bool {
    if !(3..=256).contains(&vertices.len())
        || vertices
            .iter()
            .any(|point| point.iter().any(|value| !value.is_finite()))
    {
        return false;
    }
    let mut orientation = 0.0_f64;
    let mut signed_area = 0.0_f64;
    for index in 0..vertices.len() {
        let previous = vertices[(index + vertices.len() - 1) % vertices.len()];
        let current = vertices[index];
        let next = vertices[(index + 1) % vertices.len()];
        let first = [current[0] - previous[0], current[1] - previous[1]];
        let second = [next[0] - current[0], next[1] - current[1]];
        let cross = first[0] * second[1] - first[1] * second[0];
        if !cross.is_finite() || cross == 0.0 {
            return false;
        }
        if orientation == 0.0 {
            orientation = cross.signum();
        } else if cross.signum() != orientation {
            return false;
        }
        signed_area += current[0] * next[1] - current[1] * next[0];
    }
    if !signed_area.is_finite() || signed_area == 0.0 {
        return false;
    }
    for first in 0..vertices.len() {
        let first_next = (first + 1) % vertices.len();
        for second in first + 1..vertices.len() {
            let second_next = (second + 1) % vertices.len();
            if first == second || first_next == second || second_next == first {
                continue;
            }
            if segments_intersect(
                vertices[first],
                vertices[first_next],
                vertices[second],
                vertices[second_next],
            ) {
                return false;
            }
        }
    }
    true
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    fn orientation(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }
    fn between(value: f64, left: f64, right: f64) -> bool {
        value >= left.min(right) && value <= left.max(right)
    }
    fn on_segment(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> bool {
        between(point[0], a[0], b[0]) && between(point[1], a[1], b[1])
    }

    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    if (abc < 0.0 && abd > 0.0 || abc > 0.0 && abd < 0.0)
        && (cda < 0.0 && cdb > 0.0 || cda > 0.0 && cdb < 0.0)
    {
        return true;
    }
    (abc == 0.0 && on_segment(a, b, c))
        || (abd == 0.0 && on_segment(a, b, d))
        || (cda == 0.0 && on_segment(c, d, a))
        || (cdb == 0.0 && on_segment(c, d, b))
}

/// 物理状態を持つbodyの種類です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyMotionType {
    /// 固定地形や固定構造物です。
    Fixed,
    /// 指定した次poseへ動き、衝突物へ速度を伝えるkinematic bodyです。
    KinematicPosition,
    /// 指定した速度で動くkinematic bodyです。
    KinematicVelocity,
    /// 力・重力・接触に応じて動くbodyです。
    Dynamic,
}

/// Bodyの初期poseです。角度はradianです。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BodyPose {
    /// 水平位置。
    pub x: f64,
    /// 垂直位置。
    pub y: f64,
    /// WorldのY軸が下向きのため、正値は時計回りです。
    pub angle: f64,
}

/// 物体を瞬間移動させるとき、移動途中の衝突を調べるかを指定します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyTeleportPathPolicy {
    /// 開始poseから到着poseまでの並進・回転軌跡をshape castし、障害物があれば拒否します。
    #[default]
    Check,
    /// 到着点だけ検証します。瞬間移動により途中の障害物を通過できます。
    Ignore,
}

/// 瞬間移動の到着poseにある他colliderとの重なり方針です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyTeleportOverlapPolicy {
    /// 到着時にsolid colliderとの重なりがあれば拒否します。
    #[default]
    Reject,
    /// 到着時の重なりを許可します。
    Allow,
}

/// 作者のCollisionQueries providerが返すteleport経路判定です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyTeleportQueryOutcome {
    /// providerが経路と到着方針を検査し、移動を許可します。
    Clear,
    /// 経路または到着位置に衝突があります。
    Blocked,
    /// 必要region/query dataが未準備のため、World候補を変更できません。
    Incomplete,
    /// このproviderではbody geometry/queryを扱えません。
    Unsupported,
}

/// 作者定義shapeや独自terrainに対するteleport collision queryを置換します。
///
/// callbackは副作用を持たない決定的なqueryです。provider stateが必要な場合はWorldの
/// StateStoreへ置き、StableKey/versionで自身のstate schemaを管理します。Clearを返す場合は
/// `options.path`と`options.overlap`の両方を満たすqueryを実行してください。Engineはresident
/// regionとWorld原子commitを引き続き検査します。
pub trait BodyTeleportQueryProvider: Send + Sync {
    /// 保存・診断・再現記録に使用する安定keyです。
    fn key(&self) -> StableKey;
    /// query意味を変更したときに進める版です。
    fn version(&self) -> u32;
    /// immutable World/physics viewから移動候補のcollision queryを行います。
    fn query(
        &self,
        world: &World,
        physics_candidate: &BodyPhysics,
        body: BodyId,
        previous: BodyPose,
        destination: BodyPose,
        options: BodyTeleportOptions,
    ) -> EngineResult<BodyTeleportQueryOutcome>;
}

/// teleportに使った作者collision providerの再現stampです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyTeleportProviderStamp {
    /// providerの安定key。
    pub key: StableKey,
    /// providerの意味版。
    pub version: u32,
}

/// body teleportの衝突方針です。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyTeleportOptions {
    /// 移動途中の衝突を検査するか。
    pub path: BodyTeleportPathPolicy,
    /// 到着点のsolid collider重なりを許すか。
    pub overlap: BodyTeleportOverlapPolicy,
}

/// 成功したteleportのposeと適用済み方針です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyTeleportReport {
    /// 移動したbody。
    pub body: BodyId,
    /// 移動前pose。
    pub previous: BodyPose,
    /// 移動後pose。
    pub destination: BodyPose,
    /// 移動経路のcollision queryを行ったか。
    pub path_checked: bool,
    /// 到着点のoverlap policyを適用したか。
    pub overlap_checked: bool,
    /// 標準以外のqueryを選んだ場合のprovider key/version。
    pub query_provider: Option<BodyTeleportProviderStamp>,
}

/// 独立した作者CollisionQueriesへ渡す形状と移動区間です。
#[derive(Clone, Debug, PartialEq)]
pub enum CollisionQueryGeometry {
    /// 指定poseでshape同士の重なりを調べます。
    Overlap {
        shape: ColliderShape,
        pose: BodyPose,
    },
    /// startからendまで回転を含むsweepを調べます。
    Sweep {
        shape: ColliderShape,
        start: BodyPose,
        end: BodyPose,
    },
    /// 作者providerが所有するshapeを指定poseでqueryします。
    CustomOverlap {
        shape: CollisionQueryShapeRef,
        pose: BodyPose,
    },
    /// 作者providerが所有するshapeをstartからendまで回転込みでsweepします。
    CustomSweep {
        shape: CollisionQueryShapeRef,
        start: BodyPose,
        end: BodyPose,
    },
    /// 有限長rayを調べます。directionは単位ベクトルです。
    Ray {
        origin: [f64; 2],
        direction: [f64; 2],
        max_distance: f64,
    },
}

/// CollisionQueriesが引く独自shapeの安定参照です。
///
/// 形状データはEngineへコピーせず、provider key/versionと所有者内IDから作者側で解決します。
/// これによりbeam/polylineや凹形状をqueryごとに大きなpayloadとして渡さずに扱えます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollisionQueryShapeRef {
    /// shape geometryを定義する作者providerです。
    pub provider: StableKey,
    /// geometry schema versionです。
    pub version: u32,
    /// providerが管理するowner IDです。
    pub owner: u32,
    /// owner内で安定したshape object IDです。
    pub object: u64,
    /// compound geometry内のpart IDです。
    pub part: u32,
}

/// collision queryの候補選択と衝突filterです。
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionQueryRequest {
    /// overlap、sweep、rayの幾何入力です。
    pub geometry: CollisionQueryGeometry,
    /// 対象Worldのcollision layerです。
    pub layer: LayerId,
    /// providerが解釈するcollision group maskです。
    pub collision_groups: u32,
    /// queryを発行した所有者。self/owner filterに使います。
    pub owner: Option<StableKey>,
    /// 結果から除外する剛体IDです。
    pub ignore_bodies: Vec<BodyId>,
    /// one-way surfaceの向き。未指定ならproviderの通常規則を使います。
    pub one_way_direction: Option<[f64; 2]>,
    /// trigger/sensorとの接触を含めます。
    pub include_sensors: bool,
    /// query開始時点の重なりをhitとして含めます。
    pub include_initial_overlap: bool,
    /// 接触境界の一致をhitとして含めます。
    pub include_boundary: bool,
    /// providerが幾何epsilonとして使う有限非負値です。
    pub epsilon: f64,
    /// queryを実行する論理tickとsubstepです。
    pub tick: u64,
    /// queryを実行する論理substepです。
    pub substep: u32,
    /// 同じtick内の最新候補geometryを読むよう要求します。
    pub read_latest_candidate: bool,
    /// 最初のhitだけを要求します。stable_order時はTOI/target順の先頭です。
    pub first_hit_only: bool,
    /// 安定したtime-of-impact順を要求します。
    pub stable_order: bool,
    /// 最大hit数です。上限を超えた候補を黙って成功扱いにしません。
    pub max_hits: u32,
}

/// collision queryが得た対象です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionQueryTarget {
    /// Engine BodyPhysics内の剛体とshape partです。
    Body { body: BodyId, part: u32 },
    /// World格子の地形cellです。
    Terrain { cell: CellPoint },
    /// 作者providerが所有するgeometryです。owner IDはreportのprovider key/version内で安定します。
    Custom { owner: u32, object: u64, part: u32 },
}

/// query hitの接触幾何と対象revisionです。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionQueryHit {
    /// hitしたshape/cell/entity。
    pub target: CollisionQueryTarget,
    /// 対象geometryの同じWorld Content内で解決済みmaterial IDです。
    pub material: Option<MaterialId>,
    /// shape-localではなくWorld座標の接触点です。
    pub point: [f64; 2],
    /// queryから障害物へ向かう単位normalです。
    pub normal: [f64; 2],
    /// sweep/ray区間内の衝突時刻です。overlapでは0です。
    pub time_of_impact: f64,
    /// hit geometryを最後に変更したprovider定義revisionです。
    pub geometry_revision: u64,
    /// 対象shapeに対するcoverage。部分cellなどは0より大きく1以下です。
    pub coverage: f64,
}

/// queryの完了状態です。IncompleteをNoHitとして扱ってはいけません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionQueryStatus {
    /// queryは完了し、一つ以上のhitを返しました。
    Hit,
    /// queryは完了し、hitはありません。
    NoHit,
    /// 必要geometryが未準備または未residentで、結論が未完了です。
    Incomplete,
    /// providerが要求shapeまたはquery種別を扱えません。
    Unsupported,
}

/// providerが返すquery結果です。
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionQueryResult {
    /// 完了状態。
    pub status: CollisionQueryStatus,
    /// providerが取得できたhitです。Incomplete時は部分結果を含められます。
    pub hits: Vec<CollisionQueryHit>,
    /// max_hitsに達して候補を切り捨てた場合はtrueです。
    pub truncated: bool,
    /// queryが読んだgeometry集合のrevisionです。
    pub geometry_revision: u64,
}

/// 汎用CollisionQueriesを実装し、標準body/terrain geometry以外のshapeもqueryします。
///
/// queryはread-onlyかつ決定的に実装し、結果の完全性とcoverageを保証してください。
/// Engineは入力・出力の数値、hit上限、stable-order条件を検証しますが、custom geometryの
/// broad phaseが真の候補を全て含むことや独自shapeの忠実度はprovider側の責務です。
pub trait CollisionQueriesProvider: Send + Sync {
    /// 保存・診断・再現記録に使う安定keyです。
    fn key(&self) -> StableKey;
    /// query意味を変更したときに進める版です。
    fn version(&self) -> u32;
    /// 指定したworld/physics snapshotを読むqueryです。callbackからWorldを変更できません。
    fn query(
        &self,
        world: &World,
        physics: Option<&BodyPhysics>,
        request: &CollisionQueryRequest,
    ) -> EngineResult<CollisionQueryResult>;
}

/// 汎用CollisionQueries providerの実行identityと読み取り時点です。
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionQueryReport {
    /// provider key/versionです。
    pub provider: CollisionQueryProviderStamp,
    /// 読み取り時のWorld revisionです。
    pub world_revision: u64,
    /// 読み取り時の剛体step番号です。
    pub body_step: u64,
    /// providerが返したquery結果です。
    pub result: CollisionQueryResult,
}

/// 汎用CollisionQueries providerの安定identityです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollisionQueryProviderStamp {
    /// providerのstable key。
    pub key: StableKey,
    /// query意味版。
    pub version: u32,
}

pub(crate) fn validate_collision_query(
    world: &World,
    physics: Option<&BodyPhysics>,
    request: &CollisionQueryRequest,
    result: &mut CollisionQueryResult,
) -> EngineResult<()> {
    validate_collision_query_request(request)?;
    if result.hits.len() > request.max_hits as usize {
        return Err(EngineError::capacity(
            "CollisionQueries hit数",
            result.hits.len() as u64,
            u64::from(request.max_hits),
        ));
    }
    if request.first_hit_only && result.hits.len() > 1 {
        return Err(invalid("CollisionQueries first-hit結果数"));
    }
    if result.truncated && result.hits.len() != request.max_hits as usize {
        return Err(invalid("CollisionQueries truncated hit数"));
    }
    match result.status {
        CollisionQueryStatus::Hit if result.hits.is_empty() => {
            return Err(invalid("CollisionQueries Hitにhitがありません"));
        }
        CollisionQueryStatus::NoHit if !result.hits.is_empty() || result.truncated => {
            return Err(invalid("CollisionQueries NoHitに部分結果があります"));
        }
        CollisionQueryStatus::Unsupported if !result.hits.is_empty() || result.truncated => {
            return Err(invalid("CollisionQueries Unsupportedにhitがあります"));
        }
        _ => {}
    }
    let mut last_terrain_chunk = None;
    let mut last_terrain_chunk_resident = false;
    for hit in &result.hits {
        let normal_length_squared = hit.normal[0] * hit.normal[0] + hit.normal[1] * hit.normal[1];
        if hit.point.iter().any(|value| !value.is_finite())
            || hit.normal.iter().any(|value| !value.is_finite())
            || !normal_length_squared.is_finite()
            || (normal_length_squared - 1.0).abs() > 1.0e-6
            || !hit.time_of_impact.is_finite()
            || !(0.0..=1.0).contains(&hit.time_of_impact)
            || (matches!(
                &request.geometry,
                CollisionQueryGeometry::Overlap { .. }
                    | CollisionQueryGeometry::CustomOverlap { .. }
            ) && hit.time_of_impact != 0.0)
            || !hit.coverage.is_finite()
            || hit.coverage <= 0.0
            || hit.coverage > 1.0
            || hit.geometry_revision != result.geometry_revision
        {
            return Err(invalid("CollisionQueries hit geometry"));
        }
        if let Some(material) = hit.material {
            world.content.material(material)?;
        }
        match hit.target {
            CollisionQueryTarget::Body { body, .. } => {
                physics
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::StaleId, "query body physics missing")
                    })?
                    .state(body)?;
            }
            CollisionQueryTarget::Terrain { cell } => {
                if !world.is_cell_in_bounds(cell) {
                    return Err(invalid("CollisionQueries terrain hit is not resident"));
                }
                let chunk = ChunkCoord::at(cell);
                if last_terrain_chunk != Some(chunk) {
                    last_terrain_chunk = Some(chunk);
                    last_terrain_chunk_resident = world.is_chunk_resident(chunk);
                }
                if !last_terrain_chunk_resident {
                    return Err(invalid("CollisionQueries terrain hit is not resident"));
                }
            }
            CollisionQueryTarget::Custom { .. } => {}
        }
    }
    if request.stable_order {
        result.hits.sort_unstable_by(compare_collision_hits);
    }
    Ok(())
}

pub(crate) fn validate_collision_query_request(
    request: &CollisionQueryRequest,
) -> EngineResult<()> {
    if request.max_hits == 0
        || request.max_hits > 65_536
        || (request.first_hit_only && request.max_hits != 1)
        || request.ignore_bodies.len() > 65_536
        || !request.epsilon.is_finite()
        || request.epsilon < 0.0
    {
        return Err(invalid("CollisionQueries上限またはepsilon"));
    }
    let unit_vector = |vector: [f64; 2]| {
        let length_squared = vector[0] * vector[0] + vector[1] * vector[1];
        vector.iter().all(|value| value.is_finite())
            && length_squared.is_finite()
            && (length_squared - 1.0).abs() <= 1.0e-6
    };
    if request
        .one_way_direction
        .is_some_and(|direction| !unit_vector(direction))
    {
        return Err(invalid("CollisionQueries one-way direction"));
    }
    match &request.geometry {
        CollisionQueryGeometry::Overlap { shape, pose } => {
            shape.validate_query_shape()?;
            validate_pose(*pose)?;
        }
        CollisionQueryGeometry::Sweep { shape, start, end } => {
            shape.validate_query_shape()?;
            validate_pose(*start)?;
            validate_pose(*end)?;
        }
        CollisionQueryGeometry::CustomOverlap { shape, pose } => {
            validate_custom_query_shape(shape)?;
            validate_pose(*pose)?;
        }
        CollisionQueryGeometry::CustomSweep { shape, start, end } => {
            validate_custom_query_shape(shape)?;
            validate_pose(*start)?;
            validate_pose(*end)?;
        }
        CollisionQueryGeometry::Ray {
            origin,
            direction,
            max_distance,
        } => {
            if origin.iter().any(|value| !value.is_finite())
                || !unit_vector(*direction)
                || !max_distance.is_finite()
                || *max_distance <= 0.0
            {
                return Err(invalid("CollisionQueries ray geometry"));
            }
        }
    }
    Ok(())
}

fn validate_custom_query_shape(shape: &CollisionQueryShapeRef) -> EngineResult<()> {
    if shape.version == 0 {
        return Err(invalid("CollisionQueries custom shape version"));
    }
    Ok(())
}

fn compare_collision_hits(
    left: &CollisionQueryHit,
    right: &CollisionQueryHit,
) -> std::cmp::Ordering {
    left.time_of_impact
        .total_cmp(&right.time_of_impact)
        .then_with(|| compare_collision_targets(left.target, right.target))
        .then_with(|| left.point[0].total_cmp(&right.point[0]))
        .then_with(|| left.point[1].total_cmp(&right.point[1]))
        .then_with(|| left.normal[0].total_cmp(&right.normal[0]))
        .then_with(|| left.normal[1].total_cmp(&right.normal[1]))
        .then_with(|| left.geometry_revision.cmp(&right.geometry_revision))
        .then_with(|| left.coverage.total_cmp(&right.coverage))
}

fn compare_collision_targets(
    left: CollisionQueryTarget,
    right: CollisionQueryTarget,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (
            CollisionQueryTarget::Body {
                body: left,
                part: left_part,
            },
            CollisionQueryTarget::Body {
                body: right,
                part: right_part,
            },
        ) => left.cmp(&right).then_with(|| left_part.cmp(&right_part)),
        (CollisionQueryTarget::Body { .. }, _) => Ordering::Less,
        (_, CollisionQueryTarget::Body { .. }) => Ordering::Greater,
        (
            CollisionQueryTarget::Terrain { cell: left },
            CollisionQueryTarget::Terrain { cell: right },
        ) => (left.x, left.y).cmp(&(right.x, right.y)),
        (CollisionQueryTarget::Terrain { .. }, CollisionQueryTarget::Custom { .. }) => {
            Ordering::Less
        }
        (CollisionQueryTarget::Custom { .. }, CollisionQueryTarget::Terrain { .. }) => {
            Ordering::Greater
        }
        (
            CollisionQueryTarget::Custom {
                owner: left_owner,
                object: left_object,
                part: left_part,
            },
            CollisionQueryTarget::Custom {
                owner: right_owner,
                object: right_object,
                part: right_part,
            },
        ) => left_owner
            .cmp(&right_owner)
            .then_with(|| left_object.cmp(&right_object))
            .then_with(|| left_part.cmp(&right_part)),
    }
}

/// Bodyの初期速度です。WorldのY軸が下向きのため、角速度の正値は時計回りです。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BodyVelocity {
    /// 水平速度。
    pub x: f64,
    /// 垂直速度。
    pub y: f64,
    /// 角速度。
    pub angular: f64,
}

/// bodyと一つの衝突shapeを登録する型付き定義です。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyDef {
    /// bodyの運動方式。
    pub motion: BodyMotionType,
    /// 初期pose。
    pub pose: BodyPose,
    /// 初期速度。
    pub velocity: BodyVelocity,
    /// bodyごとの重力倍率。
    pub gravity_scale: f64,
    /// 薄壁を高速で通過しないための連続衝突を有効にします。
    pub ccd: bool,
    /// 静止bodyをsolverから休止できるか。
    pub can_sleep: bool,
    /// shapeと衝突物性です。
    pub collider: ColliderDef,
}

impl BodyDef {
    /// 指定shapeを持つdynamic bodyを作ります。
    pub fn dynamic(shape: ColliderShape) -> Self {
        Self {
            motion: BodyMotionType::Dynamic,
            pose: BodyPose::default(),
            velocity: BodyVelocity::default(),
            gravity_scale: 1.0,
            ccd: false,
            can_sleep: true,
            collider: ColliderDef::new(shape),
        }
    }

    fn validate(&self) -> EngineResult<()> {
        let finite = [
            self.pose.x,
            self.pose.y,
            self.pose.angle,
            self.velocity.x,
            self.velocity.y,
            self.velocity.angular,
            self.gravity_scale,
            self.collider.density,
            self.collider.friction,
            self.collider.restitution,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite
            || self.collider.density < 0.0
            || self.collider.friction < 0.0
            || self.collider.restitution < 0.0
            || (self.motion == BodyMotionType::Dynamic && self.collider.density == 0.0)
        {
            return Err(invalid("bodyの物理値"));
        }
        drop(self.collider.shape.build()?);
        Ok(())
    }
}

/// 衝突shapeに設定する物性です。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColliderDef {
    /// 衝突shape。
    pub shape: ColliderShape,
    /// 面積あたりの質量。dynamic bodyは正値を要求します。
    pub density: f64,
    /// 接触摩擦係数。
    pub friction: f64,
    /// 反発係数。
    pub restitution: f64,
    /// trueなら接触を報告し、solverの衝突応答へ参加しません。
    pub sensor: bool,
}

impl ColliderDef {
    /// 標準物性のcollider定義を作ります。
    pub fn new(shape: ColliderShape) -> Self {
        Self {
            shape,
            density: 1.0,
            friction: 0.7,
            restitution: 0.0,
            sensor: false,
        }
    }

    fn build(&self) -> EngineResult<ColliderBuilder> {
        Ok(self
            .shape
            .build()?
            .density(self.density)
            .friction(self.friction)
            .restitution(self.restitution)
            .sensor(self.sensor))
    }
}

/// 読取・描画向けのbody poseと速度です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyState {
    /// 現在のpose。
    pub pose: BodyPose,
    /// 現在の速度。
    pub velocity: BodyVelocity,
    /// solverが休止中ならtrueです。
    pub sleeping: bool,
}

/// Rapierが使用するbody-localの質量、重心、回転慣性です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyMassProperties {
    /// dynamic bodyの質量。fixed/kinematic bodyではRapierの値に従います。
    pub mass: f64,
    /// body originからの局所重心。
    pub center_of_mass: [f64; 2],
    /// 重心まわりの2D角慣性。
    pub angular_inertia: f64,
}

/// 成功した固定物理stepの結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyStepReport {
    /// 確定した論理step番号。
    pub step: u64,
    /// このstep後に動作中となったbody数。
    pub active_bodies: u32,
    /// 固定step内の実行substep数です。Rapier標準経路はEngineが、作者provider経路はproviderが報告します。
    pub substeps: u32,
    /// Rapier以外のfixed-step providerを使った場合のidentityです。
    pub provider: Option<BodyPhysicsProviderStamp>,
    /// 独自物質連成を有効にした場合のprovider identityです。
    pub coupling_provider: Option<BodyCouplingProviderStamp>,
}

/// body solverが接触した相手です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyContactTarget {
    /// 別の剛体。
    Body(BodyId),
    /// 静的なWorld地形chunk。
    Terrain(ChunkCoord),
    /// 作者CollisionQueries/BodyPhysics providerが所有する衝突形状です。
    ///
    /// 有効なBodyPhysicsStepProviderのkey/versionと各安定IDを組み合わせ、
    /// World内の一時handleに依存しません。
    Custom {
        /// 現在有効なfixed-step provider内のowner IDです。
        owner: u32,
        /// owner内の物体IDです。
        object: u64,
        /// compound shapeなどの部位IDです。
        part: u32,
    },
}

/// 一つの剛体接触関係が固定step内で取った状態です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyContactPhase {
    /// 前substepにはなく、今回新しく始まりました。
    Enter,
    /// 前substepから継続しています。
    Stay,
    /// 前substepにはあり、今回終了しました。
    Exit,
}

/// 接触終了を発生させた理由です。通常のstep中の分離は`Separated`です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyContactExitReason {
    /// solverの更新で二つの形状が分離しました。
    Separated,
    /// 接触相手の剛体がWorld編集で除去されました。
    BodyRemoved,
    /// 接触中のterrain colliderを持つWorld chunkがunloadされました。
    TerrainUnloaded,
    /// 接触中のterrain colliderが同じchunkの地形編集で置き換わりました。
    TerrainChanged,
    /// 接触中のbody poseが明示的なWorld teleportで変更されました。次のstepで重複させません。
    Teleported,
}

/// body solverの固定stepまたはWorld編集で確定した接触eventです。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyContactEvent {
    /// stable順で並べた接触元。
    pub first: BodyContactTarget,
    /// stable順で並べた接触先。
    pub second: BodyContactTarget,
    /// 接触の開始、継続、終了。
    pub phase: BodyContactPhase,
    /// 終了eventの場合、その原因です。開始・継続では`None`です。
    pub exit_reason: Option<BodyContactExitReason>,
    /// first/second collider上の接触点を結ぶ線分の中点（World物理単位）。
    pub point: [f64; 2],
    /// firstからsecondへ向く単位normal。
    pub normal: [f64; 2],
    /// firstへsolverが加えた総力積（World物理単位）。secondには反対符号が加わります。
    pub impulse: [f64; 2],
    /// 接触を検出したstepまたは編集境界でのfirst側開始速度。地形側はNoneです。
    pub first_pre_velocity: Option<BodyVelocity>,
    /// 接触を検出したstepまたは編集境界でのfirst側終了速度。地形側はNoneです。
    pub first_post_velocity: Option<BodyVelocity>,
    /// 接触を検出したstepまたは編集境界でのsecond側開始速度。地形側はNoneです。
    pub second_pre_velocity: Option<BodyVelocity>,
    /// 接触を検出したstepまたは編集境界でのsecond側終了速度。地形側はNoneです。
    pub second_post_velocity: Option<BodyVelocity>,
    /// eventを検出した1-based fixed-step substep番号です。World編集境界のeventでは0です。
    pub substep: u32,
}

/// 剛体fixed-stepを置換した作者providerの安定identityです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyPhysicsProviderStamp {
    /// providerの安定key。
    pub key: StableKey,
    /// step semanticsの版。
    pub version: u32,
}

/// 流体・熱・地形等のbody連成を置換する作者providerの安定identityです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCouplingProviderStamp {
    /// providerの安定key。
    pub key: StableKey,
    /// 連成規則の版。
    pub version: u32,
}

/// 作者形状が流体連成へ渡す一件の境界接触sampleです。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyFluidContactSample {
    /// 接触している剛体。
    pub body: BodyId,
    /// 流体cell。
    pub fluid_cell: CellPoint,
    /// body表面上のWorld座標。
    pub surface_point: [f64; 2],
    /// 同じ力積を剛体へ適用するWorld座標。
    pub reaction_point: [f64; 2],
    /// 面積・辺長を正規化した0..=1のcoverage。
    pub coverage: f64,
}

/// 作者形状の流体排除が占有するWorld格子cellです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyFluidOccupancySample {
    /// 占有領域を所有する剛体。
    pub body: BodyId,
    /// body形状が占有するWorld cell。
    pub world_cell: CellPoint,
}

/// 現在のfluid/body接触と正規状態を渡すread-only入力です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyCouplingContext {
    /// 接触している剛体。
    pub body: BodyId,
    /// 接触しているWorld流体cell。
    pub fluid_cell: CellPoint,
    /// 接触面上のWorld座標。
    pub surface_point: [f64; 2],
    /// 流体と反作用を適用するWorld座標。
    pub reaction_point: [f64; 2],
    /// fluid cellとbody境界の接触coverage。
    pub coverage: f64,
    /// fluidのContent-local material IDと質量。
    pub fluid_material: MaterialId,
    /// fluid cellの質量。
    pub fluid_mass: u32,
    /// 現在のMaterialMotion Q16速度。
    pub fluid_velocity_q16: [i64; 2],
    /// fixed-step後のbody pose・速度です。
    pub body_state: BodyState,
    /// impulseを受け取れるbodyの運動種別です。
    pub body_motion_type: BodyMotionType,
    /// World格子cellの物理長。
    pub world_units_per_cell: f64,
    /// fixed-stepの秒数です。
    pub fixed_dt_seconds: f64,
}

/// 一つのbody/fluid接触に対する離散運動量更新です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyCouplingResponse {
    /// fluid MaterialMotionへ加えるQ16速度差です。
    pub fluid_velocity_delta_q16: [i64; 2],
    /// 指定reaction pointへbodyに加える物理単位の力積です。
    pub body_impulse: [f64; 2],
    /// 作者の力場・推進など外部供給元が与えた力積です。fluidとbodyの合計差を明示します。
    pub external_impulse: [f64; 2],
}

/// body/fluid/terrain間の連成則を独立して差し替えます。
///
/// callbackはWorldのread-only候補と、変更可能なBodyPhysics候補を受け取ります。
/// solver stateは`BodyPhysics::set_coupling_provider_state`でStateCodec保存できます。
/// Engineはfluid速度加算、finite値、容量、離散運動量収支を検査し、後続処理が失敗した場合は
/// World・physics・provider stateをまとめてrollbackします。
pub trait CouplingProvider: Send + Sync {
    /// 保存・診断・再現記録に使う安定keyです。
    fn key(&self) -> StableKey;
    /// 連成意味を変更したときに進める版です。
    fn version(&self) -> u32;
    /// 任意shapeの流体境界sampleを返します。`None`なら標準BodyStencil geometryを使います。
    ///
    /// `Some`を返す場合、Engineはsampleをstable順へ並べ替えてから流体の有限値、容量、
    /// 材料参照と離散運動量収支を検証します。戻り値は境界連成のみを置換し、body/Worldの
    /// 重なり排除geometryは別契約です。
    fn fluid_contacts(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_contacts: usize,
    ) -> EngineResult<Option<Vec<BodyFluidContactSample>>> {
        Ok(None)
    }
    /// 任意shapeのfluid排除領域を返します。`None`ならBodyStencilの占有を使います。
    ///
    /// `Some`を返す場合はbodyごとの全占有cellを指定し、Engineはcell所有の曖昧な重複を拒否して、
    /// fluid移動・particle化・reservoir化を同じWorldEdit候補で処理します。幾何sampleはtickごとに
    /// 再計算できる決定的な値とし、保存対象の形状状態はprovider側のcodec/StateStoreに置きます。
    fn fluid_occupancy(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_cells: usize,
    ) -> EngineResult<Option<Vec<BodyFluidOccupancySample>>> {
        Ok(None)
    }
    /// 接触一件の流体・剛体更新を返します。外部力積がなければzeroを指定します。
    fn couple(
        &self,
        physics_candidate: &mut BodyPhysics,
        world: &World,
        context: BodyCouplingContext,
    ) -> EngineResult<BodyCouplingResponse>;
}

/// 作者solverへ渡す固定tickの不変条件です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyPhysicsStepContext {
    /// 一回のlogical stepの秒数。描画deltaとは独立しています。
    pub fixed_dt_seconds: f64,
    /// 現在のlogical step番号。
    pub step_index: u64,
    /// providerが報告できる最大substep数。
    pub max_substeps: u32,
    /// World格子cellの物理長。
    pub world_units_per_cell: f64,
    /// provider固有stateを一回の更新で保存できる最大byte数。
    pub provider_state_byte_limit: usize,
}

impl Serialize for BodyPhysicsProviderStamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Record<'a> {
            key: &'a str,
            version: u32,
        }
        Record {
            key: self.key.as_str(),
            version: self.version,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BodyPhysicsProviderStamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Record {
            key: String,
            version: u32,
        }
        let record = Record::deserialize(deserializer)?;
        let key = StableKey::parse(&record.key).map_err(serde::de::Error::custom)?;
        Ok(Self {
            key,
            version: record.version,
        })
    }
}

impl Serialize for BodyCouplingProviderStamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Record<'a> {
            key: &'a str,
            version: u32,
        }
        Record {
            key: self.key.as_str(),
            version: self.version,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BodyCouplingProviderStamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Record {
            key: String,
            version: u32,
        }
        let record = Record::deserialize(deserializer)?;
        let key = StableKey::parse(&record.key).map_err(serde::de::Error::custom)?;
        Ok(Self {
            key,
            version: record.version,
        })
    }
}

/// World所有剛体fixed-stepを作者solverへ置換します。
///
/// callbackは読取専用Worldと複製済みBodyPhysics候補だけを使います。成功後もEngineはbackend値、
/// resident swept region、fluid連成、容量を検証して一括commitします。solver固有の正規状態は
/// `BodyPhysics::set_provider_state`でStateCodec形式として保存し、snapshotとprovider key/versionへ結びます。
/// callbackは外部副作用を避け、同じ入力・保存stateから同じcandidateを作ってください。
/// BodyPhysicsの`step`は呼ばず、BodyStateを読み、set_pose/set_velocity等で候補を更新します。
pub trait BodyPhysicsStepProvider: Send + Sync {
    /// 保存・診断・再現記録に使う安定keyです。
    fn key(&self) -> StableKey;
    /// solver意味を変更したときに進める版です。
    fn version(&self) -> u32;
    /// 固定物理stepの候補を更新します。失敗時、EngineはWorldとphysics候補を破棄します。
    fn step(
        &self,
        physics_candidate: &mut BodyPhysics,
        world: &World,
        context: BodyPhysicsStepContext,
    ) -> EngineResult<u32>;
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct TerrainSource {
    world_index: u32,
    world_generation: u32,
    layer_index: u32,
    cell_size: f64,
    world_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct TerrainChunkRevision {
    x: i32,
    y: i32,
    revision: u64,
}

/// World一つに属するRapier backendと安定BodyId対応表です。
pub struct BodyPhysics {
    settings: BodyPhysicsSettings,
    backend: RapierWorld,
    candidate_backend: RapierWorld,
    // substepごとの位置確認用scratchをWorld edit間で移動再利用します。
    step_scratch: Vec<BodySubstepStart>,
    /// 最後の標準fixed stepまたは接触状態を変えたWorld編集のevent列。保存対象ではありません。
    contact_events: Vec<BodyContactEvent>,
    contact_event_scratch: Vec<BodyContactEvent>,
    terrain_contact_events_pending: bool,
    /// teleport直後、Rapierの次回narrow-phase更新が古いpairを返す場合に一度だけ抑止します。
    pending_teleport_exits: Vec<ContactPairKey>,
    contact_before: Vec<ContactObservation>,
    contact_after: Vec<ContactObservation>,
    contact_velocity_before: Vec<ContactVelocityStart>,
    handles: BTreeMap<BodyId, RigidBodyHandle>,
    // 物理stepでは読み取り専用なのでsnapshot候補間で共有し、cell編集だけCOWします。
    local_grids: Arc<BTreeMap<BodyId, BodyLocalGrid>>,
    // stepの通常経路で全body gridを走査せず、reservoir再放出候補の有無をO(1)で判定します。
    exclusion_reservoir_body_count: usize,
    // Content照合と質量再計算は、新規・置換・復元したgridだけに限定します。
    local_grid_content_dirty: BTreeSet<BodyId>,
    joint_handles: BTreeMap<JointId, ImpulseJointHandle>,
    terrain_handles: BTreeMap<ChunkCoord, RigidBodyHandle>,
    terrain_revisions: BTreeMap<ChunkCoord, u64>,
    terrain_source: Option<TerrainSource>,
    next_body_index: u32,
    next_joint_index: u32,
    step: u64,
    step_provider: Option<BodyPhysicsProviderStamp>,
    provider_state: Option<Arc<Vec<u8>>>,
    coupling_provider: Option<BodyCouplingProviderStamp>,
    coupling_state: Option<Arc<Vec<u8>>>,
}

impl BodyPhysics {
    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        let mut backend = RapierWorld::new();
        copy_backend_state(&self.backend, &mut backend);
        let mut pending_teleport_exits = Vec::new();
        pending_teleport_exits
            .try_reserve_exact(self.pending_teleport_exits.len())
            .map_err(allocation)?;
        pending_teleport_exits.extend_from_slice(&self.pending_teleport_exits);
        let mut contact_events = Vec::new();
        contact_events
            .try_reserve_exact((self.settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        let mut contact_event_scratch = Vec::new();
        contact_event_scratch
            .try_reserve_exact((self.settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        Ok(Self {
            settings: self.settings,
            backend,
            candidate_backend: RapierWorld::new(),
            step_scratch: Vec::new(),
            contact_events,
            contact_event_scratch,
            terrain_contact_events_pending: false,
            pending_teleport_exits,
            contact_before: Vec::new(),
            contact_after: Vec::new(),
            contact_velocity_before: Vec::new(),
            handles: self.handles.clone(),
            local_grids: self.local_grids.clone(),
            exclusion_reservoir_body_count: self.exclusion_reservoir_body_count,
            local_grid_content_dirty: self.local_grid_content_dirty.clone(),
            joint_handles: self.joint_handles.clone(),
            terrain_handles: self.terrain_handles.clone(),
            terrain_revisions: self.terrain_revisions.clone(),
            terrain_source: self.terrain_source,
            next_body_index: self.next_body_index,
            next_joint_index: self.next_joint_index,
            step: self.step,
            step_provider: self.step_provider,
            provider_state: self.provider_state.clone(),
            coupling_provider: self.coupling_provider,
            coupling_state: self.coupling_state.clone(),
        })
    }

    /// 独立したphysics worldを初期化します。
    pub fn new(settings: BodyPhysicsSettings) -> EngineResult<Self> {
        let settings = settings.validate()?;
        let mut backend = RapierWorld::new();
        backend.gravity = Vector::new(settings.gravity[0], settings.gravity[1]);
        backend.integration_parameters.dt = 1.0 / f64::from(settings.step_hz);
        let mut candidate_backend = RapierWorld::new();
        candidate_backend.gravity = backend.gravity;
        candidate_backend.integration_parameters = backend.integration_parameters;
        let mut contact_events = Vec::new();
        contact_events
            .try_reserve_exact((settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        let mut contact_event_scratch = Vec::new();
        contact_event_scratch
            .try_reserve_exact((settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        Ok(Self {
            settings,
            backend,
            candidate_backend,
            step_scratch: Vec::new(),
            contact_events,
            contact_event_scratch,
            terrain_contact_events_pending: false,
            pending_teleport_exits: Vec::new(),
            contact_before: Vec::new(),
            contact_after: Vec::new(),
            contact_velocity_before: Vec::new(),
            handles: BTreeMap::new(),
            local_grids: Arc::new(BTreeMap::new()),
            exclusion_reservoir_body_count: 0,
            local_grid_content_dirty: BTreeSet::new(),
            joint_handles: BTreeMap::new(),
            terrain_handles: BTreeMap::new(),
            terrain_revisions: BTreeMap::new(),
            terrain_source: None,
            next_body_index: 0,
            next_joint_index: 0,
            step: 0,
            step_provider: None,
            provider_state: None,
            coupling_provider: None,
            coupling_state: None,
        })
    }

    /// 現在のbody数を返します。
    pub fn body_count(&self) -> u32 {
        self.handles.len() as u32
    }

    /// 最後に成功した固定step、body除去、terrain同期で確定した接触eventを返します。
    ///
    /// eventは次の成功したfixed stepまたは接触を終わらせる編集で置き換わります。
    /// body除去・terrain unload/更新eventは編集境界のためsubstepが0です。作者
    /// `BodyPhysicsStepProvider`は`set_provider_contact_events`で同じ契約のeventを候補へ登録します。
    /// 失敗した候補操作は直前に成功した一覧を保持します。
    pub fn contact_events(&self) -> &[BodyContactEvent] {
        &self.contact_events
    }

    /// 作者fixed-step providerが生成した接触event候補を登録します。
    ///
    /// 各eventのsubstepは1からproviderが報告する実行数までとし、配列は
    /// `(substep, first, second)`の昇順で、同じpairを同一substepに重複させません。
    /// 参照するbody、resident terrain chunk、event値、最大件数はcallback終了後にも検証します。
    /// 成功したfixed stepと同じWorld transactionで公開され、callback・検証・World commitの
    /// いずれかが失敗するとphysics状態とともに破棄されます。
    pub fn set_provider_contact_events(&mut self, events: &[BodyContactEvent]) -> EngineResult<()> {
        if events.len() > self.settings.contact_event_capacity as usize {
            return Err(EngineError::capacity(
                "剛体接触event数",
                events.len() as u64,
                u64::from(self.settings.contact_event_capacity),
            ));
        }
        let mut previous = None;
        for event in events {
            validate_contact_event_shape(event)?;
            let key = (event.substep, event.first, event.second);
            if event.substep == 0 || previous.is_some_and(|old| old >= key) {
                return Err(invalid("作者剛体contact event順序またはsubstep"));
            }
            previous = Some(key);
        }
        self.contact_event_scratch.clear();
        self.contact_event_scratch
            .try_reserve_exact(events.len())
            .map_err(allocation)?;
        self.contact_event_scratch.extend_from_slice(events);
        Ok(())
    }

    fn publish_contact_events(&mut self) {
        std::mem::swap(&mut self.contact_events, &mut self.contact_event_scratch);
        self.contact_event_scratch.clear();
    }

    /// terrain同期直後のstepへ、同じ原子操作で確定するchunk終了eventを引き継ぎます。
    fn seed_terrain_exit_events_for_step(&mut self) -> EngineResult<()> {
        self.contact_event_scratch.clear();
        if !self.terrain_contact_events_pending {
            return Ok(());
        }
        let count = self
            .contact_events
            .iter()
            .filter(|event| {
                event.phase == BodyContactPhase::Exit
                    && matches!(
                        event.exit_reason,
                        Some(
                            BodyContactExitReason::TerrainUnloaded
                                | BodyContactExitReason::TerrainChanged
                        )
                    )
            })
            .count();
        if count > self.settings.contact_event_capacity as usize {
            return Err(EngineError::capacity(
                "剛体接触event数",
                count as u64,
                u64::from(self.settings.contact_event_capacity),
            ));
        }
        if self.contact_event_scratch.capacity() < count {
            self.contact_event_scratch
                .try_reserve_exact(count.saturating_sub(self.contact_event_scratch.len()))
                .map_err(allocation)?;
        }
        self.contact_event_scratch
            .extend(self.contact_events.iter().copied().filter(|event| {
                event.phase == BodyContactPhase::Exit
                    && matches!(
                        event.exit_reason,
                        Some(
                            BodyContactExitReason::TerrainUnloaded
                                | BodyContactExitReason::TerrainChanged
                        )
                    )
            }));
        Ok(())
    }

    /// 現在のbody IDをstable ID順に列挙します。
    pub fn body_ids(&self) -> impl Iterator<Item = BodyId> + '_ {
        self.handles.keys().copied()
    }

    /// 固定step候補からCollisionQueriesを実行します。
    ///
    /// `World::query_body_collisions`は確定済みWorldのphysicsをqueryします。この入口は
    /// 作者`BodyPhysicsStepProvider`内で更新途中の候補physicsを明示し、`read_latest_candidate`
    /// 要求に応じた候補pose/body参照検証を行う用途です。Worldは不変で、失敗結果はstep候補全体を
    /// rollbackします。
    pub fn query_collisions(
        &self,
        world: &World,
        request: &CollisionQueryRequest,
        provider: &dyn CollisionQueriesProvider,
    ) -> EngineResult<CollisionQueryReport> {
        validate_collision_query_request(request)?;
        world.content.layer(request.layer)?;
        let stamp = CollisionQueryProviderStamp {
            key: provider.key(),
            version: provider.version(),
        };
        if stamp.version == 0 {
            return Err(invalid("CollisionQueries provider version"));
        }
        let mut result = provider.query(world, Some(self), request)?;
        validate_collision_query(world, Some(self), request, &mut result)?;
        Ok(CollisionQueryReport {
            provider: stamp,
            world_revision: world.revision(),
            body_step: self.step_index(),
            result,
        })
    }

    /// 現在のlocal grid占有と重なるWorld cellをbody ID優先で一意に返します。
    fn local_grid_world_occupancy(&self) -> EngineResult<Vec<(BodyId, CellPoint)>> {
        let mut occupied = BTreeMap::<(i32, i32), BodyId>::new();
        for (body, grid) in self.local_grids.iter() {
            if grid.cell_count() == 0 {
                continue;
            }
            let pose = self.state(*body)?.pose;
            for point in grid.stencil()?.world_cells(pose)? {
                occupied.entry((point.y, point.x)).or_insert(*body);
            }
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(occupied.len())
            .map_err(allocation)?;
        result.extend(
            occupied
                .into_iter()
                .map(|((y, x), body)| (body, CellPoint { x, y })),
        );
        Ok(result)
    }

    pub(crate) fn has_fluid_contact(&self, world: &World, layer: LayerId) -> EngineResult<bool> {
        let content = &world.content;
        let occupancy = self.local_grid_world_occupancy()?;
        for (_, point) in &occupancy {
            let candidates = [
                Some(*point),
                point.y.checked_sub(1).map(|y| CellPoint { x: point.x, y }),
                point.x.checked_sub(1).map(|x| CellPoint { x, y: point.y }),
                point.x.checked_add(1).map(|x| CellPoint { x, y: point.y }),
                point.y.checked_add(1).map(|y| CellPoint { x: point.x, y }),
            ];
            for candidate in candidates.into_iter().flatten() {
                if candidate != *point && body_occupancy_contains(&occupancy, candidate) {
                    continue;
                }
                let view = match world.cell(layer, candidate)? {
                    CellQuery::Loaded(view) => view,
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "剛体fluid接触に未読込cellがある",
                        ));
                    }
                    CellQuery::WorldBoundary => continue,
                };
                if view.block.is_some() || view.core.material == MaterialId::AIR {
                    continue;
                }
                let material = content
                    .material(view.core.material)?
                    .ok_or_else(|| invalid("剛体fluid接触material"))?;
                if matches!(
                    material.base(),
                    MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                ) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn fluid_contact_samples(
        &self,
        world: &World,
        layer: LayerId,
        world_units_per_cell: f64,
    ) -> EngineResult<Vec<BodyFluidContact>> {
        if !world_units_per_cell.is_finite()
            || world_units_per_cell <= 0.0
            || !(4.0 * world_units_per_cell).is_finite()
        {
            return Err(invalid("剛体fluid連成cell寸法"));
        }
        let mut occupied = BTreeMap::<(i32, i32), BodyId>::new();
        let mut body_world_cells = Vec::new();
        let mut occupancy = Vec::new();
        let mut contacts = Vec::new();
        let mut surface_contacts = Vec::new();
        let mut volume_contacts = Vec::new();
        let mut remaining_boundary_contacts = world.limits.job_cells;
        let mut remaining_volume_contacts = world.limits.job_cells;
        let mut boundary_segments = Vec::new();
        let mut boundary_cuts = Vec::new();
        let mut boundary_contacts = Vec::new();
        let mut body_volume_contacts = Vec::new();
        let mut volume_polygon = Vec::new();
        let mut volume_clipped = Vec::new();
        let mut volume_clip_scratch = Vec::new();
        let content = &world.content;
        let is_fluid = |point: CellPoint, boundary_is_empty: bool| -> EngineResult<bool> {
            let view = match world.cell(layer, point)? {
                CellQuery::Loaded(view) => view,
                CellQuery::Unavailable(_) => {
                    return Err(EngineError::new(
                        ErrorCode::InvalidState,
                        "剛体fluid接触cellのchunkが未読込",
                    ));
                }
                CellQuery::WorldBoundary if boundary_is_empty => return Ok(false),
                CellQuery::WorldBoundary => return Err(invalid("剛体fluid接触がWorld境界外")),
            };
            if view.block.is_some() || view.core.material == MaterialId::AIR {
                return Ok(false);
            }
            let material = content
                .material(view.core.material)?
                .ok_or_else(|| invalid("剛体fluid接触material"))?;
            Ok(matches!(
                material.base(),
                MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
            ))
        };
        for (body, grid) in self.local_grids.iter() {
            let state = self.state(*body)?;
            let stencil = grid.stencil()?;
            stencil.world_cells_into(state.pose, &mut body_world_cells)?;
            for point in &body_world_cells {
                occupied.entry((point.y, point.x)).or_insert(*body);
            }
            let aligned_volume = stencil.is_aligned_to_world_grid(state.pose, world_units_per_cell);
            if aligned_volume {
                volume_contacts
                    .try_reserve(body_world_cells.len())
                    .map_err(allocation)?;
                volume_contacts.extend(body_world_cells.iter().copied().map(|world_cell| {
                    (
                        *body,
                        BodyVolumeContact {
                            world_cell,
                            world_point: [
                                (f64::from(world_cell.x) + 0.5) * world_units_per_cell,
                                (f64::from(world_cell.y) + 0.5) * world_units_per_cell,
                            ],
                            covered_area: world_units_per_cell * world_units_per_cell,
                        },
                    )
                }));
            } else {
                stencil.volume_contacts_into(
                    state.pose,
                    world_units_per_cell,
                    remaining_volume_contacts,
                    &mut body_volume_contacts,
                    &mut volume_polygon,
                    &mut volume_clipped,
                    &mut volume_clip_scratch,
                )?;
                volume_contacts
                    .try_reserve(body_volume_contacts.len())
                    .map_err(allocation)?;
                volume_contacts.extend(
                    body_volume_contacts
                        .iter()
                        .copied()
                        .map(|contact| (*body, contact)),
                );
            }
            let body_contact_count = if aligned_volume {
                body_world_cells.len()
            } else {
                body_volume_contacts.len()
            };
            remaining_volume_contacts =
                remaining_volume_contacts
                    .checked_sub(body_contact_count)
                    .ok_or_else(|| invalid("剛体fluid volume contact予算overflow"))?;
            stencil.boundary_contacts_into(
                state.pose,
                world_units_per_cell,
                remaining_boundary_contacts,
                &mut boundary_segments,
                &mut boundary_cuts,
                &mut boundary_contacts,
            )?;
            remaining_boundary_contacts = remaining_boundary_contacts
                .checked_sub(boundary_contacts.len())
                .ok_or_else(|| invalid("剛体fluid境界contact予算overflow"))?;
            for boundary in boundary_contacts.iter().copied() {
                let point = boundary.world_cell;
                if body_world_cells
                    .binary_search_by_key(&(point.y, point.x), |cell| (cell.y, cell.x))
                    .is_ok()
                    || !is_fluid(point, true)?
                {
                    continue;
                }
                let length = boundary.covered_length;
                push_body_fluid_contact(
                    &mut surface_contacts,
                    world.limits.job_cells,
                    BodyFluidContact {
                        body: *body,
                        point,
                        surface_measure: length,
                        weighted_surface_x: boundary.world_point[0] * length,
                        weighted_surface_y: boundary.world_point[1] * length,
                        coverage: 0.0,
                        surface_point: [0.0; 2],
                        reaction_point: [0.0; 2],
                    },
                )?;
            }
        }
        occupancy
            .try_reserve_exact(occupied.len())
            .map_err(allocation)?;
        occupancy.extend(
            occupied
                .into_iter()
                .map(|((y, x), body)| (body, CellPoint { x, y })),
        );
        let maximum_contacts = occupancy
            .len()
            .checked_add(world.limits.job_cells)
            .ok_or_else(|| invalid("剛体fluid連成sample上限overflow"))?;
        contacts
            .try_reserve_exact(occupancy.len())
            .map_err(allocation)?;
        for (body, contact) in volume_contacts {
            let point = contact.world_cell;
            if body_occupancy_owner(&occupancy, point) != Some(body) || !is_fluid(point, false)? {
                continue;
            }
            let measure = 4.0 * contact.covered_area / world_units_per_cell;
            push_body_fluid_contact(
                &mut contacts,
                maximum_contacts,
                BodyFluidContact {
                    body,
                    point,
                    surface_measure: measure,
                    weighted_surface_x: contact.world_point[0] * measure,
                    weighted_surface_y: contact.world_point[1] * measure,
                    coverage: 0.0,
                    surface_point: [0.0; 2],
                    reaction_point: [0.0; 2],
                },
            )?;
        }
        for contact in surface_contacts {
            if body_occupancy_contains(&occupancy, contact.point) {
                continue;
            }
            push_body_fluid_contact(&mut contacts, maximum_contacts, contact)?;
        }

        contacts.sort_unstable_by_key(|contact| (contact.body, contact.point.y, contact.point.x));
        let mut unique_len = 0;
        for read in 0..contacts.len() {
            let contact = contacts[read];
            if unique_len != 0
                && contacts[unique_len - 1].body == contact.body
                && contacts[unique_len - 1].point == contact.point
            {
                let unique = &mut contacts[unique_len - 1];
                unique.surface_measure += contact.surface_measure;
                unique.weighted_surface_x += contact.weighted_surface_x;
                unique.weighted_surface_y += contact.weighted_surface_y;
                if !unique.surface_measure.is_finite()
                    || !unique.weighted_surface_x.is_finite()
                    || !unique.weighted_surface_y.is_finite()
                {
                    return Err(invalid("剛体fluid境界coverage overflow"));
                }
            } else {
                contacts[unique_len] = contact;
                unique_len += 1;
            }
        }
        contacts.truncate(unique_len);
        for contact in &mut contacts {
            if !contact.surface_measure.is_finite() || contact.surface_measure <= 0.0 {
                return Err(invalid("剛体fluid境界coverage measure"));
            }
            contact.coverage = (contact.surface_measure / (4.0 * world_units_per_cell)).min(1.0);
            contact.surface_point = [
                contact.weighted_surface_x / contact.surface_measure,
                contact.weighted_surface_y / contact.surface_measure,
            ];
            contact.reaction_point = [
                (f64::from(contact.point.x) + 0.5) * world_units_per_cell,
                (f64::from(contact.point.y) + 0.5) * world_units_per_cell,
            ];
            if contact
                .surface_point
                .iter()
                .chain(&contact.reaction_point)
                .any(|coordinate| !coordinate.is_finite())
            {
                return Err(invalid("剛体fluid連成作用点overflow"));
            }
        }
        Ok(contacts)
    }

    pub(crate) fn has_releasable_exclusion_reservoir(
        &self,
        world: &World,
        layer: LayerId,
    ) -> EngineResult<bool> {
        if self.exclusion_reservoir_body_count == 0 {
            return Ok(false);
        }
        let mut occupied = BTreeSet::new();
        for (_, point) in self.local_grid_world_occupancy()? {
            occupied.insert((point.y, point.x));
        }
        for grid in self.local_grids.values() {
            for entry in grid.exclusion_reservoir() {
                let origin = entry.origin();
                for (dx, dy) in [(0, 0), (0, 1), (-1, 0), (1, 0), (0, -1)] {
                    let Some(x) = origin.x.checked_add(dx) else {
                        continue;
                    };
                    let Some(y) = origin.y.checked_add(dy) else {
                        continue;
                    };
                    let key = (y, x);
                    if occupied.contains(&key) {
                        continue;
                    }
                    if matches!(
                        world.cell(layer, CellPoint { x, y })?,
                        CellQuery::Loaded(view)
                            if view.block.is_none() && view.core.material == MaterialId::AIR
                    ) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// body表面速度をQ16 cell/physics-tickへ変換します。
    fn particle_velocity_at_cell(
        &self,
        body: BodyId,
        point: CellPoint,
        world_units_per_cell: f64,
    ) -> EngineResult<[i64; 2]> {
        if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
            return Err(invalid("剛体流体排除cell寸法"));
        }
        let state = self.state(body)?;
        let radius_x = (f64::from(point.x) + 0.5) * world_units_per_cell - state.pose.x;
        let radius_y = (f64::from(point.y) + 0.5) * world_units_per_cell - state.pose.y;
        let velocity = [
            state.velocity.x - state.velocity.angular * radius_y,
            state.velocity.y + state.velocity.angular * radius_x,
        ];
        let scale = 65_536.0 / (world_units_per_cell * f64::from(self.settings.step_hz));
        let upper_exclusive = 9_223_372_036_854_775_808.0;
        let mut result = [0_i64; 2];
        for axis in 0..2 {
            let quantized = velocity[axis] * scale;
            if !quantized.is_finite() {
                return Err(invalid("剛体流体排除粒子速度"));
            }
            let rounded = quantized.round();
            if rounded < i64::MIN as f64 || rounded >= upper_exclusive {
                return Err(invalid("剛体流体排除粒子速度の整数範囲"));
            }
            result[axis] = rounded as i64;
        }
        Ok(result)
    }

    pub(crate) fn validate_content(
        &self,
        content: &Content,
        limits: super::WorldLimits,
    ) -> EngineResult<()> {
        let actual_reservoir_bodies = self
            .local_grids
            .values()
            .filter(|grid| !grid.exclusion_reservoir.is_empty())
            .count();
        if actual_reservoir_bodies != self.exclusion_reservoir_body_count {
            return Err(invalid("body exclusion reservoir body count cache"));
        }
        for id in &self.local_grid_content_dirty {
            let grid = self.local_grids.get(id).ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "dirty local grid is missing")
            })?;
            grid.validate()?;
            for cell in grid
                .cells
                .values()
                .chain(grid.exclusion_reservoir.iter().map(|entry| &entry.cell))
            {
                validate_body_cell_content(cell, content)?;
            }
            for attachment in grid.block_attachments.values() {
                let mut decoder = Decoder::new(&attachment.codec, attachment.codec.len())?;
                let mut block = BlockInstance::decode(&mut decoder)?;
                decoder.finish()?;
                block.validate()?;
                block.connect_state(content, limits)?;
                block.validate_state()?;
                let definition = content
                    .material(block.material)?
                    .ok_or_else(|| invalid("body block attachment material"))?;
                let layer = content.layer(block.layer)?.key;
                let material = definition.key();
                if definition.block_size() != attachment.size
                    || block.remaining.len() != attachment.members.len()
                {
                    return Err(invalid("body block attachment Content shape"));
                }
                for (mask_index, local_index) in &attachment.members {
                    let x = (*mask_index % u32::from(attachment.size[0])) as u16;
                    let y = (*mask_index / u32::from(attachment.size[0])) as u16;
                    if !definition.shape().occupied(x, y, attachment.size) {
                        return Err(invalid(
                            "body block attachment mask is outside Content shape",
                        ));
                    }
                    let cell = grid
                        .cells
                        .get(local_index)
                        .ok_or_else(|| invalid("body block attachment cell missing"))?;
                    if cell.material()? != material || cell.layer()? != layer {
                        return Err(invalid("body block attachment cell Content key"));
                    }
                }
            }
        }
        Ok(())
    }

    /// `engine:body-local-grid-inventory`規則をlocal cellへ適用します。
    ///
    /// cellごとに既適用epochを持つため、後から追加されたcellにも過去の規則を一度ずつ
    /// 適用できます。混合成分が同一素材へ合流した場合は有理token量を合算し、pure cellへ
    /// 正確に正規化します。空素材化や表現不能な混合・carrier関係は候補全体を拒否します。
    pub(crate) fn apply_material_remaps(
        &mut self,
        remaps: &super::MaterialRemapTable,
        content: &Content,
        limits: super::WorldLimits,
    ) -> EngineResult<()> {
        let owner = body_inventory_owner();
        let current = remaps.epoch();
        let mut updates = Vec::new();
        for (id, existing) in self.local_grids.iter() {
            if existing.remap_epoch_bounds == Some((current, current)) {
                continue;
            }
            if !existing.block_attachments.is_empty()
                && existing
                    .cells
                    .values()
                    .any(|cell| cell.remap_epoch < current)
            {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "block個体作者状態のremap移行policyが未登録です",
                ));
            }
            let mut grid = existing.clone();
            let mut changed = false;
            for cell in grid.cells.values_mut().chain(
                grid.exclusion_reservoir
                    .iter_mut()
                    .map(|entry| &mut entry.cell),
            ) {
                if cell.remap_epoch > current {
                    return Err(invalid("body local cellの未来remap epoch"));
                }
                if cell.remap_epoch == current {
                    continue;
                }
                let applied = cell.remap_epoch;
                let layer = cell.layer()?;
                let carrier =
                    remaps.resolve(owner, layer, applied, MaterialHandle(cell.material()?))?;
                let carrier_id = content.material_id(carrier)?;
                if carrier_id == MaterialId::AIR {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "剛体local cellを空へremapするには破砕・排出ruleが必要",
                    ));
                }
                cell.material = carrier.0.as_str().to_owned();

                if !cell.mixture.is_empty() {
                    let mut quantities = BTreeMap::<StableKey, super::MaterialQuantity>::new();
                    for component in &cell.mixture {
                        let target = remaps.resolve(
                            owner,
                            layer,
                            applied,
                            MaterialHandle(component.material()?),
                        )?;
                        if content.material_id(target)? == MaterialId::AIR {
                            return Err(EngineError::new(
                                ErrorCode::Conflict,
                                "剛体混合成分を空へremapするには明示的な排出ruleが必要",
                            ));
                        }
                        let quantity = quantities
                            .entry(target.0)
                            .or_insert(super::MaterialQuantity::ZERO);
                        *quantity = quantity.checked_add(component.tokens())?;
                    }
                    let mut mapped = Vec::new();
                    mapped
                        .try_reserve_exact(quantities.len())
                        .map_err(allocation)?;
                    for (material, tokens) in quantities {
                        mapped.push(BodyMaterialComponent::new(material, tokens)?);
                    }
                    if mapped.len() == 1 {
                        let only = mapped.pop().expect("一成分へ正規化済み");
                        if only.material()? != carrier.0 {
                            return Err(EngineError::new(
                                ErrorCode::Conflict,
                                "remap後の剛体混合成分とcarrierを一つのcellで表現できません",
                            ));
                        }
                        cell.tokens = only.tokens();
                        cell.mixture.clear();
                    } else {
                        cell.mixture = mapped;
                    }
                }
                cell.remap_epoch = current;
                changed = true;
            }
            let cache_mismatch = grid.remap_epoch_bounds != Some((current, current));
            if changed || cache_mismatch {
                grid.remap_epoch_bounds = Some((current, current));
                if changed {
                    grid.revision = grid
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| invalid("body local remap revision overflow"))?;
                    grid.validate()?;
                }
                updates.push((*id, grid, changed));
            }
        }
        let changed_any = updates.iter().any(|(_, _, changed)| *changed);
        if !updates.is_empty() {
            let local_grids = Arc::make_mut(&mut self.local_grids);
            for (id, grid, changed) in updates {
                local_grids.insert(id, grid);
                if changed {
                    self.local_grid_content_dirty.insert(id);
                }
            }
            if changed_any {
                self.validate_content(content, limits)?;
                self.sync_material_mass_properties(content)?;
            }
        }
        Ok(())
    }

    /// local gridを持つdynamic bodyの質量特性をContentの素材質量から再構築します。
    ///
    /// 呼出側は候補BodyPhysics上で実行し、全bodyの検証に成功した時だけWorldへ公開します。
    /// 通常の固定stepでは素材定義を再走査せず、grid編集・World restoreの境界で一度だけ計算します。
    pub(crate) fn sync_material_mass_properties(&mut self, content: &Content) -> EngineResult<()> {
        if self.local_grid_content_dirty.is_empty() {
            return Ok(());
        }
        copy_backend_state(&self.backend, &mut self.candidate_backend);
        for id in &self.local_grid_content_dirty {
            let grid = self.local_grids.get(id).ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "dirty local grid is missing")
            })?;
            let body_handle = self.handles.get(id).copied().ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "local grid body missing")
            })?;
            let is_dynamic = self
                .candidate_backend
                .bodies
                .get(body_handle)
                .is_some_and(|body| body.body_type() == RigidBodyType::Dynamic);
            if !is_dynamic {
                continue;
            }
            let collider_handle = self
                .candidate_backend
                .bodies
                .get(body_handle)
                .and_then(|body| body.colliders().first().copied())
                .ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "local grid collider missing")
                })?;
            let mass_properties = grid.material_mass_properties(content)?;
            self.candidate_backend
                .colliders
                .get_mut(collider_handle)
                .ok_or_else(|| {
                    EngineError::new(
                        ErrorCode::InvalidState,
                        "local grid collider handle missing",
                    )
                })?
                .set_mass_properties(mass_properties);
            {
                let backend = &mut self.candidate_backend;
                backend
                    .bodies
                    .get_mut(body_handle)
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "local grid body handle missing")
                    })?
                    .recompute_mass_properties_from_colliders(&backend.colliders);
            }
        }
        validate_backend_state(&self.candidate_backend)?;
        std::mem::swap(&mut self.backend, &mut self.candidate_backend);
        self.local_grid_content_dirty.clear();
        Ok(())
    }

    /// resident terrain chunkのうち、solid colliderを持つ数です。
    pub fn terrain_chunk_count(&self) -> usize {
        self.terrain_handles.len()
    }

    /// 現在登録中のjoint数です。
    pub fn joint_count(&self) -> u32 {
        self.joint_handles.len() as u32
    }

    /// jointの現在のbody endpointを読みます。
    pub fn joint_bodies(&self, id: JointId) -> EngineResult<(BodyId, BodyId)> {
        let handle = self
            .joint_handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "joint ID"))?;
        let joint = self
            .backend
            .impulse_joints
            .get(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "Rapier joint handle"))?;
        let body_id = |body_handle| {
            self.handles
                .iter()
                .find_map(|(body_id, handle)| (*handle == body_handle).then_some(*body_id))
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "joint body handle"))
        };
        Ok((body_id(joint.body1())?, body_id(joint.body2())?))
    }

    /// resident World layerのsolid cellをchunk単位のexact colliderへ反映します。
    ///
    /// World全体の複製は行わず、Worldは不変借用のまま読みます。変更chunkだけを再構築し、
    /// 旧terrain colliderを含むcandidate backend全体が準備できた時点で一括公開します。
    /// 非resident chunkはcolliderを持ちません。物理stepとWorld editの同時rollbackには
    /// `World::edit_and_step_bodies`を使います。単独同期で発生したcontact exitはこのcommitの
    /// event一覧へ公開し、World固定stepの内部同期では同じstep reportへまとめます。
    pub fn sync_solid_terrain(
        &mut self,
        world: &World,
        layer: LayerId,
        cell_size: f64,
    ) -> EngineResult<bool> {
        self.sync_solid_terrain_inner(world, layer, cell_size, false)
    }

    /// World fixed-step候補内でterrainを同期し、終了eventを同じstep reportへまとめます。
    pub(crate) fn sync_solid_terrain_for_step(
        &mut self,
        world: &World,
        layer: LayerId,
        cell_size: f64,
    ) -> EngineResult<bool> {
        self.sync_solid_terrain_inner(world, layer, cell_size, true)
    }

    fn sync_solid_terrain_inner(
        &mut self,
        world: &World,
        layer: LayerId,
        cell_size: f64,
        carry_events_into_following_step: bool,
    ) -> EngineResult<bool> {
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(invalid("terrain collider cell size"));
        }
        world.content.layer(layer)?;
        let source = TerrainSource {
            world_index: world.id.index(),
            world_generation: world.id.generation(),
            layer_index: layer.0,
            cell_size,
            world_revision: world.revision(),
        };
        let source_changed = self.terrain_source.is_none_or(|previous| {
            previous.world_index != source.world_index
                || previous.world_generation != source.world_generation
                || previous.layer_index != source.layer_index
                || previous.cell_size != source.cell_size
        });
        if !source_changed
            && self
                .terrain_source
                .is_some_and(|previous| previous.world_revision == source.world_revision)
        {
            return Ok(false);
        }
        let mut revisions = BTreeMap::new();
        for (coord, chunk) in &world.chunks {
            revisions.insert(*coord, chunk.revision);
        }
        let removed = self
            .terrain_revisions
            .keys()
            .copied()
            .filter(|coord| !revisions.contains_key(coord))
            .collect::<Vec<_>>();
        let changed = revisions
            .iter()
            .filter_map(|(coord, revision)| {
                (source_changed || self.terrain_revisions.get(coord) != Some(revision))
                    .then_some((*coord, *revision))
            })
            .collect::<Vec<_>>();
        if !source_changed && removed.is_empty() && changed.is_empty() {
            self.terrain_source = Some(source);
            return Ok(false);
        }
        let mut terrain_exit_reasons = BTreeMap::new();
        for coord in &removed {
            if self.terrain_handles.contains_key(coord) {
                terrain_exit_reasons.insert(*coord, BodyContactExitReason::TerrainUnloaded);
            }
        }
        if source_changed {
            for coord in self.terrain_handles.keys() {
                terrain_exit_reasons.insert(*coord, BodyContactExitReason::TerrainUnloaded);
            }
        } else {
            for (coord, _) in &changed {
                if self.terrain_handles.contains_key(coord) {
                    terrain_exit_reasons.insert(*coord, BodyContactExitReason::TerrainChanged);
                }
            }
        }

        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(changed.len())
            .map_err(crate::foundation::error::allocation)?;
        for (coord, revision) in &changed {
            let chunk = &world.chunks[coord];
            let page = chunk
                .layers
                .get(layer.0 as usize)
                .ok_or_else(|| invalid("terrain layer page"))?;
            let mut occupied = Vec::new();
            occupied
                .try_reserve_exact(CELL_COUNT)
                .map_err(crate::foundation::error::allocation)?;
            occupied.resize(CELL_COUNT, false);
            for (index, cell) in page.cells.iter().enumerate() {
                if cell.material == crate::world::MaterialId::AIR {
                    continue;
                }
                let material = world
                    .content
                    .material(cell.material)?
                    .ok_or_else(|| invalid("terrain references an empty material"))?;
                if material.base() == MaterialBase::Solid {
                    if matches!(
                        material.shape(),
                        BlockShape::Custom { .. } | BlockShape::Platform
                    ) {
                        return Err(EngineError::new(
                            ErrorCode::UnsupportedCapability,
                            "custom or one-way terrain collision provider is not registered",
                        ));
                    }
                    occupied[index] = true;
                }
            }
            if !occupied.iter().any(|value| *value) {
                candidates.push((*coord, *revision, None));
                continue;
            }
            let stencil = BodyStencil::new(64, 64, cell_size, [0.0, 0.0], *revision, occupied)?;
            let shape = stencil.collider_shape()?;
            let origin = coord.origin()?;
            let mut definition = BodyDef::dynamic(shape);
            definition.motion = BodyMotionType::Fixed;
            definition.pose.x = f64::from(origin.x) * cell_size;
            definition.pose.y = f64::from(origin.y) * cell_size;
            definition.validate()?;
            candidates.push((*coord, *revision, Some(definition)));
        }

        let mut removed_all = removed.into_iter().collect::<BTreeSet<_>>();
        if source_changed {
            removed_all.extend(self.terrain_handles.keys().copied());
        } else {
            removed_all.extend(changed.iter().map(|(coord, _)| *coord));
        }
        copy_backend_state(&self.backend, &mut self.candidate_backend);
        let mut candidate_terrain = self.terrain_handles.clone();
        for coord in removed_all {
            if let Some(handle) = candidate_terrain.remove(&coord) {
                self.candidate_backend.bodies.remove(
                    handle,
                    &mut self.candidate_backend.islands,
                    &mut self.candidate_backend.colliders,
                    &mut self.candidate_backend.impulse_joints,
                    &mut self.candidate_backend.multibody_joints,
                    &mut self.candidate_backend.soft_bodies,
                    true,
                );
            }
        }
        let new_count = candidates
            .iter()
            .filter(|(_, _, definition)| definition.is_some())
            .count();
        let final_count = self
            .candidate_backend
            .bodies
            .len()
            .checked_add(new_count)
            .ok_or_else(|| EngineError::capacity("physics body capacity", u64::MAX, u64::MAX))?;
        if final_count > self.settings.body_capacity as usize {
            return Err(EngineError::capacity(
                "physics body capacity including terrain",
                final_count as u64,
                u64::from(self.settings.body_capacity),
            ));
        }
        for (coord, _, definition) in candidates {
            let Some(definition) = definition else {
                continue;
            };
            let collider = definition.collider.build()?;
            let body = RigidBodyBuilder::fixed()
                .translation(Vector::new(definition.pose.x, definition.pose.y))
                .user_data(pack_terrain_chunk(coord))
                .build();
            let handle = self.candidate_backend.bodies.insert(body);
            self.candidate_backend.colliders.insert_with_parent(
                collider,
                handle,
                &mut self.candidate_backend.bodies,
            );
            candidate_terrain.insert(coord, handle);
        }
        if !self.candidate_backend.quarantine().is_empty() {
            return Err(invalid("terrain collider rebuild quarantine"));
        }
        collect_terrain_exit_events(
            &self.backend,
            &terrain_exit_reasons,
            &mut self.contact_before,
            &mut self.contact_velocity_before,
            &mut self.contact_event_scratch,
            self.settings.contact_event_capacity as usize,
        )?;
        std::mem::swap(&mut self.backend, &mut self.candidate_backend);
        self.terrain_handles = candidate_terrain;
        self.terrain_revisions = revisions;
        self.terrain_source = Some(source);
        if !self.contact_event_scratch.is_empty() {
            self.publish_contact_events();
            self.terrain_contact_events_pending = carry_events_into_following_step;
        }
        Ok(true)
    }

    /// fluid移動だけでWorld revisionが進んだ場合、変化していないterrain cacheを現revisionへ結びます。
    pub(crate) fn acknowledge_world_revision(&mut self, revision: u64) {
        if let Some(source) = &mut self.terrain_source {
            source.world_revision = revision;
        }
    }

    /// 指定定義のbodyとそのcolliderをまとめて登録します。
    pub fn insert(&mut self, definition: BodyDef) -> EngineResult<BodyId> {
        definition.validate()?;
        if self.backend.bodies.len() >= self.settings.body_capacity as usize {
            return Err(EngineError::capacity(
                "剛体数",
                self.backend.bodies.len() as u64 + 1,
                u64::from(self.settings.body_capacity),
            ));
        }
        let index = self.next_body_index;
        let next = index.checked_add(1).ok_or_else(|| {
            EngineError::capacity("剛体ID", u64::from(u32::MAX) + 1, u64::from(u32::MAX))
        })?;
        let id = BodyId::from_parts(index, 1)?;
        let motion = match definition.motion {
            BodyMotionType::Fixed => RigidBodyType::Fixed,
            BodyMotionType::KinematicPosition => RigidBodyType::KinematicPositionBased,
            BodyMotionType::KinematicVelocity => RigidBodyType::KinematicVelocityBased,
            BodyMotionType::Dynamic => RigidBodyType::Dynamic,
        };
        let data = pack_body_id(id);
        let body = RigidBodyBuilder::new(motion)
            .translation(Vector::new(definition.pose.x, definition.pose.y))
            .rotation(definition.pose.angle)
            .linvel(Vector::new(definition.velocity.x, definition.velocity.y))
            .angvel(definition.velocity.angular)
            .gravity_scale(definition.gravity_scale)
            .ccd_enabled(definition.ccd)
            .can_sleep(definition.can_sleep)
            .user_data(data)
            .build();
        let collider = definition.collider.build()?;
        let handle = self.backend.bodies.insert(body);
        self.backend
            .colliders
            .insert_with_parent(collider, handle, &mut self.backend.bodies);
        if self.handles.insert(id, handle).is_some() {
            return Err(EngineError::new(ErrorCode::Conflict, "剛体ID重複"));
        }
        self.next_body_index = next;
        Ok(id)
    }

    /// local material cellからexact compound shapeを作り、物質gridとbodyを同時に登録します。
    ///
    /// callerのshape指定はgridの占有maskへ置き換わります。物質の二重所有を避けるため、
    /// gridはBodyPhysicsだけが所有し、World cellへ仮コピーしません。失敗時はbodyもgridも
    /// 登録しません。
    pub fn insert_with_local_grid(
        &mut self,
        mut definition: BodyDef,
        grid: BodyLocalGrid,
    ) -> EngineResult<BodyId> {
        grid.validate()?;
        self.validate_local_grid_budget(None, &grid)?;
        let has_reservoir = !grid.exclusion_reservoir.is_empty();
        let reservoir_body_count = if has_reservoir {
            self.exclusion_reservoir_body_count
                .checked_add(1)
                .ok_or_else(|| invalid("reservoir body count overflow"))?
        } else {
            self.exclusion_reservoir_body_count
        };
        let stencil = grid.stencil()?;
        definition.collider.shape = stencil.collider_shape()?;
        definition.validate()?;
        let id = self.insert(definition)?;
        if Arc::make_mut(&mut self.local_grids)
            .insert(id, grid)
            .is_some()
        {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "body local grid ID duplicate",
            ));
        }
        self.local_grid_content_dirty.insert(id);
        self.exclusion_reservoir_body_count = reservoir_body_count;
        Ok(id)
    }

    /// bodyが所有するlocal material gridを読みます。
    pub fn local_grid(&self, id: BodyId) -> EngineResult<Option<&BodyLocalGrid>> {
        if !self.handles.contains_key(&id) {
            return Err(EngineError::new(ErrorCode::StaleId, "剛体ID"));
        }
        Ok(self.local_grids.get(&id))
    }

    /// World物質cellをbody所属の排除reservoirへ退避します。collider・mass propertyは更新しません。
    pub(crate) fn store_exclusion_cells(
        &mut self,
        body: BodyId,
        cells: Vec<(CellPoint, BodyLocalCell)>,
        content: &Content,
    ) -> EngineResult<()> {
        for (_, cell) in &cells {
            validate_body_cell_content(cell, content)?;
        }
        let mut grid = self
            .local_grid(body)?
            .cloned()
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body local grid未設定"))?;
        let had_reservoir = !grid.exclusion_reservoir.is_empty();
        let reservoir_body_count = if had_reservoir {
            self.exclusion_reservoir_body_count
        } else {
            self.exclusion_reservoir_body_count
                .checked_add(1)
                .ok_or_else(|| invalid("reservoir body count overflow"))?
        };
        grid.store_exclusion_cells(cells)?;
        self.validate_local_grid_budget(Some(body), &grid)?;
        grid.validate()?;
        Arc::make_mut(&mut self.local_grids).insert(body, grid);
        self.exclusion_reservoir_body_count = reservoir_body_count;
        Ok(())
    }

    /// 排除reservoirから物質cellを取り出します。呼出側は同じWorld取引で出力先を確定します。
    pub(crate) fn take_exclusion_cells(
        &mut self,
        body: BodyId,
        origins: &[CellPoint],
    ) -> EngineResult<Vec<BodyLocalCell>> {
        let mut grid = self
            .local_grid(body)?
            .cloned()
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body local grid未設定"))?;
        let had_reservoir = !grid.exclusion_reservoir.is_empty();
        let cells = grid.take_exclusion_cells(origins)?;
        self.validate_local_grid_budget(Some(body), &grid)?;
        grid.validate()?;
        let has_reservoir = !grid.exclusion_reservoir.is_empty();
        let reservoir_body_count = if had_reservoir && !has_reservoir {
            self.exclusion_reservoir_body_count
                .checked_sub(1)
                .ok_or_else(|| invalid("reservoir body count underflow"))?
        } else {
            self.exclusion_reservoir_body_count
        };
        Arc::make_mut(&mut self.local_grids).insert(body, grid);
        self.exclusion_reservoir_body_count = reservoir_body_count;
        Ok(cells)
    }

    fn validate_local_grid_budget(
        &self,
        replacing: Option<BodyId>,
        grid: &BodyLocalGrid,
    ) -> EngineResult<()> {
        let mut cells = grid.owned_cell_count();
        let mut geometry_cells = grid.geometry_cell_count();
        let mut sidecar_bytes = grid.sidecar_bytes()?;
        for (id, existing) in self.local_grids.iter() {
            if Some(*id) == replacing {
                continue;
            }
            cells = cells
                .checked_add(existing.owned_cell_count())
                .ok_or_else(|| invalid("body local cell total overflow"))?;
            geometry_cells = geometry_cells
                .checked_add(existing.geometry_cell_count())
                .ok_or_else(|| invalid("body local geometry total overflow"))?;
            sidecar_bytes = sidecar_bytes
                .checked_add(existing.sidecar_bytes()?)
                .ok_or_else(|| invalid("body local sidecar total overflow"))?;
        }
        if cells > self.settings.body_local_cell_capacity as usize {
            return Err(EngineError::capacity(
                "body local cell total",
                cells as u64,
                u64::from(self.settings.body_local_cell_capacity),
            ));
        }
        if geometry_cells > self.settings.body_local_geometry_cell_capacity as usize {
            return Err(EngineError::capacity(
                "body local geometry cell total",
                geometry_cells as u64,
                u64::from(self.settings.body_local_geometry_cell_capacity),
            ));
        }
        if sidecar_bytes > self.settings.body_local_sidecar_byte_capacity as usize {
            return Err(EngineError::capacity(
                "body local sidecar byte total",
                sidecar_bytes as u64,
                u64::from(self.settings.body_local_sidecar_byte_capacity),
            ));
        }
        Ok(())
    }

    /// bodyの物質grid・占有collider・質量特性を一つの変更として置き換えます。
    ///
    /// colliderは新しいoccupied maskから再構築し、Rapierへ質量特性を再計算させます。
    /// 候補shapeの構築やID照合が失敗した場合は元のgrid・colliderを保持します。
    pub fn replace_local_grid(&mut self, id: BodyId, grid: BodyLocalGrid) -> EngineResult<()> {
        grid.validate()?;
        if !self.handles.contains_key(&id) {
            return Err(EngineError::new(ErrorCode::StaleId, "剛体ID"));
        }
        let had_reservoir = self
            .local_grids
            .get(&id)
            .is_some_and(|existing| !existing.exclusion_reservoir.is_empty());
        let has_reservoir = !grid.exclusion_reservoir.is_empty();
        if self.local_grids.get(&id).is_some_and(|existing| {
            !existing.exclusion_reservoir.is_empty()
                && existing.exclusion_reservoir != grid.exclusion_reservoir
        }) {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "reservoir物質は専用のWorld復帰取引でだけ変更できます",
            ));
        }
        let reservoir_body_count = match (had_reservoir, has_reservoir) {
            (false, true) => self
                .exclusion_reservoir_body_count
                .checked_add(1)
                .ok_or_else(|| invalid("reservoir body count overflow"))?,
            (true, false) => self
                .exclusion_reservoir_body_count
                .checked_sub(1)
                .ok_or_else(|| invalid("reservoir body count underflow"))?,
            _ => self.exclusion_reservoir_body_count,
        };
        self.validate_local_grid_budget(Some(id), &grid)?;
        let stencil = grid.stencil()?;
        let shape = stencil.collider_shape()?.shared_shape()?;
        let body_handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        copy_backend_state(&self.backend, &mut self.candidate_backend);
        let collider_handle = self
            .candidate_backend
            .bodies
            .get(body_handle)
            .and_then(|body| body.colliders().first().copied())
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body collider missing"))?;
        if !self.candidate_backend.colliders.contains(collider_handle) {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "body collider handle missing",
            ));
        }
        self.candidate_backend
            .colliders
            .get_mut(collider_handle)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body collider missing"))?
            .set_shape(shape);
        {
            let backend = &mut self.candidate_backend;
            backend
                .bodies
                .get_mut(body_handle)
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body handle missing"))?
                .recompute_mass_properties_from_colliders(&backend.colliders);
        }
        validate_backend_state(&self.candidate_backend)?;
        std::mem::swap(&mut self.backend, &mut self.candidate_backend);
        Arc::make_mut(&mut self.local_grids).insert(id, grid);
        self.exclusion_reservoir_body_count = reservoir_body_count;
        self.local_grid_content_dirty.insert(id);
        Ok(())
    }

    /// disconnectedなmaterial componentを別々のdynamic bodyへ分けます。
    ///
    /// 元bodyのposeを共有し、各fragmentの重心速度を`v + ω×r`で再計算します。これにより
    /// cell質量・重心・慣性が同じ値で表せる範囲では、分割前後の線形・角運動量を保ちます。
    /// occupied component上のjoint anchorは対応fragmentへ移し、空隙・grid外のanchor jointは除去結果へ
    /// 記録します。未適用外力はfragment質量比、トルクは慣性比で分配し、同じ次stepへ引き継ぎます。
    /// 呼出側はWorldEdit候補上で実行し、全fragmentと容量検査が成功してからWorldをcommitします。
    fn fracture_disconnected_local_grid(
        &mut self,
        id: BodyId,
        content: &Content,
        limits: super::WorldLimits,
        provider: &dyn BodyLocalBondProvider,
        attachment_provider: Option<&dyn BodyBlockAttachmentSplitProvider>,
    ) -> EngineResult<BodyFractureReport> {
        let source_grid = self.local_grid(id)?.ok_or_else(|| {
            EngineError::new(
                ErrorCode::InvalidState,
                "fracture対象bodyにlocal gridがありません",
            )
        })?;
        let components = source_grid.connected_components(provider)?;
        if components.is_empty() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "空のbody local gridは破砕できません",
            ));
        }
        if components.len() == 1 {
            return Ok(BodyFractureReport {
                fragments: vec![id],
                removed_joints: Vec::new(),
                attachment_provider: attachment_provider.map(|policy| {
                    BodyAttachmentProviderStamp {
                        key: policy.key(),
                        version: policy.version(),
                    }
                }),
            });
        }

        let source_handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "破砕対象BodyId"))?;
        let (
            pose,
            velocity,
            gravity_scale,
            ccd,
            can_sleep,
            collider,
            source_com,
            pending_force,
            pending_torque,
        ) = {
            let source = self
                .backend
                .bodies
                .get(source_handle)
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier破砕対象body"))?;
            if source.body_type() != RigidBodyType::Dynamic {
                return Err(EngineError::new(
                    ErrorCode::UnsupportedCapability,
                    "現在の破砕APIはdynamic bodyのみ対応します",
                ));
            }
            if source.colliders().len() != 1 {
                return Err(EngineError::new(
                    ErrorCode::UnsupportedCapability,
                    "破砕にはbodyあたり一つのlocal-grid colliderが必要です",
                ));
            }
            let collider_handle = source.colliders()[0];
            let source_collider = self.backend.colliders.get(collider_handle).ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "body collider missing")
            })?;
            let source_mass = self.mass_properties(id)?;
            (
                BodyPose {
                    x: source.translation().x,
                    y: source.translation().y,
                    angle: source.rotation().angle(),
                },
                BodyVelocity {
                    x: source.linvel().x,
                    y: source.linvel().y,
                    angular: source.angvel(),
                },
                source.gravity_scale(),
                source.is_ccd_enabled(),
                source.activation().normalized_linear_threshold >= 0.0
                    && source.activation().angular_threshold >= 0.0,
                ColliderDef {
                    shape: ColliderShape::Circle { radius: 0.5 },
                    density: source_collider.density(),
                    friction: source_collider.friction(),
                    restitution: source_collider.restitution(),
                    sensor: source_collider.is_sensor(),
                },
                source_mass.center_of_mass,
                source.user_force(),
                source.user_torque(),
            )
        };
        // local anchorを切断前のoccupied cellへ対応付けます。cell境界上では接する
        // componentのうち最小indexを選び、結果が浮動小数点の探索順に依存しないようにします。
        let mut component_by_cell = BTreeMap::new();
        for (component_index, component) in components.iter().enumerate() {
            for [x, y] in component {
                let index = y
                    .checked_mul(source_grid.width)
                    .and_then(|row| row.checked_add(*x))
                    .ok_or_else(|| invalid("fracture component cell index"))?;
                component_by_cell.insert(index, component_index);
            }
        }
        let mut component_attachments = Vec::new();
        component_attachments
            .try_reserve_exact(components.len())
            .map_err(allocation)?;
        component_attachments.resize_with(components.len(), Vec::new);
        for attachment in source_grid.block_attachments.values() {
            let mut grouped_members = BTreeMap::<usize, Vec<(u32, u32)>>::new();
            for (mask_index, local_index) in &attachment.members {
                let component = component_by_cell.get(local_index).copied().ok_or_else(|| {
                    invalid("block attachment member is missing from fracture components")
                })?;
                grouped_members
                    .entry(component)
                    .or_default()
                    .push((*mask_index, *local_index));
            }
            if grouped_members.is_empty() {
                return Err(invalid("block attachment has no fracture component"));
            }
            let mut split_instances = Vec::new();
            if grouped_members.len() == 1 {
                split_instances.push(None);
            } else {
                let policy = attachment_provider.ok_or_else(|| {
                    EngineError::new(
                        ErrorCode::Conflict,
                        "block attachmentの分割には作者状態migration providerが必要です",
                    )
                })?;
                let mut decoder = Decoder::new(&attachment.codec, attachment.codec.len())?;
                let mut original = BlockInstance::decode(&mut decoder)?;
                decoder.finish()?;
                original.connect_state(content, limits)?;
                original.validate_state()?;
                let mut fragment_masks = Vec::new();
                fragment_masks
                    .try_reserve_exact(grouped_members.len())
                    .map_err(allocation)?;
                for members in grouped_members.values() {
                    let mut mask = Vec::new();
                    mask.try_reserve_exact(members.len()).map_err(allocation)?;
                    mask.extend(members.iter().map(|(mask_index, _)| *mask_index));
                    fragment_masks.push(mask);
                }
                let migrated = policy.split(
                    &original,
                    &fragment_masks,
                    source_grid.sidecar_byte_capacity as usize,
                )?;
                if migrated.len() != fragment_masks.len() {
                    return Err(invalid("block attachment provider result count"));
                }
                for (instance, expected_mask) in migrated.iter().zip(&fragment_masks) {
                    if instance.material() != original.material()
                        || instance.layer() != original.layer()
                        || instance.anchor() != original.anchor()
                        || instance.orientation() != original.orientation()
                        || instance.remaining_mask() != expected_mask
                    {
                        return Err(invalid(
                            "block attachment provider changed block identity or mask",
                        ));
                    }
                }
                split_instances.extend(migrated.into_iter().map(Some));
            }
            for ((component_index, members), migrated) in
                grouped_members.into_iter().zip(split_instances)
            {
                let codec = if let Some(instance) = migrated {
                    let mut encoder = Encoder::new(source_grid.sidecar_byte_capacity as usize);
                    instance.encode(&mut encoder)?;
                    encoder.finish()
                } else {
                    super::terrain::copy_slice(&attachment.codec)?
                };
                component_attachments[component_index]
                    .try_reserve(1)
                    .map_err(allocation)?;
                component_attachments[component_index].push(BodyLocalBlockAttachment {
                    size: attachment.size,
                    members,
                    codec,
                });
            }
        }
        let mut joints_to_migrate = Vec::new();
        joints_to_migrate
            .try_reserve_exact(self.joint_handles.len())
            .map_err(allocation)?;
        for (joint_id, joint_handle) in &self.joint_handles {
            let Some(joint) = self.backend.impulse_joints.get(*joint_handle) else {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "joint handle missing",
                ));
            };
            let (anchor, source_is_body1) = if joint.body1() == source_handle {
                (joint.data.local_anchor1(), true)
            } else if joint.body2() == source_handle {
                (joint.data.local_anchor2(), false)
            } else {
                continue;
            };
            let component =
                body_local_component_for_anchor(source_grid, &component_by_cell, anchor);
            joints_to_migrate.push((
                *joint_id,
                *joint_handle,
                joint.body1(),
                joint.body2(),
                source_is_body1,
                component,
                joint.data,
                joint.impulses,
            ));
        }

        let cell_size = source_grid.cell_size();
        let old_revision = source_grid.revision();
        let mut fragment_grids = Vec::new();
        fragment_grids
            .try_reserve_exact(components.len())
            .map_err(allocation)?;
        let mut fragment_centers = Vec::new();
        fragment_centers
            .try_reserve_exact(components.len())
            .map_err(allocation)?;
        for (component_index, component) in components.into_iter().enumerate() {
            let min_x = component.iter().map(|point| point[0]).min().unwrap();
            let min_y = component.iter().map(|point| point[1]).min().unwrap();
            let max_x = component.iter().map(|point| point[0]).max().unwrap();
            let max_y = component.iter().map(|point| point[1]).max().unwrap();
            let width = max_x - min_x + 1;
            let height = max_y - min_y + 1;
            let geometry_cells = u64::from(width) * u64::from(height);
            let geometry_limit = u32::try_from(geometry_cells)
                .map_err(|_| invalid("fragment geometry cell count"))?;
            let reservoir_count = if fragment_grids.is_empty() {
                source_grid.exclusion_reservoir.len()
            } else {
                0
            };
            let cell_limit = u32::try_from(
                component
                    .len()
                    .checked_add(reservoir_count)
                    .ok_or_else(|| invalid("fragment local cell count overflow"))?,
            )
            .map_err(|_| invalid("fragment local cell count"))?;
            let pivot = source_grid.pivot();
            let mut fragment_grid = BodyLocalGrid::new(
                width,
                height,
                cell_size,
                [
                    pivot[0] - f64::from(min_x) * cell_size,
                    pivot[1] - f64::from(min_y) * cell_size,
                ],
                old_revision,
                BodyLocalGridLimits {
                    cells: cell_limit,
                    geometry_cells: geometry_limit,
                    sidecar_bytes: source_grid.sidecar_byte_capacity,
                },
            )?;
            let mut cells = Vec::new();
            cells
                .try_reserve_exact(component.len())
                .map_err(allocation)?;
            for [x, y] in &component {
                let mut cell = source_grid
                    .cell(*x, *y)
                    .cloned()
                    .ok_or_else(|| invalid("fracture component lost a local cell"))?;
                cell.block_attachment = None;
                cell.block_mask_index = None;
                cells.push((*x - min_x, *y - min_y, cell));
            }
            fragment_grid.insert_cells_batch(cells)?;
            let component_indexes = component
                .iter()
                .map(|[x, y]| y * source_grid.width + x)
                .collect::<BTreeSet<_>>();
            for attachment in &component_attachments[component_index] {
                let mut members = Vec::new();
                members
                    .try_reserve_exact(attachment.members.len())
                    .map_err(allocation)?;
                for (mask_index, local_index) in &attachment.members {
                    let source_x = local_index % source_grid.width;
                    let source_y = local_index / source_grid.width;
                    let fragment_index = (source_y - min_y)
                        .checked_mul(width)
                        .and_then(|row| row.checked_add(source_x - min_x))
                        .ok_or_else(|| invalid("fracture block attachment index overflow"))?;
                    members.push((*mask_index, fragment_index));
                }
                fragment_grid.attach_block_state(
                    attachment.size,
                    members,
                    super::terrain::copy_slice(&attachment.codec)?,
                )?;
            }
            for (source_index, source_edge) in &source_grid.thermal_edges {
                if !component_indexes.contains(source_index) {
                    continue;
                }
                let source_x = source_index % source_grid.width;
                let source_y = source_index / source_grid.width;
                let mut fragment_edge = super::ThermalEdgeState::default();
                if source_edge.residual_x != 0 || source_edge.equilibrium_credit_x != 0 {
                    let right = source_index + 1;
                    if component_indexes.contains(&right) {
                        fragment_edge.residual_x = source_edge.residual_x;
                        fragment_edge.equilibrium_credit_x = source_edge.equilibrium_credit_x;
                    }
                }
                if source_edge.residual_y != 0 || source_edge.equilibrium_credit_y != 0 {
                    let below = source_index + source_grid.width;
                    if component_indexes.contains(&below) {
                        fragment_edge.residual_y = source_edge.residual_y;
                        fragment_edge.equilibrium_credit_y = source_edge.equilibrium_credit_y;
                    }
                }
                if fragment_edge != super::ThermalEdgeState::default() {
                    let fragment_index = (source_y - min_y) * width + (source_x - min_x);
                    fragment_grid
                        .thermal_edges
                        .insert(fragment_index, fragment_edge);
                }
            }
            for (source_edge_key, source_edge) in &source_grid.thermal_boundary_edges {
                let source_index = u32::try_from(source_edge_key / 4)
                    .map_err(|_| invalid("fracture thermal boundary source index"))?;
                if !component_indexes.contains(&source_index) {
                    continue;
                }
                let source_x = source_index % source_grid.width;
                let source_y = source_index / source_grid.width;
                let fragment_index = (source_y - min_y)
                    .checked_mul(width)
                    .and_then(|row| row.checked_add(source_x - min_x))
                    .ok_or_else(|| invalid("fracture thermal boundary index overflow"))?;
                let fragment_key = u64::from(fragment_index) * 4 + source_edge_key % 4;
                fragment_grid
                    .thermal_boundary_edges
                    .insert(fragment_key, *source_edge);
            }
            fragment_grid.validate()?;
            if fragment_grids.is_empty() {
                for entry in &source_grid.exclusion_reservoir {
                    fragment_grid.store_exclusion_cell(
                        CellPoint {
                            x: entry.origin[0],
                            y: entry.origin[1],
                        },
                        entry.cell.clone(),
                    )?;
                }
            }
            let mass = fragment_grid.material_mass_properties(content)?;
            fragment_centers.push([mass.local_com.x, mass.local_com.y]);
            fragment_grids.push(fragment_grid);
        }

        let mut velocities = Vec::new();
        velocities
            .try_reserve_exact(fragment_centers.len())
            .map_err(allocation)?;
        let (sin, cos) = pose.angle.sin_cos();
        for center in &fragment_centers {
            let local_x = center[0] - source_com[0];
            let local_y = center[1] - source_com[1];
            let offset_x = cos.mul_add(local_x, -sin * local_y);
            let offset_y = sin.mul_add(local_x, cos * local_y);
            velocities.push(BodyVelocity {
                x: velocity.x - velocity.angular * offset_y,
                y: velocity.y + velocity.angular * offset_x,
                angular: velocity.angular,
            });
        }

        let mut fragment_grid_iter = fragment_grids.into_iter();
        let source_fragment = fragment_grid_iter
            .next()
            .ok_or_else(|| invalid("fracture component plan is empty"))?;
        {
            let source = self
                .backend
                .bodies
                .get_mut(source_handle)
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier破砕対象body"))?;
            source.reset_forces(true);
            source.reset_torques(true);
        }
        self.replace_local_grid(id, source_fragment)?;
        let mut fragments = Vec::new();
        fragments
            .try_reserve_exact(velocities.len())
            .map_err(allocation)?;
        fragments.push(id);
        for (grid, fragment_velocity) in fragment_grid_iter.zip(velocities.iter().copied().skip(1))
        {
            let mut definition = BodyDef::dynamic(ColliderShape::Circle {
                radius: cell_size * 0.5,
            });
            definition.pose = pose;
            definition.velocity = fragment_velocity;
            definition.gravity_scale = gravity_scale;
            definition.ccd = ccd;
            definition.can_sleep = can_sleep;
            definition.collider = collider.clone();
            let fragment = self.insert_with_local_grid(definition, grid)?;
            fragments.push(fragment);
        }
        self.validate_content(content, limits)?;
        self.sync_material_mass_properties(content)?;
        for (fragment, fragment_velocity) in
            fragments.iter().copied().zip(velocities.iter().copied())
        {
            self.set_velocity(fragment, fragment_velocity)?;
        }
        // 重心へ加わるforceを質量比、body-localのpure torqueを慣性比で分けます。
        // 質量比例のforceは全fragmentの合力を保ち、source COMまわりの余分な軌道torqueを
        // 生みません。最後のfragmentへ浮動小数点の残差を割り当て、合計値の丸め差を抑えます。
        let mut fragment_masses = Vec::new();
        fragment_masses
            .try_reserve_exact(fragments.len())
            .map_err(allocation)?;
        let mut total_mass = 0.0;
        let mut total_inertia = 0.0;
        for fragment in &fragments {
            let properties = self.mass_properties(*fragment)?;
            total_mass += properties.mass;
            total_inertia += properties.angular_inertia;
            fragment_masses.push(properties);
        }
        if !total_mass.is_finite()
            || total_mass <= 0.0
            || !total_inertia.is_finite()
            || total_inertia <= 0.0
            || !pending_force.x.is_finite()
            || !pending_force.y.is_finite()
            || !pending_torque.is_finite()
        {
            return Err(invalid("fracture force distribution properties"));
        }
        let mut assigned_force = Vector::ZERO;
        let mut assigned_torque = 0.0;
        for index in 0..fragments.len() {
            let (force, torque) = if index + 1 == fragments.len() {
                (
                    pending_force - assigned_force,
                    pending_torque - assigned_torque,
                )
            } else {
                let properties = fragment_masses[index];
                let force = pending_force * (properties.mass / total_mass);
                let torque = pending_torque * (properties.angular_inertia / total_inertia);
                assigned_force += force;
                assigned_torque += torque;
                (force, torque)
            };
            let handle = *self
                .handles
                .get(&fragments[index])
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "fracture fragment body"))?;
            let body =
                self.backend.bodies.get_mut(handle).ok_or_else(|| {
                    EngineError::new(ErrorCode::StaleId, "fracture fragment handle")
                })?;
            body.add_force(force, true);
            body.add_torque(torque, true);
        }
        let mut removed_joints = Vec::new();
        removed_joints
            .try_reserve_exact(joints_to_migrate.len())
            .map_err(allocation)?;
        for (
            joint_id,
            old_handle,
            old_body1,
            old_body2,
            source_is_body1,
            component,
            data,
            impulses,
        ) in joints_to_migrate
        {
            let Some(component) = component else {
                self.backend
                    .impulse_joints
                    .remove(old_handle, true)
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier joint handle"))?;
                self.joint_handles.remove(&joint_id);
                removed_joints.push(joint_id);
                continue;
            };
            let fragment_id = *fragments
                .get(component)
                .ok_or_else(|| invalid("joint anchor component index"))?;
            let fragment_handle = *self
                .handles
                .get(&fragment_id)
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "joint fragment body"))?;
            self.backend
                .impulse_joints
                .remove(old_handle, true)
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier joint handle"))?;
            let (body1, body2) = if source_is_body1 {
                (fragment_handle, old_body2)
            } else {
                (old_body1, fragment_handle)
            };
            let new_handle = self.backend.impulse_joints.insert(body1, body2, data, true);
            let joint = self
                .backend
                .impulse_joints
                .get_mut(new_handle, true)
                .ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "migrated joint missing")
                })?;
            joint.impulses = impulses;
            self.joint_handles.insert(joint_id, new_handle);
        }
        validate_backend_state(&self.backend)?;
        Ok(BodyFractureReport {
            fragments,
            removed_joints,
            attachment_provider: attachment_provider.map(|policy| BodyAttachmentProviderStamp {
                key: policy.key(),
                version: policy.version(),
            }),
        })
    }

    /// 剛体と付属colliderを除去します。古いIDを再利用しません。
    pub fn remove(&mut self, id: BodyId) -> EngineResult<()> {
        if self
            .local_grids
            .get(&id)
            .is_some_and(|grid| !grid.exclusion_reservoir.is_empty())
        {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "reservoir物質をWorldへ戻してからbodyを除去してください",
            ));
        }
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        self.record_body_removal_contacts(id)?;
        self.pending_teleport_exits.retain(|pair| {
            pair.first != BodyContactTarget::Body(id) && pair.second != BodyContactTarget::Body(id)
        });
        self.joint_handles.retain(|_, joint_handle| {
            self.backend
                .impulse_joints
                .get(*joint_handle)
                .is_some_and(|joint| joint.body1() != handle && joint.body2() != handle)
        });
        self.backend.bodies.remove(
            handle,
            &mut self.backend.islands,
            &mut self.backend.colliders,
            &mut self.backend.impulse_joints,
            &mut self.backend.multibody_joints,
            &mut self.backend.soft_bodies,
            true,
        );
        self.handles.remove(&id);
        Arc::make_mut(&mut self.local_grids).remove(&id);
        self.local_grid_content_dirty.remove(&id);
        Ok(())
    }

    /// 削除前の接触相手を記録します。World側では候補physics上で呼ばれるため、
    /// 容量超過時に候補を破棄すれば、Worldと公開中physicsの両方が変更されません。
    fn record_body_removal_contacts(&mut self, removed: BodyId) -> EngineResult<()> {
        collect_contact_observations(&self.backend, &mut self.contact_before)?;
        capture_contact_velocities(&self.backend, &mut self.contact_velocity_before)?;
        self.contact_event_scratch.clear();
        let contact_count = self
            .contact_before
            .iter()
            .filter(|contact| {
                contact.key.first == BodyContactTarget::Body(removed)
                    || contact.key.second == BodyContactTarget::Body(removed)
            })
            .count();
        if contact_count > self.settings.contact_event_capacity as usize {
            return Err(EngineError::capacity(
                "剛体接触event数",
                contact_count as u64,
                self.settings.contact_event_capacity as u64,
            ));
        }
        for contact in self.contact_before.iter().copied().filter(|contact| {
            contact.key.first == BodyContactTarget::Body(removed)
                || contact.key.second == BodyContactTarget::Body(removed)
        }) {
            append_contact_event(
                &mut self.contact_event_scratch,
                self.settings.contact_event_capacity as usize,
                &self.contact_velocity_before,
                &self.backend,
                contact,
                ContactEventState {
                    phase: BodyContactPhase::Exit,
                    substep: 0,
                    exit_reason: Some(BodyContactExitReason::BodyRemoved),
                },
            )?;
            if contact.key.first == BodyContactTarget::Body(removed) {
                self.contact_event_scratch
                    .last_mut()
                    .expect("追加した接触終了event")
                    .first_post_velocity = None;
            } else {
                self.contact_event_scratch
                    .last_mut()
                    .expect("追加した接触終了event")
                    .second_post_velocity = None;
            }
        }
        self.publish_contact_events();
        self.terrain_contact_events_pending = false;
        Ok(())
    }

    /// teleport前にactiveだった接触を編集境界eventへ移します。
    ///
    /// World側では独立候補上で呼ぶため、event上限超過や不正なRapier観測はteleport全体を
    /// rollbackします。terrain同期が同じ操作で既に終了eventを出したpairは重ねて報告しません。
    fn record_body_teleport_contacts(
        &mut self,
        before: &Self,
        teleported: BodyId,
    ) -> EngineResult<()> {
        self.seed_terrain_exit_events_for_step()?;
        collect_contact_observations(&before.backend, &mut self.contact_before)?;
        capture_contact_velocities(&before.backend, &mut self.contact_velocity_before)?;
        let teleported_target = BodyContactTarget::Body(teleported);
        let existing_pending_count = self.pending_teleport_exits.len();
        for contact in self.contact_before.iter().copied().filter(|contact| {
            contact.key.first == teleported_target || contact.key.second == teleported_target
        }) {
            if self.contact_event_scratch.iter().any(|event| {
                event.phase == BodyContactPhase::Exit
                    && event.first == contact.key.first
                    && event.second == contact.key.second
            }) {
                continue;
            }
            append_contact_event(
                &mut self.contact_event_scratch,
                self.settings.contact_event_capacity as usize,
                &self.contact_velocity_before,
                &self.backend,
                contact,
                ContactEventState {
                    phase: BodyContactPhase::Exit,
                    substep: 0,
                    exit_reason: Some(BodyContactExitReason::Teleported),
                },
            )?;
            if self.pending_teleport_exits[..existing_pending_count]
                .binary_search(&contact.key)
                .is_err()
            {
                if self.pending_teleport_exits.len()
                    >= self.settings.contact_event_capacity as usize
                {
                    return Err(EngineError::capacity(
                        "保留するteleport接触pair数",
                        self.pending_teleport_exits.len() as u64 + 1,
                        u64::from(self.settings.contact_event_capacity),
                    ));
                }
                self.pending_teleport_exits
                    .try_reserve(1)
                    .map_err(allocation)?;
                self.pending_teleport_exits.push(contact.key);
            }
        }
        self.pending_teleport_exits.sort_unstable();
        self.pending_teleport_exits.dedup();
        self.contact_event_scratch
            .sort_unstable_by_key(|event| ContactPairKey {
                first: event.first,
                second: event.second,
            });
        if !self.contact_event_scratch.is_empty() {
            self.publish_contact_events();
        }
        self.terrain_contact_events_pending = false;
        Ok(())
    }

    /// 二つのbodyを固定または回転可能なjointで接続します。
    pub fn insert_joint(&mut self, definition: JointDef) -> EngineResult<JointId> {
        if definition.body1 == definition.body2
            || definition
                .local_anchor1
                .iter()
                .chain(definition.local_anchor2.iter())
                .any(|value| !value.is_finite())
        {
            return Err(invalid("joint anchors or body pair"));
        }
        if let JointKind::Revolute {
            limits: Some([min, max]),
        } = definition.kind
            && (!min.is_finite() || !max.is_finite() || min > max)
        {
            return Err(invalid("revolute joint limits"));
        }
        if self.joint_count() >= self.settings.joint_capacity {
            return Err(EngineError::capacity(
                "joint数",
                u64::from(self.joint_count()) + 1,
                u64::from(self.settings.joint_capacity),
            ));
        }
        let body1 = *self
            .handles
            .get(&definition.body1)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "joint body1"))?;
        let body2 = *self
            .handles
            .get(&definition.body2)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "joint body2"))?;
        let index = self.next_joint_index;
        let next = index.checked_add(1).ok_or_else(|| {
            EngineError::capacity("joint ID", u64::from(u32::MAX) + 1, u64::from(u32::MAX))
        })?;
        let id = JointId(index);
        let anchor1 = Vector::new(definition.local_anchor1[0], definition.local_anchor1[1]);
        let anchor2 = Vector::new(definition.local_anchor2[0], definition.local_anchor2[1]);
        let mut data: rapier2d_f64::dynamics::GenericJoint = match definition.kind {
            JointKind::Fixed => FixedJointBuilder::new()
                .local_anchor1(anchor1)
                .local_anchor2(anchor2)
                .build()
                .into(),
            JointKind::Revolute { limits } => {
                let builder = RevoluteJointBuilder::new()
                    .local_anchor1(anchor1)
                    .local_anchor2(anchor2);
                match limits {
                    Some(limits) => builder.limits(limits).build().into(),
                    None => builder.build().into(),
                }
            }
        };
        data.user_data = pack_joint_id(id);
        let handle = self.backend.impulse_joints.insert(body1, body2, data, true);
        if self.joint_handles.insert(id, handle).is_some() {
            return Err(EngineError::new(ErrorCode::Conflict, "joint ID duplicate"));
        }
        self.next_joint_index = next;
        Ok(id)
    }

    /// jointとそのsolver蓄積を除去します。IDを再利用しません。
    pub fn remove_joint(&mut self, id: JointId) -> EngineResult<()> {
        let handle = self
            .joint_handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "joint ID"))?;
        self.backend
            .impulse_joints
            .remove(handle, true)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier joint handle"))?;
        self.joint_handles.remove(&id);
        Ok(())
    }

    /// bodyの現在状態を読みます。
    pub fn state(&self, id: BodyId) -> EngineResult<BodyState> {
        let handle = self
            .handles
            .get(&id)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get(*handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        Ok(BodyState {
            pose: BodyPose {
                x: body.translation().x,
                y: body.translation().y,
                angle: body.rotation().angle(),
            },
            velocity: BodyVelocity {
                x: body.linvel().x,
                y: body.linvel().y,
                angular: body.angvel(),
            },
            sleeping: body.is_sleeping(),
        })
    }

    fn motion_type(&self, id: BodyId) -> EngineResult<BodyMotionType> {
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        Ok(match body.body_type() {
            RigidBodyType::Dynamic => BodyMotionType::Dynamic,
            RigidBodyType::Fixed => BodyMotionType::Fixed,
            RigidBodyType::KinematicPositionBased => BodyMotionType::KinematicPosition,
            RigidBodyType::KinematicVelocityBased => BodyMotionType::KinematicVelocity,
            RigidBodyType::SoftFrame => {
                return Err(EngineError::new(
                    ErrorCode::UnsupportedCapability,
                    "soft-frame bodyの連成impulseは未対応",
                ));
            }
        })
    }

    /// teleport候補のposeを変え、以前のposeを返します。World側で経路と到着点を検証してからcommitします。
    pub(crate) fn set_teleport_pose(
        &mut self,
        id: BodyId,
        pose: BodyPose,
    ) -> EngineResult<BodyPose> {
        self.set_pose(id, pose)
    }

    /// 作者solverが候補bodyのposeを更新します。World経由のstepでは後続のsweep検査を通します。
    pub fn set_pose(&mut self, id: BodyId, pose: BodyPose) -> EngineResult<BodyPose> {
        validate_pose(pose)?;
        let previous = self.state(id)?.pose;
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get_mut(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        body.set_position(Pose::new(Vector::new(pose.x, pose.y), pose.angle), true);
        self.backend
            .bodies
            .propagate_modified_body_positions_to_colliders(&mut self.backend.colliders);
        Ok(previous)
    }

    /// World-owned candidateを作者solverで更新し、resident境界と数値状態を検査します。
    pub(crate) fn step_with_provider(
        &mut self,
        before: &Self,
        world: &World,
        world_units_per_cell: f64,
        provider: &dyn BodyPhysicsStepProvider,
    ) -> EngineResult<BodyStepReport> {
        if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
            return Err(invalid("作者剛体solverのWorld cell size"));
        }
        let stamp = BodyPhysicsProviderStamp {
            key: provider.key(),
            version: provider.version(),
        };
        if stamp.version == 0 {
            return Err(invalid("BodyPhysics provider version"));
        }
        if self.step_provider.is_some_and(|active| active != stamp) {
            return Err(conflict(
                "保存済みfixed-step providerとkey/versionが一致しません",
            ));
        }
        if self.coupling_provider.is_some() {
            return Err(conflict(
                "選択済みCouplingProviderには連成対応fixed-step経路が必要です",
            ));
        }
        let next_step = self
            .step
            .checked_add(1)
            .ok_or_else(|| EngineError::capacity("剛体step番号", u64::MAX, u64::MAX - 1))?;
        let new_body_starts = capture_new_body_sweep_starts(&self.backend, &before.backend)?;
        let context = BodyPhysicsStepContext {
            fixed_dt_seconds: 1.0 / f64::from(self.settings.step_hz),
            step_index: next_step,
            max_substeps: self.settings.max_substeps,
            world_units_per_cell,
            provider_state_byte_limit: self.settings.snapshot_byte_limit as usize,
        };
        self.contact_event_scratch.clear();
        self.step_provider = Some(stamp);
        let substeps = provider.step(self, world, context)?;
        if substeps == 0 || substeps > context.max_substeps {
            return Err(EngineError::capacity(
                "作者BodyPhysics substep数",
                u64::from(substeps),
                u64::from(context.max_substeps),
            ));
        }
        validate_backend_state(&self.backend)?;
        validate_resident_body_sweep(
            &before.backend,
            &self.backend,
            world,
            world_units_per_cell,
            &new_body_starts,
        )?;
        validate_provider_contact_events(&self.contact_event_scratch, substeps, self, world)?;
        let active_bodies = u32::try_from(self.backend.islands.active_bodies().count())
            .map_err(|_| invalid("active body数"))?;
        self.publish_contact_events();
        self.terrain_contact_events_pending = false;
        self.pending_teleport_exits.clear();
        self.step = next_step;
        Ok(BodyStepReport {
            step: next_step,
            active_bodies,
            substeps,
            provider: Some(stamp),
            coupling_provider: self.coupling_provider,
        })
    }

    /// bodyの現在の質量、局所重心、重心まわりの角慣性を読みます。
    pub fn mass_properties(&self, id: BodyId) -> EngineResult<BodyMassProperties> {
        let handle = self
            .handles
            .get(&id)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get(*handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        let properties = &body.mass_properties().local_mprops;
        let inertia = properties.principal_inertia();
        Ok(BodyMassProperties {
            mass: properties.mass(),
            center_of_mass: [properties.local_com.x, properties.local_com.y],
            angular_inertia: inertia,
        })
    }

    /// 動的bodyまたは速度指定kinematic bodyの速度を変更します。
    pub fn set_velocity(&mut self, id: BodyId, velocity: BodyVelocity) -> EngineResult<()> {
        if ![velocity.x, velocity.y, velocity.angular]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(invalid("剛体速度"));
        }
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get_mut(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        if !matches!(
            body.body_type(),
            RigidBodyType::Dynamic | RigidBodyType::KinematicVelocityBased
        ) {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "body種別は速度変更を受け付けない",
            ));
        }
        body.set_linvel(Vector::new(velocity.x, velocity.y), true);
        body.set_angvel(velocity.angular, true);
        Ok(())
    }

    /// position-based kinematic bodyの次のposeを設定します。
    pub fn set_kinematic_target(&mut self, id: BodyId, pose: BodyPose) -> EngineResult<()> {
        if ![pose.x, pose.y, pose.angle]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(invalid("kinematic目標pose"));
        }
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get_mut(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        if body.body_type() != RigidBodyType::KinematicPositionBased {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "position-based kinematic bodyが必要",
            ));
        }
        body.set_next_kinematic_position(rapier2d_f64::math::Pose::new(
            Vector::new(pose.x, pose.y),
            pose.angle,
        ));
        Ok(())
    }

    /// 力積を動的bodyの重心へ一度だけ適用します。
    pub fn apply_impulse(&mut self, id: BodyId, impulse: [f64; 2]) -> EngineResult<()> {
        if impulse.iter().any(|value| !value.is_finite()) {
            return Err(invalid("剛体impulse"));
        }
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get_mut(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        if body.body_type() != RigidBodyType::Dynamic {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "dynamic bodyが必要",
            ));
        }
        body.apply_impulse(Vector::new(impulse[0], impulse[1]), true);
        Ok(())
    }

    fn apply_impulse_at_world_point(
        &mut self,
        id: BodyId,
        impulse: [f64; 2],
        point: [f64; 2],
    ) -> EngineResult<()> {
        if impulse
            .iter()
            .chain(point.iter())
            .any(|value| !value.is_finite())
        {
            return Err(invalid("剛体連成力積または作用点"));
        }
        let handle = self
            .handles
            .get(&id)
            .copied()
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
        let body = self
            .backend
            .bodies
            .get_mut(handle)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
        if body.body_type() != RigidBodyType::Dynamic {
            // fixedは拘束地形、kinematicは作者制御が運動量の外部供給先です。
            // fluid側の速度更新は行い、solverの非dynamic stateへ力積は積みません。
            return Ok(());
        }
        body.apply_impulse_at_point(
            Vector::new(impulse[0], impulse[1]),
            Vector::new(point[0], point[1]),
            true,
        );
        Ok(())
    }

    /// Rapierの候補worldを作り、有限値を検査してから固定stepを確定します。
    pub fn step(&mut self) -> EngineResult<BodyStepReport> {
        self.step_checked(|_, _| Ok(()))
    }

    /// 手元の状態を候補stepし、resident境界の侵入を検査してから確定します。
    pub(crate) fn step_in_world(
        &mut self,
        world: &World,
        cell_size: f64,
    ) -> EngineResult<BodyStepReport> {
        if self.step_provider.is_some() {
            return Err(conflict(
                "保存済みBodyPhysics providerには同じWorld step providerが必要です",
            ));
        }
        if self.coupling_provider.is_some() {
            return Err(conflict(
                "選択済みCouplingProviderにはWorld連成stepが必要です",
            ));
        }
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(invalid("物理World cell size"));
        }
        let next_step = self
            .step
            .checked_add(1)
            .ok_or_else(|| EngineError::capacity("剛体step番号", u64::MAX, u64::MAX - 1))?;
        self.seed_terrain_exit_events_for_step()?;
        copy_backend_state(&self.backend, &mut self.candidate_backend);
        let mut contact_scratch = ContactStepScratch {
            before: &mut self.contact_before,
            after: &mut self.contact_after,
            velocities_before: &mut self.contact_velocity_before,
            events: &mut self.contact_event_scratch,
            event_limit: self.settings.contact_event_capacity as usize,
            teleported_exits: &self.pending_teleport_exits,
        };
        let substeps = step_backend_with_substeps(
            &mut self.candidate_backend,
            cell_size,
            self.settings.max_substeps,
            self.settings.max_substep_displacement_cells,
            &mut self.step_scratch,
            &mut contact_scratch,
        )?;
        validate_backend_state(&self.candidate_backend)?;
        validate_resident_body_sweep(
            &self.backend,
            &self.candidate_backend,
            world,
            cell_size,
            &[],
        )?;
        let active_bodies = u32::try_from(self.candidate_backend.islands.active_bodies().count())
            .map_err(|_| invalid("active body数"))?;
        std::mem::swap(&mut self.backend, &mut self.candidate_backend);
        self.publish_contact_events();
        self.terrain_contact_events_pending = false;
        self.pending_teleport_exits.clear();
        self.step = next_step;
        Ok(BodyStepReport {
            step: next_step,
            active_bodies,
            substeps,
            provider: None,
            coupling_provider: self.coupling_provider,
        })
    }

    /// 独立候補だけを直接stepし、Worldのresident境界へ侵入する候補を返しません。
    ///
    /// 呼出側は`self`を複製候補として所有し、失敗時に破棄する必要があります。その前提で
    /// backendをもう一度複製せず、Worldとの原子的commitに必要な一回の候補複製を使います。
    pub(crate) fn step_isolated_in_world(
        &mut self,
        before: &Self,
        world: &World,
        cell_size: f64,
    ) -> EngineResult<BodyStepReport> {
        if self.step_provider.is_some() {
            return Err(conflict(
                "保存済みBodyPhysics providerには同じWorld step providerが必要です",
            ));
        }
        if self.coupling_provider.is_some() {
            return Err(conflict(
                "選択済みCouplingProviderにはWorld連成stepが必要です",
            ));
        }
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(invalid("物理World cell size"));
        }
        let next_step = self
            .step
            .checked_add(1)
            .ok_or_else(|| EngineError::capacity("剛体step番号", u64::MAX, u64::MAX - 1))?;
        // 同じWorldEditで挿入されたbodyもstep前のposeから掃引検査できるよう、
        // 新規bodyだけの軽量な始点情報を採取します。Rapier全体は複製しません。
        let new_body_starts = capture_new_body_sweep_starts(&self.backend, &before.backend)?;
        self.seed_terrain_exit_events_for_step()?;
        let mut contact_scratch = ContactStepScratch {
            before: &mut self.contact_before,
            after: &mut self.contact_after,
            velocities_before: &mut self.contact_velocity_before,
            events: &mut self.contact_event_scratch,
            event_limit: self.settings.contact_event_capacity as usize,
            teleported_exits: &self.pending_teleport_exits,
        };
        let substeps = step_backend_with_substeps(
            &mut self.backend,
            cell_size,
            self.settings.max_substeps,
            self.settings.max_substep_displacement_cells,
            &mut self.step_scratch,
            &mut contact_scratch,
        )?;
        validate_backend_state(&self.backend)?;
        validate_resident_body_sweep(
            &before.backend,
            &self.backend,
            world,
            cell_size,
            &new_body_starts,
        )?;
        let active_bodies = u32::try_from(self.backend.islands.active_bodies().count())
            .map_err(|_| invalid("active body数"))?;
        self.publish_contact_events();
        self.terrain_contact_events_pending = false;
        self.pending_teleport_exits.clear();
        self.step = next_step;
        Ok(BodyStepReport {
            step: next_step,
            active_bodies,
            substeps,
            provider: None,
            coupling_provider: self.coupling_provider,
        })
    }

    /// World-owned candidateでteleport pose、resident領域、作者の衝突方針を検証します。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn teleport_in_world(
        &mut self,
        before: &Self,
        id: BodyId,
        destination: BodyPose,
        world: &World,
        cell_size: f64,
        options: BodyTeleportOptions,
        query_provider: Option<&dyn BodyTeleportQueryProvider>,
    ) -> EngineResult<BodyTeleportReport> {
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(invalid("teleport World cell size"));
        }
        let before_handle = *before
            .handles
            .get(&id)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "teleport元剛体ID"))?;
        let candidate_handle = *self
            .handles
            .get(&id)
            .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "teleport候補剛体ID"))?;
        let previous = self.set_teleport_pose(id, destination)?;
        if options.path == BodyTeleportPathPolicy::Check {
            validate_resident_body_sweep(&before.backend, &self.backend, world, cell_size, &[])?;
        }
        let provider_stamp = if let Some(provider) = query_provider {
            let stamp = BodyTeleportProviderStamp {
                key: provider.key(),
                version: provider.version(),
            };
            if stamp.version == 0 {
                return Err(invalid("teleport CollisionQueries provider version"));
            }
            match provider.query(world, self, id, previous, destination, options)? {
                BodyTeleportQueryOutcome::Clear => Some(stamp),
                BodyTeleportQueryOutcome::Blocked => {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "作者teleport CollisionQueries providerが衝突を検出しました",
                    ));
                }
                BodyTeleportQueryOutcome::Incomplete => {
                    return Err(EngineError::new(
                        ErrorCode::InvalidState,
                        "作者teleport CollisionQueries query dataが未準備です",
                    ));
                }
                BodyTeleportQueryOutcome::Unsupported => {
                    return Err(EngineError::new(
                        ErrorCode::InvalidState,
                        "作者teleport CollisionQueries providerがshapeを扱えません",
                    ));
                }
            }
        } else {
            match options.path {
                BodyTeleportPathPolicy::Check => validate_body_teleport_path(
                    &before.backend,
                    &self.backend,
                    before_handle,
                    candidate_handle,
                    previous,
                    destination,
                    options.overlap,
                )?,
                BodyTeleportPathPolicy::Ignore
                    if options.overlap == BodyTeleportOverlapPolicy::Reject =>
                {
                    validate_body_teleport_destination(&self.backend, candidate_handle)?;
                }
                BodyTeleportPathPolicy::Ignore => {}
            }
            None
        };
        // Set_positionでattached colliderのbroad/narrow phase更新が遅延しても、到着poseのregionを
        // 必ず検査する。経路を検査するpolicyではこの追加検査は終点のresident保証を補う。
        validate_resident_body_pose(&self.backend, world, cell_size, candidate_handle)?;
        validate_backend_state(&self.backend)?;
        if previous != destination {
            self.record_body_teleport_contacts(before, id)?;
        }
        Ok(BodyTeleportReport {
            body: id,
            previous,
            destination,
            path_checked: options.path == BodyTeleportPathPolicy::Check,
            overlap_checked: options.overlap == BodyTeleportOverlapPolicy::Reject,
            query_provider: provider_stamp,
        })
    }

    fn step_checked(
        &mut self,
        validate: impl FnOnce(&RapierWorld, &RapierWorld) -> EngineResult<()>,
    ) -> EngineResult<BodyStepReport> {
        if self.step_provider.is_some() {
            return Err(conflict(
                "保存済みBodyPhysics providerには同じWorld step providerが必要です",
            ));
        }
        if self.coupling_provider.is_some() {
            return Err(conflict(
                "選択済みCouplingProviderにはWorld連成stepが必要です",
            ));
        }
        let next_step = self
            .step
            .checked_add(1)
            .ok_or_else(|| EngineError::capacity("剛体step番号", u64::MAX, u64::MAX - 1))?;
        self.seed_terrain_exit_events_for_step()?;
        copy_backend_state(&self.backend, &mut self.candidate_backend);
        collect_contact_observations(&self.backend, &mut self.contact_before)?;
        suppress_teleported_contact_pairs(&mut self.contact_before, &self.pending_teleport_exits);
        capture_contact_velocities(&self.backend, &mut self.contact_velocity_before)?;
        self.candidate_backend.step();
        collect_contact_observations(&self.candidate_backend, &mut self.contact_after)?;
        collect_contact_transitions(
            &self.candidate_backend,
            &self.contact_velocity_before,
            &self.contact_before,
            &self.contact_after,
            &mut self.contact_event_scratch,
            self.settings.contact_event_capacity as usize,
            1,
        )?;
        validate_backend_state(&self.candidate_backend)?;
        validate(&self.backend, &self.candidate_backend)?;
        let active_bodies = u32::try_from(self.candidate_backend.islands.active_bodies().count())
            .map_err(|_| invalid("active body数"))?;
        std::mem::swap(&mut self.backend, &mut self.candidate_backend);
        self.publish_contact_events();
        self.terrain_contact_events_pending = false;
        self.pending_teleport_exits.clear();
        self.step = next_step;
        Ok(BodyStepReport {
            step: next_step,
            active_bodies,
            substeps: 1,
            provider: None,
            coupling_provider: self.coupling_provider,
        })
    }

    /// backendの内部接触状態を含む、版付きsnapshotを生成します。
    pub fn snapshot(&self) -> EngineResult<Vec<u8>> {
        let header_size = SNAPSHOT_MAGIC.len() + 8;
        let byte_limit = self.settings.snapshot_byte_limit as usize;
        let payload_limit = byte_limit.checked_sub(header_size).ok_or_else(|| {
            EngineError::capacity("剛体snapshot byte数", header_size as u64, byte_limit as u64)
        })?;
        let revision_bytes = self
            .terrain_revisions
            .len()
            .checked_mul(std::mem::size_of::<TerrainChunkRevision>())
            .ok_or_else(|| {
                EngineError::capacity("terrain snapshot metadata", u64::MAX, payload_limit as u64)
            })?;
        if revision_bytes > payload_limit {
            return Err(EngineError::capacity(
                "terrain snapshot metadata",
                revision_bytes as u64,
                payload_limit as u64,
            ));
        }
        let backend_hex = self.backend_snapshot_hex()?;
        let mut payload = LimitedJsonBuffer::new(payload_limit);
        if let Err(encode_error) =
            serde_json::to_writer(&mut payload, &self.snapshot_record(&backend_hex))
        {
            return Err(match payload.failure {
                Some(LimitedJsonFailure::Capacity { required }) => EngineError::capacity(
                    "剛体snapshot byte数",
                    (header_size as u64).saturating_add(required),
                    byte_limit as u64,
                ),
                Some(LimitedJsonFailure::Allocation) => {
                    EngineError::new(ErrorCode::AllocationFailed, "剛体snapshot payload")
                }
                None => {
                    let mut error =
                        EngineError::new(ErrorCode::InvalidValue, "剛体snapshot encode");
                    error.detail = Some(Arc::from(encode_error.to_string()));
                    error
                }
            });
        }
        let payload = payload.bytes;
        let total = SNAPSHOT_MAGIC
            .len()
            .checked_add(8)
            .and_then(|size| size.checked_add(payload.len()))
            .ok_or_else(|| invalid("剛体snapshot byte数"))?;
        if total > byte_limit {
            return Err(EngineError::capacity(
                "剛体snapshot byte数",
                total as u64,
                byte_limit as u64,
            ));
        }
        let payload_len = u32::try_from(payload.len()).map_err(|_| invalid("剛体snapshot長"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(total)
            .map_err(|_| EngineError::new(ErrorCode::AllocationFailed, "剛体snapshot buffer"))?;
        bytes.extend_from_slice(SNAPSHOT_MAGIC);
        bytes.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&payload_len.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// 正確なsnapshot byte数を出力bufferなしで数えます。
    pub(crate) fn snapshot_limit(&self) -> EngineResult<usize> {
        let header_size = SNAPSHOT_MAGIC.len() + 8;
        let byte_limit = self.settings.snapshot_byte_limit as usize;
        let payload_limit = byte_limit.checked_sub(header_size).ok_or_else(|| {
            EngineError::capacity("剛体snapshot byte数", header_size as u64, byte_limit as u64)
        })?;
        let mut counter = CountingJsonWriter {
            written: 0,
            limit: payload_limit,
            required: None,
        };
        let backend_hex = self.backend_snapshot_hex()?;
        if serde_json::to_writer(&mut counter, &self.snapshot_record(&backend_hex)).is_err() {
            return Err(match counter.required {
                Some(required) => EngineError::capacity(
                    "剛体snapshot byte数",
                    (header_size as u64).saturating_add(required),
                    byte_limit as u64,
                ),
                None => EngineError::new(ErrorCode::InvalidValue, "剛体snapshot計数"),
            });
        }
        header_size
            .checked_add(counter.written)
            .ok_or_else(|| invalid("剛体snapshot byte数"))
    }

    fn backend_snapshot_hex(&self) -> EngineResult<String> {
        let byte_limit = self.settings.snapshot_byte_limit as usize;
        let required = bincode::serialized_size(&self.backend).map_err(|error| {
            let mut diagnostic =
                EngineError::new(ErrorCode::InvalidValue, "剛体backend snapshot計数");
            diagnostic.detail = Some(Arc::from(error.to_string()));
            diagnostic
        })?;
        let encoded_required = required.checked_mul(2).ok_or_else(|| {
            EngineError::capacity("剛体backend snapshot byte数", u64::MAX, byte_limit as u64)
        })?;
        if encoded_required > byte_limit as u64 {
            return Err(EngineError::capacity(
                "剛体backend snapshot byte数",
                encoded_required,
                byte_limit as u64,
            ));
        }
        let encoded_required = usize::try_from(encoded_required).map_err(|_| {
            EngineError::capacity(
                "剛体backend snapshot byte数",
                encoded_required,
                usize::MAX as u64,
            )
        })?;
        let mut hex = String::new();
        hex.try_reserve_exact(encoded_required).map_err(|_| {
            EngineError::new(ErrorCode::AllocationFailed, "剛体backend snapshot buffer")
        })?;
        let mut writer = HexSnapshotWriter {
            output: hex,
            limit: encoded_required,
        };
        bincode::serialize_into(&mut writer, &self.backend).map_err(|error| {
            let mut diagnostic =
                EngineError::new(ErrorCode::InvalidValue, "剛体backend snapshot encode");
            diagnostic.detail = Some(Arc::from(error.to_string()));
            diagnostic
        })?;
        debug_assert_eq!(writer.output.len(), encoded_required);
        Ok(writer.output)
    }

    fn snapshot_record<'a>(&'a self, backend: &'a str) -> BodyPhysicsSnapshot<'a> {
        BodyPhysicsSnapshot {
            version: SNAPSHOT_VERSION,
            settings: self.settings,
            next_body_index: self.next_body_index,
            next_joint_index: self.next_joint_index,
            terrain_source: self.terrain_source,
            terrain_chunk_revisions: TerrainRevisionList(&self.terrain_revisions),
            local_grids: BodyLocalGridList(&self.local_grids),
            step: self.step,
            step_provider: self.step_provider,
            provider_state: self.provider_state.as_deref().map(Vec::as_slice),
            coupling_provider: self.coupling_provider,
            coupling_state: self.coupling_state.as_deref().map(Vec::as_slice),
            pending_teleport_exits: ContactPairSnapshotList(&self.pending_teleport_exits),
            backend_compatibility: BACKEND_COMPATIBILITY,
            backend,
        }
    }

    /// snapshotを独立候補へ復元し、body IDとbackend handleの対応を検証します。
    pub fn restore(bytes: &[u8]) -> EngineResult<Self> {
        Self::restore_with_limit(bytes, DEFAULT_RESTORE_BYTE_LIMIT)
    }

    /// caller指定のbyte上限をdeserialize前に検査してsnapshotを復元します。
    ///
    /// 標準`restore`は64MiBまでを受け付けます。より大きなsnapshotを使うprojectは
    /// 外部save policyから許可上限を渡し、この入口でdecode時のメモリ上限を明示します。
    pub fn restore_with_limit(bytes: &[u8], max_input_bytes: usize) -> EngineResult<Self> {
        if bytes.len() > max_input_bytes {
            return Err(EngineError::capacity(
                "剛体snapshot入力byte数",
                bytes.len() as u64,
                max_input_bytes as u64,
            ));
        }
        if bytes.len() < 16 {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "剛体snapshot header",
            ));
        }
        let version = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| invalid("snapshot version"))?,
        );
        let payload_len = u32::from_le_bytes(
            bytes[12..16]
                .try_into()
                .map_err(|_| invalid("snapshot length"))?,
        ) as usize;
        let legacy_v3 =
            &bytes[..8] == LEGACY_SNAPSHOT_V3_MAGIC && version == LEGACY_SNAPSHOT_V3_VERSION;
        let legacy_v4 =
            &bytes[..8] == LEGACY_SNAPSHOT_V4_MAGIC && version == LEGACY_SNAPSHOT_V4_VERSION;
        let legacy_v5 =
            &bytes[..8] == LEGACY_SNAPSHOT_V5_MAGIC && version == LEGACY_SNAPSHOT_V5_VERSION;
        let legacy_v6 =
            &bytes[..8] == LEGACY_SNAPSHOT_V6_MAGIC && version == LEGACY_SNAPSHOT_V6_VERSION;
        let legacy_v7 =
            &bytes[..8] == LEGACY_SNAPSHOT_V7_MAGIC && version == LEGACY_SNAPSHOT_V7_VERSION;
        let legacy_v8 =
            &bytes[..8] == LEGACY_SNAPSHOT_V8_MAGIC && version == LEGACY_SNAPSHOT_V8_VERSION;
        let legacy_v9 =
            &bytes[..8] == LEGACY_SNAPSHOT_V9_MAGIC && version == LEGACY_SNAPSHOT_V9_VERSION;
        let legacy_v10 =
            &bytes[..8] == LEGACY_SNAPSHOT_V10_MAGIC && version == LEGACY_SNAPSHOT_V10_VERSION;
        let legacy_v11 =
            &bytes[..8] == LEGACY_SNAPSHOT_V11_MAGIC && version == LEGACY_SNAPSHOT_V11_VERSION;
        let current = &bytes[..8] == SNAPSHOT_MAGIC && version == SNAPSHOT_VERSION;
        if (!legacy_v3
            && !legacy_v4
            && !legacy_v5
            && !legacy_v6
            && !legacy_v7
            && !legacy_v8
            && !legacy_v9
            && !legacy_v10
            && !legacy_v11
            && !current)
            || payload_len != bytes.len() - 16
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "剛体snapshot versionまたは長さ",
            ));
        }
        let mut snapshot: OwnedBodyPhysicsSnapshot = serde_json::from_slice(&bytes[16..])
            .map_err(|_| EngineError::new(ErrorCode::InvalidEncoding, "剛体snapshot payload"))?;
        let settings = snapshot.settings.validate()?;
        if snapshot.version != version || bytes.len() > settings.snapshot_byte_limit as usize {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "剛体snapshot設定",
            ));
        }
        let backend_value = std::mem::take(&mut snapshot.backend);
        let backend: RapierWorld = if current {
            let backend_hex: String = serde_json::from_value(backend_value).map_err(|_| {
                EngineError::new(ErrorCode::InvalidEncoding, "backend snapshot hex")
            })?;
            if !backend_hex.len().is_multiple_of(2)
                || backend_hex.len() / 2 > settings.snapshot_byte_limit as usize
            {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "backend snapshot hex length",
                ));
            }
            let mut backend_bytes = Vec::new();
            backend_bytes
                .try_reserve_exact(backend_hex.len() / 2)
                .map_err(|_| {
                    EngineError::new(
                        ErrorCode::AllocationFailed,
                        "backend snapshot decode buffer",
                    )
                })?;
            let nibble = |value: u8| match value {
                b'0'..=b'9' => Some(value - b'0'),
                b'a'..=b'f' => Some(value - b'a' + 10),
                b'A'..=b'F' => Some(value - b'A' + 10),
                _ => None,
            };
            let (pairs, remainder) = backend_hex.as_bytes().as_chunks::<2>();
            if !remainder.is_empty() {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "backend snapshot hex length",
                ));
            }
            for pair in pairs {
                let high = nibble(pair[0]).ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidEncoding, "backend snapshot hex digit")
                })?;
                let low = nibble(pair[1]).ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidEncoding, "backend snapshot hex digit")
                })?;
                backend_bytes.push((high << 4) | low);
            }
            bincode::deserialize(&backend_bytes).map_err(|error| {
                let mut diagnostic =
                    EngineError::new(ErrorCode::InvalidEncoding, "backend snapshot decode");
                diagnostic.detail = Some(Arc::from(error.to_string()));
                diagnostic
            })?
        } else {
            serde_json::from_value(backend_value).map_err(|_| {
                EngineError::new(ErrorCode::InvalidEncoding, "legacy backend snapshot")
            })?
        };
        let expected_backend_compatibility = if current {
            BACKEND_COMPATIBILITY
        } else {
            LEGACY_BACKEND_COMPATIBILITY
        };
        if snapshot.backend_compatibility != expected_backend_compatibility
            || backend.gravity.x != settings.gravity[0]
            || backend.gravity.y != settings.gravity[1]
            || backend.integration_parameters.dt != 1.0 / f64::from(settings.step_hz)
        {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "剛体snapshot backend構成または固定step",
            ));
        }
        if snapshot
            .step_provider
            .is_some_and(|provider| provider.version == 0)
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "BodyPhysics provider identity",
            ));
        }
        if snapshot.step_provider.is_none() && snapshot.provider_state.is_some() {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "fixed-step provider state lacks provider identity",
            ));
        }
        if snapshot
            .coupling_provider
            .is_some_and(|provider| provider.version == 0)
            || (snapshot.coupling_provider.is_none() && snapshot.coupling_state.is_some())
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "BodyPhysics CouplingProvider identity/state",
            ));
        }
        if snapshot
            .coupling_state
            .as_ref()
            .is_some_and(|state| state.len() > settings.snapshot_byte_limit as usize)
        {
            return Err(EngineError::capacity(
                "CouplingProvider state byte数",
                snapshot
                    .coupling_state
                    .as_ref()
                    .map_or(0, |state| state.len()) as u64,
                settings.snapshot_byte_limit as u64,
            ));
        }
        let mut handles = BTreeMap::new();
        let mut local_grids = BTreeMap::new();
        let mut terrain_handles = BTreeMap::new();
        for (handle, body) in backend.bodies.iter() {
            match (body.user_data >> 64) as u64 {
                BODY_ID_TAG => {
                    let id = unpack_body_id(body.user_data)?;
                    if id.index() >= snapshot.next_body_index
                        || handles.insert(id, handle).is_some()
                    {
                        return Err(EngineError::new(ErrorCode::InvalidEncoding, "剛体ID対応"));
                    }
                }
                TERRAIN_BODY_TAG => {
                    let coord = unpack_terrain_chunk(body.user_data)?;
                    if body.body_type() != RigidBodyType::Fixed
                        || terrain_handles.insert(coord, handle).is_some()
                    {
                        return Err(EngineError::new(
                            ErrorCode::InvalidEncoding,
                            "terrain body mapping",
                        ));
                    }
                }
                _ => {
                    return Err(EngineError::new(
                        ErrorCode::InvalidEncoding,
                        "body user data tag",
                    ));
                }
            }
        }
        let mut joint_handles = BTreeMap::new();
        for (handle, joint) in backend.impulse_joints.iter() {
            let id = unpack_joint_id(joint.data.user_data)?;
            if id.index() >= snapshot.next_joint_index || joint_handles.insert(id, handle).is_some()
            {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "joint ID mapping",
                ));
            }
        }
        if snapshot.next_joint_index
            < joint_handles
                .keys()
                .next_back()
                .map_or(0, |id| id.index().saturating_add(1))
            || joint_handles.len() > settings.joint_capacity as usize
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "next joint ID or joint capacity",
            ));
        }
        if snapshot.next_body_index
            < handles
                .keys()
                .next_back()
                .map_or(0, |id| id.index().saturating_add(1))
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "次の剛体IDが既存IDと重複",
            ));
        }
        for (index, generation, grid) in snapshot.local_grids {
            let id = BodyId::from_parts(index, generation)?;
            if !handles.contains_key(&id)
                || grid.validate().is_err()
                || grid.owned_cell_count() > settings.body_local_cell_capacity as usize
                || grid.geometry_cell_count() > settings.body_local_geometry_cell_capacity as usize
                || grid.sidecar_bytes()? > settings.body_local_sidecar_byte_capacity as usize
            {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "body local grid owner or state",
                ));
            }
            let stencil = grid.stencil()?;
            if stencil.occupied_count() == 0 || local_grids.insert(id, grid).is_some() {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "body local grid occupancy or duplicate",
                ));
            }
        }
        let (local_cells, local_geometry_cells, local_sidecar_bytes) =
            local_grid_totals(&local_grids)?;
        if local_cells > settings.body_local_cell_capacity as usize
            || local_geometry_cells > settings.body_local_geometry_cell_capacity as usize
            || local_sidecar_bytes > settings.body_local_sidecar_byte_capacity as usize
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "body local grid snapshot exceeds aggregate capacity",
            ));
        }
        let mut terrain_revisions = BTreeMap::new();
        for stamp in snapshot.terrain_chunk_revisions {
            let coord = ChunkCoord::new(stamp.x, stamp.y)?;
            if terrain_revisions.insert(coord, stamp.revision).is_some() {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "duplicate terrain chunk revision",
                ));
            }
        }
        let terrain_source = snapshot.terrain_source;
        if let Some(source) = terrain_source {
            if source.world_generation == 0
                || !source.cell_size.is_finite()
                || source.cell_size <= 0.0
                || terrain_handles
                    .keys()
                    .any(|coord| !terrain_revisions.contains_key(coord))
            {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "terrain snapshot source or revision map",
                ));
            }
        } else if !terrain_handles.is_empty() || !terrain_revisions.is_empty() {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "terrain snapshot lacks source",
            ));
        }
        if backend.bodies.len() > settings.body_capacity as usize {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "剛体snapshot容量",
            ));
        }
        if snapshot.pending_teleport_exits.len() > settings.contact_event_capacity as usize {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "剛体snapshotの保留teleport接触容量",
            ));
        }
        let mut pending_teleport_exits = Vec::new();
        pending_teleport_exits
            .try_reserve_exact(snapshot.pending_teleport_exits.len())
            .map_err(allocation)?;
        for record in snapshot.pending_teleport_exits {
            let pair = record.restore()?;
            if [pair.first, pair.second].into_iter().any(|target| {
                matches!(target, BodyContactTarget::Body(id) if !handles.contains_key(&id))
            }) {
                return Err(EngineError::new(
                    ErrorCode::InvalidEncoding,
                    "保留teleport接触pairの剛体ID",
                ));
            }
            pending_teleport_exits.push(pair);
        }
        if pending_teleport_exits
            .windows(2)
            .any(|pairs| pairs[0] >= pairs[1])
        {
            return Err(EngineError::new(
                ErrorCode::InvalidEncoding,
                "重複または未整列の保留teleport接触pair",
            ));
        }
        let exclusion_reservoir_body_count = local_grids
            .values()
            .filter(|grid| !grid.exclusion_reservoir.is_empty())
            .count();
        let mut contact_events = Vec::new();
        contact_events
            .try_reserve_exact((settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        let mut contact_event_scratch = Vec::new();
        contact_event_scratch
            .try_reserve_exact((settings.contact_event_capacity as usize).min(64))
            .map_err(allocation)?;
        Ok(Self {
            settings,
            candidate_backend: RapierWorld::new(),
            backend,
            step_scratch: Vec::new(),
            contact_events,
            contact_event_scratch,
            terrain_contact_events_pending: false,
            pending_teleport_exits,
            contact_before: Vec::new(),
            contact_after: Vec::new(),
            contact_velocity_before: Vec::new(),
            handles,
            local_grid_content_dirty: local_grids.keys().copied().collect(),
            local_grids: Arc::new(local_grids),
            exclusion_reservoir_body_count,
            joint_handles,
            terrain_handles,
            terrain_revisions,
            terrain_source,
            next_body_index: snapshot.next_body_index,
            next_joint_index: snapshot.next_joint_index,
            step: snapshot.step,
            step_provider: snapshot.step_provider,
            provider_state: snapshot.provider_state.map(Arc::new),
            coupling_provider: snapshot.coupling_provider,
            coupling_state: snapshot.coupling_state.map(Arc::new),
        })
    }

    /// 確定済みstep番号を返します。
    pub fn step_index(&self) -> u64 {
        self.step
    }

    /// 現在のfixed-step providerが保存したcodec stateを復元します。
    pub fn provider_state<T: StateCodec>(&self) -> EngineResult<Option<T>> {
        let Some(bytes) = self.provider_state.as_deref() else {
            return Ok(None);
        };
        let mut decoder = Decoder::new(bytes, self.settings.snapshot_byte_limit as usize)?;
        let state = T::decode(&mut decoder)?;
        decoder.finish()?;
        Ok(Some(state))
    }

    /// 候補fixed-step providerのcodec stateをsnapshot上限内で更新します。
    pub fn set_provider_state<T: StateCodec>(&mut self, state: &T) -> EngineResult<()> {
        if self.step_provider.is_none() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "fixed-step provider identity選択前はprovider stateを設定できません",
            ));
        }
        let mut encoder = Encoder::new(self.settings.snapshot_byte_limit as usize);
        state.encode(&mut encoder)?;
        self.provider_state = Some(Arc::new(encoder.finish()));
        Ok(())
    }

    /// 候補fixed-step providerの保存stateを明示的に初期化します。
    pub fn clear_provider_state(&mut self) -> EngineResult<()> {
        if self.step_provider.is_none() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "fixed-step provider identity選択前はprovider stateを変更できません",
            ));
        }
        self.provider_state = None;
        Ok(())
    }

    pub(crate) fn select_coupling_provider(
        &mut self,
        provider: BodyCouplingProviderStamp,
    ) -> EngineResult<()> {
        if provider.version == 0 {
            return Err(invalid("CouplingProvider version"));
        }
        if self
            .coupling_provider
            .is_some_and(|active| active != provider)
        {
            return Err(conflict(
                "保存済みCouplingProviderとkey/versionが一致しません",
            ));
        }
        self.coupling_provider = Some(provider);
        Ok(())
    }

    pub(crate) fn prepare_coupling_step(&mut self) -> Option<BodyCouplingProviderStamp> {
        self.coupling_provider.take()
    }

    /// 現在のCouplingProviderが保存したcodec stateを復元します。
    pub fn coupling_provider_state<T: StateCodec>(&self) -> EngineResult<Option<T>> {
        let Some(bytes) = self.coupling_state.as_deref() else {
            return Ok(None);
        };
        let mut decoder = Decoder::new(bytes, self.settings.snapshot_byte_limit as usize)?;
        let state = T::decode(&mut decoder)?;
        decoder.finish()?;
        Ok(Some(state))
    }

    /// 候補CouplingProviderのcodec stateをsnapshot上限内で更新します。
    pub fn set_coupling_provider_state<T: StateCodec>(&mut self, state: &T) -> EngineResult<()> {
        if self.coupling_provider.is_none() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "CouplingProvider identity選択前はprovider stateを設定できません",
            ));
        }
        let mut encoder = Encoder::new(self.settings.snapshot_byte_limit as usize);
        state.encode(&mut encoder)?;
        self.coupling_state = Some(Arc::new(encoder.finish()));
        Ok(())
    }

    /// 候補CouplingProviderの保存stateを明示的に初期化します。
    pub fn clear_coupling_provider_state(&mut self) -> EngineResult<()> {
        if self.coupling_provider.is_none() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "CouplingProvider identity選択前はprovider stateを変更できません",
            ));
        }
        self.coupling_state = None;
        Ok(())
    }

    /// transaction commit直前に、非正規のsubstep scratchを候補へ移して再利用します。
    pub(crate) fn inherit_step_scratch(&mut self, source: &mut Self) {
        self.step_scratch = std::mem::take(&mut source.step_scratch);
    }
}

fn pack_body_id(id: BodyId) -> u128 {
    (u128::from(BODY_ID_TAG) << 64) | (u128::from(id.generation()) << 32) | u128::from(id.index())
}

fn unpack_body_id(data: u128) -> EngineResult<BodyId> {
    if (data >> 64) as u64 != BODY_ID_TAG {
        return Err(EngineError::new(
            ErrorCode::InvalidEncoding,
            "Rapier body user data",
        ));
    }
    let generation = (data >> 32) as u32;
    let index = data as u32;
    if generation != 1 {
        return Err(EngineError::new(
            ErrorCode::InvalidEncoding,
            "剛体ID generation",
        ));
    }
    BodyId::from_parts(index, generation)
}

fn pack_joint_id(id: JointId) -> u128 {
    (u128::from(JOINT_ID_TAG) << 64) | u128::from(id.index())
}

fn unpack_joint_id(data: u128) -> EngineResult<JointId> {
    if (data >> 64) as u64 != JOINT_ID_TAG {
        return Err(EngineError::new(
            ErrorCode::InvalidEncoding,
            "Rapier joint user data",
        ));
    }
    let index = data as u32;
    if data as u64 != u64::from(index) {
        return Err(EngineError::new(
            ErrorCode::InvalidEncoding,
            "joint ID range",
        ));
    }
    Ok(JointId(index))
}

fn validate_resident_body_sweep(
    before: &RapierWorld,
    candidate: &RapierWorld,
    world: &World,
    cell_size: f64,
    new_body_starts: &[NewBodySweepStart],
) -> EngineResult<()> {
    for handle in candidate.bodies.iter().filter_map(|(handle, body)| {
        ((body.user_data >> 64) as u64 != TERRAIN_BODY_TAG).then_some(handle)
    }) {
        let end_body = candidate
            .bodies
            .get(handle)
            .ok_or_else(|| invalid("candidate body handle"))?;
        let new_start = new_body_starts
            .iter()
            .find(|start| start.user_data == end_body.user_data);
        let start_body = before
            .bodies
            .get(handle)
            .filter(|body| body.user_data == end_body.user_data);
        if new_start.is_none() && start_body.is_none() {
            return Err(invalid("physics step前のbody state"));
        }
        let start_position = new_start
            .map(|start| Vector::new(start.position[0], start.position[1]))
            .unwrap_or_else(|| start_body.expect("body start was checked").translation());
        let end_position = end_body.translation();
        for (collider_index, collider_handle) in end_body.colliders().iter().enumerate() {
            let end_collider = candidate
                .colliders
                .get(*collider_handle)
                .ok_or_else(|| invalid("candidate collider handle"))?;
            let start_corners = new_start
                .and_then(|start| start.collider_corners.get(collider_index).copied())
                .or_else(|| {
                    before
                        .colliders
                        .get(*collider_handle)
                        .map(|collider| aabb_corners(collider.compute_aabb()))
                })
                .unwrap_or_else(|| aabb_corners(end_collider.compute_aabb()));
            let end_aabb = end_collider.compute_aabb();
            let mut radius = 0.0_f64;
            for (corners, center) in [
                (start_corners, start_position),
                (aabb_corners(end_aabb), end_position),
            ] {
                for corner in corners {
                    radius = radius.max((corner[0] - center.x).hypot(corner[1] - center.y));
                }
            }
            validate_resident_rect(
                world,
                cell_size,
                start_position.x.min(end_position.x) - radius,
                start_position.y.min(end_position.y) - radius,
                start_position.x.max(end_position.x) + radius,
                start_position.y.max(end_position.y) + radius,
            )?;
        }
    }
    Ok(())
}

/// 到着poseだけをWorld resident境界で検証します。
fn validate_resident_body_pose(
    backend: &RapierWorld,
    world: &World,
    cell_size: f64,
    handle: RigidBodyHandle,
) -> EngineResult<()> {
    let body = backend
        .bodies
        .get(handle)
        .ok_or_else(|| invalid("teleport destination body handle"))?;
    for collider_handle in body.colliders() {
        let collider = backend
            .colliders
            .get(*collider_handle)
            .ok_or_else(|| invalid("teleport destination collider handle"))?;
        let pose = collider_world_pose(body, collider);
        let aabb = collider.shape().compute_aabb(&pose);
        validate_resident_rect(
            world,
            cell_size,
            aabb.mins.x,
            aabb.mins.y,
            aabb.maxs.x,
            aabb.maxs.y,
        )?;
    }
    Ok(())
}

/// 移動中の剛体shapeを、他のsolid colliderに対して並進・回転込みでshape castします。
fn validate_body_teleport_path(
    before: &RapierWorld,
    candidate: &RapierWorld,
    before_handle: RigidBodyHandle,
    candidate_handle: RigidBodyHandle,
    start_pose: BodyPose,
    end_pose: BodyPose,
    overlap_policy: BodyTeleportOverlapPolicy,
) -> EngineResult<()> {
    let start_body = before
        .bodies
        .get(before_handle)
        .ok_or_else(|| invalid("teleport start body handle"))?;
    let end_body = candidate
        .bodies
        .get(candidate_handle)
        .ok_or_else(|| invalid("teleport end body handle"))?;
    let max_radius = body_sweep_radius(before, start_body)?;
    let min_x = start_pose.x.min(end_pose.x) - max_radius;
    let min_y = start_pose.y.min(end_pose.y) - max_radius;
    let max_x = start_pose.x.max(end_pose.x) + max_radius;
    let max_y = start_pose.y.max(end_pose.y) + max_radius;
    let translation = Vector::new(end_pose.x - start_pose.x, end_pose.y - start_pose.y);
    let angular_delta = shortest_angle_delta(start_pose.angle, end_pose.angle);

    for moving_handle in start_body.colliders() {
        let moving = before
            .colliders
            .get(*moving_handle)
            .ok_or_else(|| invalid("teleport moving collider"))?;
        if moving.is_sensor() {
            continue;
        }
        let local_pose = moving
            .position_wrt_parent()
            .copied()
            .unwrap_or_else(Pose::identity);
        // Parryのlocal_centerはshape local座標の回転中心。body原点をcollider localへ変換します。
        let local_center = local_pose.inverse() * Vector::ZERO;
        let motion =
            NonlinearRigidMotion::new(*moving.position(), local_center, translation, angular_delta);
        for (_, obstacle) in candidate.colliders.iter() {
            if obstacle.is_sensor() || obstacle.parent() == Some(candidate_handle) {
                continue;
            }
            let obstacle_aabb = obstacle.compute_aabb();
            if obstacle_aabb.maxs.x < min_x
                || obstacle_aabb.mins.x > max_x
                || obstacle_aabb.maxs.y < min_y
                || obstacle_aabb.mins.y > max_y
            {
                continue;
            }
            let stationary =
                NonlinearRigidMotion::new(*obstacle.position(), Vector::ZERO, Vector::ZERO, 0.0);
            if let Some(hit) = cast_shapes_nonlinear(
                &motion,
                moving.shape(),
                &stationary,
                obstacle.shape(),
                0.0,
                1.0,
                false,
            )
            .map_err(|_| invalid("teleport shape cast unsupported"))?
            {
                let endpoint_only = overlap_policy == BodyTeleportOverlapPolicy::Allow
                    && hit.time_of_impact >= 1.0 - 1.0e-9;
                if !endpoint_only {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "teleport経路がsolid colliderと交差します",
                    ));
                }
            }
        }
    }
    // 後段のendpoint validationは、set_position後のattached collider transformがRapierの次回
    // broad/narrow phase更新を待たないよう、body poseとlocal collider poseから再構成します。
    if end_body.translation().x != end_pose.x
        || end_body.translation().y != end_pose.y
        || shortest_angle_delta(end_body.rotation().angle(), end_pose.angle).abs() > 1.0e-12
    {
        return Err(invalid("teleport candidate body pose mismatch"));
    }
    if overlap_policy == BodyTeleportOverlapPolicy::Reject {
        validate_body_teleport_destination(candidate, candidate_handle)?;
    }
    Ok(())
}

/// 到着位置で他のsolid colliderと正確に重ならないことを確認します。
fn validate_body_teleport_destination(
    backend: &RapierWorld,
    handle: RigidBodyHandle,
) -> EngineResult<()> {
    let body = backend
        .bodies
        .get(handle)
        .ok_or_else(|| invalid("teleport destination body"))?;
    for moving_handle in body.colliders() {
        let moving = backend
            .colliders
            .get(*moving_handle)
            .ok_or_else(|| invalid("teleport destination collider"))?;
        if moving.is_sensor() {
            continue;
        }
        let moving_pose = collider_world_pose(body, moving);
        let moving_aabb = moving.shape().compute_aabb(&moving_pose);
        for (_, obstacle) in backend.colliders.iter() {
            if obstacle.is_sensor() || obstacle.parent() == Some(handle) {
                continue;
            }
            let obstacle_aabb = obstacle.compute_aabb();
            if obstacle_aabb.maxs.x < moving_aabb.mins.x
                || obstacle_aabb.mins.x > moving_aabb.maxs.x
                || obstacle_aabb.maxs.y < moving_aabb.mins.y
                || obstacle_aabb.mins.y > moving_aabb.maxs.y
            {
                continue;
            }
            let result = intersection_test(
                &moving_pose,
                moving.shape(),
                obstacle.position(),
                obstacle.shape(),
            )
            .map_err(|_| invalid("teleport destination overlap query unsupported"))?;
            if result.intersecting {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "teleport到着点がsolid colliderと重なります",
                ));
            }
        }
    }
    Ok(())
}

fn collider_world_pose(
    body: &rapier2d_f64::dynamics::RigidBody,
    collider: &rapier2d_f64::geometry::Collider,
) -> Pose {
    let local = collider
        .position_wrt_parent()
        .copied()
        .unwrap_or_else(Pose::identity);
    *body.position() * local
}

#[derive(Clone, Copy)]
struct BodySubstepStart {
    handle: rapier2d_f64::dynamics::RigidBodyHandle,
    position: Vector,
    angle: f64,
    radius: f64,
}

/// 選択中profileの最大移動条件を満たすsubstep数を保守的に見積もります。
fn required_body_substeps(
    backend: &RapierWorld,
    max_distance: f64,
    max_substeps: u32,
) -> EngineResult<u32> {
    let dt = backend.integration_parameters.dt;
    let mut required = 1_u32;
    for (_, body) in backend.bodies.iter() {
        if (body.user_data >> 64) as u64 == TERRAIN_BODY_TAG
            || matches!(
                body.body_type(),
                RigidBodyType::Fixed | RigidBodyType::SoftFrame
            )
        {
            continue;
        }
        let (linear_distance, angular_distance) = match body.body_type() {
            RigidBodyType::KinematicPositionBased => {
                let target = body.next_position();
                let displacement = (target.translation - body.translation())
                    .x
                    .hypot((target.translation - body.translation()).y);
                let angle = shortest_angle_delta(body.rotation().angle(), target.rotation.angle());
                let angular_distance = if angle == 0.0 {
                    0.0
                } else {
                    angle.abs() * body_sweep_radius(backend, body)?
                };
                (displacement, angular_distance)
            }
            RigidBodyType::Dynamic | RigidBodyType::KinematicVelocityBased => {
                let linear_velocity = body.linvel();
                let linear_speed = linear_velocity.x.hypot(linear_velocity.y);
                let angular_speed = body.angvel().abs();
                let (linear_acceleration, angular_acceleration) = if body.body_type()
                    == RigidBodyType::Dynamic
                {
                    let mass = body.mass();
                    let inertia = body.mass_properties().local_mprops.principal_inertia();
                    if !mass.is_finite() || mass <= 0.0 || !inertia.is_finite() || inertia <= 0.0 {
                        return Err(invalid("dynamic bodyのsubstep質量特性"));
                    }
                    let gravity = backend.gravity * body.gravity_scale();
                    let acceleration = gravity + body.user_force() / mass;
                    let angular_acceleration = body.user_torque() / inertia;
                    (
                        acceleration.x.hypot(acceleration.y),
                        angular_acceleration.abs(),
                    )
                } else {
                    (0.0, 0.0)
                };
                let angular_delta = angular_speed * dt + 0.5 * angular_acceleration * dt * dt;
                let angular_distance = if angular_delta == 0.0 {
                    0.0
                } else {
                    angular_delta * body_sweep_radius(backend, body)?
                };
                (
                    linear_speed * dt + 0.5 * linear_acceleration * dt * dt,
                    angular_distance,
                )
            }
            RigidBodyType::Fixed | RigidBodyType::SoftFrame => unreachable!(),
        };
        let total_distance = linear_distance + angular_distance;
        if !total_distance.is_finite() || total_distance < 0.0 {
            return Err(invalid("body substep移動予測"));
        }
        let needed = (total_distance / max_distance).ceil().max(1.0);
        if needed > f64::from(max_substeps) {
            return Err(EngineError::capacity(
                "body標準substep数",
                if needed >= u64::MAX as f64 {
                    u64::MAX
                } else {
                    needed as u64
                },
                u64::from(max_substeps),
            ));
        }
        required = required.max(needed as u32);
    }
    Ok(required)
}

/// bodyの現在poseにおける全colliderを覆う原点からの距離上限です。
fn body_sweep_radius(
    backend: &RapierWorld,
    body: &rapier2d_f64::dynamics::RigidBody,
) -> EngineResult<f64> {
    let center = body.translation();
    let mut radius = 0.0_f64;
    for collider_handle in body.colliders() {
        let collider = backend
            .colliders
            .get(*collider_handle)
            .ok_or_else(|| invalid("body substep collider"))?;
        // AABB角は円形shapeでも角まで広がるため、角速度の予測距離を過大評価します。
        // shape固有のbounding sphereなら保守性を保ちつつ、姿勢依存のAABB膨張を避けられます。
        let sphere = collider
            .shape()
            .compute_bounding_sphere(collider.position());
        let offset = sphere.center - center;
        radius = radius.max(offset.x.hypot(offset.y) + sphere.radius);
    }
    if !radius.is_finite() {
        return Err(invalid("body substep radius"));
    }
    Ok(radius)
}

fn capture_body_step_starts(
    backend: &RapierWorld,
    starts: &mut Vec<BodySubstepStart>,
    refresh_radii: bool,
) -> EngineResult<()> {
    let mut index = 0;
    for (handle, body) in backend.bodies.iter() {
        if (body.user_data >> 64) as u64 == TERRAIN_BODY_TAG {
            continue;
        }
        if let Some(start) = starts.get_mut(index) {
            if refresh_radii || start.handle != handle {
                start.handle = handle;
                start.radius = body_sweep_radius(backend, body)?;
            }
            start.position = body.translation();
            start.angle = body.rotation().angle();
        } else {
            starts.push(BodySubstepStart {
                handle,
                position: body.translation(),
                angle: body.rotation().angle(),
                radius: body_sweep_radius(backend, body)?,
            });
        }
        index += 1;
    }
    starts.truncate(index);
    Ok(())
}

fn validate_substep_displacement(
    backend: &RapierWorld,
    starts: &[BodySubstepStart],
    max_distance: f64,
) -> EngineResult<()> {
    let tolerance = max_distance * 1e-10 + f64::EPSILON;
    for start in starts {
        let body = backend
            .bodies
            .get(start.handle)
            .ok_or_else(|| invalid("body disappeared during substep"))?;
        let movement = body.translation() - start.position;
        let translation = movement.x.hypot(movement.y);
        let rotation = shortest_angle_delta(start.angle, body.rotation().angle()).abs();
        let displacement = translation + rotation * start.radius;
        if !displacement.is_finite() || displacement > max_distance + tolerance {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "Rapier substep exceeded the selected body displacement budget",
            ));
        }
    }
    Ok(())
}

fn step_backend_with_substeps(
    backend: &mut RapierWorld,
    cell_size: f64,
    max_substeps: u32,
    max_displacement_cells: f64,
    starts: &mut Vec<BodySubstepStart>,
    contact_scratch: &mut ContactStepScratch<'_>,
) -> EngineResult<u32> {
    if !max_displacement_cells.is_finite() || max_displacement_cells <= 0.0 {
        return Err(invalid("剛体substep displacement profile"));
    }
    let max_distance = cell_size * max_displacement_cells;
    if !max_distance.is_finite() || max_distance <= 0.0 {
        return Err(invalid("剛体substep distance"));
    }
    let substeps = required_body_substeps(backend, max_distance, max_substeps)?;
    let original_dt = backend.integration_parameters.dt;
    let substep_dt = original_dt / f64::from(substeps);
    if !substep_dt.is_finite() || substep_dt <= 0.0 {
        return Err(invalid("剛体substep dt"));
    }
    if starts.capacity() < backend.bodies.len() {
        starts
            .try_reserve_exact(backend.bodies.len().saturating_sub(starts.len()))
            .map_err(allocation)?;
    }
    backend.integration_parameters.dt = substep_dt;
    for substep in 0..substeps {
        collect_contact_observations(backend, contact_scratch.before)?;
        if substep == 0 {
            suppress_teleported_contact_pairs(
                contact_scratch.before,
                contact_scratch.teleported_exits,
            );
        }
        capture_contact_velocities(backend, contact_scratch.velocities_before)?;
        capture_body_step_starts(backend, starts, substep == 0)?;
        backend.step();
        validate_substep_displacement(backend, starts, max_distance)?;
        collect_contact_observations(backend, contact_scratch.after)?;
        collect_contact_transitions(
            backend,
            contact_scratch.velocities_before,
            contact_scratch.before,
            contact_scratch.after,
            contact_scratch.events,
            contact_scratch.event_limit,
            substep + 1,
        )?;
        std::mem::swap(&mut contact_scratch.before, &mut contact_scratch.after);
    }
    backend.integration_parameters.dt = original_dt;
    Ok(substeps)
}

fn shortest_angle_delta(from: f64, to: f64) -> f64 {
    let two_pi = 2.0 * std::f64::consts::PI;
    (to - from + std::f64::consts::PI).rem_euclid(two_pi) - std::f64::consts::PI
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ContactPairKey {
    first: BodyContactTarget,
    second: BodyContactTarget,
}

#[derive(Serialize, Deserialize)]
enum ContactTargetSnapshot {
    Body { index: u32, generation: u32 },
    Terrain { x: i32, y: i32 },
    Custom { owner: u32, object: u64, part: u32 },
}

impl From<BodyContactTarget> for ContactTargetSnapshot {
    fn from(target: BodyContactTarget) -> Self {
        match target {
            BodyContactTarget::Body(id) => Self::Body {
                index: id.index(),
                generation: id.generation(),
            },
            BodyContactTarget::Terrain(coord) => Self::Terrain {
                x: coord.x,
                y: coord.y,
            },
            BodyContactTarget::Custom {
                owner,
                object,
                part,
            } => Self::Custom {
                owner,
                object,
                part,
            },
        }
    }
}

impl ContactTargetSnapshot {
    fn restore(self) -> EngineResult<BodyContactTarget> {
        match self {
            Self::Body { index, generation } => Ok(BodyContactTarget::Body(BodyId::from_parts(
                index, generation,
            )?)),
            Self::Terrain { x, y } => Ok(BodyContactTarget::Terrain(ChunkCoord::new(x, y)?)),
            Self::Custom {
                owner,
                object,
                part,
            } => Ok(BodyContactTarget::Custom {
                owner,
                object,
                part,
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ContactPairSnapshot {
    first: ContactTargetSnapshot,
    second: ContactTargetSnapshot,
}

impl From<ContactPairKey> for ContactPairSnapshot {
    fn from(pair: ContactPairKey) -> Self {
        Self {
            first: pair.first.into(),
            second: pair.second.into(),
        }
    }
}

impl ContactPairSnapshot {
    fn restore(self) -> EngineResult<ContactPairKey> {
        let key = ContactPairKey {
            first: self.first.restore()?,
            second: self.second.restore()?,
        };
        if key.first >= key.second
            || (matches!(key.first, BodyContactTarget::Terrain(_))
                && matches!(key.second, BodyContactTarget::Terrain(_)))
        {
            return Err(invalid("snapshotのteleport接触pair順"));
        }
        Ok(key)
    }
}

struct ContactPairSnapshotList<'a>(&'a [ContactPairKey]);

impl Serialize for ContactPairSnapshotList<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for pair in self.0 {
            sequence.serialize_element(&ContactPairSnapshot::from(*pair))?;
        }
        sequence.end()
    }
}

/// 接触eventの発生段階と終了理由をまとめます。
#[derive(Clone, Copy)]
struct ContactEventState {
    phase: BodyContactPhase,
    substep: u32,
    exit_reason: Option<BodyContactExitReason>,
}

#[derive(Clone, Copy)]
struct ContactObservation {
    key: ContactPairKey,
    point: [f64; 2],
    normal: [f64; 2],
    impulse: [f64; 2],
    penetration: f64,
    first_collider: rapier2d_f64::geometry::ColliderHandle,
    second_collider: rapier2d_f64::geometry::ColliderHandle,
}

#[derive(Clone, Copy)]
struct ContactVelocityStart {
    body: BodyId,
    velocity: BodyVelocity,
}

struct ContactStepScratch<'a> {
    before: &'a mut Vec<ContactObservation>,
    after: &'a mut Vec<ContactObservation>,
    velocities_before: &'a mut Vec<ContactVelocityStart>,
    events: &'a mut Vec<BodyContactEvent>,
    event_limit: usize,
    teleported_exits: &'a [ContactPairKey],
}

fn suppress_teleported_contact_pairs(
    observations: &mut Vec<ContactObservation>,
    teleported_exits: &[ContactPairKey],
) {
    if teleported_exits.is_empty() {
        return;
    }
    observations.retain(|observation| teleported_exits.binary_search(&observation.key).is_err());
}

fn capture_contact_velocities(
    backend: &RapierWorld,
    velocities: &mut Vec<ContactVelocityStart>,
) -> EngineResult<()> {
    velocities.clear();
    if velocities.capacity() < backend.bodies.len() {
        velocities
            .try_reserve_exact(backend.bodies.len().saturating_sub(velocities.len()))
            .map_err(allocation)?;
    }
    for (_, body) in backend.bodies.iter() {
        if (body.user_data >> 64) as u64 != BODY_ID_TAG {
            continue;
        }
        velocities.push(ContactVelocityStart {
            body: unpack_body_id(body.user_data)?,
            velocity: BodyVelocity {
                x: body.linvel().x,
                y: body.linvel().y,
                angular: body.angvel(),
            },
        });
    }
    velocities.sort_unstable_by_key(|entry| entry.body);
    Ok(())
}

fn start_velocity_for_target(
    velocities: &[ContactVelocityStart],
    target: BodyContactTarget,
) -> Option<BodyVelocity> {
    let BodyContactTarget::Body(body) = target else {
        return None;
    };
    velocities
        .binary_search_by_key(&body, |entry| entry.body)
        .ok()
        .map(|index| velocities[index].velocity)
}

fn contact_target(
    backend: &RapierWorld,
    collider_handle: rapier2d_f64::geometry::ColliderHandle,
) -> EngineResult<(BodyContactTarget, Option<BodyVelocity>)> {
    let collider = backend
        .colliders
        .get(collider_handle)
        .ok_or_else(|| invalid("接触event collider"))?;
    let parent = collider
        .parent()
        .ok_or_else(|| invalid("接触event collider parent"))?;
    let body = backend
        .bodies
        .get(parent)
        .ok_or_else(|| invalid("接触event rigid body"))?;
    let data = body.user_data;
    match (data >> 64) as u64 {
        BODY_ID_TAG => Ok((
            BodyContactTarget::Body(unpack_body_id(data)?),
            Some(BodyVelocity {
                x: body.linvel().x,
                y: body.linvel().y,
                angular: body.angvel(),
            }),
        )),
        TERRAIN_BODY_TAG => Ok((
            BodyContactTarget::Terrain(unpack_terrain_chunk(data)?),
            None,
        )),
        _ => Err(invalid("接触event collider owner")),
    }
}

fn collect_contact_observations(
    backend: &RapierWorld,
    observations: &mut Vec<ContactObservation>,
) -> EngineResult<()> {
    observations.clear();
    for pair in backend.narrow_phase.contact_pairs() {
        if !pair.has_any_active_contact() {
            continue;
        }
        let (Some(collider1), Some(collider2)) = (
            backend.colliders.get(pair.collider1),
            backend.colliders.get(pair.collider2),
        ) else {
            // terrain再構築ではcandidate collider setを置換しても、Rapierのcontact graphが
            // 次のpipeline更新まで古いpairを保持することがあります。削除済みhandleは
            // 有効なWorld targetへ対応づけられないため、この観測だけを捨てます。
            continue;
        };
        let (target1, _) = contact_target(backend, pair.collider1)?;
        let (target2, _) = contact_target(backend, pair.collider2)?;
        if target1 == target2 {
            continue;
        }
        if matches!(target1, BodyContactTarget::Terrain(_))
            && matches!(target2, BodyContactTarget::Terrain(_))
        {
            continue;
        }
        let mut deepest: Option<(f64, [f64; 2], [f64; 2])> = None;
        for manifold in pair.manifolds() {
            let world_shape1 = manifold.subshape_pos1().prepend_to(collider1.position());
            let world_shape2 = manifold.subshape_pos2().prepend_to(collider2.position());
            for contact in &manifold.points {
                let point1 = world_shape1 * contact.local_p1;
                let point2 = world_shape2 * contact.local_p2;
                let point = point1.midpoint(point2);
                let candidate = (
                    contact.dist,
                    [point.x, point.y],
                    [manifold.data.normal.x, manifold.data.normal.y],
                );
                if deepest.is_none_or(|current| {
                    candidate.0 < current.0
                        || (candidate.0 == current.0
                            && (candidate.1[0], candidate.1[1]) < (current.1[0], current.1[1]))
                }) {
                    deepest = Some(candidate);
                }
            }
        }
        let Some((penetration, point, normal)) = deepest else {
            continue;
        };
        if !point
            .iter()
            .chain(normal.iter())
            .chain([penetration].iter())
            .all(|value| value.is_finite())
        {
            return Err(invalid("接触event形状が有限値ではありません"));
        }
        let normal_length = normal[0].hypot(normal[1]);
        if !normal_length.is_finite() || normal_length <= f64::EPSILON {
            return Err(invalid("接触event法線長"));
        }
        let normal = [normal[0] / normal_length, normal[1] / normal_length];
        let impulse = pair.total_impulse();
        if !impulse.x.is_finite() || !impulse.y.is_finite() {
            return Err(invalid("接触event力積が有限値ではありません"));
        }
        let reverse = target2 < target1;
        let (first, second) = if reverse {
            (target2, target1)
        } else {
            (target1, target2)
        };
        if observations.len() == observations.capacity() {
            observations.try_reserve(1).map_err(allocation)?;
        }
        observations.push(ContactObservation {
            key: ContactPairKey { first, second },
            point,
            normal: if reverse {
                [-normal[0], -normal[1]]
            } else {
                normal
            },
            impulse: if reverse {
                [-impulse.x, -impulse.y]
            } else {
                [impulse.x, impulse.y]
            },
            penetration,
            first_collider: if reverse {
                pair.collider2
            } else {
                pair.collider1
            },
            second_collider: if reverse {
                pair.collider1
            } else {
                pair.collider2
            },
        });
    }
    observations.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| left.penetration.total_cmp(&right.penetration))
            .then_with(|| left.point[0].total_cmp(&right.point[0]))
            .then_with(|| left.point[1].total_cmp(&right.point[1]))
            .then_with(|| left.normal[0].total_cmp(&right.normal[0]))
            .then_with(|| left.normal[1].total_cmp(&right.normal[1]))
            .then_with(|| left.impulse[0].total_cmp(&right.impulse[0]))
            .then_with(|| left.impulse[1].total_cmp(&right.impulse[1]))
    });
    compact_contact_observations(observations)?;
    Ok(())
}

/// terrain colliderを置換・除去する前に、そのchunkと接触中のbodyへ終了理由を記録します。
fn collect_terrain_exit_events(
    backend: &RapierWorld,
    reasons: &BTreeMap<ChunkCoord, BodyContactExitReason>,
    observations: &mut Vec<ContactObservation>,
    velocities: &mut Vec<ContactVelocityStart>,
    events: &mut Vec<BodyContactEvent>,
    event_limit: usize,
) -> EngineResult<()> {
    events.clear();
    if reasons.is_empty() {
        return Ok(());
    }
    collect_contact_observations(backend, observations)?;
    capture_contact_velocities(backend, velocities)?;
    let exit_count = observations
        .iter()
        .filter(|observation| {
            let terrain = [observation.key.first, observation.key.second]
                .into_iter()
                .find_map(|target| match target {
                    BodyContactTarget::Terrain(coord) if reasons.contains_key(&coord) => {
                        Some(coord)
                    }
                    _ => None,
                });
            terrain.is_some()
        })
        .count();
    if exit_count > event_limit {
        return Err(EngineError::capacity(
            "剛体接触event数",
            exit_count as u64,
            event_limit as u64,
        ));
    }
    if events.capacity() < exit_count {
        events
            .try_reserve_exact(exit_count.saturating_sub(events.len()))
            .map_err(allocation)?;
    }
    for observation in observations.iter().copied() {
        let reason = [observation.key.first, observation.key.second]
            .into_iter()
            .find_map(|target| match target {
                BodyContactTarget::Terrain(coord) => reasons.get(&coord).copied(),
                _ => None,
            });
        let Some(reason) = reason else {
            continue;
        };
        append_contact_event(
            events,
            event_limit,
            velocities,
            backend,
            observation,
            ContactEventState {
                phase: BodyContactPhase::Exit,
                substep: 0,
                exit_reason: Some(reason),
            },
        )?;
    }
    Ok(())
}

fn compact_contact_observations(observations: &mut Vec<ContactObservation>) -> EngineResult<()> {
    let mut write = 0;
    for read in 0..observations.len() {
        let candidate = observations[read];
        if write > 0 && observations[write - 1].key == candidate.key {
            let current = &mut observations[write - 1];
            current.impulse[0] += candidate.impulse[0];
            current.impulse[1] += candidate.impulse[1];
            if !current.impulse.iter().all(|value| value.is_finite()) {
                return Err(invalid("合算した接触event力積が有限値ではありません"));
            }
            if candidate.penetration < current.penetration
                || (candidate.penetration == current.penetration
                    && (candidate.point[0], candidate.point[1])
                        < (current.point[0], current.point[1]))
            {
                current.point = candidate.point;
                current.normal = candidate.normal;
                current.penetration = candidate.penetration;
                current.first_collider = candidate.first_collider;
                current.second_collider = candidate.second_collider;
            }
        } else {
            observations[write] = candidate;
            write += 1;
        }
    }
    observations.truncate(write);
    Ok(())
}

fn velocity_for_target(
    backend: &RapierWorld,
    collider_handle: rapier2d_f64::geometry::ColliderHandle,
    target: BodyContactTarget,
) -> EngineResult<Option<BodyVelocity>> {
    let Ok((actual, velocity)) = contact_target(backend, collider_handle) else {
        return Ok(None);
    };
    if actual == target {
        Ok(velocity)
    } else {
        Err(invalid("候補step中に接触eventのtargetが変わりました"))
    }
}

fn append_contact_event(
    events: &mut Vec<BodyContactEvent>,
    limit: usize,
    pre_velocities: &[ContactVelocityStart],
    after: &RapierWorld,
    observation: ContactObservation,
    state: ContactEventState,
) -> EngineResult<()> {
    if events.len() >= limit {
        return Err(EngineError::capacity(
            "剛体接触event数",
            events.len() as u64 + 1,
            limit as u64,
        ));
    }
    if events.len() == events.capacity() {
        events.try_reserve(1).map_err(allocation)?;
    }
    events.push(BodyContactEvent {
        first: observation.key.first,
        second: observation.key.second,
        phase: state.phase,
        exit_reason: state.exit_reason,
        point: observation.point,
        normal: observation.normal,
        impulse: observation.impulse,
        first_pre_velocity: start_velocity_for_target(pre_velocities, observation.key.first),
        first_post_velocity: velocity_for_target(
            after,
            observation.first_collider,
            observation.key.first,
        )?,
        second_pre_velocity: start_velocity_for_target(pre_velocities, observation.key.second),
        second_post_velocity: velocity_for_target(
            after,
            observation.second_collider,
            observation.key.second,
        )?,
        substep: state.substep,
    });
    Ok(())
}

fn collect_contact_transitions(
    after: &RapierWorld,
    pre_velocities: &[ContactVelocityStart],
    before_contacts: &[ContactObservation],
    after_contacts: &[ContactObservation],
    events: &mut Vec<BodyContactEvent>,
    event_limit: usize,
    substep: u32,
) -> EngineResult<()> {
    let (mut left, mut right) = (0, 0);
    while left < before_contacts.len() || right < after_contacts.len() {
        match (before_contacts.get(left), after_contacts.get(right)) {
            (Some(previous), Some(current)) if previous.key == current.key => {
                append_contact_event(
                    events,
                    event_limit,
                    pre_velocities,
                    after,
                    *current,
                    ContactEventState {
                        phase: BodyContactPhase::Stay,
                        substep,
                        exit_reason: None,
                    },
                )?;
                left += 1;
                right += 1;
            }
            (Some(previous), Some(current)) if previous.key < current.key => {
                append_contact_event(
                    events,
                    event_limit,
                    pre_velocities,
                    after,
                    *previous,
                    ContactEventState {
                        phase: BodyContactPhase::Exit,
                        substep,
                        exit_reason: Some(BodyContactExitReason::Separated),
                    },
                )?;
                left += 1;
            }
            (Some(_), Some(current)) => {
                append_contact_event(
                    events,
                    event_limit,
                    pre_velocities,
                    after,
                    *current,
                    ContactEventState {
                        phase: BodyContactPhase::Enter,
                        substep,
                        exit_reason: None,
                    },
                )?;
                right += 1;
            }
            (Some(previous), None) => {
                append_contact_event(
                    events,
                    event_limit,
                    pre_velocities,
                    after,
                    *previous,
                    ContactEventState {
                        phase: BodyContactPhase::Exit,
                        substep,
                        exit_reason: Some(BodyContactExitReason::Separated),
                    },
                )?;
                left += 1;
            }
            (None, Some(current)) => {
                append_contact_event(
                    events,
                    event_limit,
                    pre_velocities,
                    after,
                    *current,
                    ContactEventState {
                        phase: BodyContactPhase::Enter,
                        substep,
                        exit_reason: None,
                    },
                )?;
                right += 1;
            }
            (None, None) => break,
        }
    }
    Ok(())
}

struct NewBodySweepStart {
    user_data: u128,
    position: [f64; 2],
    collider_corners: Vec<[[f64; 2]; 4]>,
}

fn capture_new_body_sweep_starts(
    candidate: &RapierWorld,
    before: &RapierWorld,
) -> EngineResult<Vec<NewBodySweepStart>> {
    let count = candidate
        .bodies
        .iter()
        .filter(|(handle, body)| {
            (body.user_data >> 64) as u64 != TERRAIN_BODY_TAG
                && before
                    .bodies
                    .get(*handle)
                    .is_none_or(|old| old.user_data != body.user_data)
        })
        .count();
    let mut starts = Vec::new();
    starts.try_reserve_exact(count).map_err(allocation)?;
    for (handle, body) in candidate.bodies.iter() {
        if (body.user_data >> 64) as u64 == TERRAIN_BODY_TAG
            || before
                .bodies
                .get(handle)
                .is_some_and(|old| old.user_data == body.user_data)
        {
            continue;
        }
        let mut collider_corners = Vec::new();
        collider_corners
            .try_reserve_exact(body.colliders().len())
            .map_err(allocation)?;
        for collider_handle in body.colliders() {
            let collider = candidate
                .colliders
                .get(*collider_handle)
                .ok_or_else(|| invalid("new body start collider"))?;
            collider_corners.push(aabb_corners(collider.compute_aabb()));
        }
        starts.push(NewBodySweepStart {
            user_data: body.user_data,
            position: [body.translation().x, body.translation().y],
            collider_corners,
        });
    }
    Ok(starts)
}

fn aabb_corners(aabb: rapier2d_f64::parry::bounding_volume::Aabb) -> [[f64; 2]; 4] {
    [
        [aabb.mins.x, aabb.mins.y],
        [aabb.mins.x, aabb.maxs.y],
        [aabb.maxs.x, aabb.mins.y],
        [aabb.maxs.x, aabb.maxs.y],
    ]
}

fn validate_contact_event_shape(event: &BodyContactEvent) -> EngineResult<()> {
    if event.first >= event.second
        || !event.point.iter().all(|value| value.is_finite())
        || !event.normal.iter().all(|value| value.is_finite())
        || !event.impulse.iter().all(|value| value.is_finite())
    {
        return Err(invalid("剛体contact event geometryまたはstable target順"));
    }
    let normal_length_squared =
        event.normal[0] * event.normal[0] + event.normal[1] * event.normal[1];
    if !normal_length_squared.is_finite() || (normal_length_squared - 1.0).abs() > 1.0e-6 {
        return Err(invalid("剛体contact event normal"));
    }
    if matches!(event.phase, BodyContactPhase::Exit) != event.exit_reason.is_some() {
        return Err(invalid("剛体contact event exit reason"));
    }
    for (target, pre, post) in [
        (
            event.first,
            event.first_pre_velocity,
            event.first_post_velocity,
        ),
        (
            event.second,
            event.second_pre_velocity,
            event.second_post_velocity,
        ),
    ] {
        let is_body = matches!(target, BodyContactTarget::Body(_));
        if pre.is_some() != is_body || post.is_some() != is_body {
            return Err(invalid("剛体contact event velocity ownership"));
        }
        if pre.into_iter().chain(post).any(|velocity| {
            ![velocity.x, velocity.y, velocity.angular]
                .iter()
                .all(|value| value.is_finite())
        }) {
            return Err(invalid("剛体contact event velocity"));
        }
    }
    Ok(())
}

fn validate_provider_contact_events(
    events: &[BodyContactEvent],
    substeps: u32,
    physics: &BodyPhysics,
    world: &World,
) -> EngineResult<()> {
    let mut previous = None;
    for event in events {
        validate_contact_event_shape(event)?;
        let key = (event.substep, event.first, event.second);
        if event.substep == 0 || event.substep > substeps || previous.is_some_and(|old| old >= key)
        {
            return Err(invalid("作者剛体contact event substep範囲または順序"));
        }
        previous = Some(key);
        for target in [event.first, event.second] {
            match target {
                BodyContactTarget::Body(body) => {
                    physics.state(body)?;
                }
                BodyContactTarget::Terrain(chunk) if !world.chunks.contains_key(&chunk) => {
                    return Err(EngineError::new(
                        ErrorCode::StaleId,
                        "作者contact eventのterrain chunkはresidentではありません",
                    ));
                }
                BodyContactTarget::Terrain(_) => {}
                BodyContactTarget::Custom { .. } => {}
            }
        }
    }
    Ok(())
}

fn validate_backend_state(backend: &RapierWorld) -> EngineResult<()> {
    if !backend.quarantine().is_empty() {
        return Err(EngineError::new(
            ErrorCode::InvalidValue,
            "物理stepでRapierが非有限状態を隔離",
        ));
    }
    for (_, body) in backend.bodies.iter() {
        let p = body.translation();
        let v = body.linvel();
        if ![p.x, p.y, body.rotation().angle(), v.x, v.y, body.angvel()]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "物理stepの非有限状態",
            ));
        }
    }
    Ok(())
}

fn validate_resident_rect(
    world: &World,
    cell_size: f64,
    min_world_x: f64,
    min_world_y: f64,
    max_world_x: f64,
    max_world_y: f64,
) -> EngineResult<()> {
    let min_x = (min_world_x / cell_size).floor();
    let min_y = (min_world_y / cell_size).floor();
    let max_x = (max_world_x / cell_size).ceil() - 1.0;
    let max_y = (max_world_y / cell_size).ceil() - 1.0;
    if ![min_x, min_y, max_x, max_y]
        .iter()
        .all(|value| value.is_finite())
        || min_x < f64::from(i32::MIN)
        || min_y < f64::from(i32::MIN)
        || max_x > f64::from(i32::MAX)
        || max_y > f64::from(i32::MAX)
    {
        return Err(EngineError::new(
            ErrorCode::InvalidState,
            "剛体candidateがcell座標の範囲外へ移動",
        ));
    }
    let (min_x, min_y, max_x, max_y) = (min_x as i64, min_y as i64, max_x as i64, max_y as i64);
    let bounds = world.bounds();
    let world_min = bounds.min();
    let world_max = bounds.max();
    if min_x < i64::from(world_min.x)
        || min_y < i64::from(world_min.y)
        || max_x >= world_max[0]
        || max_y >= world_max[1]
    {
        return Err(EngineError::new(
            ErrorCode::InvalidState,
            "剛体candidateがWorld境界へ侵入",
        ));
    }
    let min_chunk_x = min_x.div_euclid(64);
    let min_chunk_y = min_y.div_euclid(64);
    let max_chunk_x = max_x.div_euclid(64);
    let max_chunk_y = max_y.div_euclid(64);
    let chunk_width = u64::try_from(max_chunk_x - min_chunk_x + 1)
        .map_err(|_| invalid("剛体candidate chunk幅"))?;
    let chunk_height = u64::try_from(max_chunk_y - min_chunk_y + 1)
        .map_err(|_| invalid("剛体candidate chunk高"))?;
    let chunk_count = chunk_width
        .checked_mul(chunk_height)
        .ok_or_else(|| invalid("剛体candidate chunk範囲"))?;
    if chunk_count > world.chunks.len() as u64 {
        return Err(EngineError::new(
            ErrorCode::InvalidState,
            "剛体candidateがUnavailable regionへ侵入",
        ));
    }
    for y in min_chunk_y..=max_chunk_y {
        for x in min_chunk_x..=max_chunk_x {
            let coord = ChunkCoord::new(
                i32::try_from(x).map_err(|_| invalid("candidate chunk x"))?,
                i32::try_from(y).map_err(|_| invalid("candidate chunk y"))?,
            )?;
            if !world.chunks.contains_key(&coord) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "剛体candidateがUnavailable regionへ侵入",
                ));
            }
        }
    }
    Ok(())
}

fn pack_terrain_chunk(coord: ChunkCoord) -> u128 {
    let packed = (u64::from(coord.x as u32) << 32) | u64::from(coord.y as u32);
    (u128::from(TERRAIN_BODY_TAG) << 64) | u128::from(packed)
}

fn unpack_terrain_chunk(data: u128) -> EngineResult<ChunkCoord> {
    if (data >> 64) as u64 != TERRAIN_BODY_TAG {
        return Err(EngineError::new(
            ErrorCode::InvalidEncoding,
            "Rapier terrain body user data",
        ));
    }
    let packed = data as u64;
    let x = (packed >> 32) as u32 as i32;
    let y = packed as u32 as i32;
    ChunkCoord::new(x, y)
        .map_err(|_| EngineError::new(ErrorCode::InvalidEncoding, "terrain chunk coordinate"))
}

fn copy_backend_state(source: &RapierWorld, target: &mut RapierWorld) {
    target.gravity = source.gravity;
    target
        .integration_parameters
        .clone_from(&source.integration_parameters);
    target.islands.clone_from(&source.islands);
    target.broad_phase.clone_from(&source.broad_phase);
    target.narrow_phase.clone_from(&source.narrow_phase);
    target.bodies.clone_from(&source.bodies);
    target.colliders.clone_from(&source.colliders);
    target.impulse_joints.clone_from(&source.impulse_joints);
    target.multibody_joints.clone_from(&source.multibody_joints);
    target.soft_bodies.clone_from(&source.soft_bodies);
}

#[derive(Serialize)]
struct BodyPhysicsSnapshot<'a> {
    version: u32,
    settings: BodyPhysicsSettings,
    next_body_index: u32,
    next_joint_index: u32,
    terrain_source: Option<TerrainSource>,
    terrain_chunk_revisions: TerrainRevisionList<'a>,
    local_grids: BodyLocalGridList<'a>,
    step: u64,
    step_provider: Option<BodyPhysicsProviderStamp>,
    provider_state: Option<&'a [u8]>,
    coupling_provider: Option<BodyCouplingProviderStamp>,
    coupling_state: Option<&'a [u8]>,
    pending_teleport_exits: ContactPairSnapshotList<'a>,
    backend_compatibility: &'static str,
    backend: &'a str,
}

struct BodyLocalGridList<'a>(&'a BTreeMap<BodyId, BodyLocalGrid>);

impl Serialize for BodyLocalGridList<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for (id, grid) in self.0 {
            sequence.serialize_element(&(id.index(), id.generation(), grid))?;
        }
        sequence.end()
    }
}

struct TerrainRevisionList<'a>(&'a BTreeMap<ChunkCoord, u64>);

impl Serialize for TerrainRevisionList<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for (coord, revision) in self.0 {
            sequence.serialize_element(&TerrainChunkRevision {
                x: coord.x,
                y: coord.y,
                revision: *revision,
            })?;
        }
        sequence.end()
    }
}

#[derive(Deserialize)]
struct OwnedBodyPhysicsSnapshot {
    version: u32,
    settings: BodyPhysicsSettings,
    next_body_index: u32,
    next_joint_index: u32,
    terrain_source: Option<TerrainSource>,
    terrain_chunk_revisions: Vec<TerrainChunkRevision>,
    #[serde(default)]
    local_grids: Vec<(u32, u32, BodyLocalGrid)>,
    step: u64,
    #[serde(default)]
    step_provider: Option<BodyPhysicsProviderStamp>,
    #[serde(default)]
    provider_state: Option<Vec<u8>>,
    #[serde(default)]
    coupling_provider: Option<BodyCouplingProviderStamp>,
    #[serde(default)]
    coupling_state: Option<Vec<u8>>,
    #[serde(default)]
    pending_teleport_exits: Vec<ContactPairSnapshot>,
    backend_compatibility: String,
    backend: serde_json::Value,
}

enum LimitedJsonFailure {
    Capacity { required: u64 },
    Allocation,
}

/// snapshot JSONの上限をserialize中に守り、超過分を確保しません。
struct LimitedJsonBuffer {
    bytes: Vec<u8>,
    limit: usize,
    failure: Option<LimitedJsonFailure>,
}

/// binary snapshotをJSON上で線形memoryのhex文字列として書きます。
struct HexSnapshotWriter {
    output: String,
    limit: usize,
}

impl Write for HexSnapshotWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let additional = bytes
            .len()
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("backend snapshot hex length overflow"))?;
        if self.output.len().saturating_add(additional) > self.limit {
            return Err(io::Error::other("backend snapshot hex size mismatch"));
        }
        for byte in bytes {
            self.output.push(char::from(HEX[(byte >> 4) as usize]));
            self.output.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CountingJsonWriter {
    written: usize,
    limit: usize,
    required: Option<u64>,
}

impl Write for CountingJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let required = self.written.checked_add(bytes.len()).ok_or_else(|| {
            self.required = Some(u64::MAX);
            io::Error::other("snapshot byte length overflow")
        })?;
        if required > self.limit {
            self.required = Some(required as u64);
            return Err(io::Error::other("snapshot byte limit exceeded"));
        }
        self.written = required;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LimitedJsonBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            failure: None,
        }
    }
}

impl Write for LimitedJsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let required = self.bytes.len().checked_add(bytes.len()).ok_or_else(|| {
            self.failure = Some(LimitedJsonFailure::Capacity { required: u64::MAX });
            io::Error::other("snapshot byte length overflow")
        })?;
        if required > self.limit {
            self.failure = Some(LimitedJsonFailure::Capacity {
                required: required as u64,
            });
            return Err(io::Error::other("snapshot byte limit exceeded"));
        }
        self.bytes.try_reserve(bytes.len()).map_err(|_| {
            self.failure = Some(LimitedJsonFailure::Allocation);
            io::Error::other("snapshot buffer allocation failed")
        })?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn emit_body_fluid_particles(
    editor: &mut WorldEdit<'_>,
    layer: LayerId,
    layer_key: StableKey,
    world_units_per_cell: f64,
    stranded: &[BodyFluidOverlap],
) -> EngineResult<BTreeSet<(i32, i32)>> {
    let mut emitted = BTreeSet::new();
    if stranded.is_empty() {
        return Ok(emitted);
    }
    let Some(physics) = editor.world.body_physics.as_ref() else {
        return Ok(emitted);
    };
    let available_slots =
        (editor.world.limits.particles as usize).saturating_sub(editor.world.particles.len());
    if available_slots == 0 {
        return Ok(emitted);
    }
    let used_bytes = editor
        .world
        .particles
        .iter()
        .try_fold(0usize, |total, (_, particle)| {
            total
                .checked_add(particle.payload_bytes()?)
                .ok_or_else(|| invalid("fluid排除particle byte数overflow"))
        })?;
    let mut remaining_bytes = editor
        .world
        .limits
        .particle_bytes
        .checked_sub(used_bytes)
        .ok_or_else(|| invalid("fluid排除particle byte capacity"))?;
    let occupied_particles = editor
        .world
        .particles
        .iter()
        .filter(|(_, particle)| particle.layer() == layer && !particle.is_archived())
        .map(|(_, particle)| {
            let point = particle.cell();
            (point.y, point.x)
        })
        .collect::<BTreeSet<_>>();
    let maximum_emissions = available_slots.min(stranded.len());
    let mut emissions = Vec::new();
    emissions
        .try_reserve_exact(maximum_emissions)
        .map_err(allocation)?;
    let mut channel_indexes = Vec::new();
    for (index, channel) in editor.world.channels.iter().enumerate() {
        if channel.material_layer() == Some(layer_key) {
            channel_indexes.try_reserve(1).map_err(allocation)?;
            channel_indexes.push(index);
        }
    }
    for fluid in stranded {
        if emissions.len() >= available_slots
            || occupied_particles.contains(&(fluid.source.y, fluid.source.x))
        {
            continue;
        }
        let mut required_bytes = 0usize;
        for index in &channel_indexes {
            let channel = &editor.world.channels[*index];
            let overhead = channel
                .key()
                .as_str()
                .len()
                .checked_add(8)
                .ok_or_else(|| invalid("fluid粒子channel辞書byte overflow"))?;
            let bytes = channel.material_payload(fluid.source, channel.checkpoint_limit())?;
            required_bytes = required_bytes
                .checked_add(overhead)
                .and_then(|total| total.checked_add(bytes.len()))
                .ok_or_else(|| invalid("fluid粒子channel byte overflow"))?;
        }
        if required_bytes > remaining_bytes {
            continue;
        }
        emissions.push(super::MaterialParticleEmission {
            point: fluid.source,
            velocity_q16: physics.particle_velocity_at_cell(
                fluid.body,
                fluid.source,
                world_units_per_cell,
            )?,
        });
        remaining_bytes -= required_bytes;
    }
    for emission in &emissions {
        emitted.insert((emission.point.y, emission.point.x));
    }
    if !emissions.is_empty() {
        editor.emit_material_particles_batch(layer, &emissions)?;
    }
    Ok(emitted)
}

fn move_body_fluid_cells(
    editor: &mut WorldEdit<'_>,
    layer: LayerId,
    layer_key: StableKey,
    moves: &[BodyFluidMove],
) -> EngineResult<usize> {
    if moves.is_empty() {
        return Ok(0);
    }
    let mut channel_indexes = Vec::new();
    for (index, channel) in editor.world.channels.iter().enumerate() {
        if channel.material_layer() == Some(layer_key) {
            channel_indexes.try_reserve(1).map_err(allocation)?;
            channel_indexes.push(index);
        }
    }
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(moves.len())
        .map_err(allocation)?;
    for movement in moves {
        let mut cell_payloads = Vec::new();
        cell_payloads
            .try_reserve_exact(channel_indexes.len())
            .map_err(allocation)?;
        for index in &channel_indexes {
            let channel = &editor.world.channels[*index];
            let bytes = channel.material_payload(movement.source, channel.checkpoint_limit())?;
            channel.validate_payload(&bytes)?;
            cell_payloads.push(bytes);
        }
        payloads.push(cell_payloads);
    }
    let writes_per_move = channel_indexes
        .len()
        .checked_mul(2)
        .and_then(|writes| writes.checked_add(2))
        .ok_or_else(|| invalid("fluid移送write数overflow"))?;
    let write_count = moves
        .len()
        .checked_mul(writes_per_move)
        .ok_or_else(|| invalid("fluid移送write数overflow"))?;
    editor.reserve_writes(write_count)?;

    let point_count = moves
        .len()
        .checked_mul(2)
        .ok_or_else(|| invalid("fluid移送cell数overflow"))?;
    let mut reset_points = Vec::new();
    reset_points
        .try_reserve_exact(point_count)
        .map_err(allocation)?;
    for movement in moves {
        reset_points.push(movement.source);
        reset_points.push(movement.destination);
    }
    for index in &channel_indexes {
        editor.world.channels[*index].reset_material(layer_key, &reset_points)?;
    }
    for (movement, cell_payloads) in moves.iter().zip(payloads) {
        editor.write(layer, movement.source, CellCore::AIR, None)?;
        editor.write(layer, movement.destination, movement.core, None)?;
        for (index, bytes) in channel_indexes.iter().zip(cell_payloads) {
            let current_total = editor.world.channel_bytes();
            let current_channel = editor.world.channels[*index].bytes();
            let projected_channel =
                editor.world.channels[*index].payload_bytes(movement.destination, &bytes)?;
            let projected_total = current_total
                .checked_sub(current_channel)
                .and_then(|total| total.checked_add(projected_channel))
                .ok_or_else(|| invalid("fluid移送channel byte数overflow"))?;
            if projected_total > editor.world.limits.channel_bytes {
                return Err(EngineError::capacity(
                    "fluid移送channel byte数",
                    projected_total as u64,
                    editor.world.limits.channel_bytes as u64,
                ));
            }
            editor.world.channels[*index].put_payload(movement.destination, &bytes)?;
        }
    }
    Ok(moves.len())
}

fn infer_body_cell_transfer_config(
    world: &World,
    layer: LayerId,
    point: CellPoint,
) -> EngineResult<BodyCellTransferConfig> {
    let layer_key = world.content.layer(layer)?.key;
    let mut mixture_channel = None;
    let mut thermal_channel = None;
    for channel in &world.channels {
        if channel.material_layer() != Some(layer_key) {
            continue;
        }
        match channel.mixture_reference(point) {
            Ok(_) if mixture_channel.replace(channel.key()).is_some() => {
                return Err(EngineError::new(
                    ErrorCode::UnsupportedCapability,
                    "fluid排除layerにMixtureRef channelが複数あります",
                ));
            }
            Ok(_) => (),
            Err(error) if error.code == ErrorCode::SchemaMismatch => (),
            Err(error) => return Err(error),
        }
        match channel.thermal_energy(point) {
            Ok(_) if thermal_channel.replace(channel.key()).is_some() => {
                return Err(EngineError::new(
                    ErrorCode::UnsupportedCapability,
                    "fluid排除layerにThermalCellState channelが複数あります",
                ));
            }
            Ok(_) => (),
            Err(error) if error.code == ErrorCode::SchemaMismatch => (),
            Err(error) => return Err(error),
        }
    }
    Ok(BodyCellTransferConfig {
        mixture_channel,
        thermal_channel,
    })
}

fn body_material_motion_channel(world: &World, layer: LayerId) -> EngineResult<StableKey> {
    let layer_key = world.content.layer(layer)?.key;
    let mut selected = None;
    for channel in &world.channels {
        match world.channel_material_layer::<super::MaterialMotion>(channel.key()) {
            Ok(Some(channel_layer)) if channel_layer == layer_key => {
                if selected.replace(channel.key()).is_some() {
                    return Err(EngineError::new(
                        ErrorCode::UnsupportedCapability,
                        "fluid連成layerにMaterialMotion channelが複数あります",
                    ));
                }
            }
            Ok(_) => (),
            Err(error) if error.code == ErrorCode::SchemaMismatch => (),
            Err(error) => return Err(error),
        }
    }
    selected.ok_or_else(|| {
        EngineError::new(
            ErrorCode::UnsupportedCapability,
            "fluid連成にはMaterialMotion channelが必要です",
        )
    })
}

#[derive(Clone, Copy)]
struct BodyFluidContact {
    body: BodyId,
    point: CellPoint,
    surface_measure: f64,
    weighted_surface_x: f64,
    weighted_surface_y: f64,
    coverage: f64,
    surface_point: [f64; 2],
    reaction_point: [f64; 2],
}

fn push_body_fluid_contact(
    contacts: &mut Vec<BodyFluidContact>,
    maximum: usize,
    contact: BodyFluidContact,
) -> EngineResult<()> {
    if !contact.surface_measure.is_finite()
        || contact.surface_measure <= 0.0
        || !contact.weighted_surface_x.is_finite()
        || !contact.weighted_surface_y.is_finite()
    {
        return Err(invalid("剛体fluid連成coverage sample"));
    }
    if contacts.len() >= maximum {
        return Err(EngineError::capacity(
            "剛体fluid連成samples",
            contacts.len().saturating_add(1) as u64,
            maximum as u64,
        ));
    }
    if contacts.len() == contacts.capacity() {
        contacts.try_reserve(1).map_err(allocation)?;
    }
    contacts.push(contact);
    Ok(())
}

fn body_occupancy_contains(occupancy: &[(BodyId, CellPoint)], point: CellPoint) -> bool {
    occupancy
        .binary_search_by_key(&(point.y, point.x), |(_, occupied)| {
            (occupied.y, occupied.x)
        })
        .is_ok()
}

fn body_occupancy_owner(occupancy: &[(BodyId, CellPoint)], point: CellPoint) -> Option<BodyId> {
    occupancy
        .binary_search_by_key(&(point.y, point.x), |(_, occupied)| {
            (occupied.y, occupied.x)
        })
        .ok()
        .map(|index| occupancy[index].0)
}

#[derive(Clone, Copy)]
struct BodyFluidCouplingSample {
    body: BodyId,
    point: CellPoint,
    reaction_point: [f64; 2],
    body_impulse: [f64; 2],
}

fn quantize_fluid_impulse(
    body_impulse: [f64; 2],
    fluid_mass: f64,
    world_units_per_cell: f64,
    step_hz: u32,
) -> EngineResult<[i64; 2]> {
    let mut velocity_delta = [0i64; 2];
    let velocity_scale = 65536.0 / (world_units_per_cell * f64::from(step_hz));
    if !velocity_scale.is_finite() || velocity_scale <= 0.0 {
        return Err(invalid("fluid連成の速度換算"));
    }
    for axis in 0..2 {
        let value = -body_impulse[axis] / fluid_mass * velocity_scale;
        if !value.is_finite() || value < i64::MIN as f64 || value >= 9223372036854775808.0 {
            return Err(invalid("fluid連成のQ16力積範囲"));
        }
        velocity_delta[axis] = value.round() as i64;
    }
    Ok(velocity_delta)
}

fn fluid_impulse_from_q16(
    velocity_delta_q16: [i64; 2],
    fluid_mass: f64,
    world_units_per_cell: f64,
    step_hz: u32,
) -> EngineResult<[f64; 2]> {
    let scale = fluid_mass * world_units_per_cell * f64::from(step_hz) / 65536.0;
    let impulse = [
        scale * velocity_delta_q16[0] as f64,
        scale * velocity_delta_q16[1] as f64,
    ];
    if !scale.is_finite() || impulse.iter().any(|value| !value.is_finite()) {
        return Err(invalid("fluid連成の量子化力積"));
    }
    Ok(impulse)
}

fn infer_body_cell_return_config(
    world: &World,
    layer: LayerId,
    point: CellPoint,
) -> EngineResult<BodyCellReturnConfig> {
    let transfer = infer_body_cell_transfer_config(world, layer, point)?;
    let mixture_schema = if transfer.mixture_channel.is_some() {
        Some(
            world
                .state
                .records()
                .find(|record| record.key == MixtureStore::key())
                .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "混合store"))?
                .schema,
        )
    } else {
        None
    };
    Ok(BodyCellReturnConfig {
        mixture_channel: transfer.mixture_channel,
        mixture_schema,
        thermal_channel: transfer.thermal_channel,
        remove_empty_body: false,
    })
}

fn transferred_body_cell(
    world: &World,
    layer: LayerId,
    point: CellPoint,
    config: BodyCellTransferConfig,
    allow_block: bool,
) -> EngineResult<BodyLocalCell> {
    let view = match world.cell(layer, point)? {
        CellQuery::Loaded(view) => view,
        CellQuery::Unavailable(_) => {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "剛体へ移すcellのchunkが未読込",
            ));
        }
        CellQuery::WorldBoundary => return Err(invalid("剛体へ移すcellがWorld境界外")),
    };
    if (view.block.is_some() && !allow_block) || view.core.material == MaterialId::AIR {
        return Err(EngineError::new(
            ErrorCode::Conflict,
            "剛体へ移す地形cellが物質cellではない",
        ));
    }

    let content = world.content();
    let layer_key = content.layer(layer)?.key;
    for channel in &world.channels {
        if channel.material_layer() == Some(layer_key) && !channel.material_cell_is_exact(point)? {
            return Err(invalid(
                "World cellと一対一対応しないmaterial channelの剛体移送",
            ));
        }
    }
    let material = content
        .material(view.core.material)?
        .ok_or_else(|| invalid("剛体へ移すcellの素材"))?;

    let mixture_reference = if let Some(key) = config.mixture_channel {
        if world.channel_material_layer::<MixtureRef>(key)? != Some(layer_key) {
            return Err(invalid("剛体移送の混合channel所属layer"));
        }
        match world.channel_value::<MixtureRef>(key, point)? {
            ChannelQuery::Loaded(value) => value,
            _ => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "剛体移送の混合channel coverageが未準備",
                ));
            }
        }
    } else {
        MixtureRef::NONE
    };
    if config.mixture_channel.is_none() && !mixture_reference.is_none() {
        return Err(invalid("混合cellの移送にはMixtureRef channelが必要"));
    }
    for channel in &world.channels {
        if channel.material_layer() != Some(layer_key) {
            continue;
        }
        if Some(channel.key()) != config.mixture_channel {
            match channel.mixture_reference(point) {
                Ok(reference) if !reference.is_none() => {
                    return Err(invalid("標準以外の混合参照を剛体へ移送できない"));
                }
                Ok(_) => (),
                Err(error) if error.code == ErrorCode::SchemaMismatch => (),
                Err(error) => return Err(error),
            }
        }
        if Some(channel.key()) != config.thermal_channel {
            match channel.thermal_energy(point) {
                Ok(energy) if energy != 0 => {
                    return Err(invalid(
                        "enthalpy cellの移送にはThermalCellState channelが必要",
                    ));
                }
                Ok(_) => (),
                Err(error) if error.code == ErrorCode::SchemaMismatch => (),
                Err(error) => return Err(error),
            }
        }
    }
    let components = if mixture_reference.is_none() {
        Vec::new()
    } else {
        let store: MixtureStore = world.state(MixtureStore::key())?;
        let quantities = store.quantities(content, view.core.material, mixture_reference)?;
        let mut components = Vec::new();
        components
            .try_reserve_exact(quantities.len())
            .map_err(allocation)?;
        for quantity in quantities {
            components.push(BodyMaterialComponent::new(
                quantity.material,
                quantity.tokens,
            )?);
        }
        components
    };

    let energy = if let Some(key) = config.thermal_channel {
        if world.channel_material_layer::<super::ThermalCellState>(key)? != Some(layer_key) {
            return Err(invalid("剛体移送の熱channel所属layer"));
        }
        match world.channel_value::<super::ThermalCellState>(key, point)? {
            ChannelQuery::Loaded(value) => value.energy,
            _ => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "剛体移送の熱channel coverageが未準備",
                ));
            }
        }
    } else {
        0
    };

    let mut sidecars = Vec::new();
    for channel in &world.channels {
        if channel.material_layer() != Some(layer_key)
            || Some(channel.key()) == config.mixture_channel
            || Some(channel.key()) == config.thermal_channel
            || channel.payload_is_default(point)?
        {
            continue;
        }
        let definition = content
            .channels
            .iter()
            .find(|definition| definition.key() == channel.key())
            .ok_or_else(|| invalid("剛体移送channel定義"))?;
        let payload = channel.material_payload(point, channel.checkpoint_limit())?;
        channel.validate_payload(&payload)?;
        sidecars.try_reserve(1).map_err(allocation)?;
        sidecars.push(BodyCellSidecar::new(
            channel.key(),
            definition.version(),
            payload,
        )?);
    }
    sidecars.sort_by(|left, right| left.key.cmp(&right.key));

    let mut body_cell = BodyLocalCell::new(
        layer_key,
        material.key(),
        view.core.flags,
        view.core.variation,
        energy,
        sidecars,
    )?
    .with_applied_remap_epoch(world.remaps.epoch());
    if !components.is_empty() {
        body_cell = body_cell.with_mixture(components)?;
    }
    Ok(body_cell)
}

fn place_block_instance_at_destinations(
    mut block: BlockInstance,
    size: [u16; 2],
    members: &[(u32, CellPoint)],
) -> EngineResult<BlockInstance> {
    let Some((first_mask, first_destination)) = members.first().copied() else {
        return Err(invalid("空block attachment復帰"));
    };
    for orientation in [
        super::Orientation::Original,
        super::Orientation::Right,
        super::Orientation::Down,
        super::Orientation::Left,
    ] {
        block.orientation = orientation;
        block.anchor = CellPoint { x: 0, y: 0 };
        let offset = block.point(first_mask, size)?;
        let anchor_x = i32::try_from(i64::from(first_destination.x) - i64::from(offset.x))
            .map_err(|_| invalid("block復帰anchor x"))?;
        let anchor_y = i32::try_from(i64::from(first_destination.y) - i64::from(offset.y))
            .map_err(|_| invalid("block復帰anchor y"))?;
        block.anchor = CellPoint {
            x: anchor_x,
            y: anchor_y,
        };
        if members
            .iter()
            .all(|(mask, destination)| block.point(*mask, size).is_ok_and(|p| p == *destination))
        {
            return Ok(block);
        }
    }
    Err(conflict(
        "block個体の復帰先が元maskの90度回転・平行移動と一致しません",
    ))
}

fn body_mixture_state(cell: &BodyLocalCell) -> EngineResult<MixtureState> {
    if cell.mixture.len() < 2 {
        return Err(invalid("body mixture requires multiple components"));
    }
    let mut common_denominator = 1u128;
    for component in &cell.mixture {
        let denominator = component.tokens.denominator();
        common_denominator = common_denominator
            .checked_div(body_quantity_gcd(common_denominator, denominator))
            .and_then(|value| value.checked_mul(denominator))
            .ok_or_else(|| invalid("body mixture ratio denominator overflow"))?;
    }
    let mut ratios = Vec::new();
    ratios
        .try_reserve_exact(cell.mixture.len())
        .map_err(allocation)?;
    for component in &cell.mixture {
        ratios.push(
            component
                .tokens
                .numerator()
                .checked_mul(common_denominator / component.tokens.denominator())
                .ok_or_else(|| invalid("body mixture ratio overflow"))?,
        );
    }
    let divisor = ratios.iter().copied().fold(0, body_quantity_gcd);
    let mut parts = Vec::new();
    parts
        .try_reserve_exact(cell.mixture.len())
        .map_err(allocation)?;
    for (component, ratio) in cell.mixture.iter().zip(ratios) {
        parts.push(MixturePart {
            material: component.material()?,
            ratio: u32::try_from(ratio / divisor)
                .map_err(|_| invalid("body mixture ratio exceeds World representation"))?,
        });
    }
    MixtureState::new(&parts)
}

fn body_quantity_gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn decode_body_reaction_state(
    cell: &BodyLocalCell,
    channel: StableKey,
    version: u32,
) -> EngineResult<ReactionSidecar> {
    let Some(sidecar) = cell
        .sidecars
        .binary_search_by(|sidecar| sidecar.key.as_str().cmp(channel.as_str()))
        .ok()
        .map(|index| &cell.sidecars[index])
    else {
        return Ok(ReactionSidecar::default());
    };
    if sidecar.version != version {
        return Err(EngineError::new(
            ErrorCode::SchemaMismatch,
            "body reaction sidecar version",
        ));
    }
    let mut input = Decoder::new(&sidecar.payload, sidecar.payload.len())?;
    let state = ReactionSidecar::decode(&mut input)?;
    input.finish()?;
    Ok(state)
}

fn encode_body_reaction_state(state: ReactionSidecar) -> EngineResult<Vec<u8>> {
    let mut output = Encoder::new(14);
    state.encode(&mut output)?;
    Ok(output.finish())
}

fn set_body_reaction_state(
    cell: &mut BodyLocalCell,
    channel: StableKey,
    version: u32,
    state: ReactionSidecar,
) -> EngineResult<()> {
    let position = cell
        .sidecars
        .binary_search_by(|sidecar| sidecar.key.as_str().cmp(channel.as_str()));
    match position {
        Ok(index) => {
            let payload = encode_body_reaction_state(state)?;
            cell.sidecars[index] = BodyCellSidecar::new(channel, version, payload)?;
        }
        Err(index) if state != ReactionSidecar::default() => {
            let payload = encode_body_reaction_state(state)?;
            let sidecar = BodyCellSidecar::new(channel, version, payload)?;
            cell.sidecars.try_reserve(1).map_err(allocation)?;
            cell.sidecars.insert(index, sidecar);
        }
        Err(_) => {}
    }
    Ok(())
}

fn body_reaction_cells(
    grid: &BodyLocalGrid,
    layer: StableKey,
    sidecar: StableKey,
    sidecar_version: u32,
    family: &super::EnthalpyFamily,
) -> EngineResult<Vec<ReactionCell>> {
    let mut cells = Vec::new();
    cells
        .try_reserve_exact(grid.cell_count())
        .map_err(allocation)?;
    for (index, cell) in &grid.cells {
        if cell.layer()? != layer {
            continue;
        }
        let x =
            i32::try_from(index % grid.width).map_err(|_| invalid("body reaction x coordinate"))?;
        let y =
            i32::try_from(index / grid.width).map_err(|_| invalid("body reaction y coordinate"))?;
        let material = cell.material()?;
        let quantity = if cell.mixture.is_empty() && cell.tokens.denominator() == 1 {
            u64::try_from(cell.tokens.numerator()).unwrap_or(0)
        } else {
            0
        };
        let can_react = cell.block_attachment.is_none()
            && cell.mixture.is_empty()
            && quantity > 0
            && family.contains_material(material);
        if can_react {
            let phase = family.state(cell.energy)?;
            if phase.from != material && phase.to != material {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body reaction material and enthalpy phase disagree",
                ));
            }
        }
        cells.push(ReactionCell {
            x,
            y,
            material,
            quantity: quantity.max(1),
            energy: cell.energy,
            state: decode_body_reaction_state(cell, sidecar, sidecar_version)?,
            can_react,
        });
    }
    Ok(cells)
}

/// 一つの反応outputへ移すbody作者metadataです。
#[derive(Clone, Debug)]
struct BodyReactionCellMigration {
    flags: u16,
    variation: u16,
    sidecars: Vec<BodyCellSidecar>,
}

fn body_reaction_cell_migration(
    cell: &BodyLocalCell,
    reaction_sidecar: StableKey,
) -> EngineResult<BodyReactionCellMigration> {
    let mut sidecars = Vec::new();
    sidecars
        .try_reserve_exact(cell.sidecars.len())
        .map_err(allocation)?;
    sidecars.extend(
        cell.sidecars
            .iter()
            .filter(|sidecar| sidecar.key.as_str() != reaction_sidecar.as_str())
            .cloned(),
    );
    Ok(BodyReactionCellMigration {
        flags: cell.flags,
        variation: cell.variation,
        sidecars,
    })
}

fn build_body_reaction_job(
    world: &World,
    body: BodyId,
    layer: StableKey,
    sidecar: StableKey,
    sidecar_version: u32,
    rules: &ReactionRuleSet,
    candidate_limit: usize,
) -> EngineResult<ReactionJob> {
    let layer_id = world.content.layer_id(layer)?;
    if !world.content.layer(layer_id)?.simulated {
        return Err(invalid("body reaction requires a simulated material layer"));
    }
    if sidecar_version == 0 || candidate_limit == 0 {
        return Err(invalid("body reaction rule or candidate budget"));
    }
    if candidate_limit > world.limits.job_cells {
        return Err(EngineError::capacity(
            "body reaction candidate limit",
            candidate_limit as u64,
            world.limits.job_cells as u64,
        ));
    }
    if rules.len() > world.limits.jobs as usize {
        return Err(EngineError::capacity(
            "body reaction rule count",
            rules.len() as u64,
            world.limits.jobs as u64,
        ));
    }
    let physics = world
        .body_physics
        .as_ref()
        .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?;
    let grid = physics
        .local_grid(body)?
        .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body local grid未設定"))?;
    let dimensions = grid.dimensions();
    let width = usize::try_from(dimensions[0]).map_err(|_| invalid("body reaction width"))?;
    let height = usize::try_from(dimensions[1]).map_err(|_| invalid("body reaction height"))?;
    if width > i32::MAX as usize || height > i32::MAX as usize {
        return Err(invalid("body reaction geometry exceeds coordinate range"));
    }
    let cells = body_reaction_cells(grid, layer, sidecar, sidecar_version, rules.family())?;
    let work = cells
        .len()
        .checked_mul(rules.len())
        .ok_or_else(|| invalid("body reaction candidate work overflow"))?;
    let candidate_bytes = rules.candidate_storage_bytes(candidate_limit)?;
    let scratch_bytes = cells
        .len()
        .checked_mul(std::mem::size_of::<ReactionCell>() + std::mem::size_of::<[i32; 2]>())
        .and_then(|bytes| bytes.checked_add(candidate_bytes))
        .ok_or_else(|| invalid("body reaction scratch byte overflow"))?;
    if cells.len() > world.limits.job_cells {
        return Err(EngineError::capacity(
            "body reaction cell count",
            cells.len() as u64,
            world.limits.job_cells as u64,
        ));
    }
    if work > world.limits.job_cells {
        return Err(EngineError::capacity(
            "body reaction candidate work",
            work as u64,
            world.limits.job_cells as u64,
        ));
    }
    if scratch_bytes > world.limits.job_bytes {
        return Err(EngineError::capacity(
            "body reaction scratch bytes",
            scratch_bytes as u64,
            world.limits.job_bytes as u64,
        ));
    }
    let job = ReactionJob::build(&cells, rules, candidate_limit)?;
    if !job.destinations_within(width, height) {
        return Err(EngineError::new(
            ErrorCode::Conflict,
            "body reaction output destination lies outside local grid geometry",
        ));
    }
    Ok(job)
}

/// body-local reaction ruleが発行する物質粒子をWorldへ配置する換算設定です。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyReactionParticleOutputConfig {
    /// reaction output energyを格納する、layer追従1:1のThermalCellState channelです。
    pub thermal_channel: StableKey,
    /// reaction output quantityを格納する、layer追従1:1のu64 channelです。
    pub quantity_channel: StableKey,
    /// reaction output sidecarを格納する、layer追従1:1のReactionSidecar channelです。
    pub reaction_sidecar: StableKey,
    /// 剛体物理単位からWorld cellへ変換する寸法です。
    pub world_units_per_cell: f64,
}

/// Worldの固定剛体stepへ接続する、body-local reaction scheduleの定義です。
///
/// `schedule_key`と`version`はrule、channel、particle出力設定の意味を識別します。意味を変える場合は
/// versionを進めてください。進行中Jobと異なるscheduleを同じstate_keyへ読み込むと、候補の誤適用を
/// 防ぐためConflictになります。World state用schemaには`BodyReactionScheduleState`を登録します。
#[derive(Clone, Copy)]
pub struct BodyReactionSchedule<'a> {
    /// scheduleの安定識別子。
    pub schedule_key: StableKey,
    /// scheduleの意味版。
    pub version: u32,
    /// 進行中Jobと最後に見たgrid revisionを保持するWorld StateStore key。
    pub state_key: StableKey,
    /// `BodyReactionScheduleState`を登録したschema key。
    pub state_schema: StableKey,
    /// 反応対象body。
    pub body: BodyId,
    /// 反応定義を適用するmaterial layer。
    pub layer: StableKey,
    /// body cellへ接続するReactionSidecar channel。
    pub reaction_sidecar: StableKey,
    /// ReactionSidecar channel版。
    pub sidecar_version: u32,
    /// 不変の反応rule set。意味変更時はschedule versionも更新します。
    pub rules: &'a ReactionRuleSet,
    /// 一度に候補へ変換できる反応数上限。
    pub candidate_limit: usize,
    /// 固定stepごとの決定的なreaction candidate予算。
    pub budget: usize,
    /// Someの場合、rule出力をWorld material particleへ移します。
    pub particle_output: Option<BodyReactionParticleOutputConfig>,
}

/// 固定step後、body surfaceで熱交換してから反応scheduleを進めるprofileです。
#[derive(Clone, Copy)]
pub struct BodyThermalReactionBoundary<'a> {
    /// 熱を交換するbody。
    pub body: BodyId,
    /// body local cellと対応するWorld layer key。
    pub layer: StableKey,
    /// World `ThermalCellState` channel key。
    pub energy_channel: StableKey,
    /// World/bodyの相・enthalpy定義。
    pub family: &'a EnthalpyFamily,
    /// 一tickの熱伝導設定。
    pub config: ThermalStepConfig,
    /// 境界交換後にbody-local cell間の熱伝導も実行します。
    pub conduct_body_interior: bool,
}

/// 一つのbody reaction scheduleの固定step結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyReactionScheduleReport {
    /// schedule対象body。
    pub body: BodyId,
    /// Jobの次回実行状態。
    pub status: crate::runtime::schedule::WorkStatus,
    /// この固定stepでのreaction件数・延期数です。
    pub reaction: ReactionStepReport,
}

/// 保存可能なbody reaction scheduleのJobとlast-seen grid revisionです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyReactionScheduleState {
    schedule_key: StableKey,
    version: u32,
    body: BodyId,
    layer: StableKey,
    reaction_sidecar: StableKey,
    sidecar_version: u32,
    grid_revision: u64,
    job: ReactionJob,
}

impl StateCodec for BodyReactionScheduleState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self.version == 0 || self.sidecar_version == 0 {
            return Err(invalid("body reaction schedule state version"));
        }
        self.schedule_key.encode(out)?;
        out.u32(self.version)?;
        self.body.encode(out)?;
        self.layer.encode(out)?;
        self.reaction_sidecar.encode(out)?;
        out.u32(self.sidecar_version)?;
        out.u64(self.grid_revision)?;
        self.job.encode(out)
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let state = Self {
            schedule_key: StableKey::decode(input)?,
            version: input.u32()?,
            body: BodyId::decode(input)?,
            layer: StableKey::decode(input)?,
            reaction_sidecar: StableKey::decode(input)?,
            sidecar_version: input.u32()?,
            grid_revision: input.u64()?,
            job: ReactionJob::decode(input)?,
        };
        if state.version == 0 || state.sidecar_version == 0 {
            return Err(invalid("body reaction schedule state version"));
        }
        Ok(state)
    }
}

impl World {
    /// 保存済みbody reaction scheduleを実行し、反応と剛体fixed stepを一つのWorld候補で確定します。
    ///
    /// scheduleはBodyId順に実行します。未完Jobは再開し、完了Jobはbody-local grid revisionが変わるまで
    /// 再走査しません。反応・state store・terrain連成・物理stepのどれかが失敗した場合は全候補を破棄します。
    pub fn step_bodies_with_reactions(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        schedules: &[BodyReactionSchedule<'_>],
    ) -> EngineResult<(Vec<BodyReactionScheduleReport>, BodyStepReport)> {
        self.step_bodies_with_thermal_reactions(
            collision_layer,
            world_units_per_cell,
            &[],
            schedules,
        )
    }

    /// fluid連成の後にbody/world熱交換・body内部熱拡散・反応を同じtick候補で実行します。
    ///
    /// thermal profileはbody ID、layer、energy channel順で確定し、反応scheduleより先に適用します。
    /// heat、reaction、particle出力、World/physicsのどれかが失敗した場合は候補全体を破棄します。
    pub fn step_bodies_with_thermal_reactions(
        &mut self,
        collision_layer: LayerId,
        world_units_per_cell: f64,
        thermal_boundaries: &[BodyThermalReactionBoundary<'_>],
        schedules: &[BodyReactionSchedule<'_>],
    ) -> EngineResult<(Vec<BodyReactionScheduleReport>, BodyStepReport)> {
        let mut ordered: Vec<&BodyReactionSchedule<'_>> = Vec::new();
        ordered
            .try_reserve_exact(schedules.len())
            .map_err(allocation)?;
        ordered.extend(schedules);
        ordered.sort_unstable_by_key(|schedule| {
            (
                schedule.body.index(),
                schedule.body.generation(),
                schedule.schedule_key,
            )
        });
        let mut state_keys = BTreeSet::new();
        let mut scheduled_bodies = BTreeSet::new();
        for schedule in &ordered {
            if schedule.version == 0
                || schedule.sidecar_version == 0
                || schedule.candidate_limit == 0
                || schedule.budget == 0
            {
                return Err(invalid("body reaction schedule configuration"));
            }
            if !state_keys.insert(schedule.state_key) || !scheduled_bodies.insert(schedule.body) {
                return Err(conflict("reaction schedule state keyまたはbody重複"));
            }
        }

        let mut ordered_thermal: Vec<&BodyThermalReactionBoundary<'_>> = Vec::new();
        ordered_thermal
            .try_reserve_exact(thermal_boundaries.len())
            .map_err(allocation)?;
        ordered_thermal.extend(thermal_boundaries);
        ordered_thermal.sort_unstable_by_key(|profile| {
            (
                profile.body.index(),
                profile.body.generation(),
                profile.layer,
                profile.energy_channel,
            )
        });
        for pair in ordered_thermal.windows(2) {
            if pair[0].body == pair[1].body && pair[0].layer == pair[1].layer {
                return Err(conflict("同一body/layerのthermal boundary profile重複"));
            }
        }
        let mut group_start = 0;
        while group_start < ordered_thermal.len() {
            let body = ordered_thermal[group_start].body;
            let mut group_end = group_start + 1;
            while group_end < ordered_thermal.len() && ordered_thermal[group_end].body == body {
                group_end += 1;
            }
            if let Some(interior) = ordered_thermal[group_start..group_end]
                .iter()
                .find(|profile| profile.conduct_body_interior)
                && ordered_thermal[group_start..group_end]
                    .iter()
                    .any(|profile| {
                        profile.family != interior.family || profile.config != interior.config
                    })
            {
                return Err(conflict(
                    "body内部熱伝導を行うprofile群でenthalpy family/configが一致しません",
                ));
            }
            group_start = group_end;
        }

        self.edit_and_step_bodies_with_post_step(
            collision_layer,
            world_units_per_cell,
            |_| Ok(()),
            |editor| {
                let mut group_start = 0;
                while group_start < ordered_thermal.len() {
                    let body = ordered_thermal[group_start].body;
                    let mut group_end = group_start + 1;
                    while group_end < ordered_thermal.len()
                        && ordered_thermal[group_end].body == body
                    {
                        group_end += 1;
                    }
                    let mut conduct_interior = None;
                    for profile in &ordered_thermal[group_start..group_end] {
                        let layer = editor.world.content.layer_id(profile.layer)?;
                        editor.step_body_world_thermal_boundary(
                            profile.body,
                            layer,
                            profile.energy_channel,
                            profile.family,
                            profile.config,
                            world_units_per_cell,
                        )?;
                        if profile.conduct_body_interior {
                            conduct_interior = Some((profile.family, profile.config));
                        }
                    }
                    if let Some((family, config)) = conduct_interior {
                    editor.step_body_local_thermal(
                            body,
                            family,
                            config,
                    )?;
                }
                    group_start = group_end;
            }
            let mut reports = Vec::new();
            let mut states = Vec::new();
            reports
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            states
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for schedule in &ordered {
                let grid_revision = editor
                    .world
                    .body_physics
                    .as_ref()
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?
                    .local_grid(schedule.body)?
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "reaction body local grid未設定")
                    })?
                    .revision();
                let previous = match editor
                    .world
                    .state::<BodyReactionScheduleState>(schedule.state_key)
                {
                    Ok(state) => {
                        if state.schedule_key != schedule.schedule_key
                            || state.version != schedule.version
                            || state.body != schedule.body
                            || state.layer != schedule.layer
                            || state.reaction_sidecar != schedule.reaction_sidecar
                            || state.sidecar_version != schedule.sidecar_version
                        {
                            return Err(conflict(
                                "進行中reaction Jobのschedule identityが一致しません。state keyを変更するかJobを明示取消してください",
                            ));
                        }
                        Some(state)
                    }
                    Err(error) if error.code == ErrorCode::MissingKey => None,
                    Err(error) => return Err(error),
                };
                let reusable = previous
                    .as_ref()
                    .is_some_and(|state| state.grid_revision == grid_revision);
                if reusable
                    && previous
                        .as_ref()
                        .is_some_and(|state| state.job.work_status()
                            == crate::runtime::schedule::WorkStatus::Complete)
                {
                    reports.push(BodyReactionScheduleReport {
                        body: schedule.body,
                        status: crate::runtime::schedule::WorkStatus::Complete,
                        reaction: ReactionStepReport::default(),
                    });
                    continue;
                }
                let resume = if reusable {
                    previous.map(|state| state.job)
                } else {
                    None
                };
                let (job, reaction) = if let Some(output) = schedule.particle_output {
                    editor.step_body_local_reactions_inner(
                        schedule.body,
                        schedule.layer,
                        schedule.reaction_sidecar,
                        schedule.sidecar_version,
                        schedule.rules,
                        schedule.candidate_limit,
                        schedule.budget,
                        resume,
                        schedule.state_key,
                        schedule.state_schema,
                        Some(output),
                        false,
                    )?
                } else {
                    editor.step_body_local_reactions_inner(
                        schedule.body,
                        schedule.layer,
                        schedule.reaction_sidecar,
                        schedule.sidecar_version,
                        schedule.rules,
                        schedule.candidate_limit,
                        schedule.budget,
                        resume,
                        schedule.state_key,
                        schedule.state_schema,
                        None,
                        false,
                    )?
                };
                let next_grid_revision = editor
                    .world
                    .body_physics
                    .as_ref()
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?
                    .local_grid(schedule.body)?
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "reaction body local grid消失")
                    })?
                    .revision();
                states.push((
                    schedule.state_key,
                    schedule.state_schema,
                    BodyReactionScheduleState {
                        schedule_key: schedule.schedule_key,
                        version: schedule.version,
                        body: schedule.body,
                        layer: schedule.layer,
                        reaction_sidecar: schedule.reaction_sidecar,
                        sidecar_version: schedule.sidecar_version,
                        grid_revision: next_grid_revision,
                        job: job.clone(),
                    },
                ));
                reports.push(BodyReactionScheduleReport {
                    body: schedule.body,
                    status: job.work_status(),
                    reaction,
                });
            }
            if !states.is_empty() {
                editor.reserve_writes(states.len())?;
                let mut transaction = editor.world.state.transaction()?;
                for (key, schema, state) in &states {
                    transaction.put(*key, *schema, state)?;
                }
                transaction.commit()?;
            }
            Ok(reports)
            },
        )
        .map(|((), reports, step)| (reports, step))
    }
}

fn body_reaction_particle_output_transform(
    state: BodyState,
    grid: &BodyLocalGrid,
    local: [i32; 2],
    relative_velocity_q16: [i64; 2],
    step_hz: u32,
    world_units_per_cell: f64,
) -> EngineResult<(CellPoint, [i64; 2])> {
    if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 || step_hz == 0 {
        return Err(invalid("body reaction particle unit conversion"));
    }
    let pivot = grid.pivot();
    let local_center = [
        (f64::from(local[0]) + 0.5 - pivot[0]) * grid.cell_size(),
        (f64::from(local[1]) + 0.5 - pivot[1]) * grid.cell_size(),
    ];
    let offset = rotate_local_point(local_center, state.pose.angle);
    let position = [state.pose.x + offset.x, state.pose.y + offset.y];
    let world_cell = |coordinate: f64| -> EngineResult<i32> {
        let scaled = (coordinate / world_units_per_cell).floor();
        if !scaled.is_finite() || scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
            return Err(invalid("body reaction particle world cell range"));
        }
        Ok(scaled as i32)
    };
    let body_velocity = [
        state.velocity.x - state.velocity.angular * offset.y,
        state.velocity.y + state.velocity.angular * offset.x,
    ];
    let q16_scale = 65_536.0 / (world_units_per_cell * f64::from(step_hz));
    if !position.iter().all(|value| value.is_finite())
        || !body_velocity.iter().all(|value| value.is_finite())
        || !q16_scale.is_finite()
    {
        return Err(invalid("body reaction particle transform"));
    }
    let convert_velocity = |value: f64, relative: i64| -> EngineResult<i64> {
        let body_q16 = (value * q16_scale).round();
        if !body_q16.is_finite() || body_q16 < i64::MIN as f64 || body_q16 > i64::MAX as f64 {
            return Err(invalid("body reaction particle velocity range"));
        }
        i64::try_from(body_q16 as i128 + i128::from(relative))
            .map_err(|_| invalid("body reaction particle velocity sum"))
    };
    Ok((
        CellPoint {
            x: world_cell(position[0])?,
            y: world_cell(position[1])?,
        },
        [
            convert_velocity(body_velocity[0], relative_velocity_q16[0])?,
            convert_velocity(body_velocity[1], relative_velocity_q16[1])?,
        ],
    ))
}

impl WorldEdit<'_> {
    /// BodyLocalGridの現在値から保存可能なReactionJob候補を構築します。
    ///
    /// Jobは呼出側がWorld StateStoreへ保存し、後続tickで同じrulesとsidecar定義を使って
    /// `step_body_local_reactions`または`step_body_local_reactions_with_particle_output`へ戻します。
    /// 反応sourceは混合なしの整数token cellに限り、出力先も固定local grid内である必要があります。
    #[allow(clippy::too_many_arguments)]
    pub fn build_body_local_reaction_job(
        &mut self,
        body: BodyId,
        layer: StableKey,
        reaction_sidecar: StableKey,
        sidecar_version: u32,
        rules: &ReactionRuleSet,
        candidate_limit: usize,
    ) -> EngineResult<ReactionJob> {
        self.run(|editor| {
            let job = build_body_reaction_job(
                editor.world,
                body,
                layer,
                reaction_sidecar,
                sidecar_version,
                rules,
                candidate_limit,
            )?;
            Ok(job)
        })
    }

    /// 保存済みreaction候補をbody local cellへ原子的に適用します。
    ///
    /// material/quantity/occupancyが変わる場合はshape、revision、質量・慣性を一度だけ
    /// 更新します。reaction候補が要求するenergy deltaと実際のbody-local energy差を照合し、
    /// sourceが残る部分消費ではflags・variation・opaque sidecarを残存sourceへ保持し、変更後materialの
    /// Content検証を通します。source全消費時にopaque stateを出力へ移す作者policyがなければ取引全体を拒否します。
    #[allow(clippy::too_many_arguments)]
    pub fn step_body_local_reactions(
        &mut self,
        body: BodyId,
        layer: StableKey,
        reaction_sidecar: StableKey,
        sidecar_version: u32,
        rules: &ReactionRuleSet,
        candidate_limit: usize,
        budget: usize,
        resume: Option<ReactionJob>,
        job_key: StableKey,
        job_schema: StableKey,
    ) -> EngineResult<(ReactionJob, ReactionStepReport)> {
        self.step_body_local_reactions_inner(
            body,
            layer,
            reaction_sidecar,
            sidecar_version,
            rules,
            candidate_limit,
            budget,
            resume,
            job_key,
            job_schema,
            None,
            true,
        )
    }

    /// 保存済みreaction候補を適用し、rule指定のparticle出力も同じWorldEditで発行します。
    ///
    /// 出力先はbody local output cellのpose変換位置です。そこが未読込、占有済み、または
    /// material-following channelに既存payloadを持つ場合、その候補は次のbudgetへ延期します。
    /// particle速度はruleのQ16 cell/tick速度へbody表面速度を加算します。
    #[allow(clippy::too_many_arguments)]
    pub fn step_body_local_reactions_with_particle_output(
        &mut self,
        body: BodyId,
        layer: StableKey,
        reaction_sidecar: StableKey,
        sidecar_version: u32,
        rules: &ReactionRuleSet,
        candidate_limit: usize,
        budget: usize,
        resume: Option<ReactionJob>,
        job_key: StableKey,
        job_schema: StableKey,
        output_config: BodyReactionParticleOutputConfig,
    ) -> EngineResult<(ReactionJob, ReactionStepReport)> {
        self.step_body_local_reactions_inner(
            body,
            layer,
            reaction_sidecar,
            sidecar_version,
            rules,
            candidate_limit,
            budget,
            resume,
            job_key,
            job_schema,
            Some(output_config),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn step_body_local_reactions_inner(
        &mut self,
        body: BodyId,
        layer: StableKey,
        reaction_sidecar: StableKey,
        sidecar_version: u32,
        rules: &ReactionRuleSet,
        candidate_limit: usize,
        budget: usize,
        resume: Option<ReactionJob>,
        job_key: StableKey,
        job_schema: StableKey,
        particle_output_config: Option<BodyReactionParticleOutputConfig>,
        persist_job: bool,
    ) -> EngineResult<(ReactionJob, ReactionStepReport)> {
        self.run(|editor| {
            let layer_id = editor.world.content.layer_id(layer)?;
            if !editor.world.content.layer(layer_id)?.simulated {
                return Err(invalid("body reaction requires a simulated material layer"));
            }
            if budget == 0 || sidecar_version == 0 {
                return Err(invalid("body reaction budget or sidecar version"));
            }
            let max_write_work = budget
                .checked_mul(2)
                .ok_or_else(|| invalid("body reaction write budget overflow"))?;
            if max_write_work > editor.world.limits.edit_cells {
                return Err(EngineError::capacity(
                    "body reaction write budget",
                    max_write_work as u64,
                    editor.world.limits.edit_cells as u64,
                ));
            }
            let physics = editor
                .world
                .body_physics
                .as_ref()
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?;
            let source_grid = physics.local_grid(body)?.ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
            })?;
            let dimensions = source_grid.dimensions();
            let source_grid_revision = source_grid.revision;
            let width = usize::try_from(dimensions[0]).map_err(|_| invalid("body reaction width"))?;
            let height = usize::try_from(dimensions[1]).map_err(|_| invalid("body reaction height"))?;
            let mut job = match resume {
                Some(job) => job,
                None => build_body_reaction_job(
                    editor.world,
                    body,
                    layer,
                    reaction_sidecar,
                    sidecar_version,
                    rules,
                    candidate_limit,
                )?,
            };
            let candidate_count = job.candidate_count();
            let candidate_bytes = rules.candidate_storage_bytes(candidate_count)?;
            let sidecar_bytes = source_grid.sidecar_bytes()?;
            let track_author_state = source_grid.cells.values().any(|cell| {
                cell.layer().is_ok_and(|cell_layer| cell_layer == layer)
                    && (cell.flags != 0
                        || cell.variation != 0
                        || cell.sidecars.iter().any(|sidecar| {
                            sidecar.key.as_str() != reaction_sidecar.as_str()
                        }))
            });
            let transition_candidate_capacity = if track_author_state { budget } else { 0 };
            let cell_scratch = source_grid
                .cell_count()
                .checked_mul(
                    std::mem::size_of::<ReactionCell>() * 3
                        + std::mem::size_of::<[i32; 2]>() * 2
                        + std::mem::size_of::<BodyLocalCell>()
                        + std::mem::size_of::<usize>() * 4,
                )
                .ok_or_else(|| invalid("body reaction scratch byte overflow"))?;
            let scratch = cell_scratch
                .checked_add(sidecar_bytes)
                .and_then(|bytes| bytes.checked_add(candidate_bytes))
                .and_then(|bytes| {
                    transition_candidate_capacity
                        .checked_mul(
                            std::mem::size_of::<ReactionCandidate>()
                                + std::mem::size_of::<(u32, usize)>(),
                        )
                        .and_then(|migration_bytes| bytes.checked_add(migration_bytes))
                })
                .and_then(|bytes| bytes.checked_add(sidecar_bytes))
                .ok_or_else(|| invalid("body reaction scratch byte overflow"))?;
            if candidate_limit > editor.world.limits.job_cells {
                return Err(EngineError::capacity(
                    "body reaction candidate limit",
                    candidate_limit as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            if candidate_count > candidate_limit {
                return Err(EngineError::capacity(
                    "body reaction candidate count",
                    candidate_count as u64,
                    candidate_limit as u64,
                ));
            }
            if scratch > editor.world.limits.job_bytes {
                return Err(EngineError::capacity(
                    "body reaction scratch bytes",
                    scratch as u64,
                    editor.world.limits.job_bytes as u64,
                ));
            }
            if !job.destinations_within(width, height) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body reaction output destination lies outside local grid geometry",
                ));
            }
            let mut occupied_other_layers = Vec::new();
            let extra_particle_candidates = if particle_output_config.is_some() {
                budget
            } else {
                0
            };
            occupied_other_layers
                .try_reserve_exact(
                    source_grid
                        .cell_count()
                        .saturating_add(extra_particle_candidates),
                )
                .map_err(allocation)?;
            let mut particle_destinations = Vec::new();
            if let Some(config) = particle_output_config {
                particle_destinations
                    .try_reserve_exact(budget)
                    .map_err(allocation)?;
                let output_scratch = budget
                    .checked_mul(
                        std::mem::size_of::<(CellPoint, [i32; 2], [i64; 2])>() * 2
                            + std::mem::size_of::<([i32; 2], [i64; 2])>()
                            + std::mem::size_of::<(CellPoint, [i64; 2], ReactionCell)>()
                            + std::mem::size_of::<(
                                CellPoint,
                                [i64; 2],
                                ReactionCell,
                                Option<BodyReactionCellMigration>,
                            )>(),
                    )
                    .ok_or_else(|| invalid("body reaction particle scratch overflow"))?;
                let total_scratch = scratch
                    .checked_add(output_scratch)
                    .ok_or_else(|| invalid("body reaction particle scratch overflow"))?;
                if total_scratch > editor.world.limits.job_bytes {
                    return Err(EngineError::capacity(
                        "body reaction particle scratch bytes",
                        total_scratch as u64,
                        editor.world.limits.job_bytes as u64,
                    ));
                }
                if config.reaction_sidecar != reaction_sidecar {
                    return Err(invalid("body reaction particle sidecar must match the job"));
                }
                if config.thermal_channel == config.quantity_channel
                    || config.thermal_channel == config.reaction_sidecar
                    || config.quantity_channel == config.reaction_sidecar
                {
                    return Err(invalid("body reaction particle channel key overlap"));
                }
                for valid in [
                    (
                        editor
                            .world
                            .channel_layout::<ThermalCellState>(config.thermal_channel)?
                            .origin
                            == (CellPoint { x: 0, y: 0 })
                            && editor
                                .world
                                .channel_layout::<ThermalCellState>(config.thermal_channel)?
                                .x_scale
                                .ratio()
                                == (1, 1)
                            && editor
                                .world
                                .channel_layout::<ThermalCellState>(config.thermal_channel)?
                                .y_scale
                                .ratio()
                                == (1, 1)
                            && editor.world.channel_material_layer::<ThermalCellState>(
                                config.thermal_channel,
                            )? == Some(layer),
                    ),
                    (
                        editor
                            .world
                            .channel_layout::<u64>(config.quantity_channel)?
                            .origin
                            == (CellPoint { x: 0, y: 0 })
                            && editor
                                .world
                                .channel_layout::<u64>(config.quantity_channel)?
                                .x_scale
                                .ratio()
                                == (1, 1)
                            && editor
                                .world
                                .channel_layout::<u64>(config.quantity_channel)?
                                .y_scale
                                .ratio()
                                == (1, 1)
                            && editor
                                .world
                                .channel_material_layer::<u64>(config.quantity_channel)?
                                == Some(layer),
                    ),
                    (
                        editor
                            .world
                            .channel_layout::<ReactionSidecar>(config.reaction_sidecar)?
                            .origin
                            == (CellPoint { x: 0, y: 0 })
                            && editor
                                .world
                                .channel_layout::<ReactionSidecar>(config.reaction_sidecar)?
                                .x_scale
                                .ratio()
                                == (1, 1)
                            && editor
                                .world
                                .channel_layout::<ReactionSidecar>(config.reaction_sidecar)?
                                .y_scale
                                .ratio()
                                == (1, 1)
                            && editor.world.channel_material_layer::<ReactionSidecar>(
                                config.reaction_sidecar,
                            )? == Some(layer),
                    ),
                ] {
                    if !valid.0 {
                        return Err(invalid("body reaction particle channels must be layer 1:1"));
                    }
                }
                let layer_key = editor.world.content.layer(layer_id)?.key;
                let output_count = job
                    .candidates()
                    .iter()
                    .take(budget)
                    .filter(|candidate| candidate.particle_velocity_q16.is_some())
                    .count();
                let material_channel_count = editor
                    .world
                    .channels
                    .iter()
                    .filter(|channel| {
                        channel.material_layer() == Some(layer_key)
                    })
                    .count();
                let output_writes = output_count
                    .checked_mul(
                        6_usize
                            .checked_add(material_channel_count)
                            .ok_or_else(|| invalid("body reaction particle write count overflow"))?,
                    )
                    .ok_or_else(|| invalid("body reaction particle write count overflow"))?;
                let total_writes = max_write_work
                    .checked_add(output_writes)
                    .ok_or_else(|| invalid("body reaction particle write count overflow"))?;
                if total_writes > editor.world.limits.edit_cells {
                    return Err(EngineError::capacity(
                        "body reaction particle write budget",
                        total_writes as u64,
                        editor.world.limits.edit_cells as u64,
                    ));
                }
                let physics = editor
                    .world
                    .body_physics
                    .as_ref()
                    .ok_or_else(|| invalid("BodyPhysics disappeared during reaction"))?;
                let body_state = physics.state(body)?;
                let step_hz = physics.settings.step_hz;
                let mut seen_world_points = Vec::new();
                seen_world_points
                    .try_reserve_exact(budget)
                    .map_err(allocation)?;
                for candidate in job.candidates().iter().take(budget) {
                    let Some(relative_velocity) = candidate.particle_velocity_q16 else {
                        continue;
                    };
                    let local = candidate.destination;
                    let (point, velocity) = body_reaction_particle_output_transform(
                        body_state,
                        source_grid,
                        local,
                        relative_velocity,
                        step_hz,
                        config.world_units_per_cell,
                    )?;
                    let active = editor
                        .world
                        .chunks
                        .get(&ChunkCoord::at(point))
                        .is_some_and(|chunk| chunk.active);
                    let view = if active {
                        editor.world.cell(layer_id, point)?
                    } else {
                        super::CellQuery::Unavailable(ChunkCoord::at(point))
                    };
                    let available = matches!(view, super::CellQuery::Loaded(cell) if cell.block.is_none() && cell.core.material == super::MaterialId::AIR)
                        && !editor.world.particles.iter().any(|(_, particle)| {
                            !particle.is_archived()
                                && particle.layer() == layer_id
                                && particle.cell() == point
                        })
                        && editor
                            .world
                            .channels
                            .iter()
                            .filter(|channel| channel.material_layer() == Some(layer_key))
                            .try_fold(true, |available, channel| {
                                Ok(available && channel.payload_is_default(point)?)
                            })?;
                    if !available {
                        occupied_other_layers.push(local);
                    } else {
                        seen_world_points.push((point, local, velocity));
                    }
                }
                seen_world_points.sort_unstable_by_key(|entry| (entry.0.y, entry.0.x));
                let mut blocked = Vec::new();
                blocked
                    .try_reserve_exact(seen_world_points.len())
                    .map_err(allocation)?;
                for pair in seen_world_points.windows(2) {
                    if pair[0].0 == pair[1].0 {
                        blocked.push(pair[0].1);
                        blocked.push(pair[1].1);
                    }
                }
                blocked.sort_unstable();
                blocked.dedup();
                occupied_other_layers.extend(blocked.iter().copied());
                particle_destinations.extend(
                    seen_world_points
                        .into_iter()
                        .filter(|(_, local, _)| blocked.binary_search(local).is_err())
                        .map(|(point, local, velocity)| (local, point, velocity)),
                );
                particle_destinations.sort_unstable_by_key(|(_, point, _)| (point.y, point.x));
                let available_slots = (editor.world.limits.particles as usize)
                    .saturating_sub(editor.world.particles.len());
                if particle_destinations.len() > available_slots {
                    occupied_other_layers.extend(
                        particle_destinations
                            .drain(available_slots..)
                            .map(|(local, _, _)| local),
                    );
                }
            } else if job
                .candidates()
                .iter()
                .any(|candidate| candidate.particle_velocity_q16.is_some())
            {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body local reaction particle output requires an explicit migration policy",
                ));
            }
            occupied_other_layers.sort_unstable();
            occupied_other_layers.dedup();
            let mut cells = body_reaction_cells(
                source_grid,
                layer,
                reaction_sidecar,
                sidecar_version,
                rules.family(),
            )?;
            let mut before_cells = Vec::new();
            before_cells
                .try_reserve_exact(cells.len())
                .map_err(allocation)?;
            before_cells.extend_from_slice(&cells);
            let energy_before = cells.iter().try_fold(0_i128, |sum, cell| {
                sum.checked_add(i128::from(cell.energy))
                    .ok_or_else(|| invalid("body reaction input energy overflow"))
            })?;
            for (index, cell) in &source_grid.cells {
                if cell.layer()? == layer {
                    continue;
                }
                occupied_other_layers.push([
                    i32::try_from(index % source_grid.width)
                        .map_err(|_| invalid("body reaction occupied x"))?,
                    i32::try_from(index / source_grid.width)
                        .map_err(|_| invalid("body reaction occupied y"))?,
                ]);
            }
            occupied_other_layers.sort_unstable();
            occupied_other_layers.dedup();
            let other_count = occupied_other_layers
                .len()
                .checked_add(source_grid.exclusion_reservoir.len())
                .ok_or_else(|| invalid("body reaction non-target cell count overflow"))?;
            let max_reaction_cells = (source_grid.cell_capacity as usize)
                .checked_sub(other_count)
                .ok_or_else(|| invalid("body reaction local grid capacity underflow"))?;
            let mut reaction_particle_outputs = Vec::new();
            reaction_particle_outputs
                .try_reserve_exact(budget)
                .map_err(allocation)?;
            let mut committed_candidates = Vec::new();
            committed_candidates
                .try_reserve_exact(transition_candidate_capacity.min(job.remaining()))
                .map_err(allocation)?;
            let report = job.step_with_particle_outputs_and_transitions(
                &mut cells,
                rules,
                &occupied_other_layers,
                max_reaction_cells,
                budget,
                Some(&mut reaction_particle_outputs),
                track_author_state.then_some(&mut committed_candidates),
            )?;
            committed_candidates.sort_unstable_by_key(|candidate| {
                source_grid
                    .index(candidate.source[0] as u32, candidate.source[1] as u32)
                    .unwrap_or(u32::MAX)
            });
            if committed_candidates.windows(2).any(|pair| {
                source_grid.index(pair[0].source[0] as u32, pair[0].source[1] as u32)
                    == source_grid.index(pair[1].source[0] as u32, pair[1].source[1] as u32)
            }) {
                return Err(conflict("body reaction committed a source more than once"));
            }
            let mut committed_by_destination = Vec::new();
            committed_by_destination
                .try_reserve_exact(committed_candidates.len())
                .map_err(allocation)?;
            committed_by_destination.extend(committed_candidates.iter().enumerate().filter_map(|(candidate_index, candidate)| {
                source_grid
                    .index(candidate.destination[0] as u32, candidate.destination[1] as u32)
                    .map(|index| (index, candidate_index))
            }));
            committed_by_destination.sort_unstable_by_key(|(index, _)| *index);
            if committed_by_destination.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(conflict("body reaction has duplicate output destinations"));
            }
            let mut emitted_outputs = Vec::new();
            emitted_outputs
                .try_reserve_exact(reaction_particle_outputs.len())
                .map_err(allocation)?;
            for (local, _) in reaction_particle_outputs {
                let Some((_, world_point, world_velocity)) = particle_destinations
                    .iter()
                    .find(|(candidate_local, _, _)| *candidate_local == local)
                    .copied()
                else {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "committed body reaction particle has no reserved World destination",
                    ));
                };
                let output_index = cells
                    .binary_search_by_key(&(local[1], local[0]), |cell| (cell.y, cell.x))
                    .map_err(|_| invalid("body reaction particle output cell missing"))?;
                let output = cells.remove(output_index);
                if output.quantity == 0 {
                    return Err(invalid("body reaction particle output mismatch"));
                }
                let output_index = source_grid
                    .index(local[0] as u32, local[1] as u32)
                    .ok_or_else(|| invalid("body reaction particle destination index"))?;
                let migration = committed_by_destination
                    .binary_search_by_key(&output_index, |(index, _)| *index)
                    .ok()
                    .map(|entry_index| committed_candidates[committed_by_destination[entry_index].1])
                    .filter(|candidate| {
                        candidate.state_transfer == ReactionStateTransfer::MigrateToOutput
                    })
                    .map(|candidate| {
                        let source_index = source_grid
                            .index(candidate.source[0] as u32, candidate.source[1] as u32)
                            .ok_or_else(|| invalid("body reaction particle migration source"))?;
                        let source = source_grid
                            .cells
                            .get(&source_index)
                            .ok_or_else(|| invalid("body reaction particle migration source missing"))?;
                        body_reaction_cell_migration(source, reaction_sidecar)
                    })
                    .transpose()?;
                emitted_outputs.push((world_point, world_velocity, output, migration));
            }
            if report.committed != 0 {
                let energy_after = cells.iter().try_fold(0_i128, |sum, cell| {
                    sum.checked_add(i128::from(cell.energy))
                        .ok_or_else(|| invalid("body reaction output energy overflow"))
                })?.checked_add(emitted_outputs.iter().try_fold(0_i128, |sum, (_, _, cell, _)| {
                    sum.checked_add(i128::from(cell.energy))
                        .ok_or_else(|| invalid("body reaction particle energy overflow"))
                })?).ok_or_else(|| invalid("body reaction total energy overflow"))?;
                let expected_energy = energy_before
                    .checked_add(report.declared_energy_delta)
                    .ok_or_else(|| invalid("body reaction expected energy overflow"))?;
                if energy_after != expected_energy {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "body reaction energy differs from declared rule delta",
                    ));
                }
                let mut changed = 0usize;
                let mut shape_or_mass_changed = false;
                // WorldEditは独立候補上で動くため、rollback用にBodyLocalGridを重ねて複製する
                // 必要はない。ここでは変更cellだけを確保し、全検証後に候補gridへ反映する。
                let mut removals = Vec::new();
                removals
                    .try_reserve_exact(source_grid.cell_count())
                    .map_err(allocation)?;
                let mut updates = Vec::new();
                updates
                    .try_reserve_exact(source_grid.cell_count())
                    .map_err(allocation)?;
                let mut new_cells = Vec::new();
                new_cells
                    .try_reserve_exact(cells.len())
                    .map_err(allocation)?;
                let mut target_indexes = Vec::new();
                target_indexes
                    .try_reserve_exact(source_grid.cell_count())
                    .map_err(allocation)?;
                target_indexes.extend(
                    source_grid
                        .cells
                        .iter()
                        .filter_map(|(index, cell)| {
                            cell.layer()
                                .is_ok_and(|cell_layer| cell_layer == layer)
                                .then_some(*index)
                        }),
                );
                for index in target_indexes {
                    let x = i32::try_from(index % source_grid.width)
                        .map_err(|_| invalid("body reaction source x"))?;
                    let y = i32::try_from(index / source_grid.width)
                        .map_err(|_| invalid("body reaction source y"))?;
                    let new_index = cells.binary_search_by_key(&(y, x), |cell| (cell.y, cell.x));
                    let original = source_grid
                        .cells
                        .get(&index)
                        .ok_or_else(|| invalid("body reaction source disappeared"))?;
                    let Some(new_index) = new_index.ok() else {
                        let committed = committed_candidates
                            .binary_search_by_key(&index, |candidate| {
                                source_grid
                                    .index(candidate.source[0] as u32, candidate.source[1] as u32)
                                    .unwrap_or(u32::MAX)
                            })
                            .ok()
                            .map(|candidate_index| committed_candidates[candidate_index]);
                        let has_author_state = original.flags != 0
                            || original.variation != 0
                            || original.sidecars.iter().any(|sidecar| {
                                sidecar.key.as_str() != reaction_sidecar.as_str()
                            });
                        if has_author_state
                            && committed.is_none_or(|candidate| {
                                matches!(candidate.state_transfer, ReactionStateTransfer::RetainSource)
                            })
                        {
                            return Err(EngineError::new(
                                ErrorCode::Conflict,
                                "body reaction author state has no declared transfer policy",
                            ));
                        }
                        removals.push(index);
                        shape_or_mass_changed = true;
                        changed = changed
                            .checked_add(1)
                            .ok_or_else(|| invalid("body reaction change count overflow"))?;
                        continue;
                    };
                    let result = cells[new_index];
                    let old = before_cells
                    .binary_search_by_key(&(y, x), |cell| (cell.y, cell.x))
                    .ok()
                    .map(|old_index| before_cells[old_index])
                    .ok_or_else(|| invalid("body reaction original cell mapping"))?;
                    if old == result {
                        continue;
                    }
                    if !old.can_react {
                        return Err(invalid("body reaction changed a non-reactive contact cell"));
                    }
                    let phase = rules.family().state(result.energy)?;
                    if phase.from != result.material && phase.to != result.material {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body reaction output material and enthalpy phase disagree",
                        ));
                    }
                    let mut updated = original.clone();
                    let material_changed = updated.material()? != result.material;
                    let quantity = super::MaterialQuantity::new(u128::from(result.quantity), 1)?;
                    shape_or_mass_changed |= material_changed || updated.tokens != quantity;
                    updated.material = result.material.as_str().to_owned();
                    updated.tokens = quantity;
                    updated.energy = result.energy;
                    if material_changed {
                        updated.remap_epoch = editor.world.remaps.epoch();
                    }
                    set_body_reaction_state(
                        &mut updated,
                        reaction_sidecar,
                        sidecar_version,
                        result.state,
                    )?;
                    validate_body_cell_content(&updated, &editor.world.content)?;
                    updates.push((index, updated));
                    changed = changed
                        .checked_add(1)
                        .ok_or_else(|| invalid("body reaction change count overflow"))?;
                }
                for result in &cells {
                    let x = u32::try_from(result.x)
                        .map_err(|_| invalid("body reaction output x"))?;
                    let y = u32::try_from(result.y)
                        .map_err(|_| invalid("body reaction output y"))?;
                    let index = source_grid
                        .index(x, y)
                        .ok_or_else(|| invalid("body reaction output destination"))?;
                    if source_grid.cells.contains_key(&index) {
                        continue;
                    }
                    let phase = rules.family().state(result.energy)?;
                    if phase.from != result.material && phase.to != result.material {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body reaction output material and enthalpy phase disagree",
                        ));
                    }
                    let migration = committed_by_destination
                        .binary_search_by_key(&index, |(output_index, _)| *output_index)
                        .ok()
                        .map(|entry_index| committed_candidates[committed_by_destination[entry_index].1])
                        .filter(|candidate| {
                            candidate.state_transfer == ReactionStateTransfer::MigrateToOutput
                        })
                        .map(|candidate| {
                            let source_index = source_grid
                                .index(candidate.source[0] as u32, candidate.source[1] as u32)
                                .ok_or_else(|| invalid("body reaction output migration source"))?;
                            let source = source_grid
                                .cells
                                .get(&source_index)
                                .ok_or_else(|| invalid("body reaction output migration source missing"))?;
                            body_reaction_cell_migration(source, reaction_sidecar)
                        })
                        .transpose()?;
                    let (flags, variation, mut sidecars) = migration.map_or_else(
                        || (0, 0, Vec::new()),
                        |state| (state.flags, state.variation, state.sidecars),
                    );
                    if result.state != ReactionSidecar::default() {
                        sidecars.try_reserve(1).map_err(allocation)?;
                        sidecars.retain(|existing| existing.key.as_str() != reaction_sidecar.as_str());
                        sidecars.push(BodyCellSidecar::new(
                            reaction_sidecar,
                            sidecar_version,
                            encode_body_reaction_state(result.state)?,
                        )?);
                    }
                    sidecars.sort_unstable_by(|left, right| left.key.cmp(&right.key));
                    let output = BodyLocalCell::new(
                        layer,
                        result.material,
                        flags,
                        variation,
                        result.energy,
                        sidecars,
                    )?
                    .with_tokens(super::MaterialQuantity::new(u128::from(result.quantity), 1)?)?
                    .with_applied_remap_epoch(editor.world.remaps.epoch());
                    validate_body_cell_content(&output, &editor.world.content)?;
                    new_cells.push((index, output));
                    shape_or_mass_changed = true;
                    changed = changed
                        .checked_add(1)
                        .ok_or_else(|| invalid("body reaction change count overflow"))?;
                }
                if changed != 0 {
                    editor.reserve_writes(changed)?;
                    let physics = editor
                        .world
                        .body_physics
                        .as_mut()
                        .ok_or_else(|| invalid("BodyPhysics disappeared during reaction"))?;
                    {
                        let grid = Arc::make_mut(&mut physics.local_grids)
                            .get_mut(&body)
                            .ok_or_else(|| invalid("body local grid disappeared during reaction"))?;
                        for index in removals {
                            grid.cells.remove(&index);
                            grid.clear_thermal_edges_for_removed_cell(index);
                        }
                        for (index, cell) in updates.into_iter().chain(new_cells) {
                            grid.cells.insert(index, cell);
                        }
                        if shape_or_mass_changed {
                            grid.revision = source_grid_revision
                                .checked_add(1)
                                .ok_or_else(|| invalid("body reaction grid revision overflow"))?;
                            grid.remap_epoch_bounds = None;
                        }
                        grid.validate()?;
                    }
                    physics.validate_local_grid_budget(
                        Some(body),
                        physics
                            .local_grid(body)?
                            .ok_or_else(|| invalid("body local grid disappeared during reaction"))?,
                    )?;
                    if shape_or_mass_changed {
                        physics.local_grid_content_dirty.insert(body);
                        physics.validate_content(&editor.world.content, editor.world.limits)?;
                        physics.sync_material_mass_properties(&editor.world.content)?;
                    } else {
                        physics.local_grid_content_dirty.insert(body);
                        physics.validate_content(&editor.world.content, editor.world.limits)?;
                        physics.local_grid_content_dirty.remove(&body);
                    }
                }
            }
            if let Some(config) = particle_output_config {
                for (point, velocity, output, migration) in emitted_outputs {
                    let material = editor
                        .world
                        .content
                        .material_id(MaterialHandle(output.material))?;
                    editor.set_cell(
                        layer_id,
                        point,
                        CellCore {
                            material,
                            flags: migration.as_ref().map_or(0, |state| state.flags),
                            variation: migration.as_ref().map_or(0, |state| state.variation),
                        },
                    )?;
                    editor.set_channel(
                        config.thermal_channel,
                        point,
                        ThermalCellState {
                            energy: output.energy,
                        },
                    )?;
                    editor.set_channel(config.quantity_channel, point, output.quantity)?;
                    editor.set_channel(config.reaction_sidecar, point, output.state)?;
                    if let Some(migration) = migration {
                        for sidecar in migration.sidecars {
                            let key = sidecar.key()?;
                            editor.world.content.validate_body_sidecar(
                                layer,
                                key,
                                sidecar.version,
                                &sidecar.payload,
                            )?;
                            let channel_index = editor
                                .world
                                .channels
                                .iter()
                                .position(|channel| channel.key() == key)
                                .ok_or_else(|| invalid("migrated sidecar channel is not registered"))?;
                            if editor.world.channels[channel_index].material_layer() != Some(layer) {
                                return Err(invalid("migrated sidecar channel targets a different layer"));
                            }
                            editor.world.channels[channel_index].put_payload(point, &sidecar.payload)?;
                        }
                    }
                    editor.emit_material_particle(layer_id, point, velocity)?;
                }
            }
            if persist_job {
                editor.set_state(job_key, job_schema, &job)?;
            }
            Ok((job, report))
        })
    }

    /// BodyLocalGrid内部の断熱熱伝導をE06のenthalpy solverで一度計算します。
    ///
    /// energy、phase material、body-local edge端数は同じWorldEdit候補へ適用します。
    /// familyに含まれない相、質量が変わるphase切替、範囲・容量・整数収支の失敗では
    /// WorldとBodyPhysicsを公開しません。phase切替は未混合carrierに限ります。
    /// Worldとの境界熱交換とreactionは別の連成入力が必要です。
    pub fn step_body_local_thermal(
        &mut self,
        body: BodyId,
        family: &super::EnthalpyFamily,
        config: super::ThermalStepConfig,
    ) -> EngineResult<super::ThermalStepReport> {
        self.run(|editor| {
            let physics = editor
                .world
                .body_physics
                .as_ref()
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?;
            let grid = physics.local_grid(body)?.ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
            })?;
            let dimensions = grid.dimensions();
            let width =
                usize::try_from(dimensions[0]).map_err(|_| invalid("body thermal width"))?;
            let height =
                usize::try_from(dimensions[1]).map_err(|_| invalid("body thermal height"))?;
            let len = width
                .checked_mul(height)
                .ok_or_else(|| invalid("body thermal cell count overflow"))?;
            let writes_limit = grid
                .cell_count()
                .checked_mul(3)
                .ok_or_else(|| invalid("body thermal edit count overflow"))?;
            let scratch_per_cell = std::mem::size_of::<super::ThermalCellState>() * 2
                + std::mem::size_of::<super::ThermalEdgeState>() * 2
                + std::mem::size_of::<i128>()
                + std::mem::size_of::<super::ThermalLookup>()
                + std::mem::size_of::<bool>();
            let scratch_bytes = len
                .checked_mul(scratch_per_cell)
                .ok_or_else(|| invalid("body thermal scratch byte overflow"))?;
            if len == 0
                || len > editor.world.limits.job_cells
                || writes_limit > editor.world.limits.edit_cells
                || scratch_bytes > editor.world.limits.job_bytes
            {
                return Err(EngineError::capacity(
                    "body thermal extent or scratch",
                    len as u64,
                    editor
                        .world
                        .limits
                        .job_cells
                        .min(editor.world.limits.edit_cells) as u64,
                ));
            }

            let initial_energy = family.energy_bounds().0;
            let mut input = Vec::new();
            let mut input_edges = Vec::new();
            let mut output = Vec::new();
            let mut output_edges = Vec::new();
            let mut delta = Vec::new();
            let mut lookup = Vec::new();
            let mut active = Vec::new();
            input.try_reserve_exact(len).map_err(allocation)?;
            input_edges.try_reserve_exact(len).map_err(allocation)?;
            output.try_reserve_exact(len).map_err(allocation)?;
            output_edges.try_reserve_exact(len).map_err(allocation)?;
            delta.try_reserve_exact(len).map_err(allocation)?;
            lookup.try_reserve_exact(len).map_err(allocation)?;
            active.try_reserve_exact(len).map_err(allocation)?;
            input.resize(
                len,
                super::ThermalCellState {
                    energy: initial_energy,
                },
            );
            input_edges.resize(len, super::ThermalEdgeState::default());
            output.resize(len, super::ThermalCellState::default());
            output_edges.resize(len, super::ThermalEdgeState::default());
            delta.resize(len, 0_i128);
            lookup.resize(len, super::ThermalLookup::default());
            active.resize(len, false);
            for (index, cell) in &grid.cells {
                let position = *index as usize;
                let phase = family.state(cell.energy())?;
                let carrier = cell.material()?;
                if !family.contains_material(carrier)
                    || (phase.from != carrier && phase.to != carrier)
                {
                    return Err(invalid("body material and enthalpy phase disagree"));
                }
                input[position] = super::ThermalCellState {
                    energy: cell.energy(),
                };
                input_edges[position] = grid.thermal_edges.get(index).copied().unwrap_or_default();
                active[position] = true;
            }

            let report = super::diffuse_enthalpy_into(
                width,
                height,
                family,
                &input,
                &input_edges,
                |index| active[index],
                config,
                &mut delta,
                &mut lookup,
                &mut output,
                &mut output_edges,
            )?;
            let mut cell_updates = Vec::new();
            cell_updates
                .try_reserve_exact(grid.cell_count())
                .map_err(allocation)?;
            let mut changed_cells = 0usize;
            let content = Arc::clone(&editor.world.content);
            let remap_epoch = editor.world.remaps.epoch();
            let mut body_energy_before = 0_i128;
            let mut body_energy_after = 0_i128;
            for (index, cell) in &grid.cells {
                let energy = output[*index as usize].energy;
                body_energy_before = body_energy_before
                    .checked_add(i128::from(cell.energy()))
                    .ok_or_else(|| invalid("body thermal input energy overflow"))?;
                body_energy_after = body_energy_after
                    .checked_add(i128::from(energy))
                    .ok_or_else(|| invalid("body thermal output energy overflow"))?;
                let next_material = family.material_at_energy(energy)?;
                let current_material = cell.material()?;
                if next_material != current_material {
                    if cell.block_attachment.is_some() {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "block個体の熱相変化policyが未登録です",
                        ));
                    }
                    let next_id = content.material_id(MaterialHandle(next_material))?;
                    if next_id == MaterialId::AIR {
                        return Err(invalid("body thermal phase cannot become air"));
                    }
                    let before = content
                        .material(content.material_id(MaterialHandle(current_material))?)?
                        .ok_or_else(|| invalid("body thermal source material"))?;
                    let after = content
                        .material(next_id)?
                        .ok_or_else(|| invalid("body thermal phase material"))?;
                    if cell.mixture().is_empty() && before.mass_per_cell() != after.mass_per_cell()
                    {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body thermal phase change alters material mass",
                        ));
                    }
                    let mut candidate_cell = cell.clone();
                    candidate_cell.material = next_material.as_str().to_owned();
                    candidate_cell.remap_epoch = remap_epoch;
                    candidate_cell.energy = energy;
                    // 混合cellはcarrierの表示素材を替えても、保存済み成分tokenを動かさない。
                    // Content検証が新carrierの質量と成分質量の有理値一致を確認するため、
                    // 一致しない相変化だけをここで原子的に拒否する。
                    validate_body_cell_content(&candidate_cell, &content)?;
                }
                if energy != cell.energy() || next_material != current_material {
                    changed_cells = changed_cells
                        .checked_add(1)
                        .ok_or_else(|| invalid("body thermal changed cell count overflow"))?;
                }
                cell_updates.push((*index, energy, next_material, remap_epoch));
            }
            if body_energy_before != body_energy_after {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body-local thermal step did not conserve occupied-cell energy",
                ));
            }
            let mut report = report;
            report.total_energy = body_energy_before;
            let mut next_edges = BTreeMap::new();
            for (index, edge) in output_edges.into_iter().enumerate() {
                if edge == super::ThermalEdgeState::default() {
                    continue;
                }
                let x = index % width;
                let y = index / width;
                let has_x_edge = edge.residual_x != 0 || edge.equilibrium_credit_x != 0;
                let has_y_edge = edge.residual_y != 0 || edge.equilibrium_credit_y != 0;
                if (has_x_edge && (x + 1 >= width || !active[index] || !active[index + 1]))
                    || (has_y_edge && (y + 1 >= height || !active[index] || !active[index + width]))
                {
                    return Err(invalid("body thermal output edge endpoints"));
                }
                next_edges.insert(index as u32, edge);
            }
            let removed_or_changed_edges = grid
                .thermal_edges
                .iter()
                .filter(|(index, edge)| next_edges.get(index) != Some(*edge))
                .count();
            let added_edges = next_edges
                .keys()
                .filter(|index| !grid.thermal_edges.contains_key(index))
                .count();
            let edge_changes = removed_or_changed_edges
                .checked_add(added_edges)
                .ok_or_else(|| invalid("body thermal edge change count overflow"))?;
            let writes = changed_cells
                .checked_add(edge_changes)
                .ok_or_else(|| invalid("body thermal write count overflow"))?;
            if writes == 0 {
                return Ok(report);
            }
            editor.reserve_writes(writes)?;
            let physics = editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| invalid("BodyPhysics disappeared during thermal edit"))?;
            let next_grid = Arc::make_mut(&mut physics.local_grids)
                .get_mut(&body)
                .ok_or_else(|| invalid("body local grid disappeared during thermal edit"))?;
            let (_, material_changed) = next_grid.apply_thermal_step(&cell_updates, next_edges)?;
            if material_changed {
                physics.local_grid_content_dirty.insert(body);
                physics.validate_content(&content, editor.world.limits)?;
                physics.sync_material_mass_properties(&content)?;
            }
            Ok(report)
        })
    }

    /// BodyLocalGridの露出面とWorld energy channelの熱を一tick交換します。
    ///
    /// BodyStencilの回転境界交差長で面のcoverageを求め、同じ整数energy量をbodyとWorldへ
    /// 反対符号で適用します。面端数はbody-local gridへ保存されます。World境界は断熱、
    /// 未読込regionやenergy channel coverage不足は候補全体を失敗させます。body内部の熱伝導
    /// も同じ候補で進める場合は、このmethodと`step_body_local_thermal`を同じWorld::edit内で
    /// 呼び出してください。
    #[allow(clippy::too_many_arguments)]
    pub fn step_body_world_thermal_boundary(
        &mut self,
        body: BodyId,
        layer: LayerId,
        energy_channel: StableKey,
        family: &super::EnthalpyFamily,
        config: super::ThermalStepConfig,
        world_units_per_cell: f64,
    ) -> EngineResult<super::ThermalStepReport> {
        self.step_body_world_thermal_boundary_inner(
            body,
            layer,
            energy_channel,
            family,
            config,
            world_units_per_cell,
            None,
        )
    }

    /// 作者定義の形状境界を標準の保存則付き熱交換へ接続します。
    ///
    /// providerはBodyLocalGridの各接触面とWorld cellのcoverageを返します。World/physicsの変更は
    /// 標準APIと同じWorldEdit候補へ積み、未読込cell、無効なlocal cell、過大な面長、容量不足、
    /// phase/mass不整合のいずれかで全てrollbackします。geometry計算はstateless・決定的にし、
    /// 動的なshape状態はprovider callbackの外でStateStore/codecを通じて管理してください。
    #[allow(clippy::too_many_arguments)]
    pub fn step_body_world_thermal_boundary_with_provider(
        &mut self,
        body: BodyId,
        layer: LayerId,
        energy_channel: StableKey,
        family: &super::EnthalpyFamily,
        config: super::ThermalStepConfig,
        world_units_per_cell: f64,
        provider: &dyn BodyThermalBoundaryProvider,
    ) -> EngineResult<super::ThermalStepReport> {
        self.step_body_world_thermal_boundary_inner(
            body,
            layer,
            energy_channel,
            family,
            config,
            world_units_per_cell,
            Some(provider),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn step_body_world_thermal_boundary_inner(
        &mut self,
        body: BodyId,
        layer: LayerId,
        energy_channel: StableKey,
        family: &super::EnthalpyFamily,
        config: super::ThermalStepConfig,
        world_units_per_cell: f64,
        provider: Option<&dyn BodyThermalBoundaryProvider>,
    ) -> EngineResult<super::ThermalStepReport> {
        self.run(|editor| {
            const FLUX_SCALE: i128 = 1_i128 << 32;
            const COVERAGE_SCALE: i128 = 1_i128 << 16;
            if config.conductance_denominator == 0
                || !world_units_per_cell.is_finite()
                || world_units_per_cell <= 0.0
            {
                return Err(invalid("body/world thermal step settings"));
            }
            let layer_def = editor.world.content.layer(layer)?;
            if !layer_def.simulated {
                return Err(invalid("body/world thermal coupling requires a simulated layer"));
            }
            let channel_layout = editor
                .world
                .channel_layout::<super::ThermalCellState>(energy_channel)?;
            if channel_layout.origin != (CellPoint { x: 0, y: 0 })
                || channel_layout.x_scale.ratio() != (1, 1)
                || channel_layout.y_scale.ratio() != (1, 1)
                || editor
                    .world
                    .channel_material_layer::<super::ThermalCellState>(energy_channel)?
                    != Some(layer_def.key)
            {
                return Err(invalid("body/world thermal channel must be a 1:1 layer channel"));
            }
            let physics = editor
                .world
                .body_physics
                .as_ref()
                .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "BodyPhysics未初期化"))?;
            let grid = physics.local_grid(body)?.ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
            })?;
            let state = physics.state(body)?;
            let mut contacts = match provider {
                Some(provider) => provider.boundary_contacts(
                    body,
                    grid,
                    state.pose,
                    world_units_per_cell,
                    editor.world.limits.job_cells,
                )?,
                None => grid.stencil()?.boundary_contacts(
                    state.pose,
                    world_units_per_cell,
                    editor.world.limits.job_cells,
                )?,
            };
            contacts.sort_unstable_by_key(|contact| {
                (
                    contact.local_cell[1],
                    contact.local_cell[0],
                    contact.face,
                    contact.world_cell.y,
                    contact.world_cell.x,
                )
            });
            if contacts.len() > editor.world.limits.job_cells {
                return Err(EngineError::capacity(
                    "body/world thermal contacts",
                    contacts.len() as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            for contact in &contacts {
                let Some(index) = grid.index(contact.local_cell[0], contact.local_cell[1]) else {
                    return Err(invalid("body/world thermal provider local coordinate"));
                };
                if !grid.cells.contains_key(&index)
                    || !contact.covered_length.is_finite()
                    || contact.covered_length <= 0.0
                    || contact.covered_length > grid.cell_size()
                    || !contact.world_point.iter().all(|value| value.is_finite())
                {
                    return Err(invalid("body/world thermal provider contact geometry"));
                }
            }
            let content = Arc::clone(&editor.world.content);
            let remap_epoch = editor.world.remaps.epoch();
            let mut body_deltas = BTreeMap::<u32, i128>::new();
            let mut world_deltas = BTreeMap::<(i32, i32), i128>::new();
            let mut boundary_states = BTreeMap::<
                (u32, BodyThermalBoundaryFace),
                BodyThermalBoundaryEdgeState,
            >::new();
            let mut face_coverage = BTreeMap::<(u32, BodyThermalBoundaryFace), u32>::new();
            let mut report = super::ThermalStepReport::default();
            for contact in contacts {
                let world_cell = match editor.world.cell(layer, contact.world_cell)? {
                    CellQuery::Loaded(cell) => cell,
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "body/world thermal boundary touches an unavailable cell",
                        ));
                    }
                    CellQuery::WorldBoundary => continue,
                };
                if world_cell.core.material == MaterialId::AIR {
                    continue;
                }
                let chunk = ChunkCoord {
                    x: contact.world_cell.x.div_euclid(64),
                    y: contact.world_cell.y.div_euclid(64),
                };
                if !editor
                    .world
                    .chunks
                    .get(&chunk)
                    .is_some_and(|chunk| chunk.active)
                {
                    continue;
                }
                let world_thermal = match editor
                    .world
                    .channel_value::<super::ThermalCellState>(energy_channel, contact.world_cell)?
                {
                    ChannelQuery::Loaded(value) => value,
                    ChannelQuery::Unavailable(_) | ChannelQuery::WorldBoundary => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "body/world thermal energy channel coverage is incomplete",
                        ));
                    }
                };
                let body_index = grid
                    .index(contact.local_cell[0], contact.local_cell[1])
                    .ok_or_else(|| invalid("body/world thermal local coordinate"))?;
                let body_cell = grid
                    .cells
                    .get(&body_index)
                    .ok_or_else(|| invalid("body/world thermal contact cell is empty"))?;
                if body_cell.layer()? != layer_def.key {
                    continue;
                }
                let body_energy = body_cell.energy();
                let body_material = body_cell.material()?;
                let world_material = content
                    .material(world_cell.core.material)?
                    .ok_or_else(|| invalid("body/world thermal material definition"))?
                    .key();
                let (body_phase, body_band) = family.state_and_band(body_energy)?;
                let (world_phase, world_band) = family.state_and_band(world_thermal.energy)?;
                if !family.contains_material(body_material)
                    || (body_phase.from != body_material && body_phase.to != body_material)
                    || (!family.contains_material(world_material)
                        || (world_phase.from != world_material && world_phase.to != world_material))
                {
                    return Err(invalid("body/world material and enthalpy phase disagree"));
                }
                let face_key = (body_index, contact.face);
                let coverage_q16 = (contact.covered_length / grid.cell_size()
                    * COVERAGE_SCALE as f64)
                    .round();
                if !coverage_q16.is_finite() || coverage_q16 < 0.0 {
                    return Err(invalid("body/world thermal face coverage"));
                }
                let coverage_q16 = u32::try_from(coverage_q16 as u64)
                    .map_err(|_| invalid("body/world thermal coverage conversion"))?;
                let covered = face_coverage.entry(face_key).or_default();
                let remaining = 65536_u32.saturating_sub(*covered);
                let coverage_q16 = coverage_q16.min(remaining);
                *covered = covered
                    .checked_add(coverage_q16)
                    .ok_or_else(|| invalid("body/world thermal coverage sum"))?;
                if coverage_q16 == 0 {
                    continue;
                }
                let temperature_delta = i128::from(body_phase.temperature_mk)
                    .checked_sub(i128::from(world_phase.temperature_mk))
                    .ok_or_else(|| invalid("body/world thermal temperature delta"))?;
                if temperature_delta == 0 {
                    continue;
                }
                let (hot_energy, cold_energy, hot_lookup, cold_lookup) =
                    if temperature_delta > 0 {
                        (
                            body_energy,
                            world_thermal.energy,
                            super::ThermalLookup {
                                temperature_mk: body_phase.temperature_mk,
                                band_index: body_band,
                            },
                            super::ThermalLookup {
                                temperature_mk: world_phase.temperature_mk,
                                band_index: world_band,
                            },
                        )
                    } else {
                        (
                            world_thermal.energy,
                            body_energy,
                            super::ThermalLookup {
                                temperature_mk: world_phase.temperature_mk,
                                band_index: world_band,
                            },
                            super::ThermalLookup {
                                temperature_mk: body_phase.temperature_mk,
                                band_index: body_band,
                            },
                        )
                    };
                let equilibrium = family.equilibrium_transfer(
                    hot_energy,
                    cold_energy,
                    hot_lookup,
                    cold_lookup,
                )?;
                let coverage = i128::from(coverage_q16);
                let weighted_equilibrium = i128::from(equilibrium)
                    .checked_mul(coverage)
                    .and_then(|value| value.checked_add(COVERAGE_SCALE - 1))
                    .ok_or_else(|| invalid("body/world thermal equilibrium coverage"))?
                    / COVERAGE_SCALE;
                let previous = boundary_states.get(&face_key).copied().unwrap_or_else(|| {
                    grid.thermal_boundary_edge_state(
                        contact.local_cell[0],
                        contact.local_cell[1],
                        contact.face,
                    )
                    .unwrap_or_default()
                });
                if previous.residual <= -i64::try_from(FLUX_SCALE).unwrap()
                    || previous.residual >= i64::try_from(FLUX_SCALE).unwrap()
                {
                    return Err(invalid("body/world thermal Q32 residual range"));
                }
                let credit_units = weighted_equilibrium
                    .checked_add(i128::from(previous.equilibrium_credit))
                    .ok_or_else(|| invalid("body/world thermal equilibrium credit overflow"))?;
                let equilibrium_cap = i64::try_from(credit_units / 8)
                    .map_err(|_| invalid("body/world thermal equilibrium cap"))?;
                let next_credit = u8::try_from(credit_units % 8)
                    .map_err(|_| invalid("body/world thermal equilibrium credit"))?;
                let denominator = i128::from(config.conductance_denominator)
                    .checked_mul(COVERAGE_SCALE)
                    .ok_or_else(|| invalid("body/world thermal conductance denominator"))?;
                let scaled_numerator = temperature_delta
                    .checked_mul(i128::from(config.conductance_numerator))
                    .and_then(|value| value.checked_mul(coverage))
                    .and_then(|value| value.checked_mul(FLUX_SCALE))
                    .ok_or_else(|| invalid("body/world thermal flux numerator overflow"))?;
                let pending = scaled_numerator
                    .checked_div(denominator)
                    .and_then(|value| value.checked_add(i128::from(previous.residual)))
                    .ok_or_else(|| invalid("body/world thermal pending flux overflow"))?;
                let raw_flux = i64::try_from(pending / FLUX_SCALE)
                    .map_err(|_| invalid("body/world thermal flux range"))?;
                let amount = raw_flux.clamp(-equilibrium_cap, equilibrium_cap);
                let residual = if amount == raw_flux {
                    i64::try_from(pending % FLUX_SCALE)
                        .map_err(|_| invalid("body/world thermal Q32 residual conversion"))?
                } else {
                    0
                };
                let energy_delta = i128::from(amount)
                    .checked_neg()
                    .ok_or_else(|| invalid("body/world thermal body delta overflow"))?;
                let accumulated_body_delta = body_deltas.entry(body_index).or_default();
                *accumulated_body_delta = accumulated_body_delta
                    .checked_add(energy_delta)
                    .ok_or_else(|| invalid("body/world thermal accumulated body delta"))?;
                let world_delta = world_deltas
                    .entry((contact.world_cell.y, contact.world_cell.x))
                    .or_default();
                *world_delta = world_delta
                    .checked_add(i128::from(amount))
                    .ok_or_else(|| invalid("body/world thermal accumulated World delta"))?;
                boundary_states.insert(
                    face_key,
                    BodyThermalBoundaryEdgeState {
                        residual,
                        equilibrium_credit: next_credit,
                    },
                );
                report.edges = report
                    .edges
                    .checked_add(1)
                    .ok_or_else(|| invalid("body/world thermal edge count overflow"))?;
            }

            let mut body_updates = Vec::new();
            body_updates
                .try_reserve_exact(body_deltas.len())
                .map_err(allocation)?;
            let mut world_updates = Vec::new();
            world_updates
                .try_reserve_exact(world_deltas.len())
                .map_err(allocation)?;
            let mut total_before = 0_i128;
            let mut total_after = 0_i128;
            let mut changed_body_cells = 0usize;
            let mut changed_world_cells = 0usize;
            let mut changed_world_materials = 0usize;
            for (index, delta) in &body_deltas {
                let cell = grid
                    .cells
                    .get(index)
                    .ok_or_else(|| invalid("body/world thermal body cell disappeared"))?;
                let before = cell.energy();
                let after = i64::try_from(
                    i128::from(before)
                        .checked_add(*delta)
                        .ok_or_else(|| invalid("body/world thermal body energy overflow"))?,
                )
                .map_err(|_| invalid("body/world thermal body energy range"))?;
                let current_material = cell.material()?;
                let next_material = family.material_at_energy(after)?;
                let next_material_id = content.material_id(MaterialHandle(next_material))?;
                if next_material_id == MaterialId::AIR {
                    return Err(invalid("body/world thermal body phase became air"));
                }
                if next_material != current_material {
                    if cell.block_attachment.is_some() || !cell.mixture().is_empty() {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body/world thermal phase transition requires a plain unattached carrier",
                        ));
                    }
                    let before_def = content
                        .material(content.material_id(MaterialHandle(current_material))?)?
                        .ok_or_else(|| invalid("body/world thermal body source definition"))?;
                    let after_def = content
                        .material(next_material_id)?
                        .ok_or_else(|| invalid("body/world thermal body target definition"))?;
                    if before_def.mass_per_cell() != after_def.mass_per_cell() {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body/world thermal phase transition changes material mass",
                        ));
                    }
                }
                let mut candidate = cell.clone();
                candidate.material = next_material.as_str().to_owned();
                candidate.remap_epoch = remap_epoch;
                candidate.energy = after;
                validate_body_cell_content(&candidate, &content)?;
                total_before = total_before
                    .checked_add(i128::from(before))
                    .ok_or_else(|| invalid("body/world thermal total input"))?;
                total_after = total_after
                    .checked_add(i128::from(after))
                    .ok_or_else(|| invalid("body/world thermal total output"))?;
                if before != after || current_material != next_material {
                    changed_body_cells += 1;
                }
                body_updates.push((*index, after, next_material, remap_epoch));
            }
            for ((y, x), delta) in &world_deltas {
                let point = CellPoint { x: *x, y: *y };
                let cell = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(cell) => cell,
                    _ => return Err(invalid("body/world thermal World cell changed during candidate")),
                };
                let before_state = match editor
                    .world
                    .channel_value::<super::ThermalCellState>(energy_channel, point)?
                {
                    ChannelQuery::Loaded(state) => state,
                    _ => return Err(invalid("body/world thermal energy state disappeared")),
                };
                let after_energy = i64::try_from(
                    i128::from(before_state.energy)
                        .checked_add(*delta)
                        .ok_or_else(|| invalid("body/world thermal World energy overflow"))?,
                )
                .map_err(|_| invalid("body/world thermal World energy range"))?;
                let current_material = content
                    .material(cell.core.material)?
                    .ok_or_else(|| invalid("body/world thermal World material definition"))?;
                let thermal_state = family.state(after_energy)?;
                if !family.contains_material(current_material.key())
                    || (thermal_state.from != current_material.key()
                        && thermal_state.to != current_material.key())
                {
                    return Err(invalid("body/world thermal World material and enthalpy phase disagree"));
                }
                let next_material = family.material_at_energy(after_energy)?;
                let next_material_id = content.material_id(MaterialHandle(next_material))?;
                if next_material_id == MaterialId::AIR {
                    return Err(invalid("body/world thermal World phase became air"));
                }
                if next_material_id != cell.core.material {
                    if cell.block.is_some() {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body/world thermal phase transition requires a terrain migration rule",
                        ));
                    }
                    let target = content
                        .material(next_material_id)?
                        .ok_or_else(|| invalid("body/world thermal World target definition"))?;
                    if target.mass_per_cell() != current_material.mass_per_cell() {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "body/world thermal phase transition changes World material mass",
                        ));
                    }
                    changed_world_materials += 1;
                }
                total_before = total_before
                    .checked_add(i128::from(before_state.energy))
                    .ok_or_else(|| invalid("body/world thermal total input"))?;
                total_after = total_after
                    .checked_add(i128::from(after_energy))
                    .ok_or_else(|| invalid("body/world thermal total output"))?;
                if before_state.energy != after_energy {
                    changed_world_cells += 1;
                }
                world_updates.push((point, cell.core, before_state, after_energy, next_material_id));
            }
            let boundary_changes = boundary_states
                .iter()
                .filter(|((index, face), state)| {
                    let x = *index % grid.width;
                    let y = *index / grid.width;
                    grid.thermal_boundary_edge_state(x, y, *face) != Some(**state)
                })
                .count();
            let body_writes = changed_body_cells
                .checked_add(boundary_changes)
                .ok_or_else(|| invalid("body/world thermal body write count overflow"))?;
            let writes = body_writes
                .checked_add(changed_world_cells)
                .and_then(|value| value.checked_add(changed_world_materials))
                .ok_or_else(|| invalid("body/world thermal write count overflow"))?;
            if writes == 0 {
                return Ok(report);
            }
            // World setterは自身の変更を予約するため、ここではbody-local grid分だけ先行予約する。
            editor.reserve_writes(body_writes)?;
            for (point, before_core, before_state, after_energy, material) in world_updates {
                if material != before_core.material {
                    editor.set_cell(
                        layer,
                        point,
                        super::CellCore {
                            material,
                            ..before_core
                        },
                    )?;
                }
                if before_state.energy != after_energy {
                    editor.set_channel(
                        energy_channel,
                        point,
                        super::ThermalCellState {
                            energy: after_energy,
                        },
                    )?;
                }
            }
            let physics = editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| invalid("BodyPhysics disappeared during body/world thermal edit"))?;
            let next_grid = Arc::make_mut(&mut physics.local_grids)
                .get_mut(&body)
                .ok_or_else(|| invalid("body local grid disappeared during body/world thermal edit"))?;
            let next_grid_revision = if changed_body_cells > 0 {
                Some(
                    next_grid
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| invalid("body/world thermal grid revision overflow"))?,
                )
            } else {
                None
            };
            let mut material_changed = false;
            for (index, energy, material, epoch) in body_updates {
                let cell = next_grid
                    .cells
                    .get_mut(&index)
                    .ok_or_else(|| invalid("body thermal update cell disappeared"))?;
                material_changed |= cell.material()? != material;
                cell.material = material.as_str().to_owned();
                cell.remap_epoch = epoch;
                cell.energy = energy;
            }
            for ((index, face), state) in boundary_states {
                let x = index % next_grid.width;
                let y = index / next_grid.width;
                next_grid.set_thermal_boundary_edge_state(x, y, face, state)?;
            }
            if let Some(revision) = next_grid_revision {
                next_grid.revision = revision;
            }
            if material_changed {
                physics.local_grid_content_dirty.insert(body);
                physics.validate_content(&content, editor.world.limits)?;
                physics.sync_material_mass_properties(&content)?;
            }
            if total_before != total_after {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "body/world thermal boundary step did not conserve combined energy",
                ));
            }
            report.total_energy = total_before;
            Ok(report)
        })
    }

    /// fluid重なり・境界面から素材Q16速度と剛体へ同じ量子化力積を反対向きに適用します。
    pub(super) fn apply_body_fluid_coupling(
        &mut self,
        layer: LayerId,
        world_units_per_cell: f64,
    ) -> EngineResult<usize> {
        self.run(|editor| {
            if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
                return Err(invalid("剛体流体連成cell寸法"));
            }
            let Some(physics) = editor.world.body_physics.as_ref() else {
                return Ok(0);
            };
            let settings = physics.settings;
            let contacts =
                physics.fluid_contact_samples(editor.world, layer, world_units_per_cell)?;
            editor.world.content.layer(layer)?;
            let content = Arc::clone(&editor.world.content);
            let mut motion_channel = None;
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(contacts.len())
                .map_err(allocation)?;
            for contact in contacts {
                let body = contact.body;
                let point = contact.point;
                let view = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(view) => view,
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "剛体fluid連成cellのchunkが未読込",
                        ));
                    }
                    CellQuery::WorldBoundary => {
                        return Err(invalid("剛体fluid連成cellがWorld境界外"));
                    }
                };
                if view.block.is_some() || view.core.material == MaterialId::AIR {
                    continue;
                }
                let material = content
                    .material(view.core.material)?
                    .ok_or_else(|| invalid("剛体fluid連成material"))?;
                if !matches!(
                    material.base(),
                    MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                ) {
                    continue;
                }
                let props = material.motion_properties();
                let buoyancy_active = props.buoyancy_scale_q16 != 0
                    && settings.gravity.iter().any(|component| *component != 0.0);
                if props.drag_q16 == 0 && !buoyancy_active {
                    continue;
                }
                let motion_key = match motion_channel {
                    Some(key) => key,
                    None => {
                        let key = body_material_motion_channel(editor.world, layer)?;
                        motion_channel = Some(key);
                        key
                    }
                };
                let motion = match editor
                    .world
                    .channel_value::<super::MaterialMotion>(motion_key, point)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "剛体fluid連成motion coverageが未準備",
                        ));
                    }
                };
                let state = physics.state(body)?;
                let offset = [
                    contact.surface_point[0] - state.pose.x,
                    contact.surface_point[1] - state.pose.y,
                ];
                let point_velocity = [
                    state.velocity.x - state.velocity.angular * offset[1],
                    state.velocity.y + state.velocity.angular * offset[0],
                ];
                let fluid_velocity = [
                    motion.velocity_q16[0] as f64
                        * world_units_per_cell
                        * f64::from(settings.step_hz)
                        / 65536.0,
                    motion.velocity_q16[1] as f64
                        * world_units_per_cell
                        * f64::from(settings.step_hz)
                        / 65536.0,
                ];
                let mass = f64::from(material.mass_per_cell());
                let drag = f64::from(props.drag_q16) / 65536.0;
                let buoyancy = f64::from(props.buoyancy_scale_q16) / 65536.0;
                let mut impulse = [
                    mass * (fluid_velocity[0] - point_velocity[0]) * drag
                        - mass * settings.gravity[0] * buoyancy / f64::from(settings.step_hz),
                    mass * (fluid_velocity[1] - point_velocity[1]) * drag
                        - mass * settings.gravity[1] * buoyancy / f64::from(settings.step_hz),
                ];
                impulse[0] *= contact.coverage;
                impulse[1] *= contact.coverage;
                let length = impulse[0].hypot(impulse[1]);
                if !length.is_finite() {
                    return Err(invalid("剛体流体力積"));
                }
                if length > settings.max_fluid_coupling_impulse {
                    let scale = settings.max_fluid_coupling_impulse / length;
                    impulse[0] *= scale;
                    impulse[1] *= scale;
                }
                if impulse != [0.0, 0.0] {
                    samples.push(BodyFluidCouplingSample {
                        body,
                        point,
                        reaction_point: contact.reaction_point,
                        body_impulse: impulse,
                    });
                }
            }
            for sample in &samples {
                let view = match editor.world.cell(layer, sample.point)? {
                    CellQuery::Loaded(view) => view,
                    _ => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "fluid連成cellがstep中に利用不能",
                        ));
                    }
                };
                let material = content
                    .material(view.core.material)?
                    .ok_or_else(|| invalid("fluid連成material"))?;
                let fluid_mass = f64::from(material.mass_per_cell());
                let delta = quantize_fluid_impulse(
                    sample.body_impulse,
                    fluid_mass,
                    world_units_per_cell,
                    settings.step_hz,
                )?;
                let actual = fluid_impulse_from_q16(
                    delta,
                    fluid_mass,
                    world_units_per_cell,
                    settings.step_hz,
                )?;
                let key = motion_channel.ok_or_else(|| invalid("fluid連成motion channel cache"))?;
                let mut motion = match editor
                    .world
                    .channel_value::<super::MaterialMotion>(key, sample.point)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "fluid連成motion coverageが未準備",
                        ));
                    }
                };
                for (axis, velocity_delta) in delta.into_iter().enumerate() {
                    motion.velocity_q16[axis] = motion.velocity_q16[axis]
                        .checked_add(velocity_delta)
                        .ok_or_else(|| invalid("fluid連成velocity overflow"))?;
                }
                editor.set_channel(key, sample.point, motion)?;
                editor
                    .world
                    .body_physics
                    .as_mut()
                    .ok_or_else(|| invalid("fluid連成physics消失"))?
                    .apply_impulse_at_world_point(
                        sample.body,
                        [-actual[0], -actual[1]],
                        sample.reaction_point,
                    )?;
            }
            Ok(samples.len())
        })
    }

    /// 作者CouplingProviderが流体接触一件ずつの保存則を満たす更新を返します。
    pub(super) fn apply_body_fluid_coupling_with_provider(
        &mut self,
        layer: LayerId,
        world_units_per_cell: f64,
        provider: &dyn CouplingProvider,
        read_world: &World,
        custom_contacts: Option<Vec<BodyFluidContactSample>>,
    ) -> EngineResult<usize> {
        self.run(|editor| {
            if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
                return Err(invalid("CouplingProviderのcell寸法"));
            }
            let mut physics = editor
                .world
                .body_physics
                .take()
                .ok_or_else(|| invalid("CouplingProviderのBodyPhysics候補"))?;
            let result = (|| {
                let settings = physics.settings;
                let contacts = match custom_contacts {
                    Some(mut samples) => {
                        if samples.len() > editor.world.limits.job_cells {
                            return Err(EngineError::capacity(
                                "CouplingProvider fluid contacts",
                                samples.len() as u64,
                                editor.world.limits.job_cells as u64,
                            ));
                        }
                        for sample in &samples {
                            physics.state(sample.body)?;
                            if !sample.coverage.is_finite()
                                || sample.coverage <= 0.0
                                || sample.coverage > 1.0
                                || sample
                                    .surface_point
                                    .iter()
                                    .chain(sample.reaction_point.iter())
                                    .any(|value| !value.is_finite())
                            {
                                return Err(invalid("CouplingProvider custom fluid contact"));
                            }
                        }
                        samples.sort_by(|left, right| {
                            left.body
                                .cmp(&right.body)
                                .then_with(|| left.fluid_cell.y.cmp(&right.fluid_cell.y))
                                .then_with(|| left.fluid_cell.x.cmp(&right.fluid_cell.x))
                                .then_with(|| {
                                    left.surface_point[0].total_cmp(&right.surface_point[0])
                                })
                                .then_with(|| {
                                    left.surface_point[1].total_cmp(&right.surface_point[1])
                                })
                                .then_with(|| {
                                    left.reaction_point[0].total_cmp(&right.reaction_point[0])
                                })
                                .then_with(|| {
                                    left.reaction_point[1].total_cmp(&right.reaction_point[1])
                                })
                        });
                        samples
                            .into_iter()
                            .map(|sample| BodyFluidContact {
                                body: sample.body,
                                point: sample.fluid_cell,
                                surface_measure: sample.coverage,
                                weighted_surface_x: sample.surface_point[0] * sample.coverage,
                                weighted_surface_y: sample.surface_point[1] * sample.coverage,
                                coverage: sample.coverage,
                                surface_point: sample.surface_point,
                                reaction_point: sample.reaction_point,
                            })
                            .collect()
                    }
                    None => {
                        physics.fluid_contact_samples(editor.world, layer, world_units_per_cell)?
                    }
                };
                let content = Arc::clone(&editor.world.content);
                let mut motion_channel = None;
                let mut applied = 0usize;
                for contact in contacts {
                    let view = match editor.world.cell(layer, contact.point)? {
                        CellQuery::Loaded(view) => view,
                        CellQuery::Unavailable(_) => {
                            return Err(EngineError::new(
                                ErrorCode::InvalidState,
                                "CouplingProviderの流体cellが未読込",
                            ));
                        }
                        CellQuery::WorldBoundary => {
                            return Err(invalid("CouplingProviderの流体cellがWorld境界外"));
                        }
                    };
                    if view.block.is_some() || view.core.material == MaterialId::AIR {
                        continue;
                    }
                    let material = content
                        .material(view.core.material)?
                        .ok_or_else(|| invalid("CouplingProviderのfluid material"))?;
                    if !matches!(
                        material.base(),
                        MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                    ) {
                        continue;
                    }
                    let key = match motion_channel {
                        Some(key) => key,
                        None => {
                            let key = body_material_motion_channel(editor.world, layer)?;
                            motion_channel = Some(key);
                            key
                        }
                    };
                    let motion = match editor
                        .world
                        .channel_value::<super::MaterialMotion>(key, contact.point)?
                    {
                        ChannelQuery::Loaded(value) => value,
                        _ => {
                            return Err(EngineError::new(
                                ErrorCode::InvalidState,
                                "CouplingProviderのMaterialMotion coverageがない",
                            ));
                        }
                    };
                    let body_state = physics.state(contact.body)?;
                    let body_motion_type = physics.motion_type(contact.body)?;
                    let response = provider.couple(
                        &mut physics,
                        read_world,
                        BodyCouplingContext {
                            body: contact.body,
                            fluid_cell: contact.point,
                            surface_point: contact.surface_point,
                            reaction_point: contact.reaction_point,
                            coverage: contact.coverage,
                            fluid_material: view.core.material,
                            fluid_mass: material.mass_per_cell(),
                            fluid_velocity_q16: motion.velocity_q16,
                            body_state,
                            body_motion_type,
                            world_units_per_cell,
                            fixed_dt_seconds: 1.0 / f64::from(settings.step_hz),
                        },
                    )?;
                    let body_length = response.body_impulse[0].hypot(response.body_impulse[1]);
                    let external_length =
                        response.external_impulse[0].hypot(response.external_impulse[1]);
                    let fluid_length = (response.fluid_velocity_delta_q16[0] as f64)
                        .hypot(response.fluid_velocity_delta_q16[1] as f64);
                    if !body_length.is_finite()
                        || !external_length.is_finite()
                        || !fluid_length.is_finite()
                        || response.body_impulse.iter().any(|value| !value.is_finite())
                        || response
                            .external_impulse
                            .iter()
                            .any(|value| !value.is_finite())
                        || body_length > settings.max_fluid_coupling_impulse
                        || external_length > settings.max_fluid_coupling_impulse
                    {
                        return Err(invalid("CouplingProviderの有限値または力積上限"));
                    }
                    let delta = response.fluid_velocity_delta_q16;
                    let actual_fluid_impulse = fluid_impulse_from_q16(
                        delta,
                        f64::from(material.mass_per_cell()),
                        world_units_per_cell,
                        settings.step_hz,
                    )?;
                    let balance = [
                        response.body_impulse[0] + actual_fluid_impulse[0]
                            - response.external_impulse[0],
                        response.body_impulse[1] + actual_fluid_impulse[1]
                            - response.external_impulse[1],
                    ];
                    let tolerance = 1.0e-9
                        * 1.0_f64
                            .max(body_length)
                            .max(external_length)
                            .max(actual_fluid_impulse[0].hypot(actual_fluid_impulse[1]));
                    if balance[0].hypot(balance[1]) > tolerance {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "CouplingProviderの離散運動量収支が一致しません",
                        ));
                    }
                    if body_motion_type != BodyMotionType::Dynamic
                        && response.body_impulse != [0.0, 0.0]
                    {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "dynamic以外のbodyへ連成impulseは適用できません",
                        ));
                    }
                    let mut next_motion = motion;
                    for axis in 0..2 {
                        next_motion.velocity_q16[axis] = next_motion.velocity_q16[axis]
                            .checked_add(response.fluid_velocity_delta_q16[axis])
                            .ok_or_else(|| invalid("CouplingProviderのQ16速度overflow"))?;
                    }
                    if response.fluid_velocity_delta_q16 != [0, 0] {
                        editor.set_channel(key, contact.point, next_motion)?;
                    }
                    if response.body_impulse != [0.0, 0.0] {
                        physics.apply_impulse_at_world_point(
                            contact.body,
                            response.body_impulse,
                            contact.reaction_point,
                        )?;
                    }
                    applied = applied
                        .checked_add(1)
                        .ok_or_else(|| invalid("CouplingProvider sample数overflow"))?;
                }
                Ok(applied)
            })();
            editor.world.body_physics = Some(physics);
            result
        })
    }

    /// physics候補poseと重なるfluid cellを空き先、particle pool、reservoirの順で予約します。
    pub(super) fn displace_body_fluid_overlaps(
        &mut self,
        layer: LayerId,
        world_units_per_cell: f64,
        custom_occupancy: Option<PreparedBodyFluidOccupancy>,
    ) -> EngineResult<usize> {
        self.run(|editor| {
            if !world_units_per_cell.is_finite() || world_units_per_cell <= 0.0 {
                return Err(invalid("剛体流体排除cell寸法"));
            }
            let Some(physics) = editor.world.body_physics.as_ref() else {
                return Ok(0);
            };
            let layer_key = editor.world.content.layer(layer)?.key;
            let content = Arc::clone(&editor.world.content);
            let mut occupied = BTreeSet::new();
            let mut fluids = Vec::new();
            if let Some(prepared) = custom_occupancy {
                fluids = prepared.overlaps;
                for sample in prepared.occupancy {
                    occupied.insert((sample.world_cell.y, sample.world_cell.x));
                }
            } else {
                let occupancy = physics.local_grid_world_occupancy()?;
                fluids
                    .try_reserve_exact(occupancy.len())
                    .map_err(allocation)?;
                for (body, point) in occupancy {
                    occupied.insert((point.y, point.x));
                    let view = match editor.world.cell(layer, point)? {
                        CellQuery::Loaded(view) => view,
                        CellQuery::Unavailable(_) => {
                            return Err(EngineError::new(
                                ErrorCode::InvalidState,
                                "剛体fluid occupancyに未読込cellがある",
                            ));
                        }
                        CellQuery::WorldBoundary => {
                            return Err(invalid("剛体fluid occupancyがWorld境界外"));
                        }
                    };
                    if view.block.is_some() || view.core.material == MaterialId::AIR {
                        continue;
                    }
                    let material = content
                        .material(view.core.material)?
                        .ok_or_else(|| invalid("剛体fluid occupancy material"))?;
                    if matches!(
                        material.base(),
                        MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                    ) {
                        fluids.push(BodyFluidOverlap {
                            body,
                            source: point,
                            core: view.core,
                        });
                    }
                }
            }
            let mut destinations = BTreeSet::new();
            let mut moves = Vec::new();
            let mut stranded = Vec::new();
            moves.try_reserve_exact(fluids.len()).map_err(allocation)?;
            stranded
                .try_reserve_exact(fluids.len())
                .map_err(allocation)?;
            for fluid in fluids {
                let mut destination = None;
                for (dx, dy) in [(0, 1), (-1, 0), (1, 0), (0, -1)] {
                    let Some(x) = fluid.source.x.checked_add(dx) else {
                        continue;
                    };
                    let Some(y) = fluid.source.y.checked_add(dy) else {
                        continue;
                    };
                    let candidate = CellPoint { x, y };
                    let key = (y, x);
                    if occupied.contains(&key) || destinations.contains(&key) {
                        continue;
                    }
                    match editor.world.cell(layer, candidate)? {
                        CellQuery::Loaded(view)
                            if view.block.is_none() && view.core.material == MaterialId::AIR =>
                        {
                            destination = Some(candidate);
                            destinations.insert(key);
                            break;
                        }
                        CellQuery::Loaded(_) | CellQuery::Unavailable(_) => (),
                        CellQuery::WorldBoundary => (),
                    }
                }
                if let Some(destination) = destination {
                    moves.push(BodyFluidMove {
                        source: fluid.source,
                        destination,
                        core: fluid.core,
                    });
                } else {
                    stranded.push(fluid);
                }
            }
            let moved_to_empty = move_body_fluid_cells(editor, layer, layer_key, &moves)?;
            let particle_sources = if stranded.is_empty() {
                BTreeSet::new()
            } else {
                emit_body_fluid_particles(
                    editor,
                    layer,
                    layer_key,
                    world_units_per_cell,
                    &stranded,
                )?
            };
            let mut reservoir_by_body = BTreeMap::<BodyId, Vec<CellPoint>>::new();
            for fluid in &stranded {
                if !particle_sources.contains(&(fluid.source.y, fluid.source.x)) {
                    reservoir_by_body
                        .entry(fluid.body)
                        .or_default()
                        .push(fluid.source);
                }
            }
            for (body, sources) in reservoir_by_body {
                let config = infer_body_cell_transfer_config(editor.world, layer, sources[0])?;
                editor.store_body_exclusion_reservoir_cells(layer, body, &sources, config)?;
            }

            // 排除で解放された領域も含め、原点→下→左→右→上の順で返却先を選びます。
            // 予約先はbody間で共有し、一つのWorld cellを複数reservoirへ割り当てません。
            let mut release_destinations = BTreeSet::new();
            let mut releases = BTreeMap::<BodyId, Vec<BodyExclusionReservoirReturn>>::new();
            if let Some(physics) = editor.world.body_physics.as_ref() {
                for (body, grid) in physics.local_grids.iter() {
                    for entry in grid.exclusion_reservoir() {
                        let origin = entry.origin();
                        for (dx, dy) in [(0, 0), (0, 1), (-1, 0), (1, 0), (0, -1)] {
                            let Some(x) = origin.x.checked_add(dx) else {
                                continue;
                            };
                            let Some(y) = origin.y.checked_add(dy) else {
                                continue;
                            };
                            let destination = CellPoint { x, y };
                            let key = (y, x);
                            if occupied.contains(&key) || release_destinations.contains(&key) {
                                continue;
                            }
                            if matches!(
                                editor.world.cell(layer, destination)?,
                                CellQuery::Loaded(view)
                                    if view.block.is_none() && view.core.material == MaterialId::AIR
                            ) {
                                release_destinations.insert(key);
                                releases.entry(*body).or_default().push(
                                    BodyExclusionReservoirReturn {
                                        origin,
                                        destination,
                                    },
                                );
                                break;
                            }
                        }
                    }
                }
            }
            let mut released = 0usize;
            for (body, returns) in releases {
                let config = infer_body_cell_return_config(editor.world, layer, returns[0].origin)?;
                released = released
                    .checked_add(
                        editor
                            .release_body_exclusion_reservoir_cells(layer, body, &returns, config)?
                            .cells,
                    )
                    .ok_or_else(|| invalid("reservoir release count overflow"))?;
            }
            moved_to_empty
                .checked_add(stranded.len())
                .and_then(|count| count.checked_add(released))
                .ok_or_else(|| invalid("fluid exclusion transaction count overflow"))
        })
    }

    /// body local gridの切断済みcomponentを複数のdynamic bodyへ一括破砕します。
    ///
    /// material、熱、sidecar、shape、質量特性をcomponentごとに保ち、各fragmentへ元bodyの
    /// poseと`v + ω×r`の初速度を割り当てます。occupied component上のjoint anchorは対応fragmentへ
    /// 移し、anchorが空隙またはgrid外にあるjointは結果で報告して除去します。未適用外力とトルクは
    /// fragment質量比・慣性比で分配します。各fragmentの容量検査とcommitが成功した候補だけをWorldへ公開します。
    pub fn fracture_body_local_grid(
        &mut self,
        body: crate::foundation::id::BodyId,
        provider: &dyn BodyLocalBondProvider,
    ) -> EngineResult<BodyFractureReport> {
        self.run(|editor| {
            let content = Arc::clone(&editor.world.content);
            editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
                .fracture_disconnected_local_grid(
                    body,
                    &content,
                    editor.world.limits,
                    provider,
                    None,
                )
        })
    }

    /// block個体maskが複数fragmentへ分かれる際の作者状態変換providerを指定して破砕します。
    pub fn fracture_body_local_grid_with_attachment_provider(
        &mut self,
        body: crate::foundation::id::BodyId,
        bond_provider: &dyn BodyLocalBondProvider,
        attachment_provider: &dyn BodyBlockAttachmentSplitProvider,
    ) -> EngineResult<BodyFractureReport> {
        self.run(|editor| {
            if attachment_provider.version() == 0 {
                return Err(invalid("block attachment split provider version"));
            }
            let content = Arc::clone(&editor.world.content);
            editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
                .fracture_disconnected_local_grid(
                    body,
                    &content,
                    editor.world.limits,
                    bond_provider,
                    Some(attachment_provider),
                )
        })
    }

    /// 粒子を同一WorldEdit候補内で着地させ、運動量を保って既存剛体へ再固定します。
    ///
    /// bodyのposeと各粒子のQ16位置・速度を固定stepのworld単位へ変換し、粒子化前のbodyと
    /// 粒子群の線形・角運動量を合算します。着地payload・熱・素材量・sidecarを通常の
    /// World→body batch経路へ渡し、変更後のbody重心・慣性に一致する速度と角速度を設定します。
    /// bodyはdynamicでなければなりません。particle pool・body容量・channel・数値検査の
    /// どの段階で失敗しても、粒子、cell、bodyを一括rollbackします。
    pub fn reattach_material_particles_to_body(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        reattachments: &[BodyParticleReattachment],
        world_units_per_cell: f64,
        config: BodyCellTransferConfig,
    ) -> EngineResult<BodyParticleReattachmentReport> {
        self.run(|editor| {
            if reattachments.is_empty()
                || !world_units_per_cell.is_finite()
                || world_units_per_cell <= 0.0
            {
                return Err(invalid("剛体粒子再固定batchまたはcell物理寸法"));
            }
            let (body_state, body_mass, step_hz) = {
                let physics = editor.world.body_physics.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
                let handle = *physics
                    .handles
                    .get(&body)
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "粒子再固定body"))?;
                let rigid_body =
                    physics.backend.bodies.get(handle).ok_or_else(|| {
                        EngineError::new(ErrorCode::StaleId, "粒子再固定Rapier body")
                    })?;
                if rigid_body.body_type() != RigidBodyType::Dynamic {
                    return Err(EngineError::new(
                        ErrorCode::UnsupportedCapability,
                        "粒子再固定で運動量を継承できるのはdynamic bodyのみです",
                    ));
                }
                (
                    physics.state(body)?,
                    physics.mass_properties(body)?,
                    physics.settings.step_hz,
                )
            };

            let mut ordered = Vec::new();
            ordered
                .try_reserve_exact(reattachments.len())
                .map_err(allocation)?;
            let mut target_particles = BTreeMap::new();
            let mut destinations = BTreeSet::new();
            for transfer in reattachments {
                if transfer.particle.world() != editor.world.id {
                    return Err(EngineError::new(ErrorCode::WrongScope, "粒子のWorld"));
                }
                let particle = editor.world.material_particle(transfer.particle)?;
                if particle.layer() != layer {
                    return Err(EngineError::new(
                        ErrorCode::WrongScope,
                        "再固定する粒子のmaterial layer",
                    ));
                }
                if particle.is_archived() {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "退避中の粒子は再固定できません",
                    ));
                }
                let point = particle.cell();
                let key = (point.y, point.x);
                if target_particles
                    .insert(key, transfer.particle.id())
                    .is_some()
                    || !destinations.insert(transfer.destination)
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "粒子再固定のsourceまたはbody destination重複",
                    ));
                }
                ordered.push((
                    *transfer,
                    point,
                    particle.position_q16(),
                    particle.velocity_q16(),
                ));
            }
            ordered.sort_unstable_by_key(|(_, point, _, _)| (point.y, point.x));

            // 選択外の粒子が同じcellを占有していないことをpool一回の走査で検証します。
            for (id, particle) in editor.world.particles.iter() {
                if particle.layer() != layer || particle.is_archived() {
                    continue;
                }
                let point = particle.cell();
                if let Some(expected) = target_particles.get(&(point.y, point.x))
                    && *expected != id
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "再固定先cellに別の物質粒子が存在",
                    ));
                }
            }

            let mut handles = Vec::new();
            handles
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            let mut cell_transfers = Vec::new();
            cell_transfers
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for (transfer, point, _, _) in &ordered {
                handles.push(transfer.particle);
                cell_transfers.push(BodyCellTransfer {
                    source: *point,
                    destination: transfer.destination,
                });
            }

            let dt_scale = world_units_per_cell * f64::from(step_hz) / 65_536.0;
            let position_scale = world_units_per_cell / 65_536.0;
            if !dt_scale.is_finite() || !position_scale.is_finite() {
                return Err(invalid("剛体粒子再固定のQ16単位変換"));
            }
            let old_center_offset =
                rotate_local_point(body_mass.center_of_mass, body_state.pose.angle);
            let old_center = Vector::new(body_state.pose.x, body_state.pose.y) + old_center_offset;
            let old_momentum =
                Vector::new(body_state.velocity.x, body_state.velocity.y) * body_mass.mass;
            let mut total_momentum = old_momentum;
            let mut total_angular_momentum = body_mass.angular_inertia
                * body_state.velocity.angular
                + old_center
                    .x
                    .mul_add(old_momentum.y, -old_center.y * old_momentum.x);

            for handle in handles {
                editor.land_uncontested_particle(handle)?;
            }

            for (_, point, position_q16, velocity_q16) in &ordered {
                let cell = transferred_body_cell(editor.world, layer, *point, config, false)?;
                let mass = body_local_cell_mass(&cell, &editor.world.content)?;
                let position = Vector::new(
                    position_q16[0] as f64 * position_scale,
                    position_q16[1] as f64 * position_scale,
                );
                let momentum = Vector::new(
                    velocity_q16[0] as f64 * dt_scale,
                    velocity_q16[1] as f64 * dt_scale,
                ) * mass;
                if !position.x.is_finite()
                    || !position.y.is_finite()
                    || !momentum.x.is_finite()
                    || !momentum.y.is_finite()
                {
                    return Err(invalid("剛体粒子再固定の運動量"));
                }
                total_momentum += momentum;
                total_angular_momentum += position.x.mul_add(momentum.y, -position.y * momentum.x);
            }

            editor.transfer_material_cells_to_body(layer, body, &cell_transfers, config)?;

            let physics =
                editor.world.body_physics.as_mut().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
            let next_mass = physics.mass_properties(body)?;
            let new_center = {
                let state = physics.state(body)?;
                Vector::new(state.pose.x, state.pose.y)
                    + rotate_local_point(next_mass.center_of_mass, state.pose.angle)
            };
            if next_mass.mass <= 0.0 || next_mass.angular_inertia <= 0.0 {
                return Err(invalid("剛体粒子再固定後の質量特性"));
            }
            let linear = total_momentum / next_mass.mass;
            let angular = (total_angular_momentum
                - new_center
                    .x
                    .mul_add(total_momentum.y, -new_center.y * total_momentum.x))
                / next_mass.angular_inertia;
            if !linear.x.is_finite() || !linear.y.is_finite() || !angular.is_finite() {
                return Err(invalid("剛体粒子再固定後の速度"));
            }
            physics.set_velocity(
                body,
                BodyVelocity {
                    x: linear.x,
                    y: linear.y,
                    angular,
                },
            )?;
            Ok(BodyParticleReattachmentReport {
                cells: ordered.len(),
            })
        })
    }

    /// 静止particleを既存bodyへ再固定する旧互換入口です。cell物理寸法にはlocal gridのcell sizeを使います。
    pub fn reattach_stationary_material_particles_to_body(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        reattachments: &[BodyParticleReattachment],
        config: BodyCellTransferConfig,
    ) -> EngineResult<BodyParticleReattachmentReport> {
        self.run(|editor| {
            let physics =
                editor.world.body_physics.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
            if physics.state(body)?.velocity != BodyVelocity::default() {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "互換の静止粒子再固定入口は静止bodyのみ受け付けます",
                ));
            }
            for transfer in reattachments {
                let particle = editor.world.material_particle(transfer.particle)?;
                if particle.velocity_q16() != [0, 0] {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "互換の静止粒子再固定入口は静止particleのみ受け付けます",
                    ));
                }
            }
            let cell_size = physics
                .local_grid(body)?
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "body local grid未設定"))?
                .cell_size();
            editor.reattach_material_particles_to_body(
                layer,
                body,
                reattachments,
                cell_size,
                config,
            )
        })
    }

    /// body local cellを物質粒子へ一括変換します。
    ///
    /// bodyから一度World cellへ戻した後、同じWorldEdit候補内で既存particle batch APIへ渡します。
    /// そのためMixtureStore、熱、material-following channel、pool容量とslot世代の規則を共有し、
    /// 中間cellが確定Worldへ公開されません。どちらかの段階で失敗すればbodyとWorld全体をrollbackします。
    pub fn transfer_body_cells_to_material_particles(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        transfers: &[BodyCellParticleTransfer],
        config: BodyCellReturnConfig,
    ) -> EngineResult<BodyCellParticleTransferReport> {
        self.run(|editor| {
            if transfers.is_empty() {
                return Err(invalid("空のbody→particle batch"));
            }
            let mut returns = Vec::new();
            returns
                .try_reserve_exact(transfers.len())
                .map_err(allocation)?;
            let mut emissions = Vec::new();
            emissions
                .try_reserve_exact(transfers.len())
                .map_err(allocation)?;
            for transfer in transfers {
                returns.push(BodyCellReturn {
                    source: transfer.source,
                    destination: transfer.destination,
                });
                emissions.push(super::MaterialParticleEmission {
                    point: transfer.destination,
                    velocity_q16: transfer.velocity_q16,
                });
            }
            let returned =
                editor.transfer_body_cells_to_material_cells(layer, body, &returns, config)?;
            let particles = editor.emit_material_particles_batch(layer, &emissions)?;
            Ok(BodyCellParticleTransferReport {
                cells: returned.cells,
                body_removed: returned.body_removed,
                particles,
            })
        })
    }

    /// damage ruleが選んだbody cellを粒子へ移し、残存gridを連結成分へ分割します。
    ///
    /// damage量・硬度・tool条件・粒子化閾値は作品側が判定し、選択済みcellと出力先を渡します。
    /// 物質・energy・sidecarは既存のbody→particle取引で保持します。BodyPhysics単位とWorld
    /// cell速度単位を変換し、残存bodyの重心速度・角速度を補正して、移動cellを含む線形・角運動量を
    /// 保ちます。未適用force/torqueは粒子へ持ち越す標準表現がないため、事前に明示的な積分を求めます。
    /// 粒子pool、fragment body容量、joint移設のどれかが失敗しても外側のWorldEdit全体をrollbackします。
    pub fn apply_body_damage_cells_to_particles(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        transfers: &[BodyCellParticleTransfer],
        world_units_per_cell: f64,
        return_config: BodyCellReturnConfig,
        bond_provider: &dyn BodyLocalBondProvider,
    ) -> EngineResult<BodyDamageReport> {
        self.apply_body_damage_cells_internal(
            layer,
            body,
            transfers,
            BodyDamageConfig {
                world_units_per_cell,
                return_config,
            },
            bond_provider,
            None,
        )
    }

    /// block attachmentの作者状態移行providerを指定して、damage cellを粒子化・破砕します。
    pub fn apply_body_damage_cells_to_particles_with_attachment_provider(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        transfers: &[BodyCellParticleTransfer],
        config: BodyDamageConfig,
        bond_provider: &dyn BodyLocalBondProvider,
        attachment_provider: &dyn BodyBlockAttachmentParticleizationProvider,
    ) -> EngineResult<BodyDamageReport> {
        self.apply_body_damage_cells_internal(
            layer,
            body,
            transfers,
            config,
            bond_provider,
            Some(attachment_provider),
        )
    }

    fn apply_body_damage_cells_internal(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        transfers: &[BodyCellParticleTransfer],
        config: BodyDamageConfig,
        bond_provider: &dyn BodyLocalBondProvider,
        attachment_provider: Option<&dyn BodyBlockAttachmentParticleizationProvider>,
    ) -> EngineResult<BodyDamageReport> {
        self.run(|editor| {
            if transfers.is_empty()
                || !config.world_units_per_cell.is_finite()
                || config.world_units_per_cell <= 0.0
            {
                return Err(invalid("body damage particle batchまたはWorld cell scale"));
            }
            let available_attachment_stamp = match attachment_provider {
                Some(provider) => {
                    let version = provider.version();
                    if version == 0 {
                        return Err(invalid("block particleization provider version"));
                    }
                    Some(BodyAttachmentProviderStamp {
                        key: provider.key(),
                        version,
                    })
                }
                None => None,
            };
            let attachment_migrated = prepare_body_attachment_particleization(
                editor,
                layer,
                body,
                transfers,
                config.return_config,
                attachment_provider,
            )?;
            let attachment_stamp = if attachment_migrated {
                available_attachment_stamp
            } else {
                None
            };
            let content = Arc::clone(&editor.world.content);
            let (old_state, old_mass, grid, step_hz, pending_force, pending_torque) = {
                let physics = editor.world.body_physics.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
                let state = physics.state(body)?;
                let properties = physics.mass_properties(body)?;
                let handle = *physics
                    .handles
                    .get(&body)
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
                let backend_body = physics
                    .backend
                    .bodies
                    .get(handle)
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "Rapier剛体handle"))?;
                if backend_body.body_type() != RigidBodyType::Dynamic {
                    return Err(EngineError::new(
                        ErrorCode::UnsupportedCapability,
                        "damage粒子化にはdynamic bodyが必要です",
                    ));
                }
                (
                    state,
                    properties,
                    physics.local_grid(body)?.cloned().ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
                    })?,
                    physics.settings.step_hz,
                    backend_body.user_force(),
                    backend_body.user_torque(),
                )
            };
            if pending_force != Vector::ZERO || pending_torque != 0.0 {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "damage粒子化の前に未適用force/torqueを積分してください",
                ));
            }

            let mut particle_momentum = Vector::ZERO;
            let mut particle_angular_momentum = 0.0;
            let (sin, cos) = old_state.pose.angle.sin_cos();
            let dt = 1.0 / f64::from(step_hz);
            for transfer in transfers {
                let cell = grid
                    .cell(transfer.source[0], transfer.source[1])
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::MissingKey, "damage source body cell")
                    })?;
                let mass = body_local_cell_mass(cell, &content)?;
                let velocity = Vector::new(
                    transfer.velocity_q16[0] as f64 * config.world_units_per_cell / (65536.0 * dt),
                    transfer.velocity_q16[1] as f64 * config.world_units_per_cell / (65536.0 * dt),
                );
                let position = Vector::new(
                    (f64::from(transfer.destination.x) + 0.5) * config.world_units_per_cell,
                    (f64::from(transfer.destination.y) + 0.5) * config.world_units_per_cell,
                );
                if !mass.is_finite() || !velocity.x.is_finite() || !velocity.y.is_finite() {
                    return Err(invalid("damage particle mass or velocity conversion"));
                }
                let momentum = velocity * mass;
                particle_momentum += momentum;
                particle_angular_momentum +=
                    position.x.mul_add(momentum.y, -position.y * momentum.x);
            }

            let old_com_offset = Vector::new(
                cos.mul_add(
                    old_mass.center_of_mass[0],
                    -sin * old_mass.center_of_mass[1],
                ),
                sin.mul_add(old_mass.center_of_mass[0], cos * old_mass.center_of_mass[1]),
            );
            let old_com = Vector::new(old_state.pose.x, old_state.pose.y) + old_com_offset;
            let old_momentum =
                Vector::new(old_state.velocity.x, old_state.velocity.y) * old_mass.mass;
            let old_angular_momentum = old_mass.angular_inertia * old_state.velocity.angular
                + old_com
                    .x
                    .mul_add(old_momentum.y, -old_com.y * old_momentum.x);

            let particle_transfer = editor.transfer_body_cells_to_material_particles(
                layer,
                body,
                transfers,
                config.return_config,
            )?;
            let fracture = if particle_transfer.body_removed {
                let linear_error = old_momentum - particle_momentum;
                let angular_error = old_angular_momentum - particle_angular_momentum;
                let linear_tolerance =
                    old_momentum.x.abs().max(old_momentum.y.abs()).max(1.0) * 1e-10;
                let angular_tolerance = old_angular_momentum.abs().max(1.0) * 1e-10;
                if linear_error.x.abs().max(linear_error.y.abs()) > linear_tolerance
                    || angular_error.abs() > angular_tolerance
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "最後のbody cell粒子化では全運動量を粒子へ引き継ぐ必要があります",
                    ));
                }
                None
            } else {
                let physics = editor.world.body_physics.as_mut().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
                let remaining = physics.mass_properties(body)?;
                if remaining.mass <= 0.0 || remaining.angular_inertia <= 0.0 {
                    return Err(invalid("remaining body mass properties"));
                }
                let remaining_momentum = old_momentum - particle_momentum;
                let velocity = BodyVelocity {
                    x: remaining_momentum.x / remaining.mass,
                    y: remaining_momentum.y / remaining.mass,
                    angular: 0.0,
                };
                let state = physics.state(body)?;
                let (sin, cos) = state.pose.angle.sin_cos();
                let com_offset = Vector::new(
                    cos.mul_add(
                        remaining.center_of_mass[0],
                        -sin * remaining.center_of_mass[1],
                    ),
                    sin.mul_add(
                        remaining.center_of_mass[0],
                        cos * remaining.center_of_mass[1],
                    ),
                );
                let com = Vector::new(state.pose.x, state.pose.y) + com_offset;
                let remaining_angular_momentum = old_angular_momentum - particle_angular_momentum;
                let angular = (remaining_angular_momentum
                    - com
                        .x
                        .mul_add(remaining_momentum.y, -com.y * remaining_momentum.x))
                    / remaining.angular_inertia;
                if !velocity.x.is_finite() || !velocity.y.is_finite() || !angular.is_finite() {
                    return Err(invalid("damage transfer momentum result"));
                }
                physics.set_velocity(
                    body,
                    BodyVelocity {
                        angular,
                        ..velocity
                    },
                )?;
                Some(editor.fracture_body_local_grid(body, bond_provider)?)
            };
            Ok(BodyDamageReport {
                particle_transfer,
                fracture,
                attachment_provider: attachment_stamp,
            })
        })
    }

    /// body local cellを一括してWorldへ戻し、material・energy・sidecar所有権を移します。
    ///
    /// destinationは読込済み空cellでなければなりません。純material cellは1token、混合cellは
    /// 元のcarrierと同じ1tokenで、成分比率をWorldの整数ratio形式へ正確に変換できる場合だけ
    /// 表現します。block attachmentは残存mask全体を同じbatchへ含め、戻し先が元maskと一致する
    /// 平行移動または90度回転である必要があります。部分復帰や非格子形状の復帰は作者状態を保って
    /// 拒否します。量・熱・channel・空き容量が合わない場合もbodyとWorldの候補をcommitしません。
    pub fn transfer_body_cells_to_material_cells(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        returns: &[BodyCellReturn],
        config: BodyCellReturnConfig,
    ) -> EngineResult<BodyCellReturnReport> {
        self.run(|editor| {
            if returns.is_empty() {
                return Err(invalid("空のWorld復帰batch"));
            }
            if config.mixture_channel.is_some() != config.mixture_schema.is_some() {
                return Err(invalid("World復帰にはmixture channelとschemaを同時指定"));
            }
            editor.reserve_writes(returns.len())?;

            let mut ordered = Vec::new();
            ordered
                .try_reserve_exact(returns.len())
                .map_err(allocation)?;
            ordered.extend_from_slice(returns);
            ordered.sort_unstable_by_key(|entry| {
                (
                    entry.source[1],
                    entry.source[0],
                    entry.destination.y,
                    entry.destination.x,
                )
            });
            if ordered.windows(2).any(|pair| {
                pair[0].source == pair[1].source || pair[0].destination == pair[1].destination
            }) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "World復帰sourceまたはdestination重複",
                ));
            }

            let content = Arc::clone(&editor.world.content);
            let layer_key = content.layer(layer)?.key;
            if let Some(key) = config.mixture_channel
                && editor.world.channel_material_layer::<MixtureRef>(key)? != Some(layer_key)
            {
                return Err(invalid("World復帰mixture channelの所属layer"));
            }
            if let Some(key) = config.thermal_channel
                && editor
                    .world
                    .channel_material_layer::<super::ThermalCellState>(key)?
                    != Some(layer_key)
            {
                return Err(invalid("World復帰thermal channelの所属layer"));
            }
            let mut grid = editor
                .world
                .body_physics
                .as_ref()
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
                .local_grid(body)?
                .cloned()
                .ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
                })?;
            let mut local_cells = Vec::new();
            let mut cores = Vec::new();
            let mut mixtures = Vec::new();
            let mut destinations = Vec::new();
            for entry in &ordered {
                let cell = grid
                    .cell(entry.source[0], entry.source[1])
                    .cloned()
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::MissingKey, "body local source cell")
                    })?;
                if cell.layer()? != layer_key || cell.tokens != super::MaterialQuantity::ONE {
                    return Err(invalid("World復帰cellのlayerまたはtoken量"));
                }
                let material_key = cell.material()?;
                let material = content.material_id(MaterialHandle(material_key))?;
                if material == MaterialId::AIR {
                    return Err(invalid("World復帰cellのcarrier material"));
                }
                let mixture = if cell.mixture.is_empty() {
                    None
                } else {
                    if config.mixture_channel.is_none() {
                        return Err(invalid("混合cellのWorld復帰にはMixtureStoreが必要"));
                    }
                    Some(body_mixture_state(&cell)?)
                };
                let destination = match editor.world.cell(layer, entry.destination)? {
                    CellQuery::Loaded(view)
                        if view.block.is_none() && view.core == CellCore::AIR =>
                    {
                        entry.destination
                    }
                    CellQuery::Loaded(_) => {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "World復帰先は空きcellではない",
                        ));
                    }
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "World復帰先chunkが未読込",
                        ));
                    }
                    CellQuery::WorldBoundary => return Err(invalid("World復帰先が境界外")),
                };
                for channel in &editor.world.channels {
                    if channel.material_layer() == Some(layer_key)
                        && !channel.material_cell_is_exact(destination)?
                    {
                        return Err(invalid("World復帰先とmaterial channelが一対一でない"));
                    }
                }
                if cell.energy != 0 && config.thermal_channel.is_none() {
                    return Err(invalid("熱energyのWorld復帰にはthermal channelが必要"));
                }
                for sidecar in &cell.sidecars {
                    let key = sidecar.key()?;
                    if Some(key) == config.mixture_channel || Some(key) == config.thermal_channel {
                        return Err(invalid("予約channelはbody sidecarに格納できない"));
                    }
                    content.validate_body_sidecar(
                        layer_key,
                        key,
                        sidecar.version(),
                        sidecar.payload(),
                    )?;
                    if !editor
                        .world
                        .channels
                        .iter()
                        .any(|channel| channel.key() == key)
                    {
                        return Err(EngineError::new(
                            ErrorCode::MissingKey,
                            "World復帰sidecar channel",
                        ));
                    }
                }
                local_cells.push(cell.clone());
                cores.push(CellCore {
                    material,
                    flags: cell.flags,
                    variation: cell.variation,
                });
                mixtures.push(mixture);
                destinations.push(destination);
            }

            let mut return_indexes = BTreeMap::new();
            for (return_index, entry) in ordered.iter().enumerate() {
                let local_index = entry.source[1]
                    .checked_mul(grid.width)
                    .and_then(|row| row.checked_add(entry.source[0]))
                    .ok_or_else(|| invalid("body return local index overflow"))?;
                if return_indexes.insert(local_index, return_index).is_some() {
                    return Err(conflict("body return source duplicate"));
                }
            }
            let mut attachment_ids = BTreeSet::new();
            for cell in &local_cells {
                if let Some(id) = cell.block_attachment {
                    attachment_ids.insert(id);
                }
            }
            let mut destination_block_ids = vec![None; ordered.len()];
            for attachment_id in &attachment_ids {
                let attachment = grid
                    .block_attachments
                    .get(attachment_id)
                    .ok_or_else(|| invalid("body block attachment missing"))?;
                let mut block_members = Vec::new();
                block_members
                    .try_reserve_exact(attachment.members.len())
                    .map_err(allocation)?;
                for (mask_index, local_index) in &attachment.members {
                    let return_index = *return_indexes.get(local_index).ok_or_else(|| {
                        conflict("block個体をWorldへ戻すには残存mask全体が必要です")
                    })?;
                    let cell = &local_cells[return_index];
                    if cell.block_attachment != Some(*attachment_id)
                        || cell.block_mask_index != Some(*mask_index)
                    {
                        return Err(invalid("block attachmentとbody cell対応の不一致"));
                    }
                    block_members.push((*mask_index, destinations[return_index]));
                }
                let mut decoder = Decoder::new(&attachment.codec, attachment.codec.len())?;
                let block = BlockInstance::decode(&mut decoder)?;
                decoder.finish()?;
                if block.layer != layer {
                    return Err(conflict("block個体の復帰layer不一致"));
                }
                let material = content
                    .material(block.material)?
                    .ok_or_else(|| invalid("block素材"))?;
                for (_, local_index) in &attachment.members {
                    let return_index = return_indexes[local_index];
                    if local_cells[return_index].material()? != material.key() {
                        return Err(conflict("block個体cellのmaterialが変更されています"));
                    }
                }
                let mut block =
                    place_block_instance_at_destinations(block, attachment.size, &block_members)?;
                block.connect_state(&content, editor.world.limits)?;
                let id = editor.world.blocks.insert(block)?;
                for (_, local_index) in &attachment.members {
                    let return_index = return_indexes[local_index];
                    destination_block_ids[return_index] = Some(id);
                }
            }

            if config.mixture_channel.is_some()
                && !editor
                    .world
                    .state
                    .records()
                    .any(|record| record.key == MixtureStore::key())
            {
                return Err(EngineError::new(ErrorCode::MissingKey, "MixtureStore"));
            }

            for channel in &mut editor.world.channels {
                if channel.material_layer() == Some(layer_key) {
                    channel.reset_material(layer_key, &destinations)?;
                }
            }
            for ((destination, core), block) in
                destinations.iter().zip(cores).zip(&destination_block_ids)
            {
                editor.write(layer, *destination, core, *block)?;
            }

            if let (Some(channel), Some(schema)) = (config.mixture_channel, config.mixture_schema) {
                for (destination, mixture) in destinations.iter().zip(mixtures) {
                    let reference = if let Some(mixture) = mixture {
                        editor.register_mixture(schema, &mixture)?
                    } else {
                        MixtureRef::NONE
                    };
                    if !reference.is_none() {
                        editor.set_channel(channel, *destination, reference)?;
                    }
                }
            }
            if let Some(channel) = config.thermal_channel {
                for (destination, cell) in destinations.iter().zip(&local_cells) {
                    if cell.energy != 0 {
                        editor.set_channel(
                            channel,
                            *destination,
                            super::ThermalCellState {
                                energy: cell.energy,
                            },
                        )?;
                    }
                }
            }

            for (destination, cell) in destinations.iter().zip(&local_cells) {
                for sidecar in &cell.sidecars {
                    let key = sidecar.key()?;
                    let index = editor
                        .world
                        .channels
                        .iter()
                        .position(|channel| channel.key() == key)
                        .ok_or_else(|| {
                            EngineError::new(ErrorCode::MissingKey, "sidecar channel")
                        })?;
                    let current_bytes = editor.world.channels[index].bytes();
                    let projected_bytes = editor.world.channels[index]
                        .payload_bytes(*destination, sidecar.payload())?;
                    let total_bytes = editor
                        .world
                        .channel_bytes()
                        .checked_sub(current_bytes)
                        .and_then(|bytes| bytes.checked_add(projected_bytes))
                        .ok_or_else(|| invalid("sidecar channel byte count overflow"))?;
                    if total_bytes > editor.world.limits.channel_bytes {
                        return Err(EngineError::capacity(
                            "channel byte数",
                            total_bytes as u64,
                            editor.world.limits.channel_bytes as u64,
                        ));
                    }
                    editor.reserve_writes(1)?;
                    editor.world.channels[index].put_payload(*destination, sidecar.payload())?;
                }
            }

            let physics =
                editor.world.body_physics.as_mut().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
            let will_empty_body = grid.cell_count() == ordered.len();
            if will_empty_body && !config.remove_empty_body {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "最後のbody cellを戻すにはremove_empty_bodyを明示してください",
                ));
            }
            let mut sources = Vec::new();
            sources
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            sources.extend(ordered.iter().map(|entry| entry.source));
            for attachment_id in &attachment_ids {
                let attachment = grid
                    .block_attachments
                    .remove(attachment_id)
                    .ok_or_else(|| invalid("body block attachment missing on removal"))?;
                for (_, local_index) in attachment.members {
                    let cell = grid
                        .cells
                        .get_mut(&local_index)
                        .ok_or_else(|| invalid("body block attachment cell missing on removal"))?;
                    cell.block_attachment = None;
                    cell.block_mask_index = None;
                }
            }
            grid.remove_cells_batch(&sources)?;
            if will_empty_body {
                let handle = physics
                    .handles
                    .get(&body)
                    .copied()
                    .ok_or_else(|| EngineError::new(ErrorCode::StaleId, "剛体ID"))?;
                let has_joint = physics.joint_handles.values().any(|joint_handle| {
                    physics
                        .backend
                        .impulse_joints
                        .get(*joint_handle)
                        .is_some_and(|joint| joint.body1() == handle || joint.body2() == handle)
                });
                if has_joint {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "body除去前に接続jointを明示処理してください",
                    ));
                }
                physics.remove(body)?;
            } else {
                physics.replace_local_grid(body, grid)?;
                physics.validate_content(&content, editor.world.limits)?;
                physics.sync_material_mass_properties(&content)?;
            }
            Ok(BodyCellReturnReport {
                cells: ordered.len(),
                body_removed: will_empty_body,
            })
        })
    }

    /// 一つの地形cellを、物質・熱・material channelごと剛体local gridへ移します。
    pub fn transfer_material_cell_to_body(
        &mut self,
        layer: LayerId,
        point: CellPoint,
        body: crate::foundation::id::BodyId,
        local: [u32; 2],
        config: BodyCellTransferConfig,
    ) -> EngineResult<()> {
        self.transfer_material_cells_to_body(
            layer,
            body,
            &[BodyCellTransfer {
                source: point,
                destination: local,
            }],
            config,
        )
    }

    /// 複数の地形cellを一度のbody grid再構築・WorldEditで剛体へ移します。
    ///
    /// sourceとdestinationの重複、非物質cell、完全選択でないblock個体、混合/熱状態の未対応、
    /// channel不整合、容量不足があれば候補全体を破棄します。block個体は残存mask全体を選び、
    /// 作者状態と元mask indexを保持するattachmentとして移します。MixtureStore参照は正確な
    /// 成分tokenへ展開し、指定されたenthalpyとmaterial-following payloadも移します。
    /// 位置固定channelはworld座標の状態なので移しません。複数cellの抽出時にこのbatch APIを
    /// 使うことで、body grid複製、Rapier候補、shape再構築を移送ごとに一度だけ行います。
    pub fn transfer_material_cells_to_body(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        transfers: &[BodyCellTransfer],
        config: BodyCellTransferConfig,
    ) -> EngineResult<()> {
        self.run(|editor| {
            if transfers.is_empty() {
                return Err(invalid("空の剛体cell移送"));
            }
            editor.reserve_writes(transfers.len())?;

            let mut ordered = Vec::new();
            ordered
                .try_reserve_exact(transfers.len())
                .map_err(allocation)?;
            ordered.extend_from_slice(transfers);
            ordered.sort_unstable_by_key(|transfer| {
                (
                    transfer.source.x,
                    transfer.source.y,
                    transfer.destination[0],
                    transfer.destination[1],
                )
            });
            if ordered.windows(2).any(|pair| {
                pair[0].source == pair[1].source || pair[0].destination == pair[1].destination
            }) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "剛体cell移送のsourceまたはdestination重複",
                ));
            }

            let content = Arc::clone(&editor.world.content);
            let local_grid_dimensions = editor
                .world
                .body_physics
                .as_ref()
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
                .local_grid(body)?
                .ok_or_else(|| invalid("body local grid未設定"))?;
            let local_grid_width = local_grid_dimensions.width;
            let local_grid_height = local_grid_dimensions.height;
            let mut block_members = BTreeMap::<crate::world::BlockId, Vec<(u32, u32)>>::new();
            let mut transfer_blocks = Vec::new();
            transfer_blocks
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for transfer in &ordered {
                let view = editor.loaded(layer, transfer.source)?;
                let Some(reference) = view.block else {
                    transfer_blocks.push(None);
                    continue;
                };
                if reference.world != editor.world.id {
                    return Err(EngineError::new(ErrorCode::WrongScope, "blockの所属World"));
                }
                let block = editor.world.blocks.get(reference.id)?;
                if block.layer != layer || block.dormant.is_some() {
                    return Err(conflict("移送対象blockのlayerまたはregion状態"));
                }
                let def = content
                    .material(block.material)?
                    .ok_or_else(|| invalid("block素材"))?;
                let size = def.block_size();
                let mask_index = block
                    .remaining
                    .iter()
                    .copied()
                    .find(|index| {
                        block
                            .point(*index, size)
                            .is_ok_and(|point| point == transfer.source)
                    })
                    .ok_or_else(|| invalid("block maskと地形cellの不整合"))?;
                if transfer.destination[0] >= local_grid_width
                    || transfer.destination[1] >= local_grid_height
                {
                    return Err(invalid("block body local destination"));
                }
                let local_index = transfer.destination[1]
                    .checked_mul(local_grid_width)
                    .and_then(|index| index.checked_add(transfer.destination[0]))
                    .ok_or_else(|| invalid("block body local index overflow"))?;
                let members = block_members.entry(reference.id).or_default();
                members.try_reserve(1).map_err(allocation)?;
                members.push((mask_index, local_index));
                transfer_blocks.push(Some(reference.id));
            }
            let mut block_attachments = Vec::new();
            block_attachments
                .try_reserve_exact(block_members.len())
                .map_err(allocation)?;
            for (id, members) in &block_members {
                let block = editor.world.blocks.get(*id)?;
                if members.len() != block.remaining.len()
                    || block
                        .remaining
                        .iter()
                        .any(|mask| !members.iter().any(|(member_mask, _)| member_mask == mask))
                {
                    return Err(conflict("block個体を物体へ移すには残存mask全体が必要です"));
                }
                let def = content
                    .material(block.material)?
                    .ok_or_else(|| invalid("block素材"))?;
                let mut encoder = Encoder::new(editor.world.limits.state_bytes);
                block.encode(&mut encoder)?;
                block_attachments.push((
                    def.block_size(),
                    super::terrain::copy_slice(members)?,
                    encoder.finish(),
                ));
            }
            let mut cells = Vec::new();
            cells.try_reserve_exact(ordered.len()).map_err(allocation)?;
            for (transfer, block_id) in ordered.iter().zip(&transfer_blocks) {
                cells.push(transferred_body_cell(
                    editor.world,
                    layer,
                    transfer.source,
                    config,
                    block_id.is_some(),
                )?);
            }

            let physics =
                editor.world.body_physics.as_mut().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
            let mut grid = physics.local_grid(body)?.cloned().ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
            })?;
            let mut body_cells = Vec::new();
            body_cells
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for (transfer, cell) in ordered.iter().zip(cells) {
                body_cells.push((transfer.destination[0], transfer.destination[1], cell));
            }
            grid.insert_cells_batch(body_cells)?;
            for (size, members, codec) in block_attachments {
                grid.attach_block_state(size, members, codec)?;
            }
            physics.replace_local_grid(body, grid)?;
            physics.validate_content(&content, editor.world.limits)?;
            physics.sync_material_mass_properties(&content)?;

            let mut points = Vec::new();
            points
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for transfer in &ordered {
                editor.write(layer, transfer.source, CellCore::AIR, None)?;
                points.push(transfer.source);
            }
            for id in block_members.keys().copied() {
                editor.world.blocks.remove(id)?;
            }
            let layer_key = content.layer(layer)?.key;
            for channel in &mut editor.world.channels {
                if channel.material_layer() == Some(layer_key) {
                    channel.reset_material(layer_key, &points)?;
                }
            }
            Ok(())
        })
    }

    /// body占有cellと重なった粉体・液体・気体を、同一WorldEditで排除reservoirへ移します。
    ///
    /// reservoir cellは衝突shapeや剛体massへ加えず、物質量・混合成分・energy・material sidecarを
    /// 保存します。cellがbodyの現在占有mask外にある、排除対象のfluid baseでない、またはreservoir・
    /// sidecar容量が足りない場合はWorld cellとbody stateのどちらも変更しません。
    pub fn store_body_exclusion_reservoir_cell(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        source: CellPoint,
        config: BodyCellTransferConfig,
    ) -> EngineResult<()> {
        self.store_body_exclusion_reservoir_cells(layer, body, &[source], config)
            .map(|_| ())
    }

    /// 複数の重なった流体cellをreservoirへ一括退避します。
    ///
    /// body occupancy、fluid種別、chunk状態、reservoir/sidecar容量を先に検証します。
    /// material channelのresetとWorld cellの空化も同じWorldEdit候補にまとめます。
    pub fn store_body_exclusion_reservoir_cells(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        sources: &[CellPoint],
        config: BodyCellTransferConfig,
    ) -> EngineResult<usize> {
        self.run(|editor| {
            if sources.is_empty() {
                return Err(invalid("空の排除reservoir batch"));
            }
            editor.reserve_writes(sources.len())?;
            let content = Arc::clone(&editor.world.content);
            let (state, grid) = {
                let physics = editor.world.body_physics.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
                (
                    physics.state(body)?,
                    physics.local_grid(body)?.cloned().ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
                    })?,
                )
            };
            let occupied = grid.stencil()?.world_cells(state.pose)?;
            let mut ordered_sources = Vec::new();
            ordered_sources
                .try_reserve_exact(sources.len())
                .map_err(allocation)?;
            ordered_sources.extend_from_slice(sources);
            ordered_sources.sort_unstable_by_key(|point| (point.y, point.x));
            if ordered_sources.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(conflict("排除reservoir source重複"));
            }
            for source in &ordered_sources {
                if occupied
                    .binary_search_by_key(&(source.y, source.x), |point| (point.y, point.x))
                    .is_err()
                {
                    return Err(conflict("排除対象cellはbody occupancyと重なっていません"));
                }
            }
            let mut entries = Vec::new();
            entries
                .try_reserve_exact(ordered_sources.len())
                .map_err(allocation)?;
            for source in &ordered_sources {
                let source_cell = match editor.world.cell(layer, *source)? {
                    CellQuery::Loaded(view)
                        if view.block.is_none() && view.core.material != MaterialId::AIR =>
                    {
                        view.core
                    }
                    CellQuery::Loaded(_) => {
                        return Err(conflict("body exclusion source is empty or owned by block"));
                    }
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "body exclusion source is unavailable",
                        ));
                    }
                    CellQuery::WorldBoundary => {
                        return Err(invalid("body exclusion source is outside World"));
                    }
                };
                let material = content
                    .material(source_cell.material)?
                    .ok_or_else(|| invalid("body exclusion material"))?;
                if !matches!(
                    material.base(),
                    MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                ) {
                    return Err(EngineError::new(
                        ErrorCode::UnsupportedCapability,
                        "排除reservoirは粉体・液体・気体cellに対応します",
                    ));
                }
                entries.push((
                    *source,
                    transferred_body_cell(editor.world, layer, *source, config, false)?,
                ));
            }
            let layer_key = content.layer(layer)?.key;
            editor
                .world
                .body_physics
                .as_mut()
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化"))?
                .store_exclusion_cells(body, entries, &content)?;
            for channel in &mut editor.world.channels {
                if channel.material_layer() == Some(layer_key) {
                    channel.reset_material(layer_key, &ordered_sources)?;
                }
            }
            for source in &ordered_sources {
                editor.write(layer, *source, CellCore::AIR, None)?;
            }
            Ok(ordered_sources.len())
        })
    }

    /// 排除reservoirの物質を空きWorld cellへ一括で戻します。
    ///
    /// 混合成分、energy、flags、variation、material sidecarをWorld channelへ復元します。
    /// bodyに占有されているcellや読込前のregionへは出力せず、全出力とreservoir消費のどれかが
    /// 失敗した場合は同一WorldEdit全体をrollbackします。
    pub fn release_body_exclusion_reservoir_cells(
        &mut self,
        layer: LayerId,
        body: crate::foundation::id::BodyId,
        returns: &[BodyExclusionReservoirReturn],
        config: BodyCellReturnConfig,
    ) -> EngineResult<BodyExclusionReleaseReport> {
        self.run(|editor| {
            if returns.is_empty()
                || config.mixture_channel.is_some() != config.mixture_schema.is_some()
            {
                return Err(invalid(
                    "body reservoir release batch or mixture configuration",
                ));
            }
            editor.reserve_writes(returns.len())?;
            let mut ordered = Vec::new();
            ordered
                .try_reserve_exact(returns.len())
                .map_err(allocation)?;
            ordered.extend_from_slice(returns);
            ordered.sort_unstable_by_key(|entry| {
                (
                    entry.origin.y,
                    entry.origin.x,
                    entry.destination.y,
                    entry.destination.x,
                )
            });
            if ordered.windows(2).any(|pair| {
                pair[0].origin == pair[1].origin || pair[0].destination == pair[1].destination
            }) {
                return Err(conflict(
                    "body reservoir release source or destination duplicate",
                ));
            }

            let content = Arc::clone(&editor.world.content);
            let layer_key = content.layer(layer)?.key;
            let (grid, pose) = {
                let physics = editor.world.body_physics.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
                (
                    physics.local_grid(body)?.cloned().ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidState, "body local grid未設定")
                    })?,
                    physics.state(body)?.pose,
                )
            };
            if let Some(key) = config.mixture_channel
                && editor.world.channel_material_layer::<MixtureRef>(key)? != Some(layer_key)
            {
                return Err(invalid("body reservoir mixture channel layer"));
            }
            if let Some(key) = config.thermal_channel
                && editor
                    .world
                    .channel_material_layer::<super::ThermalCellState>(key)?
                    != Some(layer_key)
            {
                return Err(invalid("body reservoir thermal channel layer"));
            }
            let occupied = grid.stencil()?.world_cells(pose)?;
            let mut cells = Vec::new();
            cells.try_reserve_exact(ordered.len()).map_err(allocation)?;
            let mut cores = Vec::new();
            cores.try_reserve_exact(ordered.len()).map_err(allocation)?;
            let mut mixtures = Vec::new();
            mixtures
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            let mut destinations = Vec::new();
            destinations
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            for entry in &ordered {
                let cell = grid
                    .exclusion_reservoir()
                    .binary_search_by_key(&(entry.origin.y, entry.origin.x), |stored| {
                        (stored.origin().y, stored.origin().x)
                    })
                    .ok()
                    .map(|index| grid.exclusion_reservoir()[index].cell().clone())
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::MissingKey, "body exclusion reservoir cell")
                    })?;
                if cell.layer()? != layer_key || cell.tokens != super::MaterialQuantity::ONE {
                    return Err(invalid("body reservoir cell layer or token quantity"));
                }
                let material_id = content.material_id(MaterialHandle(cell.material()?))?;
                let material = content
                    .material(material_id)?
                    .ok_or_else(|| invalid("body reservoir material"))?;
                if !matches!(
                    material.base(),
                    MaterialBase::Powder | MaterialBase::Liquid | MaterialBase::Gas
                ) {
                    return Err(invalid("body reservoir contains non-fluid material"));
                }
                let mixture = if cell.mixture.is_empty() {
                    None
                } else {
                    if config.mixture_channel.is_none() {
                        return Err(invalid(
                            "body reservoir mixture requires MixtureStore channel",
                        ));
                    }
                    Some(body_mixture_state(&cell)?)
                };
                let destination = entry.destination;
                if occupied
                    .binary_search_by_key(&(destination.y, destination.x), |point| {
                        (point.y, point.x)
                    })
                    .is_ok()
                {
                    return Err(conflict(
                        "body reservoir destination is occupied by its body",
                    ));
                }
                match editor.world.cell(layer, destination)? {
                    CellQuery::Loaded(view)
                        if view.block.is_none() && view.core == CellCore::AIR => {}
                    CellQuery::Loaded(_) => {
                        return Err(conflict("body reservoir destination is not empty"));
                    }
                    CellQuery::Unavailable(_) => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "body reservoir destination is unavailable",
                        ));
                    }
                    CellQuery::WorldBoundary => {
                        return Err(invalid("body reservoir destination is outside World"));
                    }
                }
                for channel in &editor.world.channels {
                    if channel.material_layer() == Some(layer_key)
                        && !channel.material_cell_is_exact(destination)?
                    {
                        return Err(invalid(
                            "body reservoir destination lacks exact channel coverage",
                        ));
                    }
                }
                if cell.energy != 0 && config.thermal_channel.is_none() {
                    return Err(invalid("body reservoir energy requires thermal channel"));
                }
                for sidecar in &cell.sidecars {
                    let key = sidecar.key()?;
                    if Some(key) == config.mixture_channel || Some(key) == config.thermal_channel {
                        return Err(invalid(
                            "reserved channel cannot be a body reservoir sidecar",
                        ));
                    }
                    content.validate_body_sidecar(
                        layer_key,
                        key,
                        sidecar.version(),
                        sidecar.payload(),
                    )?;
                    if !editor
                        .world
                        .channels
                        .iter()
                        .any(|channel| channel.key() == key)
                    {
                        return Err(EngineError::new(
                            ErrorCode::MissingKey,
                            "body reservoir sidecar channel",
                        ));
                    }
                }
                cores.push(CellCore {
                    material: material_id,
                    flags: cell.flags,
                    variation: cell.variation,
                });
                cells.push(cell);
                mixtures.push(mixture);
                destinations.push(destination);
            }
            if config.mixture_channel.is_some()
                && !editor
                    .world
                    .state
                    .records()
                    .any(|record| record.key == MixtureStore::key())
            {
                return Err(EngineError::new(ErrorCode::MissingKey, "MixtureStore"));
            }

            for channel in &mut editor.world.channels {
                if channel.material_layer() == Some(layer_key) {
                    channel.reset_material(layer_key, &destinations)?;
                }
            }
            for (destination, core) in destinations.iter().zip(cores) {
                editor.write(layer, *destination, core, None)?;
            }
            if let (Some(channel), Some(schema)) = (config.mixture_channel, config.mixture_schema) {
                for (destination, mixture) in destinations.iter().zip(mixtures) {
                    let reference = if let Some(mixture) = mixture {
                        editor.register_mixture(schema, &mixture)?
                    } else {
                        MixtureRef::NONE
                    };
                    if !reference.is_none() {
                        editor.set_channel(channel, *destination, reference)?;
                    }
                }
            }
            if let Some(channel) = config.thermal_channel {
                for (destination, cell) in destinations.iter().zip(&cells) {
                    if cell.energy != 0 {
                        editor.set_channel(
                            channel,
                            *destination,
                            super::ThermalCellState {
                                energy: cell.energy,
                            },
                        )?;
                    }
                }
            }
            for (destination, cell) in destinations.iter().zip(&cells) {
                for sidecar in &cell.sidecars {
                    let key = sidecar.key()?;
                    let index = editor
                        .world
                        .channels
                        .iter()
                        .position(|channel| channel.key() == key)
                        .ok_or_else(|| {
                            EngineError::new(
                                ErrorCode::MissingKey,
                                "body reservoir sidecar channel",
                            )
                        })?;
                    let current_bytes = editor.world.channels[index].bytes();
                    let projected_bytes = editor.world.channels[index]
                        .payload_bytes(*destination, sidecar.payload())?;
                    let total_bytes = editor
                        .world
                        .channel_bytes()
                        .checked_sub(current_bytes)
                        .and_then(|bytes| bytes.checked_add(projected_bytes))
                        .ok_or_else(|| invalid("body reservoir sidecar byte count overflow"))?;
                    if total_bytes > editor.world.limits.channel_bytes {
                        return Err(EngineError::capacity(
                            "channel byte数",
                            total_bytes as u64,
                            editor.world.limits.channel_bytes as u64,
                        ));
                    }
                    editor.reserve_writes(1)?;
                    editor.world.channels[index].put_payload(*destination, sidecar.payload())?;
                }
            }
            let physics =
                editor.world.body_physics.as_mut().ok_or_else(|| {
                    EngineError::new(ErrorCode::InvalidState, "BodyPhysics未初期化")
                })?;
            let mut origins = Vec::new();
            origins
                .try_reserve_exact(ordered.len())
                .map_err(allocation)?;
            origins.extend(ordered.iter().map(|entry| entry.origin));
            physics.take_exclusion_cells(body, &origins)?;
            Ok(BodyExclusionReleaseReport {
                cells: ordered.len(),
            })
        })
    }

    /// 連結成分探索recordをStateStoreへ作り、seedと入力revisionを初期化します。
    pub fn begin_body_component_search(
        &mut self,
        record: StableKey,
        schema: StableKey,
        layer: LayerId,
        seed: CellPoint,
        cell_limit: u32,
        provider: &dyn BodyBondProvider,
    ) -> EngineResult<()> {
        self.run(|editor| {
            if editor
                .world
                .state
                .records()
                .any(|entry| entry.key == record)
            {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "component Job recordは既に存在します",
                ));
            }
            let job = BodyComponentSearchJob::new(editor.world, layer, seed, cell_limit, provider)?;
            editor.set_state(record, schema, &job)
        })
    }

    /// 予算内で連結成分Jobを進め、frontier・revisionをWorldと同時保存します。
    pub fn step_body_component_search(
        &mut self,
        record: StableKey,
        schema: StableKey,
        budget: usize,
        provider: &dyn BodyBondProvider,
    ) -> EngineResult<BodyComponentSearchStatus> {
        self.run(|editor| {
            let mut job: BodyComponentSearchJob = editor.world.state(record)?;
            let status = job.step(editor.world, budget, provider)?;
            editor.set_state(record, schema, &job)?;
            Ok(status)
        })
    }

    /// Ready componentのrevisionを再照合し、成分cellとjob終端状態を原子的に公開します。
    pub fn transfer_body_component_job(
        &mut self,
        record: StableKey,
        schema: StableKey,
        body: crate::foundation::id::BodyId,
        destination_origin: [u32; 2],
        config: BodyCellTransferConfig,
    ) -> EngineResult<BodyComponentTransferOutcome> {
        self.run(|editor| {
            let mut job: BodyComponentSearchJob = editor.world.state(record)?;
            if job.world != editor.world.id() {
                return Err(EngineError::new(
                    ErrorCode::WrongScope,
                    "component Jobの所属World",
                ));
            }
            if job.status != BodyComponentSearchStatus::Ready {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "Readyでないcomponent Jobは移送できない",
                ));
            }
            if job.inputs_changed(editor.world) {
                job.status = BodyComponentSearchStatus::Invalidated;
                editor.set_state(record, schema, &job)?;
                return Ok(BodyComponentTransferOutcome::Invalidated);
            }
            if job.inputs_unavailable(editor.world) {
                job.status = BodyComponentSearchStatus::WaitingForRegion;
                editor.set_state(record, schema, &job)?;
                return Ok(BodyComponentTransferOutcome::WaitingForRegion);
            }
            let layer = editor.world.content.layer_id(job.layer)?;
            let transfers = job.transfers(destination_origin)?;
            let count = transfers.len();
            editor.transfer_material_cells_to_body(layer, body, &transfers, config)?;
            job.mark_transferred();
            editor.set_state(record, schema, &job)?;
            Ok(BodyComponentTransferOutcome::Transferred { cells: count })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{WorldBounds, WorldLimits};
    use super::*;
    use crate::foundation::id::WorldId;

    #[test]
    fn provider_contact_event_shape_requires_consistent_geometry_and_velocities() {
        let mut physics = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
        let body = physics
            .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))
            .unwrap();
        let valid = BodyContactEvent {
            first: BodyContactTarget::Body(body),
            second: BodyContactTarget::Terrain(ChunkCoord::new(0, 0).unwrap()),
            phase: BodyContactPhase::Enter,
            exit_reason: None,
            point: [1.0, 2.0],
            normal: [0.0, 1.0],
            impulse: [0.0, 0.5],
            first_pre_velocity: Some(BodyVelocity::default()),
            first_post_velocity: Some(BodyVelocity::default()),
            second_pre_velocity: None,
            second_post_velocity: None,
            substep: 1,
        };
        assert!(validate_contact_event_shape(&valid).is_ok());

        let invalid_normal = BodyContactEvent {
            normal: [0.0, 0.0],
            ..valid
        };
        assert!(validate_contact_event_shape(&invalid_normal).is_err());
        let missing_exit_reason = BodyContactEvent {
            phase: BodyContactPhase::Exit,
            ..valid
        };
        assert!(validate_contact_event_shape(&missing_exit_reason).is_err());
        let terrain_velocity = BodyContactEvent {
            second_pre_velocity: Some(BodyVelocity::default()),
            ..valid
        };
        assert!(validate_contact_event_shape(&terrain_velocity).is_err());
    }

    #[test]
    fn rotated_body_fluid_volume_samples_use_partial_cell_coverage() {
        let layer_key = StableKey::parse("test:terrain").unwrap();
        let stone_key = StableKey::parse("test:stone").unwrap();
        let water_key = StableKey::parse("test:water").unwrap();
        let mut registry = super::super::ContentRegistry::new(Default::default()).unwrap();
        registry
            .layer(super::super::LayerDef {
                key: layer_key,
                role: super::super::LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        registry
            .register(
                super::super::MaterialBuilder::new(stone_key.as_str(), MaterialBase::Solid)
                    .unwrap(),
            )
            .unwrap();
        registry
            .register(
                super::super::MaterialBuilder::new(water_key.as_str(), MaterialBase::Liquid)
                    .unwrap(),
            )
            .unwrap();
        let content = registry.freeze().unwrap();
        let layer = content.layer_id(layer_key).unwrap();
        let water = content.material_id(MaterialHandle(water_key)).unwrap();
        let mut world = World::new(
            WorldId::from_parts(0, 1).unwrap(),
            0,
            WorldBounds::expandable(),
            content,
            WorldLimits::default(),
        )
        .unwrap();
        world
            .edit(|edit| {
                edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                for y in 0..64 {
                    for x in 0..64 {
                        edit.set_cell(
                            layer,
                            CellPoint { x, y },
                            CellCore {
                                material: water,
                                flags: 0,
                                variation: 0,
                            },
                        )?;
                    }
                }
                Ok(())
            })
            .unwrap();
        world
            .configure_body_physics(BodyPhysicsSettings {
                gravity: [0.0, 0.0],
                ..BodyPhysicsSettings::default()
            })
            .unwrap();
        let mut grid = BodyLocalGrid::new(
            1,
            1,
            1.0,
            [0.5, 0.5],
            0,
            BodyLocalGridLimits {
                cells: 1,
                geometry_cells: 1,
                sidecar_bytes: 64,
            },
        )
        .unwrap();
        grid.set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
        let pose = BodyPose {
            x: 8.5,
            y: 8.5,
            angle: std::f64::consts::FRAC_PI_4,
        };
        let mut definition = BodyDef::dynamic(ColliderShape::Cuboid {
            half_extents: [0.5, 0.5],
        });
        definition.pose = pose;
        let body = world
            .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
            .unwrap();
        let physics = world.body_physics().unwrap();
        let stencil = physics
            .local_grid(body)
            .unwrap()
            .unwrap()
            .stencil()
            .unwrap();
        let occupancy = stencil
            .world_cells(pose)
            .unwrap()
            .into_iter()
            .map(|point| (point.y, point.x))
            .collect::<BTreeSet<_>>();
        let samples = physics.fluid_contact_samples(&world, layer, 1.0).unwrap();
        let volume = samples
            .iter()
            .filter(|sample| {
                sample.body == body && occupancy.contains(&(sample.point.y, sample.point.x))
            })
            .collect::<Vec<_>>();
        assert!(!volume.is_empty());
        assert!(volume.iter().any(|sample| sample.coverage < 1.0));
        assert!((volume.iter().map(|sample| sample.coverage).sum::<f64>() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn fracture_distributes_queued_force_and_torque_without_loss() {
        let mut registry = super::super::ContentRegistry::new(Default::default()).unwrap();
        registry
            .layer(super::super::LayerDef {
                key: StableKey::parse("test:terrain").unwrap(),
                role: super::super::LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        registry
            .register(
                super::super::MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap(),
            )
            .unwrap();
        let content = registry.freeze().unwrap();
        let mut physics = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let mut grid = BodyLocalGrid::new(
            3,
            1,
            1.0,
            [0.0, 0.0],
            0,
            BodyLocalGridLimits {
                cells: 2,
                geometry_cells: 3,
                sidecar_bytes: 64,
            },
        )
        .unwrap();
        for x in [0, 2] {
            grid.set_cell(
                x,
                0,
                BodyLocalCell::new(
                    StableKey::parse("test:terrain").unwrap(),
                    StableKey::parse("test:stone").unwrap(),
                    0,
                    0,
                    0,
                    Vec::new(),
                )
                .unwrap(),
            )
            .unwrap();
        }
        let id = physics
            .insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
            .unwrap();
        let handle = physics.handles[&id];
        physics
            .backend
            .bodies
            .get_mut(handle)
            .unwrap()
            .add_force(Vector::new(12.0, -6.0), true);
        physics
            .backend
            .bodies
            .get_mut(handle)
            .unwrap()
            .add_torque(4.0, true);

        let report = physics
            .fracture_disconnected_local_grid(
                id,
                &content,
                super::super::WorldLimits::default(),
                &SameMaterialBodyLocalBondProvider,
                None,
            )
            .unwrap();

        let mut force = Vector::ZERO;
        let mut torque = 0.0;
        let mut per_fragment = Vec::new();
        let fragments = report.fragments;
        for fragment in &fragments {
            let handle = physics.handles[fragment];
            let body = physics.backend.bodies.get(handle).unwrap();
            force += body.user_force();
            torque += body.user_torque();
            per_fragment.push((body.user_force(), body.user_torque()));
        }
        assert_eq!(per_fragment.len(), 2);
        for (fragment_force, fragment_torque) in per_fragment {
            assert!((fragment_force.x - 6.0).abs() < 1e-12);
            assert!((fragment_force.y + 3.0).abs() < 1e-12);
            assert!((fragment_torque - 2.0).abs() < 1e-12);
        }
        assert!((force.x - 12.0).abs() < 1e-12, "force={force:?}");
        assert!((force.y + 6.0).abs() < 1e-12, "force={force:?}");
        assert!((torque - 4.0).abs() < 1e-12, "torque={torque}");

        physics.step().unwrap();
        let mut momentum = Vector::ZERO;
        let mut angular_momentum = 0.0;
        for fragment in fragments {
            let properties = physics.mass_properties(fragment).unwrap();
            let state = physics.state(fragment).unwrap();
            momentum.x += properties.mass * state.velocity.x;
            momentum.y += properties.mass * state.velocity.y;
            let relative_x = properties.center_of_mass[0] - 1.5;
            let relative_y = properties.center_of_mass[1] - 0.5;
            angular_momentum += properties.angular_inertia * state.velocity.angular
                + properties.mass * (relative_x * state.velocity.y - relative_y * state.velocity.x);
        }
        let dt = 1.0 / f64::from(physics.settings.step_hz);
        assert!((momentum.x - 12.0 * dt).abs() < 1e-10);
        assert!((momentum.y + 6.0 * dt).abs() < 1e-10);
        assert!((angular_momentum - 4.0 * dt).abs() < 1e-8);
    }

    #[test]
    fn local_grid_candidate_shares_unchanged_cells_and_copies_on_edit() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
        let mut grid = BodyLocalGrid::new(
            1,
            1,
            1.0,
            [0.5, 0.5],
            0,
            BodyLocalGridLimits {
                cells: 1,
                geometry_cells: 1,
                sidecar_bytes: 64,
            },
        )
        .unwrap();
        grid.set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
        let id = world
            .insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
            .unwrap();
        assert!(world.local_grid_content_dirty.contains(&id));
        let mut candidate = world.try_copy().unwrap();
        assert!(Arc::ptr_eq(&world.local_grids, &candidate.local_grids));

        candidate.remove(id).unwrap();

        assert!(!Arc::ptr_eq(&world.local_grids, &candidate.local_grids));
        assert!(world.local_grid_content_dirty.contains(&id));
        assert!(!candidate.local_grid_content_dirty.contains(&id));
        assert_eq!(world.local_grid(id).unwrap().unwrap().cell_count(), 1);
        assert_eq!(candidate.body_count(), 0);
    }

    fn box_shape(half: f64) -> ColliderShape {
        ColliderShape::Cuboid {
            half_extents: [half, half],
        }
    }

    #[test]
    fn dynamic_body_hits_fixed_ground_and_rotates() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 10.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let mut ground = BodyDef::dynamic(ColliderShape::Cuboid {
            half_extents: [8.0, 0.25],
        });
        ground.motion = BodyMotionType::Fixed;
        ground.pose.y = 8.0;
        world.insert(ground).unwrap();
        let mut falling = BodyDef::dynamic(box_shape(0.5));
        falling.pose.y = 2.0;
        falling.velocity.angular = 1.0;
        let id = world.insert(falling).unwrap();
        for _ in 0..120 {
            world.step().unwrap();
        }
        let state = world.state(id).unwrap();
        assert!((6.5..8.5).contains(&state.pose.y), "{state:?}");
        assert_ne!(state.pose.angle, 0.0);
    }

    #[test]
    fn ccd_body_does_not_cross_thin_fixed_wall() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let mut wall = BodyDef::dynamic(ColliderShape::Cuboid {
            half_extents: [0.02, 4.0],
        });
        wall.motion = BodyMotionType::Fixed;
        world.insert(wall).unwrap();
        let mut projectile = BodyDef::dynamic(ColliderShape::Circle { radius: 0.1 });
        projectile.pose.x = -3.0;
        projectile.velocity.x = 400.0;
        projectile.ccd = true;
        let id = world.insert(projectile).unwrap();
        world.step().unwrap();
        assert!(world.state(id).unwrap().pose.x < 0.2);
    }

    #[test]
    fn snapshot_resume_matches_uninterrupted_next_steps() {
        let mut source = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
        let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.4 });
        definition.pose.y = -2.0;
        let id = source.insert(definition).unwrap();
        for _ in 0..7 {
            source.step().unwrap();
        }
        let bytes = source.snapshot().unwrap();
        let mut restored = BodyPhysics::restore(&bytes).unwrap();
        for _ in 0..30 {
            source.step().unwrap();
            restored.step().unwrap();
        }
        assert_eq!(source.state(id).unwrap(), restored.state(id).unwrap());
        assert_eq!(source.snapshot().unwrap(), restored.snapshot().unwrap());
    }

    #[test]
    fn invalid_shape_capacity_and_stale_ids_are_rejected() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let invalid_body = BodyDef::dynamic(ColliderShape::Circle { radius: f64::NAN });
        assert!(world.insert(invalid_body).is_err());
        let id = world
            .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 }))
            .unwrap();
        assert!(
            world
                .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 }))
                .is_err()
        );
        world.remove(id).unwrap();
        assert!(world.state(id).is_err());
        let next = world
            .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 }))
            .unwrap();
        assert_ne!(id, next);
    }

    #[test]
    fn malformed_or_over_budget_snapshot_is_rejected() {
        let world = BodyPhysics::new(BodyPhysicsSettings {
            snapshot_byte_limit: 64,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        assert_eq!(
            world.snapshot().unwrap_err().code,
            ErrorCode::CapacityExceeded
        );
        let mut normal = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
        normal
            .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 }))
            .unwrap();
        let mut bytes = normal.snapshot().unwrap();
        assert_eq!(
            BodyPhysics::restore_with_limit(&bytes, bytes.len() - 1)
                .err()
                .unwrap()
                .code,
            ErrorCode::CapacityExceeded
        );
        bytes[12] ^= 1;
        assert!(BodyPhysics::restore(&bytes).is_err());

        let mut malformed_hex = normal.snapshot().unwrap();
        let backend_field = b"\"backend\":\"";
        let hex_start = malformed_hex
            .windows(backend_field.len())
            .position(|window| window == backend_field)
            .unwrap()
            + backend_field.len();
        malformed_hex[hex_start] = b'g';
        match BodyPhysics::restore(&malformed_hex) {
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidEncoding),
            Ok(_) => panic!("不正hexの剛体snapshotを拒否しませんでした"),
        }
    }

    #[test]
    fn non_finite_backend_step_is_discarded_atomically() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let id = world
            .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 }))
            .unwrap();
        let handle = world.handles[&id];
        world
            .backend
            .bodies
            .get_mut(handle)
            .unwrap()
            .set_linvel(Vector::new(f64::INFINITY, 0.0), true);
        assert!(world.step().is_err());
        assert!(
            world
                .backend
                .bodies
                .get(handle)
                .unwrap()
                .linvel()
                .x
                .is_infinite()
        );
        assert_eq!(world.step_index(), 0);
    }

    #[test]
    fn negative_gravity_scale_and_capsule_shape_remain_author_controlled() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 10.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let mut definition = BodyDef::dynamic(ColliderShape::Capsule {
            start: [-0.5, 0.0],
            end: [0.5, 0.0],
            radius: 0.25,
        });
        definition.gravity_scale = -1.0;
        let id = world.insert(definition).unwrap();
        world.step().unwrap();
        assert!(world.state(id).unwrap().velocity.y < 0.0);
    }

    #[test]
    fn stencil_rotation_and_rectangle_partition_preserve_exact_occupancy() {
        let stencil = BodyStencil::new(
            3,
            3,
            1.0,
            [1.5, 1.5],
            7,
            vec![true, true, true, true, false, true, true, true, true],
        )
        .unwrap();
        let rectangles = stencil.rectangle_decomposition();
        assert_eq!(rectangles.len(), 4);
        let unrotated = stencil
            .world_cells(BodyPose {
                x: 10.5,
                y: 20.5,
                angle: 0.0,
            })
            .unwrap();
        assert_eq!(unrotated.len(), 8);
        assert!(!unrotated.contains(&CellPoint { x: 10, y: 20 }));
        let quarter_turn = BodyStencil::new(1, 1, 1.0, [0.5, 0.5], 1, vec![true])
            .unwrap()
            .world_cells(BodyPose {
                x: 10.5,
                y: 20.5,
                angle: std::f64::consts::FRAC_PI_2,
            })
            .unwrap();
        assert_eq!(quarter_turn, vec![CellPoint { x: 10, y: 20 }]);
        let clockwise = BodyStencil::new(2, 1, 1.0, [0.5, 0.5], 1, vec![false, true])
            .unwrap()
            .world_cells(BodyPose {
                x: 10.5,
                y: 20.5,
                angle: std::f64::consts::FRAC_PI_2,
            })
            .unwrap();
        assert_eq!(clockwise, vec![CellPoint { x: 10, y: 21 }]);
        let rotated = stencil
            .world_cells(BodyPose {
                x: 10.5,
                y: 20.5,
                angle: std::f64::consts::FRAC_PI_4,
            })
            .unwrap();
        assert!(rotated.len() > unrotated.len());
        assert_eq!(stencil.revision(), 7);

        let mut physics = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let mut body = BodyDef::dynamic(stencil.collider_shape().unwrap());
        body.pose.x = 10.5;
        body.pose.y = 20.5;
        let id = physics.insert(body).unwrap();
        physics.step().unwrap();
        assert_eq!(physics.state(id).unwrap().pose.x, 10.5);
    }

    #[test]
    fn stencil_sweep_is_bounded_and_includes_rotation_arc() {
        let stencil = BodyStencil::new(2, 1, 1.0, [1.0, 0.5], 1, vec![true, true]).unwrap();
        let start = BodyPose {
            x: 0.5,
            y: 0.5,
            angle: 0.0,
        };
        let end = BodyPose {
            x: 0.5,
            y: 0.5,
            angle: std::f64::consts::PI,
        };
        assert!(stencil.sampled_sweep_world_cells(start, end, 1).is_err());
        let swept = stencil.sampled_sweep_world_cells(start, end, 32).unwrap();
        let mut scratch = vec![CellPoint { x: 100, y: 100 }];
        stencil
            .sampled_sweep_world_cells_into(start, end, 32, &mut scratch)
            .unwrap();
        assert_eq!(scratch, swept);
        let endpoints = stencil
            .world_cells(start)
            .unwrap()
            .into_iter()
            .chain(stencil.world_cells(end).unwrap())
            .map(|point| (point.x, point.y))
            .collect::<BTreeSet<_>>();
        assert!(swept.len() > endpoints.len());
    }

    #[test]
    fn stencil_rejects_bad_shape_and_coordinate_overflow() {
        assert!(BodyStencil::new(2, 2, 1.0, [0.0, 0.0], 0, vec![true]).is_err());
        let stencil = BodyStencil::new(1, 1, 1.0, [0.5, 0.5], 0, vec![true]).unwrap();
        assert!(
            stencil
                .world_cells(BodyPose {
                    x: f64::MAX,
                    y: 0.0,
                    angle: 0.0,
                })
                .is_err()
        );
    }

    #[test]
    fn rectangle_batch_matches_scalar_cell_geometry_reference() {
        let cases = [
            (4, 4, vec![true; 16]),
            (
                4,
                4,
                vec![
                    true, false, true, true, true, true, false, true, false, true, true, false,
                    true, true, false, true,
                ],
            ),
        ];
        for (width, height, occupied) in cases {
            let stencil = BodyStencil::new(width, height, 1.0, [1.5, 2.0], 1, occupied).unwrap();
            for angle in [
                0.0,
                0.37,
                std::f64::consts::FRAC_PI_4,
                std::f64::consts::FRAC_PI_2,
            ] {
                let pose = BodyPose {
                    x: -3.25,
                    y: 7.5,
                    angle,
                };
                let actual = stencil.world_cells(pose).unwrap();
                let mut reference = Vec::new();
                for index in 0..stencil.occupied.len() {
                    if !stencil.occupied[index] {
                        continue;
                    }
                    let x = (index as u32) % stencil.width;
                    let y = (index as u32) / stencil.width;
                    let rect = StencilRect {
                        min_x: f64::from(x) - stencil.pivot[0],
                        min_y: f64::from(y) - stencil.pivot[1],
                        width: 1.0,
                        height: 1.0,
                    };
                    append_polygon_cells(stencil.transformed_rect(rect, pose), 1.0, &mut reference)
                        .unwrap();
                }
                reference.sort_unstable_by_key(|point| (point.y, point.x));
                reference.dedup();
                assert_eq!(actual, reference, "mask={width}x{height}, angle={angle}");
            }
        }
    }

    #[test]
    fn revolute_joint_state_survives_snapshot_and_body_removal_cleans_mapping() {
        let mut world = BodyPhysics::new(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
        let first = world.insert(BodyDef::dynamic(box_shape(0.5))).unwrap();
        let mut second_def = BodyDef::dynamic(box_shape(0.5));
        second_def.pose.x = 1.0;
        let second = world.insert(second_def).unwrap();
        let joint = world
            .insert_joint(JointDef {
                body1: first,
                body2: second,
                local_anchor1: [0.5, 0.0],
                local_anchor2: [-0.5, 0.0],
                kind: JointKind::Revolute {
                    limits: Some([-0.25, 0.25]),
                },
            })
            .unwrap();
        assert_eq!(world.joint_count(), 1);
        world.step().unwrap();
        let mut restored = BodyPhysics::restore(&world.snapshot().unwrap()).unwrap();
        assert_eq!(restored.joint_count(), 1);
        assert_eq!(restored.snapshot().unwrap(), world.snapshot().unwrap());
        for _ in 0..10 {
            world.step().unwrap();
            restored.step().unwrap();
        }
        assert_eq!(restored.snapshot().unwrap(), world.snapshot().unwrap());
        restored.remove(first).unwrap();
        assert_eq!(restored.joint_count(), 0);
        assert!(restored.remove_joint(joint).is_err());
        assert!(
            restored
                .insert_joint(JointDef {
                    body1: first,
                    body2: second,
                    local_anchor1: [0.0, 0.0],
                    local_anchor2: [0.0, 0.0],
                    kind: JointKind::Fixed,
                })
                .is_err()
        );
    }
}
