//! 仕様 24 §4: 完全なブロックの上に置く設備。チャンクの退避と独立して名前で保存する。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use charon_core::cell::FLAG_BLOCK;
use charon_core::world::World;
use serde::{Deserialize, Serialize};

use crate::blocks::{self, BlockPos, BreakError, PlaceError, Placed};
use crate::defs::Definitions;
use crate::game::{EVENTS_MAX, GameEvent, GameState};
use crate::items::ItemId;
use crate::{EntityId, EntityWorld};

pub const FIXTURES_FILE: &str = "fixtures.ron";
pub const MAX_FIXTURES: usize = 4096;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub item_name: String,
    pub material_name: String,
    pub contents: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixtureMap(BTreeMap<BlockPos, Fixture>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixtureError {
    NotOwned,
    UnknownFixture(String),
    Occupied,
    TooMany,
    Inactive,
    NotEmpty,
    NoRoom,
    Place(PlaceError),
    Break(BreakError),
}

#[derive(Debug)]
pub enum FixtureSaveError {
    Io(std::io::Error),
    Parse(String),
    TooLarge,
    TooMany,
    Duplicate(BlockPos),
    UnknownFixture(String),
    UnknownMaterial(String),
    InvalidContents,
}

impl std::fmt::Display for FixtureSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "fixtures.ron の読み書きに失敗: {err}"),
            Self::Parse(err) => write!(f, "fixtures.ron が壊れています: {err}"),
            Self::TooLarge => f.write_str("fixtures.ron は 16 MiB を超えています"),
            Self::TooMany => f.write_str("fixtures.ron は 4096 件を超えています"),
            Self::Duplicate(pos) => {
                write!(f, "fixtures.ron に座標 ({}, {}) が重複しています", pos.bx, pos.by)
            }
            Self::UnknownFixture(name) => {
                write!(f, "fixtures.ron に未定義の設備 {name} があります")
            }
            Self::UnknownMaterial(name) => {
                write!(f, "fixtures.ron に未定義の材質 {name} があります")
            }
            Self::InvalidContents => f.write_str("fixtures.ron の箱の個数が不正です"),
        }
    }
}

impl std::error::Error for FixtureSaveError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFixture {
    bx: i32,
    by: i32,
    fixture: Fixture,
}

impl FixtureMap {
    pub fn get(&self, pos: BlockPos) -> Option<&Fixture> {
        self.0.get(&pos)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BlockPos, &Fixture)> {
        self.0.iter()
    }

    /// チャンクが読み込み済みで、同じ材質の FLAG_BLOCK が 64 セル揃っているときだけ有効。
    pub fn active(&self, world: &World, pos: BlockPos) -> bool {
        let Some(fixture) = self.0.get(&pos) else { return false };
        let cells = pos.cells();
        cells.len() == blocks::BLOCK_CELLS
            && cells.iter().all(|&cell_pos| {
                world.chunk(cell_pos.chunk()).is_some() && {
                    let cell = world.get(cell_pos);
                    cell.has(FLAG_BLOCK)
                        && world.materials().get(cell.material).name == fixture.material_name
                }
            })
    }

    pub fn to_ron(&self) -> Result<String, FixtureSaveError> {
        if self.len() > MAX_FIXTURES {
            return Err(FixtureSaveError::TooMany);
        }
        let entries: Vec<_> = self
            .0
            .iter()
            .map(|(&pos, fixture)| SavedFixture {
                bx: pos.bx,
                by: pos.by,
                fixture: fixture.clone(),
            })
            .collect();
        let text = ron::ser::to_string(&entries)
            .map_err(|err| FixtureSaveError::Parse(err.to_string()))?;
        if text.len() > MAX_BYTES {
            return Err(FixtureSaveError::TooLarge);
        }
        Ok(text)
    }

    pub fn from_ron(
        text: &str,
        defs: &Definitions,
        world: &World,
    ) -> Result<Self, FixtureSaveError> {
        if text.len() > MAX_BYTES {
            return Err(FixtureSaveError::TooLarge);
        }
        let entries: Vec<SavedFixture> =
            ron::from_str(text).map_err(|err| FixtureSaveError::Parse(err.to_string()))?;
        if entries.len() > MAX_FIXTURES {
            return Err(FixtureSaveError::TooMany);
        }
        let mut map = BTreeMap::new();
        for entry in entries {
            let fixture = entry.fixture;
            if defs
                .item_id(&fixture.item_name, world.materials())
                .and_then(|item| defs.fixture_for_item(item))
                .is_none()
            {
                return Err(FixtureSaveError::UnknownFixture(fixture.item_name));
            }
            if world.materials().id(&fixture.material_name).is_none() {
                return Err(FixtureSaveError::UnknownMaterial(fixture.material_name));
            }
            if fixture.contents.len() > 40
                || fixture.contents.values().any(|&n| !(1..=99_999).contains(&n))
            {
                return Err(FixtureSaveError::InvalidContents);
            }
            let pos = BlockPos { bx: entry.bx, by: entry.by };
            if map.insert(pos, fixture).is_some() {
                return Err(FixtureSaveError::Duplicate(pos));
            }
        }
        Ok(Self(map))
    }

