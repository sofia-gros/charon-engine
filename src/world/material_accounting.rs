//! 表現をまたぐ物質の実量と、理由付きの増減を照合する標準profile。
//!
//! archiveの読込は明示的なIO入口へ分離します。tick内ではhashへ結び付いた
//! 読取専用の集計入力を使い、未読込を空気へ置換しません。

use super::{
    ArchiveStorage, BlockId, BlockInstance, ChunkCoord, Content, LayerId, MaterialChangeReason,
    MaterialComponentQuantity, MaterialHandle, MaterialId, MaterialParticle, MaterialQuantity,
    MaterialReceiptKind, MaterialReceiptLog, MixtureRef, MixtureStore, World, WorldEdit,
    channels::ChannelState, conflict, invalid, terrain::Chunk,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
    transaction::CandidateState,
};
use std::{collections::BTreeMap, sync::Arc};

/// 収支走査の一時record数と成分種類数。容量不足で量を切り捨てません。
#[derive(Clone, Copy, Debug)]
pub struct MaterialInventoryLimits {
    /// 走査中のnative cell・粒子・provider出力とcacheを合計した所有record数。
    /// archiveはgroupごとに同じowner・carrier・組成・epochを合算します。
    pub atoms: usize,
    /// 純物質の種類数。
    pub components: usize,
}

/// 一つのlayerの共有混合参照。参照先はWorldの標準MixtureStoreです。
#[derive(Clone, Copy, Debug)]
pub struct MaterialInventoryMixture {
    /// 対象layerの安定キー。
    pub layer: StableKey,
    /// そのlayerの物質に追従するMixtureRef channel。
    pub channel: StableKey,
}

/// 作者providerが読む、residentまたはarchive内の実block状態。
pub struct MaterialInventoryBlock<'a> {
    /// 世代付きの個体ID。
    pub id: BlockId,
    /// 不変の所属World。作者状態のWorld付き参照の照合に使います。
    pub world: WorldId,
    /// 素材・schema等の不変定義。周辺の可変World状態は渡しません。
    pub content: &'a Content,
    block: &'a BlockInstance,
}
impl MaterialInventoryBlock<'_> {
    /// 完全な作者recordを型付きで復元します。未接続を空状態と扱いません。
    pub fn state<T: StateCodec + 'static>(&self, key: StableKey) -> EngineResult<T> {
        self.block
            .properties
            .as_ref()
            .ok_or_else(|| invalid("収支対象block状態が未接続"))?
            .get(key)
    }
}

/// body local grid・reservoir・独自格子の保存済み実データを数える拡張入口。
///
/// callbackは副作用を持たず、宣言したkey/versionの実装として実際の作者状態を
/// 読みます。nativeなWorld cellや粒子を重ねて出力しません。自身のcallbackへ
/// 隠した状態を物質所有者にせず、WorldまたはblockのStateStoreへ保存してください。
/// 独自の数量形式や検証器は作者Systemから別profileとして統合できます。
pub trait MaterialInventoryProvider: Send + Sync {
    /// remapのownerでもある実装識別子。標準ownerとの重複は拒否します。
    fn key(&self) -> StableKey;
    /// 意味や状態の解釈を変えた場合に更新する、0以外の版。
    fn version(&self) -> u32;
    /// Worldの作者状態に所属する実物質を出力します。
    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()>;
    /// blockに所属する追加の物質を出力します。archiveの個体も一度ずつ渡します。
    fn collect_block(
        &self,
        _block: MaterialInventoryBlock<'_>,
        _output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        Ok(())
    }
}

/// 保存再開時にも同じ設定を渡す収支profile。callbackのアドレスは保存しません。
pub struct MaterialInventoryProfile {
    key: StableKey,
    version: u32,
    mixtures: Vec<MaterialInventoryMixture>,
    providers: Vec<Arc<dyn MaterialInventoryProvider>>,
    limits: MaterialInventoryLimits,
}
impl MaterialInventoryProfile {
    /// 順序を正規化し、重複・無効版・ゼロ容量を拒否します。
    pub fn new(
        key: StableKey,
        version: u32,
        mut mixtures: Vec<MaterialInventoryMixture>,
        mut providers: Vec<Arc<dyn MaterialInventoryProvider>>,
        limits: MaterialInventoryLimits,
    ) -> EngineResult<Self> {
        if version == 0 || limits.atoms == 0 || limits.components == 0 {
            return Err(invalid("収支profileの版・容量"));
        }
        mixtures.sort_by_key(|item| item.layer);
        providers.sort_by_key(|item| item.key());
        if mixtures
            .windows(2)
            .any(|pair| pair[0].layer == pair[1].layer)
            || providers
                .windows(2)
                .any(|pair| pair[0].key() == pair[1].key())
            || providers.iter().any(|item| {
                item.version() == 0
                    || [terrain_owner(), block_owner(), particle_owner()].contains(&item.key())
            })
        {
            return Err(invalid("収支profileの重複・provider版"));
        }
        Ok(Self {
            key,
            version,
            mixtures,
            providers,
            limits,
        })
    }

    fn fingerprint(&self, world: &World) -> EngineResult<[u8; 32]> {
        let mut out = Encoder::new(usize::MAX);
        out.raw(b"CHIPRF01")?;
        self.key.encode(&mut out)?;
        out.u32(self.version)?;
        out.u32(u32::try_from(self.mixtures.len()).map_err(|_| invalid("収支混合定義数"))?)?;
        for binding in &self.mixtures {
            world.content.layer_id(binding.layer)?;
            if world.channel_material_layer::<MixtureRef>(binding.channel)? != Some(binding.layer)
                || world.validate_channel_updates::<MixtureRef>(binding.channel, &[])?
                    != MixtureRef::NONE
            {
                return Err(invalid("収支の混合channel所有layer"));
            }
            binding.layer.encode(&mut out)?;
            binding.channel.encode(&mut out)?;
        }
        out.u32(u32::try_from(self.providers.len()).map_err(|_| invalid("収支provider数"))?)?;
        for provider in &self.providers {
            provider.key().encode(&mut out)?;
            out.u32(provider.version())?;
        }
        Ok(*blake3::hash(&out.finish()).as_bytes())
    }

    fn binding(&self, world: &World, layer: LayerId) -> EngineResult<Option<StableKey>> {
        let key = world.content.layer(layer)?.key;
        Ok(self
            .mixtures
            .binary_search_by_key(&key, |item| item.layer)
            .ok()
            .map(|index| self.mixtures[index].channel))
    }
}

