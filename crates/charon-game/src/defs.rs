//! 仕様 10 §7: エンジンの既定の定義 (呪文・杖・敵) と、ゲーム側が重ねる追加・上書き。

use std::collections::BTreeSet;

use charon_core::cell::MaterialId;
use charon_core::material::{MaterialTable, Phase};
use serde::Deserialize;

use crate::items::{ArmorSlot, ItemDef, ItemId, ItemKind, Resist, WeaponClass};
use crate::wand::{Fnv, SpellError, SpellTable, Wand, WandError};

const ENGINE_SPELLS: &str = include_str!("../../../assets/spells.ron");
const ENGINE_WANDS: &str = include_str!("../../../assets/wands.ron");
const ENGINE_ENEMIES: &str = include_str!("../../../assets/enemies.ron");
const ENGINE_HOOKS: &str = include_str!("../../../assets/hooks.ron");
const ENGINE_DIGS: &str = include_str!("../../../assets/digs.ron");
const ENGINE_ITEMS: &str = include_str!("../../../assets/items.ron");
const ENGINE_RECIPES: &str = include_str!("../../../assets/recipes.ron");

const MAX_WANDS: usize = 256;
const MAX_ENEMIES: usize = 256;
const MAX_HOOKS: usize = 256;
const MAX_DIGS: usize = 256;
/// 定義のアイテムの数の上限 (`ItemId::def` の添字は `u16`)。
const MAX_ITEMS: usize = 1024;
const MAX_RECIPES: usize = 1024;

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
    UnknownItem { owner: String, item: String },
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
            Self::UnknownItem { owner, item } => {
                write!(f, "\"{owner}\" が未定義のアイテム \"{item}\" を参照しています")
            }
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
    /// 持っている武器のアイテムの名前 (仕様 22 §1)。生まれたときに装備する。
    pub weapon: Option<String>,
    /// 持っている防具のアイテムの名前。各枠で、いちばん強いものを装備する (自動装備、仕様 19 §6)。
    pub armor: Vec<String>,
    /// 倒したときに落とす材質のセル (名前と数。`cells:` の接頭辞は任意)。
    pub loot: Vec<(String, u32)>,
    /// ボスか (体力のバーが出る。倒すと `GameEvent::BossDefeated`)。
    pub boss: bool,
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
    #[serde(default)]
    weapon: Option<String>,
    #[serde(default)]
    armor: Vec<String>,
    #[serde(default)]
    loot: Vec<(String, i64)>,
    #[serde(default)]
    boss: bool,
}

#[derive(Deserialize)]
enum RawWeaponClass {
    Melee,
    Ranged,
    Wand,
}

#[derive(Deserialize)]
enum RawSlot {
    Head,
    Body,
    Legs,
    Accessory,
}

#[derive(Deserialize)]
enum RawKind {
    Misc,
    Tool {
        #[serde(default)]
        pick: i64,
        #[serde(default)]
        axe: i64,
    },
    Weapon {
        class: RawWeaponClass,
        #[serde(default)]
        damage: i64,
        #[serde(default)]
        wand: Option<String>,
    },
    Armor {
        slot: RawSlot,
        #[serde(default)]
        defense: i64,
        #[serde(default)]
        fire: i64,
        #[serde(default)]
        poison: i64,
        #[serde(default)]
        cold: i64,
    },
    Consumable {
        #[serde(default)]
        heal: i64,
        #[serde(default)]
        mana: i64,
    },
}

#[derive(Deserialize)]
struct RawItem {
    name: String,
    #[serde(default)]
    replace: bool,
    kind: RawKind,
}

#[derive(Deserialize)]
struct RawRecipe {
    name: String,
    #[serde(default)]
    replace: bool,
    inputs: Vec<(String, i64)>,
    output: (String, i64),
    #[serde(default)]
    station: Option<String>,
}

/// レシピ (仕様 19 §4)。入力・出力・作業台は、解決済みの `ItemId`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipeDef {
    pub name: String,
    pub inputs: Vec<(ItemId, u32)>,
    pub output: (ItemId, u32),
    /// 近くにあるべきブロック (作業台など) のアイテム。
    pub station: Option<ItemId>,
}

