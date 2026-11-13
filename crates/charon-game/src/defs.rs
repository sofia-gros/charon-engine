//! 仕様 10 §7: エンジンの既定の定義 (呪文・杖・敵) と、ゲーム側が重ねる追加・上書き。

use std::collections::BTreeSet;

use charon_core::material::MaterialTable;
use serde::Deserialize;

use crate::wand::{Fnv, SpellError, SpellTable, Wand, WandError};

const ENGINE_SPELLS: &str = include_str!("../../../assets/spells.ron");
const ENGINE_WANDS: &str = include_str!("../../../assets/wands.ron");
const ENGINE_ENEMIES: &str = include_str!("../../../assets/enemies.ron");
const ENGINE_HOOKS: &str = include_str!("../../../assets/hooks.ron");
const ENGINE_DIGS: &str = include_str!("../../../assets/digs.ron");

const MAX_WANDS: usize = 256;
const MAX_ENEMIES: usize = 256;
const MAX_HOOKS: usize = 256;
const MAX_DIGS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefsError {
    Parse { file: &'static str, reason: String },
    Spell(SpellError),
    Duplicate { kind: &'static str, name: String },
    ReplaceMissing { kind: &'static str, name: String },
    UnknownSpell { owner: String, spell: String },
    Invalid { owner: String, reason: &'static str },
    UnknownWand(String),
    UnknownHook(String),
    UnknownDig(String),
    Wand(WandError),
    TooMany(&'static str),
}

impl std::fmt::Display for DefsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { file, reason } => write!(f, "{file} を読めません: {reason}"),
            Self::Spell(e) => write!(f, "{e}"),
            Self::Duplicate { kind, name } => write!(f, "{kind} \"{name}\" が重複しています"),
            Self::ReplaceMissing { kind, name } => {
                write!(f, "置き換える {kind} \"{name}\" がありません")
            }
            Self::UnknownSpell { owner, spell } => {
                write!(f, "\"{owner}\" が未定義の呪文 \"{spell}\" を参照しています")
            }
            Self::Invalid { owner, reason } => write!(f, "\"{owner}\": {reason}"),
            Self::UnknownWand(name) => write!(f, "杖 \"{name}\" は定義されていません"),
            Self::UnknownHook(name) => write!(f, "ワイヤーフック \"{name}\" は定義されていません"),
            Self::UnknownDig(name) => write!(f, "掘る道具 \"{name}\" は定義されていません"),
            Self::Wand(e) => write!(f, "{e}"),
            Self::TooMany(kind) => write!(f, "{kind} が多すぎます"),
        }
    }
}

impl std::error::Error for DefsError {}

impl From<SpellError> for DefsError {
    fn from(error: SpellError) -> Self {
        Self::Spell(error)
    }
}

/// 杖のひな形 (仕様 10 §7.1)。`spells` は表の中の呪文の (名前空間付きの) 名前。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WandDef {
    pub name: String,
    pub spells: Vec<String>,
    pub mana_max: u32,
    pub mana_regen: u16,
    pub recharge: u16,
}

/// 敵の種類 (仕様 10 §7.1)。動きは T11.4。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnemyDef {
    pub name: String,
    pub width: u8,
    pub height: u8,
    pub max_hp: i32,
    pub walk_speed: i16,
    pub jump_speed: i16,
    pub sight: u16,
    pub attack: Option<String>,
    pub attack_interval: u16,
    /// 当たり判定が重なる目標に、`CONTACT_INTERVAL` ごとに与えるダメージ (仕様 10 §8.1)。
    pub contact_damage: i32,
}

/// ワイヤーフックの定義 (仕様 10 §9.3)。長さはセル、速さは 1/16 セル / tick。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookDef {
    pub name: String,
    pub max_length: u16,
    pub min_length: u8,
    pub speed: u16,
    pub segments: u8,
    pub gravity: u8,
    pub iterations: u8,
    pub reel_speed: u8,
    pub pull_accel: u8,
    pub pull_max: u8,
    pub cooldown: u16,
}

