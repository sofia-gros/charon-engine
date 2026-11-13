//! 仕様 10 §14: 設定 (UI の大きさ・キャラクターの大きさ・光の品質・キー割り当て) と、`settings.ron` への保存。
//!
//! キーは `winit` の `KeyCode` の名前 (`"KeyA"`・`"Space"` など) で保存する。操作 (`Action`) ごとに複数のキーを割り当てられ、
//! 1 つのキーは 1 つの操作にだけ割り当てる (別の操作に割り当てると、前の操作から外れる)。

use std::collections::BTreeMap;
use std::path::Path;

use charon_render::LightQuality;
use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

/// キーで起こす操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Action {
    Left,
    Right,
    Jump,
    /// 掘る道具 (仕様 10 §12)。
    Dig,
    FreeCamera,
    TogglePaint,
    Inventory,
    Pause,
}

impl Action {
    pub const ALL: [Action; 8] = [
        Self::Left,
        Self::Right,
        Self::Jump,
        Self::Dig,
        Self::FreeCamera,
        Self::TogglePaint,
        Self::Inventory,
        Self::Pause,
    ];

    /// 画面に出す名前。
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Move left",
            Self::Right => "Move right",
            Self::Jump => "Jump",
            Self::Dig => "Dig",
            Self::FreeCamera => "Free camera",
            Self::TogglePaint => "Weapon / Paint",
            Self::Inventory => "Inventory",
            Self::Pause => "Pause menu",
        }
    }
}

/// 割り当てられるキー。名前の保存・読み込みは、この表の中だけ (表にないキーは割り当てられない)。
const SUPPORTED: &[KeyCode] = &[
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::Space,
    KeyCode::Enter,
    KeyCode::Tab,
    KeyCode::Escape,
    KeyCode::Backspace,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::BracketLeft,
    KeyCode::BracketRight,
    KeyCode::Minus,
    KeyCode::Equal,
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
];

/// キーの名前 (`"KeyA"`)。表にないキーは `None`。
pub fn key_name(code: KeyCode) -> Option<String> {
    SUPPORTED.contains(&code).then(|| format!("{code:?}"))
}

/// 名前からキーを引く。表にない名前は `None`。
pub fn parse_key(name: &str) -> Option<KeyCode> {
    SUPPORTED.iter().copied().find(|code| format!("{code:?}") == name)
}

/// 操作ごとのキー。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBindings {
    keys: BTreeMap<Action, Vec<String>>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let table: [(Action, &[&str]); 8] = [
            (Action::Left, &["KeyA", "ArrowLeft"]),
            (Action::Right, &["KeyD", "ArrowRight"]),
            (Action::Jump, &["KeyW", "ArrowUp", "Space", "KeyC"]),
            (Action::Dig, &["KeyE"]),
            (Action::FreeCamera, &["KeyF"]),
            (Action::TogglePaint, &["KeyB"]),
            (Action::Inventory, &["KeyI"]),
            (Action::Pause, &["Escape"]),
        ];
        Self {
            keys: table
                .into_iter()
                .map(|(action, names)| (action, names.iter().map(|n| (*n).to_owned()).collect()))
                .collect(),
        }
    }
}

impl KeyBindings {
    /// `code` が割り当てられている操作。
    pub fn action_of(&self, code: KeyCode) -> Option<Action> {
        let name = key_name(code)?;
        self.keys.iter().find(|(_, names)| names.contains(&name)).map(|(&action, _)| action)
    }

    /// `action` に割り当てられたキーの名前 (割り当ての順)。
    pub fn keys_of(&self, action: Action) -> &[String] {
        self.keys.get(&action).map_or(&[], Vec::as_slice)
    }

    /// 画面に出す文字 (`"A / Left"` のように)。
    pub fn describe(&self, action: Action) -> String {
        let names: Vec<String> = self
            .keys_of(action)
            .iter()
            .map(|n| {
                n.strip_prefix("Key").or_else(|| n.strip_prefix("Arrow")).unwrap_or(n).to_owned()
            })
            .collect();
        if names.is_empty() { "-".to_owned() } else { names.join(" / ") }
    }