#[derive(Clone, Copy)]
enum Owner {
    Terrain,
    Block,
    Particle,
    Provider(usize),
}
#[derive(Clone, Copy)]
struct Atom {
    owner: Owner,
    layer: LayerId,
    material: MaterialId,
    epoch: u64,
    mixture: MixtureRef,
    tokens: MaterialQuantity,
}

/// providerの出力先。上限と素材・layerを検査してから所有recordを追加します。
pub struct MaterialInventorySink<'a> {
    world: &'a World,
    profile: &'a MaterialInventoryProfile,
    owner: Owner,
    atoms: &'a mut Vec<Atom>,
}
impl MaterialInventorySink<'_> {
    /// 保存済みの実物質を出力します。applied_epochにはその実データの既適用版を渡します。
    /// 純物質はMixtureRef::NONE、通常のcell一つはMaterialQuantity::ONEです。
    /// body/reservoirの実格子は各非空cellを出力し、共有定義を所有量へ数えません。
    pub fn add(
        &mut self,
        layer: LayerId,
        material: MaterialHandle,
        applied_epoch: u64,
        mixture: MixtureRef,
        tokens: MaterialQuantity,
    ) -> EngineResult<()> {
        self.world.content.layer(layer)?;
        let material = self.world.content.material_id(material)?;
        if applied_epoch > self.world.remaps.epoch() {
            return Err(invalid("収支所有物の未来epoch"));
        }
        push_atom(
            self.atoms,
            self.profile.limits.atoms,
            Atom {
                owner: self.owner,
                layer,
                material,
                epoch: applied_epoch,
                mixture,
                tokens,
            },
        )
    }
}

fn push_atom(atoms: &mut Vec<Atom>, limit: usize, atom: Atom) -> EngineResult<()> {
    if atom.material == MaterialId::AIR {
        if !atom.mixture.is_none() {
            return Err(invalid("空所有物に混合参照"));
        }
        return Ok(());
    }
    if atom.tokens == MaterialQuantity::ZERO {
        return Ok(());
    }
    if atoms.len() == limit {
        return Err(EngineError::capacity(
            "物質収支の所有record数",
            atoms.len() as u64 + 1,
            limit as u64,
        ));
    }
    atoms.try_reserve(1).map_err(allocation)?;
    atoms.push(atom);
    Ok(())
}

struct CachedRegion {
    hash: [u8; 32],
    region: super::ChunkRegion,
    atoms: std::ops::Range<usize>,
}

/// archiveをchecksum検証して作る読取専用の派生入力。正規状態そのものではありません。
///
/// 保存する必要はなく、再開後は明示的なIO入口で再構築します。profile・Content・
/// World・各group hashを照合し、古いcacheや欠落を0量として使うことを防ぎます。
pub struct MaterialArchiveInventory {
    world: WorldId,
    bounds: super::WorldBounds,
    content: [u8; 32],
    profile: [u8; 32],
    regions: Vec<CachedRegion>,
    atoms: Vec<Atom>,
}

/// 全表現から集計した純物質別の正確な量。carrierは成分へ重ねて数えません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialInventoryReport {
    world: WorldId,
    content: [u8; 32],
    profile: [u8; 32],
    quantities: Vec<MaterialComponentQuantity>,
}
impl MaterialInventoryReport {
    /// 安定キー順のtoken/mass。存在しない素材の量は0です。
    pub fn quantities(&self) -> &[MaterialComponentQuantity] {
        &self.quantities
    }

    /// scope・profileと純物質別の量が一致することを検査します。
    pub fn verify_conserved(&self, after: &Self) -> EngineResult<()> {
        if self != after {
            return Err(conflict("全表現の物質収支が不一致"));
        }
        Ok(())
    }
}

fn terrain_owner() -> StableKey {
    StableKey::parse("engine:terrain").expect("固定owner")
}
fn block_owner() -> StableKey {
    StableKey::parse("engine:blocks").expect("固定owner")
}
fn particle_owner() -> StableKey {
    StableKey::parse("engine:particles").expect("固定owner")
}

impl World {
    /// IO境界で全archive groupを一度ずつ読み、hash検証済みの集計入力を作ります。
    /// 同期IOを行うため、Systemのtick内からは呼ばず、準備・保存検証で使用してください。
    pub fn load_material_archive_inventory(
        &self,
        profile: &MaterialInventoryProfile,
        storage: &mut impl ArchiveStorage,
        archive_byte_limit: usize,
    ) -> EngineResult<MaterialArchiveInventory> {
        let fingerprint = profile.fingerprint(self)?;
        let groups = self.inventory_archive_groups()?;
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(groups.len())
            .map_err(allocation)?;
        let mut atoms = Vec::new();
        for (coordinate, archive) in groups {
            let loaded = self
                .region_load_task(coordinate, archive_byte_limit)?
                .load(storage)?;
            if loaded.hash != archive.hash || loaded.region != archive.region {
                return Err(conflict("収支archiveの読込対象"));
            }
            self.validate_inventory_region(&loaded)?;
            let mut channels = Vec::new();
            channels
                .try_reserve_exact(loaded.channels.len())
                .map_err(allocation)?;
            for (factory, bytes) in self.content.channels.iter().zip(&loaded.channels) {
                channels.push(factory.restore(bytes)?);
            }
            let start = atoms.len();
            collect_chunks(
                self,
                profile,
                &loaded.chunks,
                &channels,
                |id| {
                    loaded
                        .blocks
                        .binary_search_by_key(&id, |(key, _)| *key)
                        .map(|index| loaded.blocks[index].1.epoch)
                        .map_err(|_| invalid("収支archiveのblock参照"))
                },
                &mut atoms,
            )?;
            for (_, particle) in &loaded.particles {
                collect_particle(self, profile, particle, &mut atoms)?;
            }
            for (id, block) in &loaded.blocks {
                collect_block(self, profile, *id, block, &mut atoms)?;
            }
            compact_atoms(&mut atoms, start)?;
            regions.push(CachedRegion {
                hash: loaded.hash,
                region: loaded.region,
                atoms: start..atoms.len(),
            });
        }
        regions.sort_by_key(|item| item.hash);
        Ok(MaterialArchiveInventory {
            world: self.id,
            bounds: self.bounds,
            content: self.content.fingerprint(),
            profile: fingerprint,
            regions,
            atoms,
        })
    }

