//! 仕様 07 §6: 明示されたディレクトリからだけデータ MOD を読み込む。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::coords::CellPos;
use charon_core::material::{MaterialError, MaterialTable};
use charon_core::reaction::{ReactionError, ReactionTable};
use charon_core::world::World;

use crate::defs::{DefinitionFiles, Definitions, DefsError};

/// `script.ron` は利用者が明示的に許可したときだけ読む。
#[derive(Clone, Debug)]
pub struct ModDirectory {
    pub path: PathBuf,
    pub script: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModManifest {
    pub id: String,
    pub version: u32,
}

#[derive(Debug)]
pub enum ModError {
    Read { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, reason: String },
    InvalidManifest(String),
    DuplicateId(String),
    Materials(MaterialError),
    Reactions(ReactionError),
    Definitions(DefsError),
    UnknownRuleMaterial(String),
    InvalidEvery(u16),
    RuleLimit,
}

impl std::fmt::Display for ModError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::InvalidManifest(id) => write!(f, "無効な MOD ID または version: {id}"),
            Self::DuplicateId(id) => write!(f, "重複する MOD ID: {id}"),
            Self::Materials(err) => write!(f, "{err}"),
            Self::Reactions(err) => write!(f, "{err}"),
            Self::Definitions(err) => write!(f, "{err}"),
            Self::UnknownRuleMaterial(name) => write!(f, "未定義の規則材質: {name}"),
            Self::InvalidEvery(every) => write!(f, "規則の every は 1..=1024: {every}"),
            Self::RuleLimit => write!(f, "1 tick の規則実行数が 1024 を超えました"),
        }
    }
}

impl std::error::Error for ModError {}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    every: u16,
    x: i32,
    y: i32,
    from: String,
    into: String,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub every: u16,
    pub x: i32,
    pub y: i32,
    pub from: MaterialId,
    pub into: MaterialId,
}

#[derive(Clone, Debug)]
pub struct ModPack {
    pub manifests: Vec<ModManifest>,
    pub materials: Arc<MaterialTable>,
    pub reactions: Arc<ReactionTable>,
    /// エンジンの既定に、MOD の呪文・杖・敵を MOD ID の昇順で重ねたもの (仕様 10 §7.3)。
    pub definitions: Definitions,
    rules: Vec<Rule>,
}

struct Source {
    manifest: ModManifest,
    materials: String,
    reactions: String,
    spells: Option<String>,
    wands: Option<String>,
    enemies: Option<String>,
    hooks: Option<String>,
    digs: Option<String>,
    script: Option<(PathBuf, String)>,
}

fn read(path: PathBuf) -> Result<String, ModError> {
    fs::read_to_string(&path).map_err(|source| ModError::Read { path, source })
}

/// なければ `None`。読めないときだけエラー。
fn read_optional(path: PathBuf) -> Result<Option<String>, ModError> {
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ModError::Read { path, source }),
    }
}

fn resolve(name: &str, id: &str, materials: &MaterialTable) -> Result<MaterialId, ModError> {
    let qualified = format!("{id}:{name}");
    materials
        .id(&qualified)
        .or_else(|| materials.id(name))
        .ok_or_else(|| ModError::UnknownRuleMaterial(name.to_string()))
}