/// 掘る道具の定義 (仕様 10 §12)。力・半径・距離はセル単位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigDef {
    pub name: String,
    /// 1 tick に円の中の各セルへ与えるダメージ。材質の硬さに達したセルが消える (掘る時間 = 硬さ / 力)。
    pub power: u16,
    /// 削る円の半径 (セル)。
    pub radius: u8,
    /// 光線が届く距離 (セル)。
    pub range: u16,
    /// 当てていないセルのダメージが、1 tick に回復する量。
    pub recover: u16,
}

#[derive(Deserialize)]
struct RawDig {
    name: String,
    #[serde(default)]
    replace: bool,
    power: i64,
    radius: i64,
    range: i64,
    #[serde(default)]
    recover: i64,
}

#[derive(Deserialize)]
struct RawHook {
    name: String,
    #[serde(default)]
    replace: bool,
    max_length: i64,
    min_length: i64,
    speed: i64,
    segments: i64,
    gravity: i64,
    iterations: i64,
    reel_speed: i64,
    pull_accel: i64,
    pull_max: i64,
    cooldown: i64,
}

#[derive(Deserialize)]
struct RawWand {
    name: String,
    #[serde(default)]
    replace: bool,
    spells: Vec<String>,
    mana_max: i64,
    mana_regen: i64,
    #[serde(default)]
    recharge: i64,
}

#[derive(Deserialize)]
struct RawEnemy {
    name: String,
    #[serde(default)]
    replace: bool,
    width: i64,
    height: i64,
    max_hp: i64,
    walk_speed: i64,
    jump_speed: i64,
    sight: i64,
    attack: Option<String>,
    #[serde(default)]
    attack_interval: i64,
    #[serde(default)]
    contact_damage: i64,
}

/// 1 つの層のファイル。`None` はその種類の層が空。
#[derive(Clone, Copy, Debug, Default)]
pub struct DefinitionFiles<'a> {
    pub spells: Option<&'a str>,
    pub wands: Option<&'a str>,
    pub enemies: Option<&'a str>,
    pub hooks: Option<&'a str>,
    pub digs: Option<&'a str>,
}

/// 呪文・杖・敵の定義。層で重ねる。
#[derive(Clone, Debug, Default)]
pub struct Definitions {
    pub spells: SpellTable,
    wands: Vec<WandDef>,
    enemies: Vec<EnemyDef>,
    hooks: Vec<HookDef>,
    digs: Vec<DigDef>,
}

fn range(
    owner: &str,
    value: i64,
    lo: i64,
    hi: i64,
    reason: &'static str,
) -> Result<i64, DefsError> {
    if (lo..=hi).contains(&value) {
        Ok(value)
    } else {
        Err(DefsError::Invalid { owner: owner.to_owned(), reason })
    }
}

/// `replace` でなければ名前空間を付け、名前の形を検査する。
fn qualify(
    kind: &'static str,
    name: &str,
    replace: bool,
    namespace: Option<&str>,
) -> Result<String, DefsError> {
    if replace {
        return Ok(name.to_owned());
    }
    if !(1..=64).contains(&name.len())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        let _ = kind;
        return Err(DefsError::Invalid {
            owner: name.to_owned(),
            reason: "名前は ASCII 英数字と _ の 1..=64 文字です",
        });
    }
    Ok(match namespace {
        Some(id) => format!("{id}:{name}"),
        None => name.to_owned(),
    })
}

/// 同じ層の中の重複と、`replace` の有無に応じた追加・置き換えを行う。
fn place<T>(
    kind: &'static str,
    items: &mut Vec<T>,
    name_of: impl Fn(&T) -> &str,
    seen: &mut BTreeSet<String>,
    replace: bool,
    value: T,
) -> Result<(), DefsError> {
    let name = name_of(&value).to_owned();
    if !seen.insert(name.clone()) {
        return Err(DefsError::Duplicate { kind, name });
    }
    let slot = items.iter().position(|item| name_of(item) == name);
    match (replace, slot) {
        (true, None) => Err(DefsError::ReplaceMissing { kind, name }),
        (false, Some(_)) => Err(DefsError::Duplicate { kind, name }),
        (_, Some(index)) => {
            items[index] = value;
            Ok(())
        }
        (_, None) => {
            items.push(value);
            Ok(())
        }
    }
}