    /// resident/inactive cell・粒子・archive・作者providerの実所有量を集計します。
    /// IOは行いません。未準備のarchive入力・混合参照・履歴不整合はエラーです。
    pub fn material_inventory(
        &self,
        profile: &MaterialInventoryProfile,
        archives: &MaterialArchiveInventory,
    ) -> EngineResult<MaterialInventoryReport> {
        let fingerprint = profile.fingerprint(self)?;
        if archives.world != self.id
            || archives.bounds != self.bounds
            || archives.content != self.content.fingerprint()
            || archives.profile != fingerprint
        {
            return Err(conflict("収支archive入力の所属・profile"));
        }
        let mut atoms = Vec::new();
        collect_chunks(
            self,
            profile,
            &self.chunks,
            &self.channels,
            |id| Ok(self.blocks.get(id)?.epoch),
            &mut atoms,
        )?;
        for (_, particle) in self.particles.iter().filter(|(_, p)| !p.is_archived()) {
            collect_particle(self, profile, particle, &mut atoms)?;
        }
        for (id, block) in self.blocks.iter().filter(|(_, b)| b.dormant.is_none()) {
            collect_block(self, profile, id, block, &mut atoms)?;
        }
        for (index, provider) in profile.providers.iter().enumerate() {
            provider.collect_world(
                self,
                &mut MaterialInventorySink {
                    world: self,
                    profile,
                    owner: Owner::Provider(index),
                    atoms: &mut atoms,
                },
            )?;
        }
        for (_, archive) in self.inventory_archive_groups()? {
            let index = archives
                .regions
                .binary_search_by_key(&archive.hash, |item| item.hash)
                .map_err(|_| {
                    EngineError::new(ErrorCode::MissingDependency, "収支archive入力が欠落")
                })?;
            let cached = &archives.regions[index];
            if cached.region != archive.region {
                return Err(conflict("収支archiveの範囲"));
            }
            for &atom in &archives.atoms[cached.atoms.clone()] {
                push_atom(&mut atoms, profile.limits.atoms, atom)?;
            }
        }
        let mixtures = match self.state::<MixtureStore>(MixtureStore::key()) {
            Ok(store) => {
                store.validate_world(self)?;
                Some(store)
            }
            Err(error) if error.code == ErrorCode::MissingKey => None,
            Err(error) => return Err(error),
        };
        // cell journalと混合witnessも実recordから検査し、履歴を無視した数量一致にしません。
        for record in self.state.records() {
            if self
                .content
                .schemas
                .get(record.schema)?
                .is::<MaterialReceiptLog>()
            {
                let log = self.state::<MaterialReceiptLog>(record.key)?;
                if log.key() != record.key {
                    return Err(conflict("収支対象cell journalの保存キー"));
                }
                log.validate_world(self)?;
            }
        }
        let mut amounts = BTreeMap::new();
        for atom in atoms {
            let owner = match atom.owner {
                Owner::Terrain => terrain_owner(),
                Owner::Block => block_owner(),
                Owner::Particle => particle_owner(),
                Owner::Provider(index) => profile.providers[index].key(),
            };
            let material = self
                .content
                .material(atom.material)?
                .ok_or_else(|| invalid("収支の空所有物"))?
                .key();
            let resolved = self.remaps.resolve(
                owner,
                self.content.layer(atom.layer)?.key,
                atom.epoch,
                MaterialHandle(material),
            )?;
            let id = self.content.material_id(resolved)?;
            // terrain/blockから空気へremapした所有物は消費済みです。
            // archive内には旧MixtureRefが残り得ますが、carrierが消滅した
            // 時点で成分も所有されなくなるため、混合storeの参照を解釈しません。
            if id == MaterialId::AIR {
                continue;
            }
            let components = match &mixtures {
                Some(store) => store.quantities(&self.content, id, atom.mixture)?,
                None if atom.mixture.is_none() => match self.content.material(id)? {
                    Some(material) => vec![MaterialComponentQuantity {
                        material: material.key(),
                        tokens: MaterialQuantity::ONE,
                        mass: MaterialQuantity::new(material.mass_per_cell().into(), 1)?,
                    }],
                    None => Vec::new(),
                },
                None => {
                    return Err(EngineError::new(
                        ErrorCode::MissingDependency,
                        "収支の共有組成が欠落",
                    ));
                }
            };
            for amount in components {
                let tokens = multiply(amount.tokens, atom.tokens)?;
                let mass = multiply(amount.mass, atom.tokens)?;
                if !amounts.contains_key(&amount.material)
                    && amounts.len() == profile.limits.components
                {
                    return Err(EngineError::capacity(
                        "収支の素材種類数",
                        amounts.len() as u64 + 1,
                        profile.limits.components as u64,
                    ));
                }
                let entry = amounts
                    .entry(amount.material)
                    .or_insert((MaterialQuantity::ZERO, MaterialQuantity::ZERO));
                entry.0 = entry.0.checked_add(tokens)?;
                entry.1 = entry.1.checked_add(mass)?;
                if amounts.len() > profile.limits.components {
                    return Err(EngineError::capacity(
                        "収支の素材種類数",
                        amounts.len() as u64,
                        profile.limits.components as u64,
                    ));
                }
            }
        }
        let mut quantities = Vec::new();
        quantities
            .try_reserve_exact(amounts.len())
            .map_err(allocation)?;
        for (material, (tokens, mass)) in amounts {
            quantities.push(MaterialComponentQuantity {
                material,
                tokens,
                mass,
            });
        }
        if profile.fingerprint(self)? != fingerprint {
            return Err(conflict("収支走査中のprofile変更"));
        }
        Ok(MaterialInventoryReport {
            world: self.id,
            content: self.content.fingerprint(),
            profile: fingerprint,
            quantities,
        })
    }

    // chunk単位のindexをhash順に一度だけ正規化し、大きいarchive数でも二乗走査しません。
    fn inventory_archive_groups(&self) -> EngineResult<Vec<(ChunkCoord, super::ArchivedChunk)>> {
        let mut groups = Vec::new();
        groups
            .try_reserve_exact(self.archived.len())
            .map_err(allocation)?;
        for (&coordinate, &archive) in &self.archived {
            groups.push((coordinate, archive));
        }
        groups.sort_by_key(|(_, archive)| archive.hash);
        if groups
            .windows(2)
            .any(|pair| pair[0].1.hash == pair[1].1.hash && pair[0].1.region != pair[1].1.region)
        {
            return Err(conflict("収支archiveのgroup不一致"));
        }
        groups.dedup_by_key(|(_, archive)| archive.hash);
        Ok(groups)
    }