    /// `action` のキーを `code` 1 つに割り当て直す。ほかの操作に割り当てられていたら、そこから外す。
    /// 表にないキーは何も変えず `false`。
    pub fn bind(&mut self, action: Action, code: KeyCode) -> bool {
        let Some(name) = key_name(code) else { return false };
        for names in self.keys.values_mut() {
            names.retain(|n| *n != name);
        }
        self.keys.insert(action, vec![name]);
        true
    }

    /// 割り当てを既定に戻す。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 割り当ての検査: 知らないキーの名前を取り除き、1 つのキーが複数の操作にあれば最初の操作だけに残す。
    /// 全部の操作が最低 1 つのキーを持たなければ、足りない操作は既定のキーを戻す。
    fn sanitize(&mut self) {
        let defaults = Self::default();
        let mut seen = std::collections::BTreeSet::new();
        for action in Action::ALL {
            let names = self.keys.entry(action).or_default();
            names.retain(|n| parse_key(n).is_some() && seen.insert(n.clone()));
        }
        for action in Action::ALL {
            if self.keys[&action].is_empty() {
                let fallback: Vec<String> = defaults
                    .keys_of(action)
                    .iter()
                    .filter(|n| seen.insert((*n).clone()))
                    .cloned()
                    .collect();
                self.keys.insert(action, fallback);
            }
        }
    }
}

/// 設定ファイルの版。
pub const SETTINGS_VERSION: u32 = 1;
/// 設定ファイルの大きさの上限 (バイト)。
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    /// UI の大きさの倍率 (0.5〜3.0)。
    pub ui_scale: f32,
    /// キャラクターの画像の大きさの倍率 (0.25〜3.0)。
    pub character_scale: f32,
    /// 光の品質: `Off`・`Low`・`High`・`XHigh`。
    pub light: String,
    /// 音量 (0.0..=1.0): 全体・効果音・音楽 (仕様 12)。
    pub master_volume: f32,
    pub sfx_volume: f32,
    pub music_volume: f32,
    /// セルを更新する範囲の半径 (チャンク、0 = 自動、最大 16)。この外のチャンクは**コールドフリーズ**する (仕様 02 §9.4)。
    /// 古い設定ファイルにはない項目なので、なければ 0。
    #[serde(default)]
    pub sim_distance: u32,
    pub keys: KeyBindings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            ui_scale: 1.0,
            character_scale: 1.0,
            light: "High".to_owned(),
            master_volume: 1.0,
            sfx_volume: 1.0,
            music_volume: 0.7,
            sim_distance: 0,
            keys: KeyBindings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Io(String),
    Parse(String),
    Version(u32),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "設定の読み書きに失敗: {e}"),
            Self::Parse(e) => write!(f, "settings.ron を読めません: {e}"),
            Self::Version(v) => write!(f, "settings.ron の版 {v} は読めません"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// 光の品質の名前。
pub fn light_name(quality: LightQuality) -> &'static str {
    match quality {
        LightQuality::Off => "Off",
        LightQuality::Low => "Low",
        LightQuality::High => "High",
        LightQuality::XHigh => "XHigh",
    }
}

impl Settings {
    /// 光の品質 (知らない名前は `High`)。
    pub fn light_quality(&self) -> LightQuality {
        match self.light.as_str() {
            "Off" => LightQuality::Off,
            "Low" => LightQuality::Low,
            "XHigh" => LightQuality::XHigh,
            _ => LightQuality::High,
        }
    }

