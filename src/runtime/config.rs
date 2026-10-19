//! YAML・型付きpatch・既定値を統合する実行設定。
//!
//! 旧Configに含まれていたプレイヤー速度等を読み込みません。未知キーは拒否し、
//! 相対パスは設定ファイルの所在を基準に解決します。設定ファイルを自動生成しません。

use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

macro_rules! group {
    ($name:ident, $patch:ident { $($(#[$meta:meta])* $field:ident: $ty:ty = $default:expr),* $(,)? }) => {
        #[doc = "検証される実行設定です。ゲーム内容の定義は含みません。"]
        #[derive(Clone, Debug, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
        impl Default for $name { fn default() -> Self { Self { $($field: $default),* } } }
        #[doc = "指定した項目だけを上書きする型付きpatchです。"]
        #[derive(Clone, Debug, Default, Serialize)]
        pub struct $patch { $(#[serde(skip_serializing_if = "Option::is_none")] $(#[$meta])* pub $field: Option<$ty>),* }
    };
}

group!(WindowSettings, WindowPatch {
    /// 起動時の論理幅。正の値を要求し、実際のsurface制限は描画adapterで確認します。
    width: u32 = 1280,
    /// 起動時の論理高さ。pixel比率はOSの表示adapterで扱います。
    height: u32 = 720,
    /// 起動時にborderless fullscreenを要求します。
    fullscreen: bool = false,
    /// OSによるウィンドウ寸法変更を許可します。
    resizable: bool = true,
    /// OSが報告するmonitor名。存在しない指定は起動を失敗させます。
    display: Option<String> = None
});
group!(RenderSettings, RenderPatch {
    /// surfaceへの表示倍率。fitは縦横比を保つ拡大縮小、integerは整数倍率と余白・clipを使います。
    scaling: String = "fit".into(),
    /// autoまたはcompiled providerが提供するbackend名。文字列だけで実装を追加できません。
    backend: String = "auto".into(),
    /// 描画adapterへ渡す垂直同期の要求です。
    vsync: bool = true,
    /// 表示頻度の上限。固定simulationのstep_hzとは独立します。
    frame_limit: Option<u32> = None,
    /// 描画解像度の倍率。World座標や物理寸法へ影響させません。
    resolution_scale: f64 = 1.0,
    /// 描画providerへ要求する照明方式です。
    lighting: String = "grid".into()
});
group!(AudioSettings, AudioPatch {
    /// 音量の共通倍率。0から1の有限値です。
    master_volume: f32 = 1.0,
    /// 音響providerへ渡す出力機器名。Noneは既定機器です。
    device: Option<String> = None,
    /// trueなら実際に利用可能な音響providerがない起動を拒否します。
    required: bool = false
});
group!(
    AssetSettings,
    AssetPatch {
        /// 外部データの基準フォルダ。相対指定はYAMLの所在から解決します。
        root: PathBuf = "./assets".into(),
        /// 資産cacheの上限MiB。E03のcacheへ渡す実行設定です。
        cache_mib: u32 = 256,
        /// 資産decodeのworker数。autoまたは正の整数です。
        decode_workers: WorkerCount = WorkerCount::Auto("auto".into())
    }
);
group!(
    StorageSettings,
    StoragePatch {
        /// 外部データの基準フォルダ。相対指定はYAMLの所在から解決します。
        root: PathBuf = "./saves".into(),
        /// 保存サービスが保持する世代数。ゼロを認めません。
        keep_generations: u32 = 3
    }
);
group!(
    Gravity,
    GravityPatch {
        /// 共通既定重力の水平成分。有限値を要求し、適用するかは作者の物理Systemが決めます。
        x: f64 = 0.0,
        /// 共通既定重力の垂直成分。正方向は下です。
        y: f64 = 627.84
    }
);
group!(
    SimulationSettings,
    SimulationPatch {
        /// 1秒あたりの固定更新数。整数時間累積から発行し、表示回数へ依存させません。
        step_hz: u16 = 60,
        /// `.parallel()`を許可した独立作者Systemを同時実行する最大worker数。1〜64またはautoで、1なら逐次実行です。
        system_workers: WorkerCount = WorkerCount::Count(1),
        /// 新規Worldや物理providerへ渡す既定重力。ゲーム個体の構成は含みません。
        gravity: Gravity = Gravity::default()
    }
);
group!(
    StreamingSettings,
    StreamingPatch {
        /// World側の既定observer生成方針。E04のstreamingへ渡します。
        auto_observer: bool = true,
        /// 更新域の周囲に保つchunk数の既定値です。
        active_margin_chunks: u32 = 1,
        /// 常駐域の余白。更新域の余白以上である必要があります。
        resident_margin_chunks: u32 = 2
    }
);
group!(
    CapacitySettings,
    CapacityPatch {
        /// 同時に更新できるchunk数の上限です。
        active_chunks: u32 = 512,
        /// 常駐chunk数の上限。active_chunks以上を要求します。
        resident_chunks: u32 = 2048,
        /// 生存Entity数の上限です。
        entities: u32 = 65536,
        /// 物理body数の上限です。
        rigid_bodies: u32 = 2048,
        /// 物質粒子数の上限です。
        material_particles: u32 = 262144,
        /// 一つの論理tickへ提出できるcommand数。同tick再開で予算をリセットしません。
        commands_per_tick: u32 = 16384,
        /// 同時に保持するjob数の上限です。
        jobs: u32 = 4096
    }
);
group!(
    DiagnosticsSettings,
    DiagnosticsPatch {
        /// 診断サービスへ渡すログ詳細度です。
        log_level: String = "info".into(),
        /// 診断サービスへ統計収集を要求するかどうかです。
        metrics: bool = cfg!(debug_assertions)
    }
);

/// ワーカー数はautoまたは正の整数です。文字列の誤記も検証で拒否します。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkerCount {
    /// 自動選択。
    Auto(String),
    /// 明示した個数。
    Count(u16),
}

/// 統合・検証済みの設定。利用能力の解決は各serviceの起動で行います。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSettings {
    /// 設定schemaの版。既定は1です。
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    /// ウィンドウの要求。
    pub window: WindowSettings,
    /// 描画providerの要求。
    pub render: RenderSettings,
    /// 音響providerの要求。
    pub audio: AudioSettings,
    /// 資産の所在とdecode予算。
    pub assets: AssetSettings,
    /// 保存の所在と保持世代。
    pub storage: StorageSettings,
    /// 新規Worldへ渡す共通既定値。
    pub simulation: SimulationSettings,
    /// Worldの活動域に対する既定値。
    pub streaming: StreamingSettings,
    /// 容量profile。
    pub limits: CapacitySettings,
    /// 診断方針。
    pub diagnostics: DiagnosticsSettings,
    /// pluginが自分の型付きschemaで検証する実行設定。
    pub plugins: BTreeMap<String, serde_yaml::Value>,
}
fn schema_version() -> u16 {
    1
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            window: WindowSettings::default(),
            render: RenderSettings::default(),
            audio: AudioSettings::default(),
            assets: AssetSettings::default(),
            storage: StorageSettings::default(),
            simulation: SimulationSettings::default(),
            streaming: StreamingSettings::default(),
            limits: CapacitySettings::default(),
            diagnostics: DiagnosticsSettings::default(),
            plugins: BTreeMap::new(),
        }
    }
}

/// Rustから明示した設定だけを重ねます。未指定の兄弟fieldを既定値へ戻しません。
#[derive(Clone, Debug, Default, Serialize)]
pub struct EngineSettingsPatch {
    /// ウィンドウの部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPatch>,
    /// 描画の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<RenderPatch>,
    /// 音響の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioPatch>,
    /// 資産の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<AssetPatch>,
    /// 保存の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<StoragePatch>,
    /// 固定更新の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation: Option<SimulationPatch>,
    /// 活動域の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming: Option<StreamingPatch>,
    /// 容量の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<CapacityPatch>,
    /// 診断の部分変更。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticsPatch>,
}

impl EngineSettingsPatch {
    /// 表示寸法を変更します。ほかのwindow項目は維持します。
    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        let patch = self.window.get_or_insert_with(WindowPatch::default);
        patch.width = Some(width);
        patch.height = Some(height);
        self
    }
    /// 固定更新の周波数を指定します。
    pub fn step_hz(mut self, hz: u16) -> Self {
        self.simulation
            .get_or_insert_with(SimulationPatch::default)
            .step_hz = Some(hz);
        self
    }
    /// 資産の所在を指定します。
    pub fn asset_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets.get_or_insert_with(AssetPatch::default).root = Some(root.into());
        self
    }
}