impl ModPack {
    /// 起動時に一度、ディレクトリ名順で読み、すべての材質・反応を検証する。
    pub fn load(directories: &[ModDirectory]) -> Result<Self, ModError> {
        let mut ordered: Vec<_> = directories.iter().collect();
        ordered.sort_by(|a, b| {
            a.path.file_name().cmp(&b.path.file_name()).then_with(|| a.path.cmp(&b.path))
        });
        let mut sources = Vec::with_capacity(ordered.len());
        let mut ids = HashSet::new();
        for directory in ordered {
            let manifest_path = directory.path.join("manifest.ron");
            let manifest: ModManifest = ron::from_str(&read(manifest_path.clone())?)
                .map_err(|err| ModError::Parse { path: manifest_path, reason: err.to_string() })?;
            if manifest.version != 1
                || manifest.id.is_empty()
                || manifest.id.len() > 64
                || !manifest.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(ModError::InvalidManifest(manifest.id));
            }
            if !ids.insert(manifest.id.clone()) {
                return Err(ModError::DuplicateId(manifest.id));
            }
            sources.push(Source {
                manifest,
                materials: read(directory.path.join("materials.ron"))?,
                reactions: read(directory.path.join("reactions.ron"))?,
                spells: read_optional(directory.path.join("spells.ron"))?,
                wands: read_optional(directory.path.join("wands.ron"))?,
                enemies: read_optional(directory.path.join("enemies.ron"))?,
                hooks: read_optional(directory.path.join("hooks.ron"))?,
                digs: read_optional(directory.path.join("digs.ron"))?,
                script: if directory.script {
                    let path = directory.path.join("script.ron");
                    Some((path.clone(), read(path)?))
                } else {
                    None
                },
            });
        }
        let material_sources: Vec<_> = sources
            .iter()
            .map(|source| (source.manifest.id.as_str(), source.materials.as_str()))
            .collect();
        let materials =
            Arc::new(MaterialTable::from_mods(&material_sources).map_err(ModError::Materials)?);
        let reaction_sources: Vec<_> = sources
            .iter()
            .map(|source| (source.manifest.id.as_str(), source.reactions.as_str()))
            .collect();
        let reactions = Arc::new(
            ReactionTable::from_mods(&reaction_sources, &materials).map_err(ModError::Reactions)?,
        );
        let manifests = sources.iter().map(|source| source.manifest.clone()).collect();
        // 読み込みはディレクトリ順だが、実行は MOD ID 順・記載順。
        sources.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        let mut definitions =
            Definitions::engine_defaults(&materials).map_err(ModError::Definitions)?;
        for source in &sources {
            definitions
                .layer(
                    DefinitionFiles {
                        spells: source.spells.as_deref(),
                        wands: source.wands.as_deref(),
                        enemies: source.enemies.as_deref(),
                        hooks: source.hooks.as_deref(),
                        digs: source.digs.as_deref(),
                    },
                    &materials,
                    Some(&source.manifest.id),
                )
                .map_err(ModError::Definitions)?;
        }
        let mut rules = Vec::new();
        for source in sources {
            if let Some((path, text)) = source.script {
                let raw: Vec<RawRule> = ron::from_str(&text)
                    .map_err(|err| ModError::Parse { path, reason: err.to_string() })?;
                for rule in raw {
                    if !(1..=1024).contains(&rule.every) {
                        return Err(ModError::InvalidEvery(rule.every));
                    }
                    rules.push(Rule {
                        every: rule.every,
                        x: rule.x,
                        y: rule.y,
                        from: resolve(&rule.from, &source.manifest.id, &materials)?,
                        into: resolve(&rule.into, &source.manifest.id, &materials)?,
                    });
                }
            }
        }
        Ok(Self { manifests, materials, reactions, definitions, rules })
    }

    /// 同じ材質・反応表を使うワールドを作る。
    pub fn new_world(&self, seed: u64) -> World {
        World::with_reactions(Arc::clone(&self.materials), Arc::clone(&self.reactions), seed)
    }

    /// tick 開始時に呼ぶ。適用される 1025 件目には触れず、明示的なエラーを返す。
    pub fn apply_rules(&self, world: &mut World) -> Result<(), ModError> {
        let tick = world.tick();
        let mut executed = 0;
        for rule in &self.rules {
            if !tick.is_multiple_of(u64::from(rule.every)) {
                continue;
            }
            if executed == 1024 {
                return Err(ModError::RuleLimit);
            }
            executed += 1;
            let pos = CellPos::new(rule.x, rule.y);
            if world.get(pos).material == rule.from {
                world.convert(pos, rule.into);
            }
        }
        Ok(())
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}
