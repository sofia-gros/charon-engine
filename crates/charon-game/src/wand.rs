//! 仕様 10 §5: 呪文の表と杖 (マナ・詠唱間隔・再充填)。

use std::collections::BTreeSet;

use charon_core::material::MaterialTable;
use charon_core::rng;
use charon_core::world::World;
use serde::Deserialize;

use crate::projectile::{Payload, ProjectileError, ProjectileId, ProjectileWorld};

/// 呪文の散らしに使う乱数の salt。
pub const SALT_SPREAD: u32 = 0x5350_5244;
const MAX_SPELLS: usize = 256;
const MAX_WAND_SPELLS: usize = 16;
const MAX_AIM: i32 = 1_000_000;

/// `SpellTable` の添字 (記載順)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpellId(pub u16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpellError {
    Parse(String),
    Duplicate(String),
    UnknownMaterial { spell: String, material: String },
    Invalid { spell: String, reason: &'static str },
    Count(usize),
}

impl std::fmt::Display for SpellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "呪文の定義を読めません: {e}"),
            Self::Duplicate(n) => write!(f, "呪文 \"{n}\" が重複しています"),
            Self::UnknownMaterial { spell, material } => {
                write!(f, "呪文 \"{spell}\" が未定義の材質 \"{material}\" を参照しています")
            }
            Self::Invalid { spell, reason } => write!(f, "呪文 \"{spell}\": {reason}"),
            Self::Count(n) => write!(f, "呪文は 1..={MAX_SPELLS} 個です (実際は {n} 個)"),
        }
    }
}

impl std::error::Error for SpellError {}

#[derive(Deserialize)]
enum PayloadDef {
    None,
    Explode { radius: i64 },
    Deposit { material: String },
}

#[derive(Deserialize)]
struct SpellDef {
    name: String,
    mana_cost: i64,
    cast_delay: i64,
    speed: i64,
    gravity: i64,
    life: i64,
    damage: i64,
    spread: i64,
    payload: PayloadDef,
}

/// 検査済みの呪文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spell {
    pub name: String,
    pub mana_cost: u32,
    pub cast_delay: u16,
    pub speed: i16,
    pub gravity: i16,
    pub life: u16,
    pub damage: i32,
    pub spread: u8,
    pub payload: Payload,
}

#[derive(Clone, Debug, Default)]
pub struct SpellTable {
    spells: Vec<Spell>,
}

fn invalid(spell: &str, reason: &'static str) -> SpellError {
    SpellError::Invalid { spell: spell.to_owned(), reason }
}

impl SpellTable {
    /// 仕様 10 §5.1。`namespace` が `Some(id)` なら名前を `id:name` にする。
    pub fn parse(
        text: &str,
        materials: &MaterialTable,
        namespace: Option<&str>,
    ) -> Result<Self, SpellError> {
        let defs: Vec<SpellDef> =
            ron::from_str(text).map_err(|e| SpellError::Parse(e.to_string()))?;
        if !(1..=MAX_SPELLS).contains(&defs.len()) {
            return Err(SpellError::Count(defs.len()));
        }
        let mut seen = BTreeSet::new();
        let mut spells = Vec::with_capacity(defs.len());
        for def in defs {
            let name = def.name;
            if !(1..=64).contains(&name.len())
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(invalid(&name, "名前は ASCII 英数字と _ の 1..=64 文字です"));
            }
            let full = match namespace {
                Some(id) => format!("{id}:{name}"),
                None => name.clone(),
            };
            if !seen.insert(full.clone()) {
                return Err(SpellError::Duplicate(full));
            }
            let range = |value: i64, lo: i64, hi: i64, reason: &'static str| {
                if (lo..=hi).contains(&value) { Ok(value) } else { Err(invalid(&name, reason)) }
            };
            let mana_cost = range(def.mana_cost, 0, 10_000, "mana_cost は 0..=10000 です")?;
            let cast_delay = range(def.cast_delay, 0, 600, "cast_delay は 0..=600 です")?;
            let speed = range(def.speed, 1, 512, "speed は 1..=512 です")?;
            let gravity = range(def.gravity, -64, 64, "gravity は -64..=64 です")?;
            let life = range(def.life, 1, 3600, "life は 1..=3600 です")?;
            let damage = range(def.damage, 0, 10_000, "damage は 0..=10000 です")?;
            let spread = range(def.spread, 0, 128, "spread は 0..=128 です")?;
            let payload = match def.payload {
                PayloadDef::None => Payload::None,
                PayloadDef::Explode { radius } => {
                    let radius = range(radius, 1, 16, "爆発半径は 1..=16 です")?;
                    Payload::Explode { radius: radius as u8 }
                }
                PayloadDef::Deposit { material } => {
                    let id =
                        materials.id(&material).ok_or_else(|| SpellError::UnknownMaterial {
                            spell: name.clone(),
                            material: material.clone(),
                        })?;
                    Payload::Deposit { material: id }
                }
            };
            // 範囲は上で検査済みなので、以下の変換は切り捨てを起こさない。
            spells.push(Spell {
                name: full,
                mana_cost: mana_cost as u32,
                cast_delay: cast_delay as u16,
                speed: speed as i16,
                gravity: gravity as i16,
                life: life as u16,
                damage: damage as i32,
                spread: spread as u8,
                payload,
            });
        }
        Ok(Self { spells })
    }

    pub fn len(&self) -> usize {
        self.spells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spells.is_empty()
    }

    pub fn get(&self, id: SpellId) -> Option<&Spell> {
        self.spells.get(usize::from(id.0))
    }

    pub fn id(&self, name: &str) -> Option<SpellId> {
        self.spells.iter().position(|s| s.name == name).map(|i| SpellId(i as u16))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Spell> {
        self.spells.iter()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WandError {
    SpellCount,
    UnknownSpell(SpellId),
    InvalidMana,
}

impl std::fmt::Display for WandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpellCount => write!(f, "杖の呪文は 1..={MAX_WAND_SPELLS} 個です"),
            Self::UnknownSpell(id) => write!(f, "呪文 {} は表にありません", id.0),
            Self::InvalidMana => write!(f, "mana_max は 1..=10000 です"),
        }
    }
}