impl EngineSettings {
    /// 読込量を1MiBに制限して設定を統合します。I/O失敗で既定値へ隠れて戻りません。
    pub fn resolve(path: Option<&Path>, patch: &EngineSettingsPatch) -> EngineResult<Self> {
        use std::io::Read;
        let mut value = serde_yaml::to_value(Self {
            schema_version: 1,
            ..Self::default()
        })
        .map_err(|_| invalid("設定の既定値"))?;
        let base = if let Some(path) = path {
            let mut file = std::fs::File::open(path)
                .map_err(|_| EngineError::new(ErrorCode::Io, "設定ファイルを開く"))?;
            let mut text = String::new();
            file.by_ref()
                .take(1_048_577)
                .read_to_string(&mut text)
                .map_err(|_| EngineError::new(ErrorCode::Io, "設定ファイルのUTF-8読込"))?;
            if text.len() > 1_048_576 {
                return Err(EngineError::capacity(
                    "設定byte数",
                    text.len() as u64,
                    1_048_576,
                ));
            }
            let yaml: serde_yaml::Value = serde_yaml::from_str(&text)
                .map_err(|cause| invalid("設定YAMLの構文").detail(cause.to_string()))?;
            if !yaml.is_mapping() {
                return Err(invalid("設定のrootはmapping"));
            }
            merge(&mut value, yaml);
            std::path::absolute(path)
                .map_err(|_| invalid("設定の絶対パス"))?
                .parent()
                .ok_or_else(|| invalid("設定の親directory"))?
                .to_owned()
        } else {
            std::env::current_dir().map_err(|_| EngineError::new(ErrorCode::Io, "実行directory"))?
        };
        merge(
            &mut value,
            serde_yaml::to_value(patch).map_err(|_| invalid("型付き設定patch"))?,
        );
        let mut settings: Self = serde_yaml::from_value(value)
            .map_err(|cause| invalid("未知の設定キーまたは値の型").detail(cause.to_string()))?;
        settings.validate()?;
        if settings.assets.root.is_relative() {
            settings.assets.root = base.join(&settings.assets.root);
        }
        if settings.storage.root.is_relative() {
            settings.storage.root = base.join(&settings.storage.root);
        }
        Ok(settings)
    }