/// 1 つの層のファイル。`None` はその種類の層が空。
#[derive(Clone, Copy, Debug, Default)]
pub struct DefinitionFiles<'a> {
    pub spells: Option<&'a str>,
    pub wands: Option<&'a str>,
    pub enemies: Option<&'a str>,
    pub hooks: Option<&'a str>,
    pub digs: Option<&'a str>,
    pub items: Option<&'a str>,
    pub recipes: Option<&'a str>,
}

/// 呪文・杖・敵の定義。層で重ねる。
#[derive(Clone, Debug, Default)]
pub struct Definitions {
    pub spells: SpellTable,
    wands: Vec<WandDef>,
    enemies: Vec<EnemyDef>,
    hooks: Vec<HookDef>,
    digs: Vec<DigDef>,
    pub(crate) items: Vec<ItemDef>,
    recipes: Vec<RecipeDef>,
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
                items: Some(ENGINE_ITEMS),
                recipes: Some(ENGINE_RECIPES),
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
        if let Some(text) = files.items {
            next.layer_items(text, namespace)?;
        }
        if let Some(text) = files.recipes {
            next.layer_recipes(text, materials, namespace)?;
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
            let mut loot = Vec::with_capacity(raw.loot.len());
            for (item, count) in &raw.loot {
                let count = range(&name, *count, 1, 99_999, "loot の個数は 1..=99999 です")?;
                loot.push((item.clone(), count as u32));
            }
            range(&name, raw.armor.len() as i64, 0, 8, "armor は 8 個までです")?;
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
                weapon: raw.weapon,
                armor: raw.armor,
                loot,
                boss: raw.boss,
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

    fn layer_items(&mut self, text: &str, namespace: Option<&str>) -> Result<(), DefsError> {
        let raws: Vec<RawItem> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "items.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("アイテム", &raw.name, raw.replace, namespace)?;
            let kind = match raw.kind {
                RawKind::Misc => ItemKind::Misc,
                RawKind::Tool { pick, axe } => ItemKind::Tool {
                    pick: range(&name, pick, 0, 1000, "pick は 0..=1000 です")? as u16,
                    axe: range(&name, axe, 0, 1000, "axe は 0..=1000 です")? as u16,
                },
                RawKind::Weapon { class, damage, wand } => {
                    let damage =
                        range(&name, damage, 0, 10_000, "damage は 0..=10000 です")? as u16;
                    let class = match class {
                        RawWeaponClass::Melee => WeaponClass::Melee,
                        RawWeaponClass::Ranged => WeaponClass::Ranged,
                        RawWeaponClass::Wand => WeaponClass::Wand,
                    };
                    let wand = match (class, wand) {
                        (WeaponClass::Wand, Some(wand)) => {
                            let full = namespace
                                .map(|id| format!("{id}:{wand}"))
                                .filter(|full| self.wands.iter().any(|w| &w.name == full))
                                .or_else(|| {
                                    self.wands.iter().any(|w| w.name == wand).then(|| wand.clone())
                                })
                                .ok_or(DefsError::UnknownWand(wand))?;
                            Some(full)
                        }
                        (WeaponClass::Wand, None) => {
                            return Err(DefsError::Invalid {
                                owner: name,
                                reason: "杖の武器には wand (杖の名前) が必要です",
                            });
                        }
                        (_, Some(_)) => {
                            return Err(DefsError::Invalid {
                                owner: name,
                                reason: "wand は杖の武器にだけ書けます",
                            });
                        }
                        (_, None) => None,
                    };
                    ItemKind::Weapon { class, damage, wand }
                }
                RawKind::Armor { slot, defense, fire, poison, cold } => {
                    let defense =
                        range(&name, defense, 0, 1000, "defense は 0..=1000 です")? as u16;
                    let resist = Resist {
                        fire: range(&name, fire, 0, 100, "耐性は 0..=100 です")? as u8,
                        poison: range(&name, poison, 0, 100, "耐性は 0..=100 です")? as u8,
                        cold: range(&name, cold, 0, 100, "耐性は 0..=100 です")? as u8,
                    };
                    let slot = match slot {
                        RawSlot::Head => ArmorSlot::Head,
                        RawSlot::Body => ArmorSlot::Body,
                        RawSlot::Legs => ArmorSlot::Legs,
                        RawSlot::Accessory => ArmorSlot::Accessory,
                    };
                    ItemKind::Armor { slot, defense, resist }
                }
                RawKind::Consumable { heal, mana } => ItemKind::Consumable {
                    heal: range(&name, heal, 0, 10_000, "heal は 0..=10000 です")? as u16,
                    mana: range(&name, mana, 0, 10_000, "mana は 0..=10000 です")? as u16,
                },
            };
            place(
                "アイテム",
                &mut self.items,
                |d| &d.name,
                &mut seen,
                raw.replace,
                ItemDef { name, kind },
            )?;
        }
        if self.items.len() > MAX_ITEMS {
            return Err(DefsError::TooMany("アイテム"));
        }
        Ok(())
    }

