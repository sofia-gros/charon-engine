//! 世界とblock個体の作者状態をCoreのStateStoreへ接続します。
//!
//! 保存codecはschemaの安定キーと版を使用します。未接続のblock codecは一時payloadを
//! 持ち、Worldまたはregionの復元時に登録済みschemaで移行・検証してから公開します。
//! 退避中のblockは個体IDだけを保持し、大きな作者payloadはarchive側へ移します。

use super::{BlockInstance, BlockRef, Content, World, WorldEdit, WorldLimits, invalid};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, StateCodec},
    state::{StateStore, StoreLimits},
    transaction::CandidateState,
};

pub(crate) fn limits(world: WorldLimits, block: bool) -> EngineResult<StoreLimits> {
    let (records, payload_bytes) = if block {
        (world.block_state_records, world.block_state_bytes)
    } else {
        (world.state_records, world.state_bytes)
    };
    let checkpoint_bytes = payload_bytes
        .checked_add(
            (records as usize)
                .checked_mul(412)
                .ok_or_else(|| invalid("状態record metadata上限"))?,
        )
        .and_then(|v| v.checked_add(256))
        .ok_or_else(|| invalid("状態checkpoint上限"))?;
    Ok(StoreLimits {
        records,
        payload_bytes: payload_bytes as u64,
        checkpoint_bytes,
    })
}
pub(crate) fn empty(
    content: &Content,
    world: WorldLimits,
    block: bool,
) -> EngineResult<StateStore> {
    StateStore::new(
        scope(block),
        content.schemas.try_copy()?,
        limits(world, block)?,
    )
}
pub(crate) fn restore(
    bytes: &[u8],
    content: &Content,
    world: WorldLimits,
    block: bool,
) -> EngineResult<StateStore> {
    StateStore::restore(
        bytes,
        scope(block),
        content.schemas.try_copy()?,
        limits(world, block)?,
    )
}
fn scope(block: bool) -> StableKey {
    StableKey::parse(if block {
        "engine:block-state"
    } else {
        "engine:world-state"
    })
    .expect("固定の状態scopeキー")
}
impl BlockInstance {
    pub(crate) fn connect_state(
        &mut self,
        content: &Content,
        world: WorldLimits,
    ) -> EngineResult<()> {
        if self.dormant.is_some() {
            if !self.pending_properties.is_empty() {
                return Err(invalid("休止blockに作者payloadが残存"));
            }
            return Ok(());
        }
        if !self.pending_properties.is_empty() {
            self.properties = Some(restore(&self.pending_properties, content, world, true)?);
            self.pending_properties = Vec::new();
        }
        Ok(())
    }
    pub(crate) fn validate_state(&self) -> EngineResult<()> {
        if !self.pending_properties.is_empty()
            || (self.dormant.is_some() && self.properties.is_some())
        {
            return Err(invalid("未接続または休止blockの作者状態"));
        }
        if let Some(properties) = &self.properties {
            properties.validate()?;
        }
        Ok(())
    }
}
impl World {
    /// World全体の作者状態。地域ごとに凍結する値はblock個体等の所属storeへ置きます。
    pub fn state<T: StateCodec + 'static>(&self, key: StableKey) -> EngineResult<T> {
        self.state.get(key)
    }
    /// 個体に属する作者状態。退避中・別World・古いIDを区別して拒否します。
    pub fn block_state<T: StateCodec + 'static>(
        &self,
        reference: BlockRef,
        key: StableKey,
    ) -> EngineResult<T> {
        self.block(reference)?
            .properties
            .as_ref()
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "block作者状態"))?
            .get(key)
    }
    pub(crate) fn connect_block_states(&mut self) -> EngineResult<()> {
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.blocks.len())
            .map_err(crate::foundation::error::allocation)?;
        for (id, _) in self.blocks.iter() {
            ids.push(id);
        }
        for id in ids {
            self.blocks
                .get_mut(id)?
                .connect_state(&self.content, self.limits)?;
        }
        Ok(())
    }
    pub(crate) fn validate_state(&self) -> EngineResult<()> {
        self.state.validate()?;
        for (_, block) in self.blocks.iter() {
            block.validate_state()?;
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    fn touch_block_state(&mut self, reference: BlockRef) -> EngineResult<()> {
        let block = self.world.block(reference)?;
        let size = self
            .world
            .content
            .material(block.material)?
            .ok_or_else(|| invalid("block素材"))?
            .block_size();
        let mut chunks = std::collections::BTreeSet::new();
        for index in &block.remaining {
            chunks.insert(super::ChunkCoord::at(block.point(*index, size)?));
        }
        for coordinate in chunks {
            let chunk = self
                .world
                .chunks
                .get_mut(&coordinate)
                .expect("blockの所属chunk");
            chunk.revision = chunk
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("作者状態によるchunk revision上限"))?;
            chunk.edited = true;
        }
        Ok(())
    }

    pub(crate) fn put_state_encoded(
        &mut self,
        key: StableKey,
        schema: StableKey,
        payload: Vec<u8>,
    ) -> EngineResult<()> {
        self.run(|editor| {
            if key == super::MixtureStore::key() {
                let definition = editor.world.content.schemas.get(schema)?;
                if !definition.is::<super::MixtureStore>() {
                    return Err(EngineError::new(ErrorCode::SchemaMismatch, "混合storeの型"));
                }
                let mut input = Decoder::new(&payload, definition.max_bytes())?;
                let value = super::MixtureStore::decode(&mut input)?;
                input.finish()?;
                value.validate_replacement(editor.world)?;
            }
            let definition = editor.world.content.schemas.get(schema)?;
            if definition.is::<super::MaterialReceiptLog>() {
                let mut input = Decoder::new(&payload, definition.max_bytes())?;
                let value = super::MaterialReceiptLog::decode(&mut input)?;
                input.finish()?;
                value.validate_replacement(editor.world, key)?;
            }
            let mut transaction = editor.world.state.transaction()?;
            transaction.put_encoded(key, schema, payload)?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// 検証済みの物質取引だけが定義・履歴を更新します。公開APIやruntime commandは通しません。
    pub(super) fn set_material_state<T: StateCodec + 'static>(
        &mut self,
        key: StableKey,
        schema: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            let mut transaction = editor.world.state.transaction()?;
            transaction.put(key, schema, value)?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// 世界の状態も地形と同じ候補で公開します。取引失敗では両方とも元の値を保ちます。
    pub fn set_state<T: StateCodec + 'static>(
        &mut self,
        key: StableKey,
        schema: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            // 標準storeの参照indexは公開後に再利用できないため、汎用APIでも履歴を保護します。
            if key == super::MixtureStore::key() {
                let mixture = (value as &dyn std::any::Any)
                    .downcast_ref::<super::MixtureStore>()
                    .ok_or_else(|| EngineError::new(ErrorCode::SchemaMismatch, "混合storeの型"))?;
                mixture.validate_replacement(editor.world)?;
            }
            if let Some(log) =
                (value as &dyn std::any::Any).downcast_ref::<super::MaterialReceiptLog>()
            {
                log.validate_replacement(editor.world, key)?;
            }
            let mut transaction = editor.world.state.transaction()?;
            transaction.put(key, schema, value)?;
            transaction.commit()?;
            Ok(())
        })
    }
    /// blockに作者型を保存します。schemaの型・版・payload上限はCoreと同じ検証です。
    pub fn set_block_state<T: StateCodec + 'static>(
        &mut self,
        reference: BlockRef,
        key: StableKey,
        schema: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            editor.world.block(reference)?;
            let mut properties = match editor.world.blocks.get_mut(reference.id)?.properties.take()
            {
                Some(properties) => properties,
                None => empty(&editor.world.content, editor.world.limits, true)?,
            };
            let mut transaction = properties.transaction()?;
            transaction.put(key, schema, value)?;
            transaction.commit()?;
            editor.world.blocks.get_mut(reference.id)?.properties = Some(properties);
            editor.touch_block_state(reference)
        })
    }
    /// blockの任意recordを明示削除します。blockそのものの寿命は変えません。
    pub fn remove_block_state(&mut self, reference: BlockRef, key: StableKey) -> EngineResult<()> {
        self.run(|editor| {
            editor.world.block(reference)?;
            let properties = editor
                .world
                .blocks
                .get_mut(reference.id)?
                .properties
                .as_mut()
                .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "block作者状態"))?;
            let mut transaction = properties.transaction()?;
            transaction.remove(key)?;
            transaction.commit()?;
            editor.touch_block_state(reference)
        })
    }
}