impl std::error::Error for WandError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastError {
    InvalidAim,
    Projectile(ProjectileError),
}

impl std::fmt::Display for CastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAim => write!(f, "向きのベクトルが 0、または大きすぎます"),
            Self::Projectile(e) => write!(f, "弾を作れません: {e}"),
        }
    }
}

impl std::error::Error for CastError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastOutcome {
    Fired(ProjectileId),
    /// 詠唱間隔の待ち。
    Cooling,
    NoMana,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wand {
    pub spells: Vec<SpellId>,
    pub mana: u32,
    pub mana_max: u32,
    pub mana_regen: u16,
    pub recharge: u16,
    cursor: usize,
    cooldown: u16,
}

impl Wand {
    pub fn new(
        spells: Vec<SpellId>,
        mana_max: u32,
        mana_regen: u16,
        recharge: u16,
        table: &SpellTable,
    ) -> Result<Self, WandError> {
        if !(1..=MAX_WAND_SPELLS).contains(&spells.len()) {
            return Err(WandError::SpellCount);
        }
        if let Some(&id) = spells.iter().find(|&&id| table.get(id).is_none()) {
            return Err(WandError::UnknownSpell(id));
        }
        if !(1..=10_000).contains(&mana_max) {
            return Err(WandError::InvalidMana);
        }
        Ok(Self { spells, mana: mana_max, mana_max, mana_regen, recharge, cursor: 0, cooldown: 0 })
    }

    /// 次に撃つ呪文の位置。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 次に撃てるまでの tick。
    pub fn cooldown(&self) -> u16 {
        self.cooldown
    }

    /// 1 tick に 1 回、`cast` より前に呼ぶ。
    pub fn tick(&mut self) {
        self.mana = self.mana.saturating_add(u32::from(self.mana_regen)).min(self.mana_max);
        self.cooldown = self.cooldown.saturating_sub(1);
    }

    /// 仕様 10 §5.2。失敗・見送りのときは杖も弾も変えない。
    #[allow(clippy::too_many_arguments)] // 仕様 10 §5.2 の引数をそのまま並べる。
    pub fn cast(
        &mut self,
        table: &SpellTable,
        projectiles: &mut ProjectileWorld,
        world: &World,
        muzzle: (i64, i64),
        aim: (i32, i32),
        tick: u64,
        seed: u64,
    ) -> Result<CastOutcome, CastError> {
        if self.cooldown > 0 {
            return Ok(CastOutcome::Cooling);
        }
        // `new` で検査済み。表が差し替えられて見つからない場合は撃たない。
        let Some(spell) = self.spells.get(self.cursor).and_then(|&id| table.get(id)) else {
            return Ok(CastOutcome::NoMana);
        };
        if self.mana < spell.mana_cost {
            return Ok(CastOutcome::NoMana);
        }
        if (aim.0 == 0 && aim.1 == 0) || aim.0.abs() > MAX_AIM || aim.1.abs() > MAX_AIM {
            return Err(CastError::InvalidAim);
        }
        let (ax, ay) = (i64::from(aim.0), i64::from(aim.1));
        let len = ((ax * ax + ay * ay) as u64).isqrt().max(1) as i64;
        let speed = i64::from(spell.speed);
        let (mut vx, mut vy) = (ax * speed / len, ay * speed / len);
        if spell.spread > 0 {
            let spread = i64::from(spell.spread);
            let cell = (muzzle.0.div_euclid(16) as i32, muzzle.1.div_euclid(16) as i32);
            let h = rng::hash(seed, tick, cell.0, cell.1, SALT_SPREAD);
            let off = i64::from(h) % (2 * spread + 1) - spread;
            vx += -ay * speed * off / (256 * len);
            vy += ax * speed * off / (256 * len);
        }
        let clamp = |v: i64| v.clamp(-512, 512) as i16;
        let id = projectiles
            .spawn(
                world,
                muzzle.0,
                muzzle.1,
                clamp(vx),
                clamp(vy),
                spell.gravity,
                spell.life,
                spell.damage,
                spell.payload,
            )
            .map_err(CastError::Projectile)?;
        self.mana -= spell.mana_cost;
        self.cooldown = spell.cast_delay;
        self.cursor = (self.cursor + 1) % self.spells.len();
        if self.cursor == 0 {
            self.cooldown = self.cooldown.saturating_add(self.recharge);
        }
        Ok(CastOutcome::Fired(id))
    }

    /// 仕様 10 §5.2: FNV-1a 64-bit。整数はリトルエンディアン。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        };
        for id in &self.spells {
            feed(&id.0.to_le_bytes());
        }
        feed(&self.mana.to_le_bytes());
        feed(&self.mana_max.to_le_bytes());
        feed(&self.mana_regen.to_le_bytes());
        feed(&self.recharge.to_le_bytes());
        feed(&(self.cursor as u64).to_le_bytes());
        feed(&self.cooldown.to_le_bytes());
        hash
    }
}