    fn validate_inventory_region(&self, loaded: &super::LoadedRegion) -> EngineResult<()> {
        for coord in loaded
            .region
            .coordinates(self.limits.archived_chunks as usize)?
        {
            if !self
                .archived
                .get(&coord)
                .is_some_and(|entry| entry.hash == loaded.hash && entry.region == loaded.region)
                || self.chunks.contains_key(&coord)
            {
                return Err(conflict("収支archive indexが不一致"));
            }
        }
        // archiveのcellとblock maskを両方向に照合し、単に読めるbyte列を正しい所有状態としません。
        let mut references = 0usize;
        for (coordinate, chunk) in &loaded.chunks {
            if chunk.epoch > self.remaps.epoch()
                || chunk.generation.is_some() != chunk.baseline.is_some()
                || !self.bounds.intersects_chunk(*coordinate)
                || chunk.layers.len() != self.content.layers.len()
            {
                return Err(invalid("収支archiveの地形metadata"));
            }
            let origin = coordinate.origin()?;
            for (layer, page) in chunk.layers.iter().enumerate() {
                for (index, cell) in page.cells.iter().enumerate() {
                    cell.validate(&self.content)?;
                    let point = CellPoint {
                        x: origin.x + (index % 64) as i32,
                        y: origin.y + (index / 64) as i32,
                    };
                    if !self.bounds.contains(point) && *cell != super::CellCore::AIR {
                        return Err(invalid("収支archiveのWorld境界外cell"));
                    }
                    if let Some(id) = page.blocks.as_ref().and_then(|blocks| blocks[index]) {
                        let index = loaded
                            .blocks
                            .binary_search_by_key(&id, |(key, _)| *key)
                            .map_err(|_| invalid("収支archiveの未定義block"))?;
                        let block = &loaded.blocks[index].1;
                        if block.material != cell.material || block.layer.0 as usize != layer {
                            return Err(invalid("収支archiveのblock sidecar"));
                        }
                        references += 1;
                    }
                }
            }
            if let Some(base) = &chunk.baseline {
                for page in base {
                    for cell in &page.cells {
                        cell.validate(&self.content)?;
                    }
                }
            }
        }
        let mut expected = 0usize;
        for (id, block) in &loaded.blocks {
            block.validate()?;
            self.content.layer(block.layer)?;
            block.validate_state()?;
            if block.epoch > self.remaps.epoch() {
                return Err(invalid("収支archiveのblock未来epoch"));
            }
            let definition = self
                .content
                .material(block.material)?
                .ok_or_else(|| invalid("収支archiveの空block"))?;
            let size = definition.block_size();
            for &index in &block.remaining {
                if index >= u32::from(size[0]) * u32::from(size[1])
                    || !definition.shape().occupied(
                        (index % u32::from(size[0])) as u16,
                        (index / u32::from(size[0])) as u16,
                        size,
                    )
                {
                    return Err(invalid("収支archiveのblock mask範囲"));
                }
                let point = block.point(index, size)?;
                let (coordinate, index) = super::terrain::local(point);
                let page = &loaded
                    .chunks
                    .get(&coordinate)
                    .ok_or_else(|| invalid("収支archiveのblockが領域外"))?
                    .layers[block.layer.0 as usize];
                if page.blocks.as_ref().and_then(|blocks| blocks[index]) != Some(*id) {
                    return Err(invalid("収支archiveのblock残存参照"));
                }
                expected += 1;
            }
        }
        if expected != references {
            return Err(invalid("収支archiveのblock重複参照"));
        }
        let blocks = self
            .blocks
            .iter()
            .filter(|(_, block)| block.dormant == Some(loaded.hash))
            .count();
        let particles = self
            .particles
            .iter()
            .filter(|(_, particle)| particle.dormant == Some(loaded.hash))
            .count();
        if blocks != loaded.blocks.len() || particles != loaded.particles.len() {
            return Err(conflict("収支archiveの個体が欠落"));
        }
        for (id, block) in &loaded.blocks {
            let old = self.blocks.get(*id)?;
            if old.dormant != Some(loaded.hash)
                || old.material != block.material
                || old.layer != block.layer
                || old.anchor != block.anchor
                || old.orientation != block.orientation
                || old.epoch != block.epoch
            {
                return Err(conflict("収支archiveのblock metadata"));
            }
        }
        for (id, particle) in &loaded.particles {
            particle.core.validate(&self.content)?;
            if !self.bounds.contains(particle.cell()) || particle.epoch > self.remaps.epoch() {
                return Err(invalid("収支archive粒子の境界・epoch"));
            }
            self.validate_particle_payload(particle)?;
            let old = self.particles.get(*id)?;
            if old.dormant != Some(loaded.hash)
                || old.core != particle.core
                || old.layer != particle.layer
                || old.epoch != particle.epoch
                || old.position_q16() != particle.position_q16()
                || old.velocity_q16() != particle.velocity_q16()
            {
                return Err(conflict("収支archiveの粒子metadata"));
            }
        }
        Ok(())
    }
}

