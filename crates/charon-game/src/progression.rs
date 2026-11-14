//! 仕様 26 §5: 世界共有の、名前で記録する進行。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// 1 種類の進行について保存できる名前の上限。
pub const PROGRESS_MAX: usize = 256;
/// 1 つの名前の UTF-8 バイト数の上限。
pub const NAME_MAX: usize = 128;

/// `game.ron` の進行。`Vec` は既存の名前による保存形式と同じ表現。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressSave {
    #[serde(default)]
    pub bosses: Vec<String>,
    #[serde(default)]
    pub recipes: Vec<String>,
    #[serde(default)]
    pub regions: Vec<String>,
}

/// プレイヤー間で共有する進行。集合の順序は保存とハッシュでも同じ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progression {
    pub bosses: BTreeSet<String>,
    pub recipes: BTreeSet<String>,
    pub regions: BTreeSet<String>,
}

impl Progression {
    pub fn is_empty(&self) -> bool {
        self.bosses.is_empty() && self.recipes.is_empty() && self.regions.is_empty()
    }

    /// 左上ではなく、エンティティの中心セルを渡す。負の座標も床除算する。
    pub fn visit(&mut self, x: i64, y: i64) {
        let name = format!("region:{}:{}", x.div_euclid(2048), y.div_euclid(2048));
        insert_name(&mut self.regions, &name);
    }

    pub(crate) fn boss_defeated(&mut self, name: &str) {
        insert_name(&mut self.bosses, name);
    }

    pub(crate) fn crafted(&mut self, name: &str) {
        insert_name(&mut self.recipes, name);
    }

    pub fn capture(&self) -> ProgressSave {
        ProgressSave {
            bosses: self.bosses.iter().cloned().collect(),
            recipes: self.recipes.iter().cloned().collect(),
            regions: self.regions.iter().cloned().collect(),
        }
    }

    /// 名を順序と境界込みで流す。進行がないときは呼び出し元がタグを流さない。
    pub fn hash_bytes(&self, feed: &mut impl FnMut(&[u8])) {
        for (tag, names) in [(0u8, &self.bosses), (1, &self.recipes), (2, &self.regions)] {
            for name in names {
                feed(&[tag]);
                feed(&(name.len() as u32).to_le_bytes());
                feed(name.as_bytes());
            }
        }
    }
}

pub(crate) fn insert_name(names: &mut BTreeSet<String>, name: &str) {
    if !name.is_empty() && name.len() <= NAME_MAX && names.len() < PROGRESS_MAX {
        names.insert(name.to_owned());
    }
}

pub(crate) fn canonical_region(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("region:") else { return false };
    let Some((x, y)) = rest.split_once(':') else { return false };
    match (x.parse::<i64>(), y.parse::<i64>()) {
        (Ok(x), Ok(y)) => name == format!("region:{x}:{y}"),
        _ => false,
    }
}