/// 呪文名を、まず `namespace:name`、次にそのままの名前で探し、表にある名前を返す。
fn resolve_spell(table: &SpellTable, namespace: Option<&str>, name: &str) -> Option<String> {
    namespace
        .map(|id| format!("{id}:{name}"))
        .filter(|full| table.id(full).is_some())
        .or_else(|| table.id(name).map(|_| name.to_owned()))
}

impl Definitions {
    /// エンジンが埋め込んだ既定の定義 (仕様 10 §7.1)。
    pub fn engine_defaults(materials: &MaterialTable) -> Result<Self, DefsError> {
        let mut defs = Self::default();
        defs.layer(
            DefinitionFiles {
                spells: Some(ENGINE_SPELLS),
                wands: Some(ENGINE_WANDS),
                enemies: Some(ENGINE_ENEMIES),
                hooks: Some(ENGINE_HOOKS),
                digs: Some(ENGINE_DIGS),
            },
            materials,
            None,
        )?;
        Ok(defs)
    }

    /// 仕様 10 §7.2: 1 層を重ねる。失敗したときは `self` を変えない。
    pub fn layer(
        &mut self,
        files: DefinitionFiles<'_>,
        materials: &MaterialTable,
        namespace: Option<&str>,
    ) -> Result<(), DefsError> {
        let mut next = self.clone();
        if let Some(text) = files.spells {
            next.spells.layer(text, materials, namespace)?;
        }
        if let Some(text) = files.wands {
            next.layer_wands(text, namespace)?;
        }
        if let Some(text) = files.enemies {
            next.layer_enemies(text, namespace)?;
        }
        if let Some(text) = files.hooks {
            next.layer_hooks(text, namespace)?;
        }
        if let Some(text) = files.digs {
            next.layer_digs(text, namespace)?;
        }
        *self = next;
        Ok(())
    }