    fn layer_recipes(
        &mut self,
        text: &str,
        materials: &MaterialTable,
        namespace: Option<&str>,
    ) -> Result<(), DefsError> {
        let raws: Vec<RawRecipe> = ron::from_str(text)
            .map_err(|e| DefsError::Parse { file: "recipes.ron", reason: e.to_string() })?;
        let mut seen = BTreeSet::new();
        for raw in raws {
            let name = qualify("レシピ", &raw.name, raw.replace, namespace)?;
            range(&name, raw.inputs.len() as i64, 1, 8, "入力は 1..=8 種類です")?;
            let resolve = |item: &str| -> Result<ItemId, DefsError> {
                self.resolve_item(item, materials, namespace).ok_or_else(|| {
                    DefsError::UnknownItem { owner: name.clone(), item: item.to_owned() }
                })
            };
            let mut inputs = Vec::with_capacity(raw.inputs.len());
            for (item, count) in &raw.inputs {
                let count = range(&name, *count, 1, 99_999, "個数は 1..=99999 です")? as u32;
                inputs.push((resolve(item)?, count));
            }
            let out_count =
                range(&name, raw.output.1, 1, 99_999, "出力の個数は 1..=99999 です")? as u32;
            let output = (resolve(&raw.output.0)?, out_count);
            let station = raw.station.as_deref().map(resolve).transpose()?;
            let def = RecipeDef { name, inputs, output, station };
            place("レシピ", &mut self.recipes, |d| &d.name, &mut seen, raw.replace, def)?;
        }
        if self.recipes.len() > MAX_RECIPES {
            return Err(DefsError::TooMany("レシピ"));
        }
        Ok(())
    }

    /// 名前 (`cells:<材質>`・`block:<材質>`・定義の名前) を、レイヤーの名前空間も試して `ItemId` にする。
    fn resolve_item(
        &self,
        name: &str,
        materials: &MaterialTable,
        namespace: Option<&str>,
    ) -> Option<ItemId> {
        if let Some(material) = name.strip_prefix("cells:") {
            let id = materials.id(material).filter(|&id| usable(materials, id))?;
            return Some(ItemId::cells(id));
        }
        if let Some(material) = name.strip_prefix("block:") {
            let id = materials.id(material).filter(|&id| blockable(materials, id))?;
            return Some(ItemId::block(id));
        }
        let index = namespace
            .map(|id| format!("{id}:{name}"))
            .and_then(|full| self.items.iter().position(|d| d.name == full))
            .or_else(|| self.items.iter().position(|d| d.name == name))?;
        u16::try_from(index).ok().map(ItemId::def)
    }

    /// 名前から `ItemId` (材質は `materials` で引く)。知らない名前は `None`。
    pub fn item_id(&self, name: &str, materials: &MaterialTable) -> Option<ItemId> {
        self.resolve_item(name, materials, None)
    }

