//! 材質 (仕様 01 §3)。
//!
//! 材質の性質はすべて [`Material`] に持たせる。シミュレーションのコードに特定の材質名を書いてはいけない。

use std::collections::HashMap;
use std::fmt;

use crate::cell::MaterialId;

/// 材質の相。移動のしかたを決める。
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
pub enum Phase {
    Empty,
    Solid,
    Powder,
    Liquid,
    Gas,
}

/// 温度による材質の変化。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Transition {
    pub at: i16,
    pub into: MaterialId,
}

/// 材質の性質。各フィールドの意味は仕様 01 §3 を参照。
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub phase: Phase,
    pub density: u16,
    pub color: [u8; 4],
    pub color_variation: u8,

    pub gravity: u8,
    pub max_speed: u8,
    pub friction: u8,
    pub dispersion: u8,
    pub viscosity: u8,

    pub initial_temp: i16,
    pub conductivity: u8,
    pub melt: Option<Transition>,
    pub freeze: Option<Transition>,
    pub boil: Option<Transition>,
    pub ignite_at: Option<i16>,
    pub burn_time: u8,
    pub burn_into: MaterialId,
    pub burn_temp: i16,
    pub lifetime: Option<(u8, u8)>,
    pub decay_into: MaterialId,

    pub rigid_friction: f32,
    pub restitution: f32,

    pub emission: [u8; 3],
    /// 掘るときの硬さ (仕様 10 §12)。掘る道具の力 (1 tick に与えるダメージ) の合計がこの値に達すると、セルが消える。
    /// 固体は 300、粉体は 20 が既定。液体・気体・空は掘れない (0)。`u16::MAX` は掘れない固体 (`boundary`)。
    pub hardness: u16,
}

/// 材質の読み込みで起きるエラー。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MaterialError {
    /// RON の構文または型の誤り。
    Parse(String),
    /// 同じ名前が 2 回定義された (組み込みの `empty`、`boundary` との重複を含む)。
    DuplicateName(String),
    /// 定義されていない材質への参照。
    UnknownReference { material: String, field: &'static str, target: String },
    /// 値の検査に違反した。
    Invalid { material: String, reason: &'static str },
    /// 材質数が上限を超えた。
    TooMany,
}

impl fmt::Display for MaterialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "材質の定義を読めません: {e}"),
            Self::DuplicateName(n) => write!(f, "材質 \"{n}\" が重複しています"),
            Self::UnknownReference { material, field, target } => {
                write!(
                    f,
                    "材質 \"{material}\" の {field} が未定義の材質 \"{target}\" を指しています"
                )
            }
            Self::Invalid { material, reason } => write!(f, "材質 \"{material}\": {reason}"),
            Self::TooMany => write!(f, "材質が多すぎます"),
        }
    }
}

impl std::error::Error for MaterialError {}

#[derive(serde::Deserialize)]
struct RawTransition {
    at: i16,
    into: String,
}