    fn layer_wands(&mut self, text: &str, namespace: Option<&str>) -> Result<(), DefsError> {
        let raws: Vec<RawWand> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "wands.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("杖", &raw.name, raw.replace, namespace)?;
            range(&name, raw.spells.len() as i64, 1, 16, "杖の呪文は 1..=16 個です")?;
            let mana_max = range(&name, raw.mana_max, 1, 10_000, "mana_max は 1..=10000 です")?;
            let mana_regen =
                range(&name, raw.mana_regen, 0, 65_535, "mana_regen は 0..=65535 です")?;
            let recharge = range(&name, raw.recharge, 0, 65_535, "recharge は 0..=65535 です")?;
            let mut spells = Vec::with_capacity(raw.spells.len());
            for spell in &raw.spells {
                let full = resolve_spell(&self.spells, namespace, spell).ok_or_else(|| {
                    DefsError::UnknownSpell { owner: name.clone(), spell: spell.clone() }
                })?;
                spells.push(full);
            }
            let def = WandDef {
                name,
                spells,
                mana_max: mana_max as u32,
                mana_regen: mana_regen as u16,
                recharge: recharge as u16,
            };
            place("杖", &mut self.wands, |w| &w.name, &mut seen, raw.replace, def)?;
        }
        if self.wands.len() > MAX_WANDS {
            return Err(DefsError::TooMany("杖"));
        }
        Ok(())
    }

    fn layer_enemies(&mut self, text: &str, namespace: Option<&str>) -> Result<(), DefsError> {
        let raws: Vec<RawEnemy> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "enemies.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("敵", &raw.name, raw.replace, namespace)?;
            let width = range(&name, raw.width, 1, 32, "width は 1..=32 です")?;
            let height = range(&name, raw.height, 1, 32, "height は 1..=32 です")?;
            let max_hp = range(&name, raw.max_hp, 1, 10_000, "max_hp は 1..=10000 です")?;
            let walk = range(&name, raw.walk_speed, 0, 96, "walk_speed は 0..=96 です")?;
            let jump = range(&name, raw.jump_speed, 0, 96, "jump_speed は 0..=96 です")?;
            let sight = range(&name, raw.sight, 0, 1024, "sight は 0..=1024 です")?;
            let interval =
                range(&name, raw.attack_interval, 0, 3600, "attack_interval は 0..=3600 です")?;
            let contact =
                range(&name, raw.contact_damage, 0, 1000, "contact_damage は 0..=1000 です")?;
            let attack = match &raw.attack {
                Some(spell) => {
                    if interval == 0 {
                        return Err(DefsError::Invalid {
                            owner: name,
                            reason: "attack があるとき attack_interval は 1 以上です",
                        });
                    }
                    Some(resolve_spell(&self.spells, namespace, spell).ok_or_else(|| {
                        DefsError::UnknownSpell { owner: name.clone(), spell: spell.clone() }
                    })?)
                }
                None => None,
            };
            let def = EnemyDef {
                name,
                width: width as u8,
                height: height as u8,
                max_hp: max_hp as i32,
                walk_speed: walk as i16,
                jump_speed: jump as i16,
                sight: sight as u16,
                attack,
                attack_interval: interval as u16,
                contact_damage: contact as i32,
            };
            place("敵", &mut self.enemies, |e| &e.name, &mut seen, raw.replace, def)?;
        }
        if self.enemies.len() > MAX_ENEMIES {
            return Err(DefsError::TooMany("敵"));
        }
        Ok(())
    }

    fn layer_hooks(&mut self, text: &str, namespace: Option<&str>) -> Result<(), DefsError> {
        let raws: Vec<RawHook> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "hooks.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("道具", &raw.name, raw.replace, namespace)?;
            let max_length = range(&name, raw.max_length, 4, 400, "max_length は 4..=400 です")?;
            let min_length = range(&name, raw.min_length, 2, 64, "min_length は 2..=64 です")?;
            if min_length > max_length {
                return Err(DefsError::Invalid {
                    owner: name,
                    reason: "min_length は max_length 以下です",
                });
            }
            let speed = range(&name, raw.speed, 16, 512, "speed は 16..=512 です")?;
            let segments = range(&name, raw.segments, 4, 64, "segments は 4..=64 です")?;
            let gravity = range(&name, raw.gravity, 0, 64, "gravity は 0..=64 です")?;
            let iterations = range(&name, raw.iterations, 1, 32, "iterations は 1..=32 です")?;
            let reel = range(&name, raw.reel_speed, 0, 96, "reel_speed は 0..=96 です")?;
            let pull = range(&name, raw.pull_accel, 0, 64, "pull_accel は 0..=64 です")?;
            let pull_max = range(&name, raw.pull_max, 1, 192, "pull_max は 1..=192 です")?;
            let cooldown = range(&name, raw.cooldown, 0, 600, "cooldown は 0..=600 です")?;
            let def = HookDef {
                name,
                max_length: max_length as u16,
                min_length: min_length as u8,
                speed: speed as u16,
                segments: segments as u8,
                gravity: gravity as u8,
                iterations: iterations as u8,
                reel_speed: reel as u8,
                pull_accel: pull as u8,
                pull_max: pull_max as u8,
                cooldown: cooldown as u16,
            };
            place("道具", &mut self.hooks, |h| &h.name, &mut seen, raw.replace, def)?;
        }
        if self.hooks.len() > MAX_HOOKS {
            return Err(DefsError::TooMany("道具"));
        }
        Ok(())
    }

    fn layer_digs(&mut self, text: &str, namespace: Option<&str>) -> Result<(), DefsError> {
        let raws: Vec<RawDig> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "digs.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("掘る道具", &raw.name, raw.replace, namespace)?;
            let power = range(&name, raw.power, 1, 10_000, "power は 1..=10000 です")?;
            let radius = range(&name, raw.radius, 1, 24, "radius は 1..=24 です")?;
            let reach = range(&name, raw.range, 1, 400, "range は 1..=400 です")?;
            let recover = range(&name, raw.recover, 0, 10_000, "recover は 0..=10000 です")?;
            let def = DigDef {
                name,
                power: power as u16,
                radius: radius as u8,
                range: reach as u16,
                recover: recover as u16,
            };
            place("掘る道具", &mut self.digs, |d| &d.name, &mut seen, raw.replace, def)?;
        }
        if self.digs.len() > MAX_DIGS {
            return Err(DefsError::TooMany("掘る道具"));
        }
        Ok(())
    }

    pub fn dig(&self, name: &str) -> Option<&DigDef> {
        self.digs.iter().find(|d| d.name == name)
    }

    pub fn dig_names(&self) -> impl Iterator<Item = &str> {
        self.digs.iter().map(|d| d.name.as_str())
    }

    pub fn hook(&self, name: &str) -> Option<&HookDef> {
        self.hooks.iter().find(|h| h.name == name)
    }

    pub fn hook_names(&self) -> impl Iterator<Item = &str> {
        self.hooks.iter().map(|h| h.name.as_str())
    }

    pub fn wand_def(&self, name: &str) -> Option<&WandDef> {
        self.wands.iter().find(|w| w.name == name)
    }

    /// 名前から杖を作る。呪文は名前から `SpellId` を引くので、上書き後の中身で撃てる。
    pub fn wand(&self, name: &str) -> Result<Wand, DefsError> {
        let def = self.wand_def(name).ok_or_else(|| DefsError::UnknownWand(name.to_owned()))?;
        let mut ids = Vec::with_capacity(def.spells.len());
        for spell in &def.spells {
            ids.push(self.spells.id(spell).ok_or_else(|| DefsError::UnknownSpell {
                owner: def.name.clone(),
                spell: spell.clone(),
            })?);
        }
        Wand::new(ids, def.mana_max, def.mana_regen, def.recharge, &self.spells)
            .map_err(DefsError::Wand)
    }

    pub fn enemy(&self, name: &str) -> Option<&EnemyDef> {
        self.enemies.iter().find(|e| e.name == name)
    }

    pub fn wand_names(&self) -> impl Iterator<Item = &str> {
        self.wands.iter().map(|w| w.name.as_str())
    }

    pub fn enemy_names(&self) -> impl Iterator<Item = &str> {
        self.enemies.iter().map(|e| e.name.as_str())
    }

    /// 仕様 10 §7.2: 全定義の中身を FNV-1a 64-bit で流す。
    pub fn content_hash(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.bytes(&self.spells.content_hash().to_le_bytes());
        for w in &self.wands {
            hash.bytes(w.name.as_bytes());
            hash.bytes(&[0]);
            for spell in &w.spells {
                hash.bytes(spell.as_bytes());
                hash.bytes(&[0]);
            }
            hash.bytes(&w.mana_max.to_le_bytes());
            hash.bytes(&w.mana_regen.to_le_bytes());
            hash.bytes(&w.recharge.to_le_bytes());
        }
        hash.bytes(&[1]);
        for e in &self.enemies {
            hash.bytes(e.name.as_bytes());
            hash.bytes(&[0, e.width, e.height]);
            hash.bytes(&e.max_hp.to_le_bytes());
            hash.bytes(&e.walk_speed.to_le_bytes());
            hash.bytes(&e.jump_speed.to_le_bytes());
            hash.bytes(&e.sight.to_le_bytes());
            hash.bytes(e.attack.as_deref().unwrap_or("").as_bytes());
            hash.bytes(&[0]);
            hash.bytes(&e.attack_interval.to_le_bytes());
            hash.bytes(&e.contact_damage.to_le_bytes());
        }
        hash.bytes(&[2]);
        for h in &self.hooks {
            hash.bytes(h.name.as_bytes());
            hash.bytes(&[0]);
            hash.bytes(&h.max_length.to_le_bytes());
            hash.bytes(&[h.min_length]);
            hash.bytes(&h.speed.to_le_bytes());
            hash.bytes(&[
                h.segments,
                h.gravity,
                h.iterations,
                h.reel_speed,
                h.pull_accel,
                h.pull_max,
            ]);
            hash.bytes(&h.cooldown.to_le_bytes());
        }
        hash.bytes(&[3]);
        for d in &self.digs {
            hash.bytes(d.name.as_bytes());
            hash.bytes(&[0]);
            hash.bytes(&d.power.to_le_bytes());
            hash.bytes(&[d.radius]);
            hash.bytes(&d.range.to_le_bytes());
            hash.bytes(&d.recover.to_le_bytes());
        }
        hash.finish()
    }
}