    /// providerに渡す前に、環境へ依存しない値域を検証します。
    pub fn validate(&self) -> EngineResult<()> {
        if self.schema_version != 1 || self.window.width == 0 || self.window.height == 0 {
            return Err(invalid("schema_versionまたはwindow寸法"));
        }
        if !["fit", "integer"].contains(&self.render.scaling.as_str())
            || !["auto", "vulkan", "dx12", "metal"].contains(&self.render.backend.as_str())
            || !["disabled", "grid"].contains(&self.render.lighting.as_str())
            || !self.render.resolution_scale.is_finite()
            || self.render.resolution_scale <= 0.0
            || self.render.frame_limit == Some(0)
        {
            return Err(invalid("描画の要求"));
        }
        if !self.audio.master_volume.is_finite() || !(0.0..=1.0).contains(&self.audio.master_volume)
        {
            return Err(invalid("音量"));
        }
        if self.simulation.step_hz == 0
            || !self.simulation.gravity.x.is_finite()
            || !self.simulation.gravity.y.is_finite()
        {
            return Err(invalid("固定更新または重力"));
        }
        match &self.simulation.system_workers {
            WorkerCount::Auto(value) if value == "auto" => {}
            WorkerCount::Count(value) if (1..=64).contains(value) => {}
            _ => return Err(invalid("simulation.system_workers (1〜64 またはauto)")),
        }
        if self.assets.root.as_os_str().is_empty()
            || self.storage.root.as_os_str().is_empty()
            || self.assets.cache_mib == 0
            || self.storage.keep_generations < 3
        {
            return Err(invalid("資産または保存の設定"));
        }
        match &self.assets.decode_workers {
            WorkerCount::Auto(value) if value == "auto" => {}
            WorkerCount::Count(value) if *value > 0 => {}
            _ => return Err(invalid("decode_workers")),
        }
        let limits = &self.limits;
        if [
            limits.active_chunks,
            limits.resident_chunks,
            limits.entities,
            limits.rigid_bodies,
            limits.material_particles,
            limits.commands_per_tick,
            limits.jobs,
        ]
        .contains(&0)
            || limits.active_chunks > limits.resident_chunks
            || self.streaming.active_margin_chunks > self.streaming.resident_margin_chunks
        {
            return Err(invalid("容量と活動域の関係"));
        }
        if !["error", "warn", "info", "debug", "trace"]
            .contains(&self.diagnostics.log_level.as_str())
        {
            return Err(invalid("log_level"));
        }
        for key in self.plugins.keys() {
            StableKey::parse(key)?;
        }
        Ok(())
    }
}

fn merge(target: &mut serde_yaml::Value, patch: serde_yaml::Value) {
    if let (serde_yaml::Value::Mapping(base), serde_yaml::Value::Mapping(extra)) =
        (&mut *target, &patch)
    {
        for (key, value) in extra {
            if let Some(old) = base.get_mut(key) {
                merge(old, value.clone());
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = patch;
    }
}
fn invalid(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, context)
}
