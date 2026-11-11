//! データで定義する隣接材質の反応 (仕様 03 §1)。

use std::fmt;

use crate::cell::MaterialId;
use crate::material::MaterialTable;

/// 反応の確率と、左側・右側それぞれの生成物。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reaction {
    pub prob: u8,
    pub out_a: MaterialId,
    pub out_b: MaterialId,
}

/// RON の読み込み時の検査エラー。
#[derive(Debug, PartialEq, Eq)]
pub enum ReactionError {
    Parse(String),
    TooManyMaterials,
    TooManyReactions,
    UnknownMaterial(String),
    DuplicatePair(String, String),
}

impl fmt::Display for ReactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "反応の定義を読めません: {error}"),
            Self::TooManyMaterials => write!(f, "反応表に使える材質数は 1024 以下です"),
            Self::TooManyReactions => write!(f, "反応の数が u16 の索引を超えました"),
            Self::UnknownMaterial(name) => {
                write!(f, "反応が未定義の材質 {name:?} を参照しています")
            }
            Self::DuplicatePair(a, b) => write!(f, "材質 {a:?} と {b:?} の組が重複しています"),
        }
    }
}

impl std::error::Error for ReactionError {}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReaction {
    a: String,
    b: String,
    prob: u8,
    out_a: String,
    out_b: String,
}

/// 添字 `a * n + b` で反応を定数時間で引ける表。逆順も生成物を入れ替えて登録する。
#[derive(Clone, Debug)]
pub struct ReactionTable {
    n: usize,
    index: Vec<u16>,
    reactions: Vec<Reaction>,
    reactive: Vec<bool>,
}

impl ReactionTable {
    /// 反応を持たない表を作る。独自の材質テーブルと組み合わせられる。
    pub fn empty(materials: &MaterialTable) -> Result<Self, ReactionError> {
        let n = materials.len();
        if n > 1024 {
            return Err(ReactionError::TooManyMaterials);
        }
        Ok(Self { n, index: vec![0; n * n], reactions: Vec::new(), reactive: vec![false; n] })
    }

    /// 材質名を解決して、両方向の反応を登録する。
    pub fn from_ron(src: &str, materials: &MaterialTable) -> Result<Self, ReactionError> {
        let raws: Vec<RawReaction> =
            ron::from_str(src).map_err(|e| ReactionError::Parse(e.to_string()))?;
        let mut table = Self::empty(materials)?;
        for raw in raws {
            let id = |name: &str| {
                materials.id(name).ok_or_else(|| ReactionError::UnknownMaterial(name.to_string()))
            };
            let (a, b) = (id(&raw.a)?, id(&raw.b)?);
            let (out_a, out_b) = (id(&raw.out_a)?, id(&raw.out_b)?);
            if table.index[table.key(a, b)] != 0 || table.index[table.key(b, a)] != 0 {
                return Err(ReactionError::DuplicatePair(raw.a, raw.b));
            }
            table.register(a, b, Reaction { prob: raw.prob, out_a, out_b })?;
            if a != b {
                table.register(b, a, Reaction { prob: raw.prob, out_a: out_b, out_b: out_a })?;
            }
        }
        Ok(table)
    }

    /// 同梱された反応を読み込む。
    pub fn builtin(materials: &MaterialTable) -> Result<Self, ReactionError> {
        Self::from_ron(include_str!("../../../assets/reactions.ron"), materials)
    }

    fn key(&self, a: MaterialId, b: MaterialId) -> usize {
        a.0 as usize * self.n + b.0 as usize
    }

    fn register(
        &mut self,
        a: MaterialId,
        b: MaterialId,
        reaction: Reaction,
    ) -> Result<(), ReactionError> {
        let next =
            u16::try_from(self.reactions.len() + 1).map_err(|_| ReactionError::TooManyReactions)?;
        let key = self.key(a, b);
        self.index[key] = next;
        self.reactive[a.0 as usize] = true;
        self.reactions.push(reaction);
        Ok(())
    }

    /// `a` が反応の主材質になり得るか。
    pub fn is_reactive(&self, a: MaterialId) -> bool {
        self.reactive.get(a.0 as usize).copied().unwrap_or(false)
    }

    /// 順序に対応した生成物を返す。材質 ID が範囲外なら反応なし。
    pub fn get(&self, a: MaterialId, b: MaterialId) -> Option<Reaction> {
        if a.0 as usize >= self.n || b.0 as usize >= self.n {
            return None;
        }
        let index = self.index[self.key(a, b)];
        if index == 0 { None } else { self.reactions.get(index as usize - 1).copied() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_lookup_swaps_outputs() {
        let m = MaterialTable::builtin().unwrap();
        let t = ReactionTable::builtin(&m).unwrap();
        let water = m.id("water").unwrap();
        let lava = m.id("lava").unwrap();
        let forward = t.get(water, lava).unwrap();
        let reverse = t.get(lava, water).unwrap();
        assert!(t.is_reactive(water) && t.is_reactive(lava));
        assert_eq!((forward.out_a, forward.out_b), (reverse.out_b, reverse.out_a));
        assert_eq!(forward.prob, reverse.prob);
    }

    #[test]
    fn rejects_bad_references_duplicates_and_large_tables() {
        let m = MaterialTable::builtin().unwrap();
        let src = r#"[(a: "water", b: "unknown", prob: 1, out_a: "empty", out_b: "empty")]"#;
        assert!(matches!(ReactionTable::from_ron(src, &m), Err(ReactionError::UnknownMaterial(_))));
        let duplicate = r#"[
            (a: "water", b: "lava", prob: 1, out_a: "empty", out_b: "empty"),
            (a: "lava", b: "water", prob: 1, out_a: "empty", out_b: "empty"),
        ]"#;
        assert!(matches!(
            ReactionTable::from_ron(duplicate, &m),
            Err(ReactionError::DuplicatePair(_, _))
        ));
        let same = r#"[
            (a: "water", b: "water", prob: 1, out_a: "empty", out_b: "empty"),
            (a: "water", b: "water", prob: 1, out_a: "empty", out_b: "empty"),
        ]"#;
        assert!(matches!(
            ReactionTable::from_ron(same, &m),
            Err(ReactionError::DuplicatePair(_, _))
        ));
        let mut definitions = String::from("[");
        for i in 0..1023 {
            definitions.push_str(&format!("(name: \"m{i}\", phase: Solid, color: (0,0,0,0)),"));
        }
        definitions.push(']');
        let many = MaterialTable::from_ron(&definitions).unwrap();
        assert!(matches!(ReactionTable::empty(&many), Err(ReactionError::TooManyMaterials)));
    }
}