    /// `ItemId` の名前 (保存・表示用)。知らないものは `None`。
    pub fn item_name(&self, item: ItemId, materials: &MaterialTable) -> Option<String> {
        use crate::items::ItemClass;
        match item.class() {
            ItemClass::Cells(m) => {
                usable(materials, m).then(|| format!("cells:{}", materials.get(m).name))
            }
            ItemClass::Block(m) => {
                blockable(materials, m).then(|| format!("block:{}", materials.get(m).name))
            }
            ItemClass::Def(index) => self.items.get(index).map(|d| d.name.clone()),
        }
    }

    /// 定義のアイテム (`ItemId::def`) の定義。セル・ブロックは `None`。
    pub fn item_def(&self, item: ItemId) -> Option<&ItemDef> {
        match item.class() {
            crate::items::ItemClass::Def(index) => self.items.get(index),
            _ => None,
        }
    }

    pub fn item_names(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|d| d.name.as_str())
    }

    pub fn recipes(&self) -> &[RecipeDef] {
        &self.recipes
    }

    pub fn recipe(&self, name: &str) -> Option<&RecipeDef> {
        self.recipes.iter().find(|r| r.name == name)
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
            // 装備・戦利品・ボスは、あるときだけ流す (なければ、これまでと同じ値)。
            if e.weapon.is_some() || !e.armor.is_empty() || !e.loot.is_empty() || e.boss {
                hash.bytes(&[0xEE]);
                hash.bytes(e.weapon.as_deref().unwrap_or("").as_bytes());
                for a in &e.armor {
                    hash.bytes(&[0]);
                    hash.bytes(a.as_bytes());
                }
                for (item, count) in &e.loot {
                    hash.bytes(&[1]);
                    hash.bytes(item.as_bytes());
                    hash.bytes(&count.to_le_bytes());
                }
                hash.bytes(&[u8::from(e.boss)]);
            }
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
        hash.bytes(&[4]);
        for item in &self.items {
            hash.bytes(item.name.as_bytes());
            hash.bytes(&[0]);
            match &item.kind {
                ItemKind::Misc => hash.bytes(&[0]),
                ItemKind::Tool { pick, axe } => {
                    hash.bytes(&[1]);
                    hash.bytes(&pick.to_le_bytes());
                    hash.bytes(&axe.to_le_bytes());
                }
                ItemKind::Weapon { class, damage, wand } => {
                    hash.bytes(&[2, *class as u8]);
                    hash.bytes(&damage.to_le_bytes());
                    hash.bytes(wand.as_deref().unwrap_or("").as_bytes());
                    hash.bytes(&[0]);
                }
                ItemKind::Armor { slot, defense, resist } => {
                    hash.bytes(&[3, *slot as u8]);
                    hash.bytes(&defense.to_le_bytes());
                    hash.bytes(&[resist.fire, resist.poison, resist.cold]);
                }
                ItemKind::Consumable { heal, mana } => {
                    hash.bytes(&[4]);
                    hash.bytes(&heal.to_le_bytes());
                    hash.bytes(&mana.to_le_bytes());
                }
            }
        }
        hash.bytes(&[5]);
        for r in &self.recipes {
            hash.bytes(r.name.as_bytes());
            hash.bytes(&[0]);
            for (item, count) in &r.inputs {
                hash.bytes(&item.0.to_le_bytes());
                hash.bytes(&count.to_le_bytes());
            }
            hash.bytes(&[1]);
            hash.bytes(&r.output.0.0.to_le_bytes());
            hash.bytes(&r.output.1.to_le_bytes());
            hash.bytes(&r.station.map_or(u32::MAX, |s| s.0).to_le_bytes());
        }
        hash.finish()
    }
}

/// セルのアイテムにできる材質 (空・境界を除く)。
fn usable(materials: &MaterialTable, id: MaterialId) -> bool {
    id != MaterialId::EMPTY && id != MaterialId::BOUNDARY && (id.0 as usize) < materials.len()
}

/// ブロックにできる材質 (固体・粉体、仕様 17 §2)。
fn blockable(materials: &MaterialTable, id: MaterialId) -> bool {
    usable(materials, id) && matches!(materials.get(id).phase, Phase::Solid | Phase::Powder)
}