fn collect_chunks(
    world: &World,
    profile: &MaterialInventoryProfile,
    chunks: &BTreeMap<ChunkCoord, Chunk>,
    channels: &[Box<dyn ChannelState>],
    block_epoch: impl Fn(BlockId) -> EngineResult<u64>,
    atoms: &mut Vec<Atom>,
) -> EngineResult<()> {
    for (coordinate, chunk) in chunks {
        // 負座標や境界付近では直接の i32 乗算がoverflowします。
        // Worldと共通のchecked座標変換を使い、計数失敗をpanicへ変えません。
        let origin = coordinate.origin()?;
        for (index, page) in chunk.layers.iter().enumerate() {
            let layer = LayerId(index as u32);
            let channel = profile
                .binding(world, layer)?
                .map(|key| {
                    channels
                        .iter()
                        .find(|item| item.key() == key)
                        .ok_or_else(|| invalid("収支の混合channelが欠落"))
                })
                .transpose()?;
            for (index, core) in page.cells.iter().enumerate() {
                let point = CellPoint {
                    x: origin.x + (index % 64) as i32,
                    y: origin.y + (index / 64) as i32,
                };
                let mixture = channel
                    .map(|value| value.mixture_reference(point))
                    .transpose()?
                    .unwrap_or(MixtureRef::NONE);
                let block = page.blocks.as_ref().and_then(|blocks| blocks[index]);
                let (owner, epoch) = match block {
                    Some(id) => (Owner::Block, block_epoch(id)?),
                    None => (Owner::Terrain, chunk.epoch),
                };
                push_atom(
                    atoms,
                    profile.limits.atoms,
                    Atom {
                        owner,
                        layer,
                        material: core.material,
                        epoch,
                        mixture,
                        tokens: MaterialQuantity::ONE,
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn collect_particle(
    world: &World,
    profile: &MaterialInventoryProfile,
    particle: &MaterialParticle,
    atoms: &mut Vec<Atom>,
) -> EngineResult<()> {
    let mixture = profile
        .binding(world, particle.layer)?
        .map(|key| particle.inventory_reference(key))
        .transpose()?
        .unwrap_or(MixtureRef::NONE);
    push_atom(
        atoms,
        profile.limits.atoms,
        Atom {
            owner: Owner::Particle,
            layer: particle.layer,
            material: particle.core.material,
            epoch: particle.epoch,
            mixture,
            tokens: MaterialQuantity::ONE,
        },
    )
}
fn collect_block(
    world: &World,
    profile: &MaterialInventoryProfile,
    id: BlockId,
    block: &BlockInstance,
    atoms: &mut Vec<Atom>,
) -> EngineResult<()> {
    for (index, provider) in profile.providers.iter().enumerate() {
        provider.collect_block(
            MaterialInventoryBlock {
                id,
                world: world.id,
                content: &world.content,
                block,
            },
            &mut MaterialInventorySink {
                world,
                profile,
                owner: Owner::Provider(index),
                atoms,
            },
        )?;
    }
    Ok(())
}
// 同じcarrier・組成・owner・epochは量を合算します。archive全体のcell列をRAMへ
// 持ち続けず、hashごとの最小限の再換算入力を残します。epochや参照は捨てません。
fn compact_atoms(atoms: &mut Vec<Atom>, start: usize) -> EngineResult<()> {
    let key = |atom: &Atom| {
        let owner = match atom.owner {
            Owner::Terrain => (0usize, 0usize),
            Owner::Block => (1, 0),
            Owner::Particle => (2, 0),
            Owner::Provider(index) => (3, index),
        };
        (
            owner,
            atom.layer.0,
            atom.material.0,
            atom.epoch,
            atom.mixture.inventory_key(),
        )
    };
    atoms[start..].sort_unstable_by_key(key);
    let mut end = start;
    for index in start..atoms.len() {
        let atom = atoms[index];
        if end > start && key(&atoms[end - 1]) == key(&atom) {
            atoms[end - 1].tokens = atoms[end - 1].tokens.checked_add(atom.tokens)?;
        } else {
            atoms[end] = atom;
            end += 1;
        }
    }
    atoms.truncate(end);
    Ok(())
}

fn multiply(a: MaterialQuantity, b: MaterialQuantity) -> EngineResult<MaterialQuantity> {
    // 先に交差約分し、既存の標準有理数profileと同じu128中間上限を守ります。
    fn gcd(mut a: u128, mut b: u128) -> u128 {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    }
    let left = gcd(a.numerator(), b.denominator());
    let right = gcd(b.numerator(), a.denominator());
    let numerator = (a.numerator() / left)
        .checked_mul(b.numerator() / right)
        .ok_or_else(|| invalid("収支の数量積上限"))?;
    let denominator = (a.denominator() / right)
        .checked_mul(b.denominator() / left)
        .ok_or_else(|| invalid("収支の数量分母上限"))?;
    MaterialQuantity::new(numerator, denominator)
}

/// 全表現の純増減を理由とともに保存したreceipt。cell journalの個別入出力とは別に、
/// 同じ素材の共通量を相殺した差を記録し、二重計上を防ぎます。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialBalanceReceipt {
    sequence: u64,
    tick: u64,
    epoch: u64,
    reason: MaterialChangeReason,
    consumed: Vec<MaterialComponentQuantity>,
    produced: Vec<MaterialComponentQuantity>,
}
impl MaterialBalanceReceipt {
    /// journal内の連続番号。
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// 確定候補のtick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 実行時のmaterial remap epoch。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// 作者が明示した増減の理由。
    pub fn reason(&self) -> MaterialChangeReason {
        self.reason
    }
    /// 素材別の純減量。
    pub fn consumed(&self) -> &[MaterialComponentQuantity] {
        &self.consumed
    }
    /// 素材別の純増量。
    pub fn produced(&self) -> &[MaterialComponentQuantity] {
        &self.produced
    }
}

/// 全表現の基準量と増減履歴。定義・履歴の変更は専用のWorld取引へ限定します。
///
/// 古いrecordの汎用書戻しで基準や履歴を巻き戻せません。保存復元ではWorld全体を
/// 同時に戻し、profileとarchive入力を再接続してからverify_material_accountingを呼びます。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialBalanceLog {
    key: StableKey,
    baseline: MaterialInventoryReport,
    capacity: u32,
    receipts: Vec<MaterialBalanceReceipt>,
}
impl MaterialBalanceLog {
    /// 記録先のWorld StateStoreキー。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 自動削除しないreceipt件数の上限。
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
    /// 変更履歴。所有権移動だけなら新しい増減receiptを発行しません。
    pub fn receipts(&self) -> &[MaterialBalanceReceipt] {
        &self.receipts
    }
    /// 実集計して保存した開始時の量。
    pub fn baseline(&self) -> &MaterialInventoryReport {
        &self.baseline
    }
    /// 履歴を保持したまま容量を変更します。schema・Worldのbyte上限は独立しています。
    pub fn set_capacity(&mut self, capacity: u32) -> EngineResult<()> {
        if capacity == 0 || (capacity as usize) < self.receipts.len() {
            return Err(invalid("全表現収支の履歴容量"));
        }
        self.capacity = capacity;
        Ok(())
    }
    pub(super) fn validate_replacement(&self, world: &World, key: StableKey) -> EngineResult<()> {
        self.validate_world(world)?;
        let old = world.state::<Self>(key)?;
        if self.key != key
            || self.key != old.key
            || self.baseline != old.baseline
            || self.receipts != old.receipts
        {
            return Err(conflict("全表現収支の基準・履歴の書戻し"));
        }
        Ok(())
    }
    /// scope・量・履歴の構造と理由を照合します。実所有量との比較はWorldのverify入口です。
    pub fn validate_world(&self, world: &World) -> EngineResult<()> {
        if self.baseline.world != world.id
            || self.baseline.content != world.content.fingerprint()
            || self.capacity == 0
            || self.receipts.len() > self.capacity as usize
        {
            return Err(conflict("全表現収支の所属・容量"));
        }
        validate_amounts(&self.baseline.quantities, &world.content)?;
        let mut previous_tick = 0;
        let mut previous_epoch = 0;
        for (index, receipt) in self.receipts.iter().enumerate() {
            if receipt.sequence != index as u64
                || receipt.tick < previous_tick
                || receipt.tick > world.tick
                || receipt.epoch < previous_epoch
                || receipt.epoch > world.remaps.epoch()
            {
                return Err(invalid("全表現収支の履歴順・時刻"));
            }
            validate_reason(receipt.reason, &receipt.consumed, &receipt.produced)?;
            validate_amounts(&receipt.consumed, &world.content)?;
            validate_amounts(&receipt.produced, &world.content)?;
            if receipt
                .consumed
                .iter()
                .any(|a| receipt.produced.iter().any(|b| a.material == b.material))
            {
                return Err(invalid("全表現収支の差に共通素材"));
            }
            previous_tick = receipt.tick;
            previous_epoch = receipt.epoch;
        }
        self.expected(&world.content)?;
        Ok(())
    }
    fn expected(&self, content: &Content) -> EngineResult<MaterialInventoryReport> {
        let mut tokens = BTreeMap::new();
        for amount in &self.baseline.quantities {
            tokens.insert(amount.material, amount.tokens);
        }
        for receipt in &self.receipts {
            for amount in &receipt.produced {
                let value = tokens
                    .entry(amount.material)
                    .or_insert(MaterialQuantity::ZERO);
                *value = value.checked_add(amount.tokens)?;
            }
            for amount in &receipt.consumed {
                let value = tokens
                    .entry(amount.material)
                    .or_insert(MaterialQuantity::ZERO);
                *value = value.checked_sub(amount.tokens)?;
            }
        }
        let mut quantities = Vec::new();
        quantities
            .try_reserve_exact(tokens.len())
            .map_err(allocation)?;
        for (material, tokens) in tokens {
            if tokens != MaterialQuantity::ZERO {
                quantities.push(amount(content, material, tokens)?);
            }
        }
        Ok(MaterialInventoryReport {
            world: self.baseline.world,
            content: self.baseline.content,
            profile: self.baseline.profile,
            quantities,
        })
    }
}

impl World {
    /// 実集計から全表現の基準を初期化します。作者schemaにはMaterialBalanceLogを登録します。
    /// 新規baselineはこの入口だけで作り、既存recordを上書きしません。
    pub fn initialize_material_accounting(
        &mut self,
        profile: &MaterialInventoryProfile,
        archives: &MaterialArchiveInventory,
        key: StableKey,
        schema: StableKey,
        capacity: u32,
    ) -> EngineResult<()> {
        if capacity == 0
            || key == MixtureStore::key()
            || self.state.records().any(|record| record.key == key)
        {
            return Err(conflict("全表現収支の初期化先・容量"));
        }
        let baseline = self.material_inventory(profile, archives)?;
        let log = MaterialBalanceLog {
            key,
            baseline,
            capacity,
            receipts: Vec::new(),
        };
        self.edit(|edit| edit.set_material_state(key, schema, &log))
    }

    /// 基準＋理由付き増減と現在の全実量を照合します。raw編集による未記録増減も検出します。
    /// archiveが増減した場合はIO境界で入力を再構築し、保存・再開時にも呼んでください。
    pub fn verify_material_accounting(
        &self,
        profile: &MaterialInventoryProfile,
        archives: &MaterialArchiveInventory,
        key: StableKey,
    ) -> EngineResult<MaterialInventoryReport> {
        let log: MaterialBalanceLog = self.state(key)?;
        if log.key != key {
            return Err(conflict("全表現収支のjournalキー"));
        }
        log.validate_world(self)?;
        let actual = self.material_inventory(profile, archives)?;
        log.expected(&self.content)?.verify_conserved(&actual)?;
        Ok(actual)
    }

    /// 物質の変更と全表現の理由付きreceiptを一つの候補で確定します。
    /// reason=Noneでは完全な保存を要求し、所有先だけの移動へ使用します。
    /// SomeではSource/Sinkの純増減方向を検査し、Conversionでは純差を保存します。
    /// journal・schema・集計・callbackの失敗では先行編集も全て破棄します。
    /// archiveのIOを伴う操作は外で準備し、必要なhashの入力を先に用意してください。
    pub fn edit_material_accounted<R>(
        &mut self,
        profile: &MaterialInventoryProfile,
        archives: &MaterialArchiveInventory,
        key: StableKey,
        schema: StableKey,
        reason: Option<MaterialChangeReason>,
        action: impl FnOnce(&mut WorldEdit<'_>) -> EngineResult<R>,
    ) -> EngineResult<R> {
        if !self
            .state
            .records()
            .any(|record| record.key == key && record.schema == schema)
        {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "全表現収支のjournal schema",
            ));
        }
        self.verify_material_accounting(profile, archives, key)?;
        self.edit(|edit| {
            let mut log: MaterialBalanceLog = edit.world.state(key)?;
            let before = log.expected(&edit.world.content)?;
            let result = action(edit)?;
            // callback内で認めた容量変更は保持し、基準や履歴が変わっていないことも再確認します。
            log = edit.world.state(key)?;
            if log.expected(&edit.world.content)? != before {
                return Err(conflict("収支取引中の台帳変更"));
            }
            let after = edit.world.material_inventory(profile, archives)?;
            if let Some(reason) = reason {
                if log.receipts.len() == log.capacity as usize {
                    return Err(EngineError::capacity(
                        "全表現収支receipt数",
                        log.receipts.len() as u64 + 1,
                        log.capacity.into(),
                    ));
                }
                let (consumed, produced) = difference(&before, &after, &edit.world.content)?;
                validate_reason(reason, &consumed, &produced)?;
                log.receipts.try_reserve(1).map_err(allocation)?;
                log.receipts.push(MaterialBalanceReceipt {
                    sequence: log.receipts.len() as u64,
                    tick: edit.world.tick,
                    epoch: edit.world.remaps.epoch(),
                    reason,
                    consumed,
                    produced,
                });
                log.validate_world(edit.world)?;
                edit.set_material_state(key, schema, &log)?;
            } else {
                before.verify_conserved(&after)?;
            }
            Ok(result)
        })
    }
}

fn amount(
    content: &Content,
    material: StableKey,
    tokens: MaterialQuantity,
) -> EngineResult<MaterialComponentQuantity> {
    let id = content.material_id(MaterialHandle(material))?;
    let definition = content
        .material(id)?
        .ok_or_else(|| invalid("収支成分が空気"))?;
    Ok(MaterialComponentQuantity {
        material,
        tokens,
        mass: tokens.checked_mul(definition.mass_per_cell().into())?,
    })
}
fn validate_amounts(values: &[MaterialComponentQuantity], content: &Content) -> EngineResult<()> {
    if values
        .windows(2)
        .any(|pair| pair[0].material >= pair[1].material)
    {
        return Err(invalid("収支成分の順序・重複"));
    }
    for value in values {
        if value.tokens == MaterialQuantity::ZERO
            || amount(content, value.material, value.tokens)? != *value
        {
            return Err(invalid("収支成分のtoken/mass"));
        }
    }
    Ok(())
}
fn validate_reason(
    reason: MaterialChangeReason,
    consumed: &[MaterialComponentQuantity],
    produced: &[MaterialComponentQuantity],
) -> EngineResult<()> {
    if reason.version == 0 {
        return Err(invalid("全表現収支の理由版"));
    }
    match reason.kind {
        MaterialReceiptKind::Source if !consumed.is_empty() || produced.is_empty() => {
            Err(invalid("全表現収支Sourceの純増減"))
        }
        MaterialReceiptKind::Sink if consumed.is_empty() || !produced.is_empty() => {
            Err(invalid("全表現収支Sinkの純増減"))
        }
        _ => Ok(()),
    }
}
fn difference(
    before: &MaterialInventoryReport,
    after: &MaterialInventoryReport,
    content: &Content,
) -> EngineResult<(
    Vec<MaterialComponentQuantity>,
    Vec<MaterialComponentQuantity>,
)> {
    if before.world != after.world
        || before.content != after.content
        || before.profile != after.profile
    {
        return Err(conflict("全表現収支の変更scope"));
    }
    let mut quantities = BTreeMap::new();
    for value in &before.quantities {
        quantities.insert(value.material, (value.tokens, MaterialQuantity::ZERO));
    }
    for value in &after.quantities {
        quantities
            .entry(value.material)
            .or_insert((MaterialQuantity::ZERO, MaterialQuantity::ZERO))
            .1 = value.tokens;
    }
    let mut consumed = Vec::new();
    let mut produced = Vec::new();
    consumed
        .try_reserve_exact(quantities.len())
        .map_err(allocation)?;
    produced
        .try_reserve_exact(quantities.len())
        .map_err(allocation)?;
    for (material, (before, after)) in quantities {
        match before.compare(after) {
            std::cmp::Ordering::Less => {
                produced.push(amount(content, material, after.checked_sub(before)?)?)
            }
            std::cmp::Ordering::Greater => {
                consumed.push(amount(content, material, before.checked_sub(after)?)?)
            }
            std::cmp::Ordering::Equal => (),
        }
    }
    Ok((consumed, produced))
}

fn encode_amounts(values: &[MaterialComponentQuantity], out: &mut Encoder) -> EngineResult<()> {
    out.u32(u32::try_from(values.len()).map_err(|_| invalid("収支成分件数"))?)?;
    for value in values {
        value.material.encode(out)?;
        value.tokens.encode(out)?;
        value.mass.encode(out)?;
    }
    Ok(())
}
fn decode_amounts(input: &mut Decoder<'_>) -> EngineResult<Vec<MaterialComponentQuantity>> {
    let count = input.u32()? as usize;
    if count > input.remaining() / 71 {
        return Err(invalid("収支成分の切断・件数"));
    }
    let mut values = Vec::new();
    values.try_reserve_exact(count).map_err(allocation)?;
    for _ in 0..count {
        let value = MaterialComponentQuantity {
            material: StableKey::decode(input)?,
            tokens: MaterialQuantity::decode(input)?,
            mass: MaterialQuantity::decode(input)?,
        };
        if value.tokens == MaterialQuantity::ZERO
            || values
                .last()
                .is_some_and(|previous: &MaterialComponentQuantity| {
                    previous.material >= value.material
                })
        {
            return Err(invalid("収支成分の正規形"));
        }
        values.push(value);
    }
    Ok(values)
}
fn hash(input: &mut Decoder<'_>) -> EngineResult<[u8; 32]> {
    input.raw(32)?.try_into().map_err(|_| invalid("収支hash長"))
}
impl StateCodec for MaterialInventoryReport {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHINV001")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.raw(&self.profile)?;
        encode_amounts(&self.quantities, out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHINV001" {
            return Err(invalid("全表現収支report形式"));
        }
        Ok(Self {
            world: WorldId::decode(input)?,
            content: hash(input)?,
            profile: hash(input)?,
            quantities: decode_amounts(input)?,
        })
    }
}
impl StateCodec for MaterialBalanceLog {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHBAL001")?;
        self.key.encode(out)?;
        self.baseline.encode(out)?;
        out.u32(self.capacity)?;
        out.u32(u32::try_from(self.receipts.len()).map_err(|_| invalid("全表現receipt件数"))?)?;
        for value in &self.receipts {
            out.u64(value.sequence)?;
            out.u64(value.tick)?;
            out.u64(value.epoch)?;
            value.reason.key.encode(out)?;
            out.u32(value.reason.version)?;
            out.u8(match value.reason.kind {
                MaterialReceiptKind::Source => 0,
                MaterialReceiptKind::Sink => 1,
                MaterialReceiptKind::Conversion => 2,
            })?;
            encode_amounts(&value.consumed, out)?;
            encode_amounts(&value.produced, out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHBAL001" {
            return Err(invalid("全表現収支log形式"));
        }
        let key = StableKey::decode(input)?;
        let baseline = MaterialInventoryReport::decode(input)?;
        let capacity = input.u32()?;
        let count = input.u32()? as usize;
        if capacity == 0 || count > capacity as usize || count > input.remaining() / 44 {
            return Err(invalid("全表現収支log件数・切断"));
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(count).map_err(allocation)?;
        for index in 0..count {
            let sequence = input.u64()?;
            let tick = input.u64()?;
            let epoch = input.u64()?;
            let reason_key = StableKey::decode(input)?;
            let version = input.u32()?;
            let kind = match input.u8()? {
                0 => MaterialReceiptKind::Source,
                1 => MaterialReceiptKind::Sink,
                2 => MaterialReceiptKind::Conversion,
                _ => return Err(invalid("全表現収支log理由種別")),
            };
            let reason = MaterialChangeReason {
                key: reason_key,
                version,
                kind,
            };
            let consumed = decode_amounts(input)?;
            let produced = decode_amounts(input)?;
            validate_reason(reason, &consumed, &produced)?;
            if sequence != index as u64
                || receipts.last().is_some_and(|old: &MaterialBalanceReceipt| {
                    old.tick > tick || old.epoch > epoch
                })
            {
                return Err(invalid("全表現収支log順序"));
            }
            receipts.push(MaterialBalanceReceipt {
                sequence,
                tick,
                epoch,
                reason,
                consumed,
                produced,
            });
        }
        Ok(Self {
            key,
            baseline,
            capacity,
            receipts,
        })
    }
}

/// 世代保存・読込で全表現の収支検証を必須にする設定。IO境界で使用します。
pub struct MaterialAccountingSaveProfile<'a> {
    /// World rootとpluginの保存設定。
    pub world: &'a super::WorldSaveProfile,
    /// 実データを読む同一の収支profile。
    pub inventory: &'a MaterialInventoryProfile,
    /// 初期化済みのMaterialBalanceLogのキー。
    pub journal: StableKey,
    /// archive一groupの読込上限。自動拡張しません。
    pub archive_byte_limit: usize,
}
impl World {
    /// 全参照archive・全payloadの型と実量・receiptを検証してから、新世代を公開します。
    /// 収支不一致ならSaveRepositoryの保存を開始せず、既存世代を維持します。
    pub fn save_material_accounted<S: super::GenerationStorage>(
        &self,
        repository: &mut super::SaveRepository<S>,
        archives: &mut impl ArchiveStorage,
        profile: &MaterialAccountingSaveProfile<'_>,
        byte_limit: usize,
    ) -> EngineResult<super::SaveReceipt> {
        let inventory = self.load_material_archive_inventory(
            profile.inventory,
            archives,
            profile.archive_byte_limit,
        )?;
        self.verify_material_accounting(profile.inventory, &inventory, profile.journal)?;
        self.save(repository, archives, profile.world, byte_limit)
    }
    /// 完全な保存候補を読み、全表現の収支を照合したWorldだけを返します。
    /// 不一致を無視したり、未読込の所有物を0へ置換したりしません。
    pub fn load_material_accounted<S: super::GenerationStorage>(
        repository: &mut super::SaveRepository<S>,
        archives: &mut impl ArchiveStorage,
        content: Arc<Content>,
        profile: &MaterialAccountingSaveProfile<'_>,
        caps: super::WorldLimits,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        let world = Self::load(
            repository,
            archives,
            content,
            profile.world,
            caps,
            byte_limit,
        )?;
        let inventory = world.load_material_archive_inventory(
            profile.inventory,
            archives,
            profile.archive_byte_limit,
        )?;
        world.verify_material_accounting(profile.inventory, &inventory, profile.journal)?;
        Ok(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::schema::Schema;

    struct NoArchives;
    impl ArchiveStorage for NoArchives {
        fn put(&mut self, _: [u8; 32], _: &[u8]) -> EngineResult<super::super::SaveDurability> {
            Err(invalid("空archive試験のIO"))
        }
        fn read(&mut self, _: [u8; 32], _: usize) -> EngineResult<Vec<u8>> {
            Err(invalid("空archive試験のIO"))
        }
    }
    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    #[test]
    fn encoded_runtime_route_cannot_initialize_or_rewind_accounting_history() {
        let mut registry =
            super::super::ContentRegistry::new(super::super::ContentLimits::default()).unwrap();
        registry
            .layer(super::super::LayerDef {
                key: key("test:layer"),
                role: super::super::LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        registry
            .register(
                super::super::MaterialBuilder::new("test:a", super::super::MaterialBase::Powder)
                    .unwrap(),
            )
            .unwrap();
        registry
            .schema(Schema::of::<MaterialBalanceLog>(key("test:schema"), 1, 1_000_000).unwrap())
            .unwrap();
        let content = registry.freeze().unwrap();
        let mut world = World::new(
            WorldId::from_parts(0, 1).unwrap(),
            0,
            super::super::WorldBounds::expandable(),
            content,
            super::super::WorldLimits::default(),
        )
        .unwrap();
        world
            .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
            .unwrap();
        let profile = MaterialInventoryProfile::new(
            key("test:profile"),
            1,
            vec![],
            vec![],
            MaterialInventoryLimits {
                atoms: 10,
                components: 10,
            },
        )
        .unwrap();
        let cache = world
            .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
            .unwrap();
        let unchanged = world.checkpoint(1_000_000).unwrap();
        assert!(
            world
                .initialize_material_accounting(
                    &profile,
                    &cache,
                    MixtureStore::key(),
                    key("test:schema"),
                    4
                )
                .is_err()
        );
        assert_eq!(world.checkpoint(1_000_000).unwrap(), unchanged);
        world
            .initialize_material_accounting(
                &profile,
                &cache,
                key("test:log"),
                key("test:schema"),
                4,
            )
            .unwrap();
        let old: MaterialBalanceLog = world.state(key("test:log")).unwrap();
        let mut encoded = Encoder::new(1_000_000);
        old.encode(&mut encoded).unwrap();
        let encoded = encoded.finish();
        let material = world
            .content
            .material_id(MaterialHandle(key("test:a")))
            .unwrap();
        let layer = world.content.layer_id(key("test:layer")).unwrap();
        world
            .edit_material_accounted(
                &profile,
                &cache,
                key("test:log"),
                key("test:schema"),
                Some(MaterialChangeReason {
                    key: key("test:source"),
                    version: 1,
                    kind: MaterialReceiptKind::Source,
                }),
                |edit| {
                    edit.set_cell(
                        layer,
                        CellPoint { x: 1, y: 1 },
                        super::super::CellCore {
                            material,
                            ..super::super::CellCore::AIR
                        },
                    )
                },
            )
            .unwrap();
        let unchanged = world.checkpoint(1_000_000).unwrap();
        assert!(
            world
                .edit(|edit| edit.put_state_encoded(
                    key("test:log"),
                    key("test:schema"),
                    encoded.clone()
                ))
                .is_err()
        );
        assert_eq!(world.checkpoint(1_000_000).unwrap(), unchanged);
        assert!(
            world
                .edit(|edit| edit.put_state_encoded(key("test:other"), key("test:schema"), encoded))
                .is_err()
        );
        assert_eq!(world.checkpoint(1_000_000).unwrap(), unchanged);
    }
}
