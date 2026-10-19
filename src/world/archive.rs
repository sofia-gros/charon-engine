//! 編集済みregionの永続退避と復帰。
//!
//! 保存完了のreceiptを受け取るまではresidentを変更しません。跨chunkのblockは全所属
//! chunkを同時に退避し、個体IDは休止slotとして保持します。保存payloadはWorldの中に
//! 隠して保持せず、内容address付きのArchiveStorageへ移します。

use super::{
    ChunkCoord, ChunkRegion, Content, SaveDurability, World, WorldEdit, conflict, invalid,
    snapshot::{decode_chunk_pages, encode_chunk_pages},
    storage::{read_bounded, reject_link, sync_directory},
    terrain::{BlockInstance, Chunk},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    id::WorldId,
};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

/// 内容address付きregionの保存先。世代保存の整理から独立してroot参照を保持します。
pub trait ArchiveStorage {
    /// payloadをhash名で永続化します。失敗時は既存の同名objectを壊しません。
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability>;
    /// 上限付きでpayloadを読みます。呼出側でもchecksumを照合します。
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>>;
}
/// ローカルのregion archive。保存rootを移すときはこのフォルダも一緒に配布します。
pub struct DirectoryArchives {
    root: PathBuf,
}
impl DirectoryArchives {
    /// 既存objectを保持して保存先を開きます。
    pub fn new(root: impl AsRef<Path>) -> EngineResult<Self> {
        fs::create_dir_all(root.as_ref()).map_err(io)?;
        Ok(Self {
            root: fs::canonicalize(root).map_err(io)?,
        })
    }
    fn name(&self, hash: [u8; 32]) -> PathBuf {
        let mut name = String::with_capacity(69);
        for byte in hash {
            use std::fmt::Write;
            write!(&mut name, "{byte:02x}").expect("Stringへのhash書込");
        }
        name.push_str(".blob");
        self.root.join(name)
    }
}
impl ArchiveStorage for DirectoryArchives {
    fn put(&mut self, hash: [u8; 32], bytes: &[u8]) -> EngineResult<SaveDurability> {
        if blake3::hash(bytes).as_bytes() != &hash {
            return Err(invalid("archive書込hash"));
        }
        let lock_path = self.root.join("LOCK");
        reject_link(&lock_path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(io)?;
        lock.try_lock().map_err(|error| {
            EngineError::new(ErrorCode::Conflict, "archive writer競合").detail(error.to_string())
        })?;
        let target = self.name(hash);
        reject_link(&target)?;
        if target.exists() {
            let existing = read_bounded(&target, bytes.len())?;
            if existing == bytes {
                return sync_directory(&self.root);
            }
            return Err(invalid("既存archiveの破損"));
        }
        let temporary = self.root.join("WRITE.tmp");
        reject_link(&temporary)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(io)?;
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(temporary, target).map_err(io)?;
        sync_directory(&self.root)
    }
    fn read(&mut self, hash: [u8; 32], limit: usize) -> EngineResult<Vec<u8>> {
        read_bounded(&self.name(hash), limit)
    }
}
/// 書込と再読取照合を終えた退避候補。公開constructorを持たず、未保存receiptを作れません。
pub struct StoredEviction {
    world: WorldId,
    payload_bytes: usize,
    hash: [u8; 32],
    region: ChunkRegion,
    /// 実際に保存先が受理した同期の種類。
    pub durability: SaveDurability,
}
/// 保存workerへ移せる停止snapshot。正規Worldを借り続けません。
pub struct EvictionTask {
    world: WorldId,
    region: ChunkRegion,
    bytes: Vec<u8>,
}
impl EvictionTask {
    /// 保存して再読取照合します。失敗した場合にWorldを退避する権限は返しません。
    pub fn store(self, storage: &mut impl ArchiveStorage) -> EngineResult<StoredEviction> {
        let hash = *blake3::hash(&self.bytes).as_bytes();
        let durability = storage.put(hash, &self.bytes)?;
        let read = storage.read(hash, self.bytes.len())?;
        if read != self.bytes {
            return Err(invalid("保存直後のarchive再照合"));
        }
        Ok(StoredEviction {
            world: self.world,
            payload_bytes: self.bytes.len(),
            hash,
            region: self.region,
            durability,
        })
    }
}
/// 未読込chunkが参照する不変archiveの識別子。payload自体は保持しません。
#[derive(Clone, Copy, Debug)]
pub struct ArchivedChunk {
    /// 内容address。
    pub hash: [u8; 32],
    /// 同時に復帰する安全なregion。跨chunk個体を分割しません。
    pub region: ChunkRegion,
}

/// World rootと実行profileの保存設定。複数player等の追加scopeはSaveRepositoryへ登録できます。
pub struct WorldSaveProfile {
    /// World rootのscope。
    pub scope: crate::foundation::StableKey,
    /// 再現に使う実行profile。
    pub profile: crate::foundation::StableKey,
    /// 必須pluginと版。復元時は利用可能な同じ版を列挙してください。
    pub plugins: Vec<(crate::foundation::StableKey, u32)>,
}
/// IO workerへ移せる読込要求。対象hashはWorldが保存したindexから取得します。
pub struct RegionLoadTask {
    world: WorldId,
    region: ChunkRegion,
    hash: [u8; 32],
    byte_limit: usize,
    content: Arc<Content>,
    limits: super::WorldLimits,
}
/// 復号済み候補。restore_regionまではresidentにもactiveにもなりません。
pub struct LoadedRegion {
    world: WorldId,
    region: ChunkRegion,
    hash: [u8; 32],
    chunks: BTreeMap<ChunkCoord, Chunk>,
    blocks: Vec<(super::BlockId, BlockInstance)>,
    channels: Vec<Vec<u8>>,
    particles: Vec<(
        crate::foundation::id::Id<super::MaterialParticleKind>,
        super::MaterialParticle,
    )>,
}
impl RegionLoadTask {
    /// 任意のIO threadで呼べます。checksum・coverage・型・長さを確認して候補を作ります。
    pub fn load(self, storage: &mut impl ArchiveStorage) -> EngineResult<LoadedRegion> {
        let bytes = storage.read(self.hash, self.byte_limit)?;
        if blake3::hash(&bytes).as_bytes() != &self.hash {
            return Err(invalid("region archive checksum"));
        }
        let mut input = Decoder::new(&bytes, self.byte_limit)?;
        let format = input.raw(8)?;
        let with_particles = format == b"CHREGN02";
        if (!with_particles && format != b"CHREGN01")
            || WorldId::decode(&mut input)? != self.world
            || !self.content.accepts_content(
                input
                    .raw(32)?
                    .try_into()
                    .map_err(|_| invalid("archive content hash"))?,
            )
            || ChunkRegion::decode(&mut input)? != self.region
        {
            return Err(invalid("region archiveの所属・定義"));
        }
        let count = input.u32()? as usize;
        if count as u64 != self.region.chunk_count() || count > input.remaining() / 12 {
            return Err(invalid("archive chunk数"));
        }
        let mut chunks = BTreeMap::new();
        let mut previous = None;
        for _ in 0..count {
            let coordinate = ChunkCoord::decode(&mut input)?;
            if !self.region.contains(coordinate) || previous.is_some_and(|old| old >= coordinate) {
                return Err(invalid("archive chunk辞書"));
            }
            previous = Some(coordinate);
            let revision = input.u64()?;
            let edited = bool::decode(&mut input)?;
            let generation = Option::<super::GenerationStamp>::decode(&mut input)?;
            let epoch = input.u64()?;
            let (layers, baseline) = decode_chunk_pages(&mut input, self.content.layers.len())?;
            chunks.insert(
                coordinate,
                Chunk {
                    layers,
                    baseline,
                    revision,
                    edited,
                    active: false,
                    generation,
                    epoch,
                },
            );
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 9 {
            return Err(invalid("archive block数"));
        }
        let mut blocks = Vec::new();
        blocks.try_reserve_exact(count).map_err(allocation)?;
        let mut previous = None;
        for _ in 0..count {
            let id = super::BlockId::decode(&mut input)?;
            if previous.is_some_and(|old| old >= id) {
                return Err(invalid("archive block辞書"));
            }
            previous = Some(id);
            let mut block = BlockInstance::decode(&mut input)?;
            block.connect_state(&self.content, self.limits)?;
            if block.dormant.is_some() {
                return Err(invalid("archive内の休止block"));
            }
            blocks.push((id, block));
        }
        let count = input.u32()? as usize;
        if count != self.content.channels.len() {
            return Err(invalid("archive channel数"));
        }
        let mut channels = Vec::new();
        channels.try_reserve_exact(count).map_err(allocation)?;
        for factory in &self.content.channels {
            if crate::foundation::StableKey::decode(&mut input)? != factory.key() {
                return Err(invalid("archive channel辞書"));
            }
            channels.push(super::terrain::copy_slice(input.bytes(self.byte_limit)?)?);
        }
        let mut particles = Vec::new();
        if with_particles {
            let count = input.u32()? as usize;
            if count > self.limits.particles as usize || count > input.remaining() / 65 {
                return Err(invalid("archive粒子数"));
            }
            particles.try_reserve_exact(count).map_err(allocation)?;
            let mut previous = None;
            let mut total = 0usize;
            for _ in 0..count {
                let id =
                    crate::foundation::id::Id::<super::MaterialParticleKind>::decode(&mut input)?;
                let particle = super::MaterialParticle::decode(&mut input)?;
                total = total
                    .checked_add(particle.payload_bytes()?)
                    .ok_or_else(|| invalid("archive粒子byte数overflow"))?;
                if previous.is_some_and(|old| old >= id)
                    || particle.is_archived()
                    || !self.region.contains(ChunkCoord::at(particle.cell()))
                    || total > self.limits.particle_bytes
                {
                    return Err(invalid("archive粒子の辞書・所属・容量"));
                }
                previous = Some(id);
                particles.push((id, particle));
            }
        }
        input.finish()?;
        Ok(LoadedRegion {
            world: self.world,
            region: self.region,
            hash: self.hash,
            chunks,
            blocks,
            channels,
            particles,
        })
    }
}
impl World {
    /// resident snapshotと全参照archiveを一つのrootへ保存します。
    /// archiveは一つずつ読み、未読込世界全体をRAMへ集めません。
    pub fn save<S: super::GenerationStorage>(
        &self,
        repository: &mut super::SaveRepository<S>,
        archives: &mut impl ArchiveStorage,
        profile: &WorldSaveProfile,
        byte_limit: usize,
    ) -> EngineResult<super::SaveReceipt> {
        let metadata = super::SaveMetadata {
            profile: profile.profile,
            content_hash: self.content.fingerprint(),
            tick: self.tick,
            plugins: super::terrain::copy_slice(&profile.plugins)?,
        };
        let root = super::SaveRecord {
            scope: profile.scope,
            kind: crate::foundation::StableKey::parse("engine:world")?,
            version: 1,
            bytes: self.checkpoint(byte_limit)?,
        };
        let hashes: std::collections::BTreeSet<_> =
            self.archived.values().map(|archive| archive.hash).collect();
        let limit = repository.limits().blob_bytes;
        let records = std::iter::once(Ok(root)).chain(hashes.into_iter().map(|hash| {
            let bytes = archives.read(hash, limit)?;
            if blake3::hash(&bytes).as_bytes() != &hash {
                return Err(invalid("保存rootが参照するarchiveの破損"));
            }
            Ok(super::SaveRecord {
                scope: archive_scope(hash)?,
                kind: crate::foundation::StableKey::parse("engine:region")?,
                version: 1,
                bytes,
            })
        }));
        repository.save_stream(&metadata, records)
    }
    /// 最後の完全なrootからWorldと参照archiveを復元します。必須scopeを黙って捨てません。
    pub fn load<S: super::GenerationStorage>(
        repository: &mut super::SaveRepository<S>,
        archives: &mut impl ArchiveStorage,
        content: Arc<Content>,
        profile: &WorldSaveProfile,
        caps: super::WorldLimits,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        repository.read_transaction(|repository| {
            Self::load_locked(repository, archives, content, profile, caps, byte_limit)
        })
    }
    fn load_locked<S: super::GenerationStorage>(
        repository: &mut super::SaveRepository<S>,
        archives: &mut impl ArchiveStorage,
        content: Arc<Content>,
        profile: &WorldSaveProfile,
        caps: super::WorldLimits,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        let manifest = repository.latest_manifest()?;
        if manifest.metadata.profile != profile.profile
            || !content.accepts_content(manifest.metadata.content_hash)
        {
            return Err(EngineError::new(
                ErrorCode::MissingDependency,
                "保存のprofileまたはContent",
            ));
        }
        for required in &manifest.metadata.plugins {
            if !profile.plugins.contains(required) {
                return Err(EngineError::new(
                    ErrorCode::MissingDependency,
                    "保存が要求するpluginと版",
                ));
            }
        }
        let root = manifest
            .records
            .iter()
            .find(|record| record.scope == profile.scope)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "保存World scope"))?;
        if root.kind != crate::foundation::StableKey::parse("engine:world")? || root.version != 1 {
            return Err(invalid("World root codec"));
        }
        let record = repository.read_record(manifest.generation, root)?;
        let world = World::restore(&record.bytes, content, caps, byte_limit)?;
        if world.tick != manifest.metadata.tick {
            return Err(invalid("World rootとmanifestのtick不一致"));
        }
        let hashes: std::collections::BTreeSet<_> = world
            .archived
            .values()
            .map(|archive| archive.hash)
            .collect();
        let mut wanted = std::collections::BTreeMap::new();
        for hash in hashes {
            wanted.insert(archive_scope(hash)?, hash);
        }
        for entry in &manifest.records {
            if entry.scope == profile.scope {
                continue;
            }
            let hash = wanted.remove(&entry.scope).ok_or_else(|| {
                EngineError::new(
                    ErrorCode::MissingDependency,
                    "追加の必須scopeには復元handlerが必要",
                )
            })?;
            if entry.kind != crate::foundation::StableKey::parse("engine:region")?
                || entry.version != 1
            {
                return Err(invalid("region root record"));
            }
            let bytes = repository.read_record(manifest.generation, entry)?.bytes;
            if blake3::hash(&bytes).as_bytes() != &hash {
                return Err(invalid("rootの参照archive hash"));
            }
            archives.put(hash, &bytes)?;
            let checked = archives.read(hash, bytes.len())?;
            if checked != bytes {
                return Err(invalid("復元archiveの再照合"));
            }
        }
        if !wanted.is_empty() {
            return Err(invalid("保存rootに参照archiveが欠落"));
        }
        Ok(world)
    }
    /// 保存済み・未読込のindex。返る値に地形payloadの保持権はありません。
    pub fn archived_chunk(&self, coord: ChunkCoord) -> Option<ArchivedChunk> {
        self.archived.get(&coord).copied()
    }
    /// 準備中snapshotと現在の地形は独立します。receiptが戻るまでは何も解放しません。
    pub fn prepare_eviction(
        &self,
        region: ChunkRegion,
        byte_limit: usize,
    ) -> EngineResult<EvictionTask> {
        self.check_evictable(region)?;
        let coordinates = region.coordinates(self.limits.resident_chunks as usize)?;
        let mut out = Encoder::new(byte_limit);
        out.raw(b"CHREGN02")?;
        self.id.encode(&mut out)?;
        out.raw(&self.content.fingerprint())?;
        region.encode(&mut out)?;
        out.u32(coordinates.len() as u32)?;
        for coord in coordinates {
            let chunk = &self.chunks[&coord];
            coord.encode(&mut out)?;
            out.u64(chunk.revision)?;
            chunk.edited.encode(&mut out)?;
            chunk.generation.encode(&mut out)?;
            out.u64(chunk.epoch)?;
            encode_chunk_pages(chunk, &mut out)?;
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.blocks.len())
            .map_err(allocation)?;
        for (id, block) in self.blocks.iter() {
            if block.dormant.is_none() && self.block_intersects(block, region)? {
                ids.push(id);
            }
        }
        out.u32(ids.len() as u32)?;
        for id in ids {
            id.encode(&mut out)?;
            self.blocks.get(id)?.encode(&mut out)?;
        }
        out.u32(self.channels.len() as u32)?;
        for channel in &self.channels {
            channel.key().encode(&mut out)?;
            out.bytes(&channel.extract(region)?)?;
        }
        let count = self
            .particles
            .iter()
            .filter(|(_, p)| !p.is_archived() && region.contains(ChunkCoord::at(p.cell())))
            .count();
        out.u32(count as u32)?;
        for (id, particle) in self
            .particles
            .iter()
            .filter(|(_, p)| !p.is_archived() && region.contains(ChunkCoord::at(p.cell())))
        {
            id.encode(&mut out)?;
            particle.encode(&mut out)?;
        }
        Ok(EvictionTask {
            world: self.id,
            region,
            bytes: out.finish(),
        })
    }
    fn block_intersects(&self, block: &BlockInstance, region: ChunkRegion) -> EngineResult<bool> {
        let size = self
            .content
            .material(block.material)?
            .ok_or_else(|| invalid("block素材"))?
            .block_size();
        for index in &block.remaining {
            if region.contains(ChunkCoord::at(block.point(*index, size)?)) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn check_evictable(&self, region: ChunkRegion) -> EngineResult<()> {
        for coord in region.coordinates(self.limits.resident_chunks as usize)? {
            if !self.chunks.contains_key(&coord) {
                return Err(conflict("退避対象regionが全てresidentではない"));
            }
            if self.job_reserves(coord) {
                return Err(conflict("準備中の地形Jobがregionを予約中"));
            }
            if self
                .leases
                .iter()
                .any(|(_, lease)| lease.region.contains(coord))
            {
                return Err(conflict("lease保持中のregionは退避不可"));
            }
            if self.requests.iter().any(|(_, request)| {
                request.region.contains(coord)
                    && !matches!(
                        request.status,
                        super::RequestStatus::Published
                            | super::RequestStatus::Cancelled
                            | super::RequestStatus::Failed
                    )
            }) {
                return Err(conflict("未完了要求のregionは退避不可"));
            }
        }
        for (_, block) in self.blocks.iter() {
            if block.dormant.is_none() && self.block_intersects(block, region)? {
                let size = self
                    .content
                    .material(block.material)?
                    .ok_or_else(|| invalid("block素材"))?
                    .block_size();
                for index in &block.remaining {
                    if !region.contains(ChunkCoord::at(block.point(*index, size)?)) {
                        return Err(conflict("跨chunk個体の全所属regionが必要"));
                    }
                }
            }
        }
        Ok(())
    }
    /// archive indexが指す安全なgroup全体の読込入力を作ります。
    pub fn region_load_task(
        &self,
        coordinate: ChunkCoord,
        byte_limit: usize,
    ) -> EngineResult<RegionLoadTask> {
        let archive = self
            .archived
            .get(&coordinate)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "保存済みregion index"))?;
        Ok(RegionLoadTask {
            world: self.id,
            region: archive.region,
            hash: archive.hash,
            byte_limit,
            content: Arc::clone(&self.content),
            limits: self.limits,
        })
    }
    pub(crate) fn validate_archives(&self) -> EngineResult<()> {
        if self.archived.len() > self.limits.archived_chunks as usize {
            return Err(EngineError::capacity(
                "archive chunk index数",
                self.archived.len() as u64,
                self.limits.archived_chunks.into(),
            ));
        }
        for (coord, archive) in &self.archived {
            if !self.bounds.intersects_chunk(*coord)
                || self.chunks.contains_key(coord)
                || !archive.region.contains(*coord)
            {
                return Err(invalid("archive indexの所属"));
            }
            let anchor = self
                .archived
                .get(&archive.region.min())
                .ok_or_else(|| invalid("archive group anchorなし"))?;
            if anchor.hash != archive.hash || anchor.region != archive.region {
                return Err(invalid("archive group anchor不一致"));
            }
            if *coord != archive.region.min() {
                continue;
            }
            for part in archive
                .region
                .coordinates(self.limits.archived_chunks as usize)?
            {
                if !self.archived.get(&part).is_some_and(|value| {
                    value.hash == archive.hash && value.region == archive.region
                }) {
                    return Err(invalid("archive groupの欠落"));
                }
            }
        }
        let hashes: std::collections::BTreeSet<_> =
            self.archived.values().map(|archive| archive.hash).collect();
        for (_, block) in self.blocks.iter() {
            if let Some(hash) = block.dormant
                && !hashes.contains(&hash)
            {
                return Err(invalid("休止blockのarchiveなし"));
            }
        }
        Ok(())
    }
}

