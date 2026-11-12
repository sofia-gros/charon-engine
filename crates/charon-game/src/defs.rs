//! 仕様 10 §7: エンジンの既定の定義 (呪文・杖・敵) と、ゲーム側が重ねる追加・上書き。

use std::collections::BTreeSet;

use charon_core::material::MaterialTable;
use serde::Deserialize;

use crate::wand::{Fnv, SpellError, SpellTable, Wand, WandError};

const ENGINE_SPELLS: &str = include_str!("../../../assets/spells.ron");
const ENGINE_WANDS: &str = include_str!("../../../assets/wands.ron");
const ENGINE_ENEMIES: &str = include_str!("../../../assets/enemies.ron");

const MAX_WANDS: usize = 256;
const MAX_ENEMIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefsError {
    Parse { file: &'static str, reason: String },
    Spell(SpellError),
    Duplicate { kind: &'static str, name: String },
    ReplaceMissing { kind: &'static str, name: String },
    UnknownSpell { owner: String, spell: String },
    Invalid { owner: String, reason: &'static str },
    UnknownWand(String),
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
}

/// 呪文・杖・敵の定義。層で重ねる。
#[derive(Clone, Debug, Default)]
pub struct Definitions {
    pub spells: SpellTable,
    wands: Vec<WandDef>,
    enemies: Vec<EnemyDef>,
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
        hash.finish()
    }
}