    /// 古い世界にファイルがなければ空。壊れたデータは捨てずにエラーにする。
    pub fn read(root: &Path, defs: &Definitions, world: &World) -> Result<Self, FixtureSaveError> {
        let path = root.join(FIXTURES_FILE);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_BYTES as u64 => return Err(FixtureSaveError::TooLarge),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(FixtureSaveError::Io(err)),
        }
        let text = std::fs::read_to_string(path).map_err(FixtureSaveError::Io)?;
        Self::from_ron(&text, defs, world)
    }

    /// 同じディレクトリへ一時ファイルを書き、同期してから置換する。
    pub fn write(&self, root: &Path) -> Result<(), FixtureSaveError> {
        let text = self.to_ron()?;
        std::fs::create_dir_all(root).map_err(FixtureSaveError::Io)?;
        let target = root.join(FIXTURES_FILE);
        for suffix in 0..32 {
            let temporary = root.join(format!("{FIXTURES_FILE}.tmp.{suffix}"));
            let result = {
                let mut file =
                    match std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
                    {
                        Ok(file) => file,
                        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(err) => return Err(FixtureSaveError::Io(err)),
                    };
                file.write_all(text.as_bytes()).and_then(|()| file.sync_all())
            };
            if let Err(err) = result {
                let _ = std::fs::remove_file(&temporary);
                return Err(FixtureSaveError::Io(err));
            }
            let result = std::fs::rename(&temporary, &target);
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            return result.map_err(FixtureSaveError::Io);
        }
        Err(FixtureSaveError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "一時ファイルの名前が埋まっています",
        )))
    }

    pub fn state_hash(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for (pos, fixture) in &self.0 {
            for byte in
                pos.bx
                    .to_le_bytes()
                    .into_iter()
                    .chain(pos.by.to_le_bytes())
                    .chain(fixture.item_name.bytes())
                    .chain([0])
                    .chain(fixture.material_name.bytes())
                    .chain([0])
                    .chain(fixture.contents.iter().flat_map(|(name, count)| {
                        name.bytes().chain([0]).chain(count.to_le_bytes())
                    }))
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        hash
    }
}

impl GameState {
    /// 所有する設備アイテムを原子的に完全ブロックへ置く。傷んだ同じ設備なら metadata を保つ。
    pub fn place_fixture(
        &mut self,
        world: &mut World,
        entities: &mut EntityWorld,
        id: EntityId,
        pos: BlockPos,
        item: ItemId,
        defs: &Definitions,
    ) -> Result<Placed, FixtureError> {
        let def = defs
            .fixture_for_item(item)
            .ok_or_else(|| FixtureError::UnknownFixture(format!("{}", item.0)))?;
        let inventory = self.inventories.get(&id).ok_or(FixtureError::NotOwned)?;
        if inventory.item_count(item) == 0 {
            return Err(FixtureError::NotOwned);
        }
        let item_name = defs
            .item_name(item, world.materials())
            .ok_or_else(|| FixtureError::UnknownFixture(def.name.clone()))?;
        let material_name = world.materials().get(def.material).name.clone();
        if let Some(existing) = self.fixtures.get(pos) {
            if existing.item_name != item_name || existing.material_name != material_name {
                return Err(FixtureError::Occupied);
            }
            if self.fixtures.active(world, pos) {
                return Err(FixtureError::Occupied);
            }
        } else if self.fixtures.len() >= MAX_FIXTURES {
            return Err(FixtureError::TooMany);
        }
        let placed =
            blocks::place_block(world, entities, pos, def.material).map_err(FixtureError::Place)?;
        if let Some(inventory) = self.inventories.get_mut(&id) {
            inventory.take_item(item, 1);
        }
        self.fixtures.0.entry(pos).or_insert_with(|| Fixture {
            item_name,
            material_name,
            contents: BTreeMap::new(),
        });
        if self.events.len() < EVENTS_MAX {
            self.events.push(GameEvent::BlockPlaced(pos, def.material));
        }
        Ok(placed)
    }

    /// 有効な空設備だけ撤去し、設備アイテムを戻す。失敗時はどれも変更しない。
    pub fn remove_fixture(
        &mut self,
        world: &mut World,
        id: EntityId,
        pos: BlockPos,
        defs: &Definitions,
    ) -> Result<(), FixtureError> {
        let fixture = self.fixtures.get(pos).ok_or(FixtureError::Inactive)?;
        if !self.fixtures.active(world, pos) {
            return Err(FixtureError::Inactive);
        }
        if !fixture.contents.is_empty() {
            return Err(FixtureError::NotEmpty);
        }
        let item = defs
            .item_id(&fixture.item_name, world.materials())
            .filter(|&item| defs.fixture_for_item(item).is_some())
            .ok_or_else(|| FixtureError::UnknownFixture(fixture.item_name.clone()))?;
        let inventory = self.inventories.get(&id).ok_or(FixtureError::NotOwned)?;
        if !inventory.can_hold(item, 1) {
            return Err(FixtureError::NoRoom);
        }
        let material = blocks::break_block(world, pos).map_err(FixtureError::Break)?;
        if let Some(inventory) = self.inventories.get_mut(&id) {
            inventory.add_item(item, 1);
        }
        self.fixtures.0.remove(&pos);
        if self.events.len() < EVENTS_MAX {
            self.events.push(GameEvent::BlockBroken(pos, material));
        }
        Ok(())
    }
}