fn d_gravity() -> u8 {
    4
}
fn d_max_speed() -> u8 {
    96
}
fn d_temp() -> i16 {
    20
}
fn d_empty() -> String {
    "empty".to_string()
}
fn d_friction() -> f32 {
    0.6
}
fn d_restitution() -> f32 {
    0.1
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMaterial {
    name: String,
    phase: Phase,
    #[serde(default)]
    density: u16,
    color: [u8; 4],
    #[serde(default)]
    color_variation: u8,
    #[serde(default = "d_gravity")]
    gravity: u8,
    #[serde(default = "d_max_speed")]
    max_speed: u8,
    #[serde(default)]
    friction: u8,
    #[serde(default)]
    dispersion: u8,
    #[serde(default)]
    viscosity: u8,
    #[serde(default = "d_temp")]
    initial_temp: i16,
    #[serde(default)]
    conductivity: u8,
    #[serde(default)]
    melt: Option<RawTransition>,
    #[serde(default)]
    freeze: Option<RawTransition>,
    #[serde(default)]
    boil: Option<RawTransition>,
    #[serde(default)]
    ignite_at: Option<i16>,
    #[serde(default)]
    burn_time: u8,
    #[serde(default = "d_empty")]
    burn_into: String,
    #[serde(default)]
    burn_temp: i16,
    #[serde(default)]
    lifetime: Option<(u8, u8)>,
    #[serde(default = "d_empty")]
    decay_into: String,
    #[serde(default = "d_friction")]
    rigid_friction: f32,
    #[serde(default = "d_restitution")]
    restitution: f32,
    #[serde(default)]
    emission: [u8; 3],
    #[serde(default)]
    hardness: Option<u16>,
}

/// ローカル定義への短い参照だけを名前空間化する。組み込み名と他 MOD の完全修飾名は保つ。
pub(crate) fn namespace_reference(
    id: &str,
    local: &std::collections::HashSet<String>,
    name: &mut String,
) {
    if local.contains(name) {
        *name = format!("{id}:{name}");
    }
}

/// 組み込みの材質 (`empty`、`boundary`) の共通部分。
fn builtin_material(name: &str, phase: Phase, density: u16, conductivity: u8) -> Material {
    Material {
        name: name.to_string(),
        phase,
        density,
        color: [0, 0, 0, 0],
        color_variation: 0,
        gravity: d_gravity(),
        max_speed: d_max_speed(),
        friction: 0,
        dispersion: 0,
        viscosity: 0,
        initial_temp: d_temp(),
        conductivity,
        melt: None,
        freeze: None,
        boil: None,
        ignite_at: None,
        burn_time: 0,
        burn_into: MaterialId::EMPTY,
        burn_temp: 0,
        lifetime: None,
        decay_into: MaterialId::EMPTY,
        rigid_friction: d_friction(),
        restitution: d_restitution(),
        emission: [0, 0, 0],
        hardness: if name == "boundary" { u16::MAX } else { 0 },
    }
}

/// 全材質の表。添字は [`MaterialId`]。
#[derive(Clone, Debug)]
pub struct MaterialTable {
    materials: Vec<Material>,
    by_name: HashMap<String, MaterialId>,
    /// 起動時に一度だけ解決した炎の ID。定義がなければ炎を生成しない。
    pub fire: Option<MaterialId>,
}

impl MaterialTable {
    /// RON の文字列から読む。規則は仕様 01 §3。
    pub fn from_ron(src: &str) -> Result<Self, MaterialError> {
        let raws: Vec<RawMaterial> =
            ron::from_str(src).map_err(|e| MaterialError::Parse(e.to_string()))?;
        Self::from_raws(raws)
    }

    /// 組み込み材質の後ろに、渡された順で名前空間付きの MOD 材質を追加する。
    /// すべての定義を集めてから参照を解決するため、MOD 間の前方参照も使える。
    pub fn from_mods(mods: &[(&str, &str)]) -> Result<Self, MaterialError> {
        let mut raws: Vec<RawMaterial> =
            ron::from_str(include_str!("../../../assets/materials.ron"))
                .map_err(|e| MaterialError::Parse(e.to_string()))?;
        for &(id, src) in mods {
            let mut added: Vec<RawMaterial> =
                ron::from_str(src).map_err(|e| MaterialError::Parse(e.to_string()))?;
            let local: std::collections::HashSet<_> =
                added.iter().map(|material| material.name.as_str().to_owned()).collect();
            for material in &mut added {
                for transition in [&mut material.melt, &mut material.freeze, &mut material.boil]
                    .into_iter()
                    .flatten()
                {
                    namespace_reference(id, &local, &mut transition.into);
                }
                namespace_reference(id, &local, &mut material.burn_into);
                namespace_reference(id, &local, &mut material.decay_into);
                material.name = format!("{id}:{}", material.name);
            }
            raws.extend(added);
        }
        Self::from_raws(raws)
    }

    fn from_raws(raws: Vec<RawMaterial>) -> Result<Self, MaterialError> {
        if raws.len() + 2 > u16::MAX as usize {
            return Err(MaterialError::TooMany);
        }

        // 1 パス目: 名前に ID を割り当てる。
        let mut by_name: HashMap<String, MaterialId> = HashMap::new();
        by_name.insert("empty".to_string(), MaterialId::EMPTY);
        by_name.insert("boundary".to_string(), MaterialId::BOUNDARY);
        for (i, raw) in raws.iter().enumerate() {
            let id = MaterialId((i + 2) as u16);
            if by_name.insert(raw.name.clone(), id).is_some() {
                return Err(MaterialError::DuplicateName(raw.name.clone()));
            }
        }

        // 2 パス目: 検査し、参照を ID に解決する。
        let mut materials = vec![
            // 空気はゆっくり熱を運ぶ (仕様 03 §2)。
            builtin_material("empty", Phase::Empty, 0, 8),
            builtin_material("boundary", Phase::Solid, u16::MAX, 0),
        ];
        for raw in raws {
            let invalid = |reason| MaterialError::Invalid { material: raw.name.clone(), reason };
            if raw.max_speed > 127 {
                return Err(invalid("max_speed は 127 以下にする"));
            }
            if let Some((min, max)) = raw.lifetime
                && !(1 <= min && min <= max)
            {
                return Err(invalid("lifetime は 1 <= min <= max にする"));
            }
            if raw.ignite_at.is_some() && raw.burn_time == 0 {
                return Err(invalid("ignite_at があるなら burn_time は 1 以上にする"));
            }

            let resolve = |field: &'static str, target: &str| {
                by_name.get(target).copied().ok_or_else(|| MaterialError::UnknownReference {
                    material: raw.name.clone(),
                    field,
                    target: target.to_string(),
                })
            };
            let transition = |field: &'static str, t: &Option<RawTransition>| {
                t.as_ref()
                    .map(|t| Ok(Transition { at: t.at, into: resolve(field, &t.into)? }))
                    .transpose()
            };

            materials.push(Material {
                phase: raw.phase,
                density: raw.density,
                color: raw.color,
                color_variation: raw.color_variation,
                gravity: raw.gravity,
                max_speed: raw.max_speed,
                friction: raw.friction,
                dispersion: raw.dispersion,
                viscosity: raw.viscosity,
                initial_temp: raw.initial_temp,
                conductivity: raw.conductivity,
                melt: transition("melt", &raw.melt)?,
                freeze: transition("freeze", &raw.freeze)?,
                boil: transition("boil", &raw.boil)?,
                ignite_at: raw.ignite_at,
                burn_time: raw.burn_time,
                burn_into: resolve("burn_into", &raw.burn_into)?,
                burn_temp: raw.burn_temp,
                lifetime: raw.lifetime,
                decay_into: resolve("decay_into", &raw.decay_into)?,
                rigid_friction: raw.rigid_friction,
                restitution: raw.restitution,
                emission: raw.emission,
                hardness: match raw.phase {
                    Phase::Solid => raw.hardness.unwrap_or(300),
                    Phase::Powder => raw.hardness.unwrap_or(20),
                    _ => 0,
                },
                name: raw.name,
            });
        }

        // 高温で A -> B、低温で B -> A のしきい値が重なると毎 tick 振動する。
        for (i, material) in materials.iter().enumerate() {
            for hot in [material.boil, material.melt].into_iter().flatten() {
                let target = &materials[hot.into.0 as usize];
                if target.freeze.is_some_and(|cold| cold.into.0 as usize == i && cold.at >= hot.at)
                {
                    return Err(MaterialError::Invalid {
                        material: material.name.clone(),
                        reason: "相変化が往復して振動します",
                    });
                }
            }
        }

        let fire = by_name.get("fire").copied();
        Ok(Self { materials, by_name, fire })
    }

    /// リポジトリの `assets/materials.ron` を埋め込んで読む。
    pub fn builtin() -> Result<Self, MaterialError> {
        Self::from_ron(include_str!("../../../assets/materials.ron"))
    }

    /// 添字で引く。範囲外は `boundary` を返す。
    pub fn get(&self, id: MaterialId) -> &Material {
        self.materials.get(id.0 as usize).unwrap_or(&self.materials[1])
    }

    /// 名前から ID を引く。
    pub fn id(&self, name: &str) -> Option<MaterialId> {
        self.by_name.get(name).copied()
    }

    /// 材質数 (組み込みの 2 つを含む)。
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    /// 常に false (組み込みの材質が必ずある)。
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAND: &str = r#"(name: "sand", phase: Powder, density: 1600, color: (1, 2, 3, 255))"#;

    #[test]
    fn builtin_has_17_materials() {
        let t = MaterialTable::builtin().unwrap();
        assert_eq!(t.len(), 17);
        assert_eq!(t.id("empty"), Some(MaterialId::EMPTY));
        assert_eq!(t.id("boundary"), Some(MaterialId::BOUNDARY));
        assert_eq!(t.id("stone"), Some(MaterialId(2)));
    }

    /// 掘る硬さ (仕様 10 §12): 既定は固体 300・粉体 20・液体や気体は 0、`boundary` は掘れない。
    #[test]
    fn hardness_defaults_by_phase() {
        let t = MaterialTable::builtin().unwrap();
        let hardness = |name: &str| t.get(t.id(name).unwrap()).hardness;
        assert_eq!(hardness("stone"), 600, "石は materials.ron で 600");
        assert_eq!(hardness("ice"), 200);
        assert_eq!(hardness("wood"), 150);
        assert_eq!(hardness("sand"), 20, "粉体の既定");
        assert_eq!(hardness("water"), 0, "液体は掘れない");
        assert_eq!(hardness("steam"), 0, "気体は掘れない");
        assert_eq!(hardness("boundary"), u16::MAX);
        assert_eq!(hardness("empty"), 0);
        let custom = MaterialTable::from_ron(
            r#"[(name: "rock", phase: Solid, density: 2000, color: (1, 1, 1, 255), hardness: Some(1234)),
                (name: "soft", phase: Solid, density: 100, color: (1, 1, 1, 255)),
                (name: "mud", phase: Powder, density: 900, color: (1, 1, 1, 255), hardness: Some(5)),
                (name: "juice", phase: Liquid, density: 1000, color: (1, 1, 1, 255), hardness: Some(99))]"#,
        )
        .unwrap();
        let h = |name: &str| custom.get(custom.id(name).unwrap()).hardness;
        assert_eq!((h("rock"), h("soft"), h("mud")), (1234, 300, 5));
        assert_eq!(h("juice"), 0, "液体は指定しても掘れない");
    }

    #[test]
    fn builtin_materials_are_fixed() {
        let t = MaterialTable::from_ron("[]").unwrap();
        assert_eq!(t.len(), 2);
        let empty = t.get(MaterialId::EMPTY);
        assert_eq!((empty.phase, empty.density, empty.conductivity), (Phase::Empty, 0, 8));
        let boundary = t.get(MaterialId::BOUNDARY);
        assert_eq!((boundary.phase, boundary.density), (Phase::Solid, u16::MAX));
    }

    #[test]
    fn defaults_are_applied() {
        let t = MaterialTable::from_ron(&format!("[{SAND}]")).unwrap();
        let m = t.get(MaterialId(2));
        assert_eq!((m.gravity, m.max_speed, m.initial_temp), (4, 96, 20));
        assert_eq!((m.burn_into, m.decay_into), (MaterialId::EMPTY, MaterialId::EMPTY));
        assert_eq!(m.rigid_friction, 0.6);
    }

    #[test]
    fn references_are_resolved_in_any_order() {
        let src = r#"[
            (name: "water", phase: Liquid, color: (0, 0, 0, 0), boil: Some((at: 100, into: "steam"))),
            (name: "steam", phase: Gas, color: (0, 0, 0, 0)),
        ]"#;
        let t = MaterialTable::from_ron(src).unwrap();
        let water = t.get(t.id("water").unwrap());
        assert_eq!(water.boil, Some(Transition { at: 100, into: t.id("steam").unwrap() }));
    }

    #[test]
    fn out_of_range_id_is_boundary() {
        let t = MaterialTable::from_ron("[]").unwrap();
        assert_eq!(t.get(MaterialId(999)).name, "boundary");
    }

    #[test]
    fn unknown_reference_is_error() {
        let src = r#"[(name: "a", phase: Solid, color: (0, 0, 0, 0), decay_into: "nope")]"#;
        assert!(matches!(
            MaterialTable::from_ron(src),
            Err(MaterialError::UnknownReference { field: "decay_into", .. })
        ));
    }

    #[test]
    fn duplicate_name_is_error() {
        let src = format!("[{SAND}, {SAND}]");
        assert_eq!(
            MaterialTable::from_ron(&src).err(),
            Some(MaterialError::DuplicateName("sand".to_string()))
        );
        let src = r#"[(name: "empty", phase: Solid, color: (0, 0, 0, 0))]"#;
        assert!(matches!(MaterialTable::from_ron(src), Err(MaterialError::DuplicateName(_))));
    }

    #[test]
    fn invalid_values_are_errors() {
        for bad in [
            r#"(name: "a", phase: Solid, color: (0, 0, 0, 0), max_speed: 128)"#,
            r#"(name: "a", phase: Solid, color: (0, 0, 0, 0), lifetime: Some((0, 3)))"#,
            r#"(name: "a", phase: Solid, color: (0, 0, 0, 0), lifetime: Some((5, 3)))"#,
            r#"(name: "a", phase: Solid, color: (0, 0, 0, 0), ignite_at: Some(100))"#,
        ] {
            let r = MaterialTable::from_ron(&format!("[{bad}]"));
            assert!(matches!(r, Err(MaterialError::Invalid { .. })), "{bad}");
        }
    }

    #[test]
    fn unknown_field_is_error() {
        let src = r#"[(name: "a", phase: Solid, color: (0, 0, 0, 0), typo: 1)]"#;
        assert!(matches!(MaterialTable::from_ron(src), Err(MaterialError::Parse(_))));
    }
}