fn archive_scope(hash: [u8; 32]) -> EngineResult<crate::foundation::StableKey> {
    let mut name = String::from("engine:region-");
    for byte in hash {
        use std::fmt::Write;
        write!(&mut name, "{byte:02x}").expect("固定長hashの文字列化");
    }
    crate::foundation::StableKey::parse(&name)
}
impl WorldEdit<'_> {
    /// 保存・再照合済みのreceiptでだけ解放します。保存中に編集された候補は再試行が必要です。
    pub fn finish_eviction(&mut self, stored: StoredEviction) -> EngineResult<()> {
        self.run(|editor| {
            if stored.world != editor.world.id {
                return Err(EngineError::new(
                    ErrorCode::WrongScope,
                    "退避receiptのWorld",
                ));
            }
            editor.world.check_evictable(stored.region)?;
            let current = editor
                .world
                .prepare_eviction(stored.region, stored.payload_bytes)?;
            if blake3::hash(&current.bytes).as_bytes() != &stored.hash {
                return Err(conflict("保存中に対象regionが変更"));
            }
            let mut ids = Vec::new();
            ids.try_reserve_exact(editor.world.blocks.len())
                .map_err(allocation)?;
            for (id, block) in editor.world.blocks.iter() {
                if block.dormant.is_none() && editor.world.block_intersects(block, stored.region)? {
                    ids.push(id);
                }
            }
            for id in ids {
                let block = editor.world.blocks.get_mut(id)?;
                block.remaining = Vec::new();
                block.properties = None;
                block.pending_properties = Vec::new();
                block.dormant = Some(stored.hash);
            }
            for channel in &mut editor.world.channels {
                channel.clear(stored.region)?;
            }
            let ids = editor
                .world
                .particles
                .iter()
                .filter(|(_, p)| {
                    !p.is_archived() && stored.region.contains(ChunkCoord::at(p.cell()))
                })
                .map(|(id, _)| id)
                .collect::<Vec<_>>();
            for id in ids {
                editor
                    .world
                    .particles
                    .get_mut(id)?
                    .evict_payload(stored.hash);
            }
            for coord in stored
                .region
                .coordinates(editor.world.limits.resident_chunks as usize)?
            {
                editor.world.chunks.remove(&coord);
                editor.world.archived.insert(
                    coord,
                    ArchivedChunk {
                        hash: stored.hash,
                        region: stored.region,
                    },
                );
            }
            Ok(())
        })
    }
    /// checksum確認済み候補を一括復帰します。古いepochには現在の素材変換を適用します。
    pub fn restore_region(&mut self, loaded: LoadedRegion) -> EngineResult<()> {
        self.run(|editor| {
            if loaded.world != editor.world.id {
                return Err(EngineError::new(ErrorCode::WrongScope, "読込regionのWorld"));
            }
            for coord in loaded
                .region
                .coordinates(editor.world.limits.archived_chunks as usize)?
            {
                if !editor
                    .world
                    .archived
                    .get(&coord)
                    .is_some_and(|entry| entry.hash == loaded.hash && entry.region == loaded.region)
                    || editor.world.chunks.contains_key(&coord)
                {
                    return Err(conflict("読込中にarchive indexが変更"));
                }
            }
            for (id, block) in loaded.blocks {
                if editor.world.blocks.get(id)?.dormant != Some(loaded.hash) {
                    return Err(conflict("休止blockの世代が不一致"));
                }
                *editor.world.blocks.get_mut(id)? = block;
            }
            for (coord, chunk) in loaded.chunks {
                editor.world.archived.remove(&coord);
                editor.world.chunks.insert(coord, chunk);
            }
            for (channel, bytes) in editor.world.channels.iter_mut().zip(loaded.channels) {
                channel.merge(&bytes)?;
            }
            let expected = editor
                .world
                .particles
                .iter()
                .filter(|(_, p)| p.dormant == Some(loaded.hash))
                .count();
            if expected != loaded.particles.len() {
                return Err(conflict("復帰粒子の欠落"));
            }
            for (id, particle) in loaded.particles {
                let old = editor.world.particles.get(id)?;
                if old.dormant != Some(loaded.hash)
                    || old.position_q16() != particle.position_q16()
                    || old.velocity_q16() != particle.velocity_q16()
                    || old.layer != particle.layer
                    || old.core != particle.core
                    || old.epoch != particle.epoch
                {
                    return Err(conflict("復帰粒子の世代・metadata不一致"));
                }
                *editor.world.particles.get_mut(id)? = particle;
            }
            editor.world.apply_material_remaps()?;
            Ok(())
        })
    }
}
fn io(error: std::io::Error) -> EngineError {
    EngineError::new(ErrorCode::Io, "region archiveのI/O").detail(error.to_string())
}