    /// 値を範囲に収め、キー割り当てを検査する (外から読んだ値、またはゲームが組み立てた値を使える形にする)。
    pub fn sanitize(&mut self) {
        let clamp = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { default }
        };
        self.ui_scale = clamp(self.ui_scale, 0.5, 3.0, 1.0);
        self.character_scale = clamp(self.character_scale, 0.25, 3.0, 1.0);
        self.master_volume = clamp(self.master_volume, 0.0, 1.0, 1.0);
        self.sfx_volume = clamp(self.sfx_volume, 0.0, 1.0, 1.0);
        self.music_volume = clamp(self.music_volume, 0.0, 1.0, 0.7);
        self.sim_distance = self.sim_distance.min(charon_world::streaming::MAX_SIM_DISTANCE);
        self.keys.sanitize();
    }

    pub fn to_ron(&self) -> Result<String, SettingsError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| SettingsError::Parse(e.to_string()))
    }

    pub fn from_ron(text: &str) -> Result<Self, SettingsError> {
        let mut settings: Self =
            ron::from_str(text).map_err(|e| SettingsError::Parse(e.to_string()))?;
        if settings.version != SETTINGS_VERSION {
            return Err(SettingsError::Version(settings.version));
        }
        settings.sanitize();
        Ok(settings)
    }

    /// `path` に書く (先に一時ファイルへ書いて入れ替える)。
    pub fn write(&self, path: &Path) -> Result<(), SettingsError> {
        let text = self.to_ron()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| SettingsError::Io(e.to_string()))?;
        }
        let temporary = path.with_extension("ron.tmp");
        std::fs::write(&temporary, text).map_err(|e| SettingsError::Io(e.to_string()))?;
        std::fs::rename(&temporary, path).map_err(|e| SettingsError::Io(e.to_string()))
    }

    /// `path` を読む。なければ `Ok(None)`。
    pub fn read(path: &Path) -> Result<Option<Self>, SettingsError> {
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_BYTES => {
                return Err(SettingsError::Parse("大きすぎます".to_owned()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SettingsError::Io(e.to_string())),
        }
        let text = std::fs::read_to_string(path).map_err(|e| SettingsError::Io(e.to_string()))?;
        Self::from_ron(&text).map(Some)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use super::*;

    #[test]
    fn key_names_roundtrip_and_unknown_keys_are_refused() {
        for code in SUPPORTED {
            let name = key_name(*code).unwrap();
            assert_eq!(parse_key(&name), Some(*code), "{name}");
        }
        assert_eq!(key_name(KeyCode::KeyA).as_deref(), Some("KeyA"));
        assert_eq!(key_name(KeyCode::ShiftLeft), None, "表にないキー");
        assert_eq!(parse_key("Nope"), None);
    }

    #[test]
    fn defaults_cover_every_action_without_overlap() {
        let keys = KeyBindings::default();
        let mut seen = std::collections::BTreeSet::new();
        for action in Action::ALL {
            assert!(!keys.keys_of(action).is_empty(), "{action:?}");
            for name in keys.keys_of(action) {
                assert!(seen.insert(name.clone()), "キーが重なる: {name}");
                assert_eq!(keys.action_of(parse_key(name).unwrap()), Some(action));
            }
        }
        assert_eq!(keys.action_of(KeyCode::KeyQ), None);
        assert_eq!(keys.describe(Action::Left), "A / Left");
    }

    #[test]
    fn binding_moves_a_key_from_the_old_action() {
        let mut keys = KeyBindings::default();
        assert!(keys.bind(Action::Dig, KeyCode::KeyG));
        assert_eq!(keys.keys_of(Action::Dig), ["KeyG"], "割り当て直すと 1 つになる");
        assert_eq!(keys.action_of(KeyCode::KeyE), None, "前のキーは空く");
        // すでにほかの操作にあるキーは、その操作から外れる。
        assert!(keys.bind(Action::Dig, KeyCode::KeyF));
        assert_eq!(keys.action_of(KeyCode::KeyF), Some(Action::Dig));
        assert!(keys.keys_of(Action::FreeCamera).is_empty());
        assert!(!keys.bind(Action::Dig, KeyCode::ShiftLeft), "表にないキーは割り当てない");
        assert_eq!(keys.action_of(KeyCode::KeyF), Some(Action::Dig));
        keys.reset();
        assert_eq!(keys, KeyBindings::default());
    }

    #[test]
    fn sim_distance_roundtrips_clamps_and_is_optional_in_old_files() {
        let settings = Settings { sim_distance: 5, ..Settings::default() };
        let loaded = Settings::from_ron(&settings.to_ron().unwrap()).unwrap();
        assert_eq!(loaded.sim_distance, 5);
        let mut wild = Settings { sim_distance: 9999, ..Settings::default() };
        wild.sanitize();
        assert_eq!(wild.sim_distance, charon_world::streaming::MAX_SIM_DISTANCE);
        // `sim_distance` が増える前の設定ファイル (その項目がない) も読めて、自動 (0) になる。
        let text = settings.to_ron().unwrap();
        let old: String = text
            .lines()
            .filter(|line| !line.contains("sim_distance"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!old.contains("sim_distance"));
        assert_eq!(Settings::from_ron(&old).unwrap().sim_distance, 0);
    }

    #[test]
    fn settings_roundtrip_and_clamp() {
        let mut settings =
            Settings { ui_scale: 1.5, light: "XHigh".to_owned(), ..Settings::default() };
        settings.keys.bind(Action::Dig, KeyCode::KeyG);
        let loaded = Settings::from_ron(&settings.to_ron().unwrap()).unwrap();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.light_quality(), LightQuality::XHigh);
        // 範囲外は収める。
        let mut wild =
            Settings { ui_scale: 99.0, character_scale: f32::NAN, ..Settings::default() };
        wild.sanitize();
        assert_eq!((wild.ui_scale, wild.character_scale), (3.0, 1.0));
        wild.light = "unknown".to_owned();
        assert_eq!(wild.light_quality(), LightQuality::High);
    }

    #[test]
    fn volumes_are_clamped() {
        let mut settings = Settings {
            master_volume: 5.0,
            sfx_volume: -1.0,
            music_volume: f32::NAN,
            ..Settings::default()
        };
        settings.sanitize();
        assert_eq!(
            (settings.master_volume, settings.sfx_volume, settings.music_volume),
            (1.0, 0.0, 0.7)
        );
    }

    #[test]
    fn broken_key_tables_are_repaired() {
        let mut settings = Settings::default();
        // 知らない名前・重複・空にされた操作。
        settings.keys.keys.insert(Action::Left, vec!["Bogus".to_owned()]);
        settings.keys.keys.insert(Action::Right, vec!["KeyD".to_owned(), "KeyD".to_owned()]);
        settings.keys.keys.insert(Action::Jump, vec!["KeyD".to_owned()]);
        settings.keys.keys.remove(&Action::Pause);
        settings.sanitize();
        assert_eq!(settings.keys.keys_of(Action::Left), ["KeyA", "ArrowLeft"], "既定に戻る");
        assert_eq!(settings.keys.keys_of(Action::Right), ["KeyD"], "重複は 1 つ");
        assert!(
            !settings.keys.keys_of(Action::Jump).is_empty(),
            "取られたキーの代わりに既定が入る"
        );
        assert_eq!(settings.keys.action_of(KeyCode::KeyD), Some(Action::Right));
        assert_eq!(settings.keys.keys_of(Action::Pause), ["Escape"]);
        for action in Action::ALL {
            assert!(!settings.keys.keys_of(action).is_empty(), "{action:?}");
        }
    }

    #[test]
    fn settings_file_io() {
        let dir = std::env::temp_dir().join(format!("charon-settings-{}", std::process::id()));
        let path = dir.join("nested/settings.ron");
        assert_eq!(Settings::read(&path), Ok(None), "なければ None");
        let settings = Settings { ui_scale: 2.0, ..Settings::default() };
        settings.write(&path).unwrap();
        settings.write(&path).unwrap();
        assert_eq!(Settings::read(&path).unwrap(), Some(settings));
        assert!(!path.with_extension("ron.tmp").exists(), "一時ファイルは残らない");
        std::fs::write(&path, "garbage").unwrap();
        assert!(Settings::read(&path).is_err());
        let old = Settings::default().to_ron().unwrap().replace("version: 1", "version: 9");
        std::fs::write(&path, old).unwrap();
        assert_eq!(Settings::read(&path), Err(SettingsError::Version(9)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
