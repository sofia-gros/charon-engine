//! 独自の状態とprovider状態を保存するstore、および候補の原子的な公開。
//!
//! 正規storeへ書込参照を直接返さず、編集は独立した候補で行います。
//! Rustの可変借用により、同じstoreへ二つの候補を同時commitできません。

use super::codec::{Decoder, Encoder, StateCodec, invalid};
use super::error::{EngineError, EngineResult, ErrorCode, allocation};
use super::key::StableKey;
use super::schema::{SchemaRegistry, copy_bytes, mismatch};

/// 正規storeが所有できるrecord数とpayload合計byte数です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    /// record個数の上限。
    pub records: u32,
    /// payload合計の上限。候補bufferの二重保持分は別途予算へ計上します。
    pub payload_bytes: u64,
    /// 保存file全体の上限。
    pub checkpoint_bytes: usize,
}

struct Record {
    key: StableKey,
    schema: StableKey,
    version: u32,
    payload: Vec<u8>,
}

/// 検査・独自codecが読む不変のrecordです。payloadを書き換える権限は含みません。
pub struct StateRecordView<'a> {
    /// recordの安定キー。
    pub key: StableKey,
    /// 解釈するschemaの安定キー。
    pub schema: StableKey,
    /// schemaの現在版。
    pub version: u32,
    /// 検証済みの保存payload。
    pub payload: &'a [u8],
}

/// scopeとschema registryを持つ、安定キー順の正規状態です。
pub struct StateStore {
    scope: StableKey,
    revision: u64,
    limits: StoreLimits,
    registry: SchemaRegistry,
    records: Vec<Record>,
    payload_bytes: u64,
}

impl StateStore {
    /// 空のstoreを構築します。schema registryの所有権を受け取ります。
    pub fn new(
        scope: StableKey,
        registry: SchemaRegistry,
        limits: StoreLimits,
    ) -> EngineResult<Self> {
        if limits.records == 0 || limits.checkpoint_bytes == 0 {
            return Err(EngineError::new(ErrorCode::InvalidValue, "状態store容量"));
        }
        Ok(Self {
            scope,
            revision: 0,
            limits,
            registry,
            records: Vec::new(),
            payload_bytes: 0,
        })
    }

    /// 所有する保存scopeの安定キーです。
    pub fn scope(&self) -> StableKey {
        self.scope
    }
    /// commitするたびに増える世代です。最大値でのcommitは拒否します。
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// 正規recordの数を返します。
    pub fn len(&self) -> usize {
        self.records.len()
    }
    /// recordがないかを返します。
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    /// 正規payloadの合計byte数です。
    pub fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    /// 同じscope・schema・容量を持つ独立したStore候補を作ります。
    ///
    /// 作者状態migrationなどで既存recordを元に候補を再構成し、変更後も通常のschema検証とtransactionを
    /// 使うための明示的な複製です。record数とpayload合計byte数に比例する確保が発生します。
    pub fn fork(&self) -> EngineResult<Self> {
        self.try_copy()
    }

    /// 安定キー順の不変recordを公開します。制作ツールだけのprivate権限を不要にします。
    pub fn records(&self) -> impl Iterator<Item = StateRecordView<'_>> {
        self.records.iter().map(|record| StateRecordView {
            key: record.key,
            schema: record.schema,
            version: record.version,
            payload: &record.payload,
        })
    }

    /// 型とschemaを検証して作者値を復元します。内部bufferへの可変参照は渡しません。
    pub fn get<T: StateCodec + 'static>(&self, key: StableKey) -> EngineResult<T> {
        let index = self
            .index(key)
            .map_err(|_| EngineError::new(ErrorCode::MissingKey, "状態record"))?;
        let record = &self.records[index];
        let schema = self.registry.get(record.schema)?;
        if !schema.is::<T>() || record.version != schema.version() {
            return Err(mismatch());
        }
        let mut input = Decoder::new(&record.payload, schema.max_bytes())?;
        let value = T::decode(&mut input)?;
        input.finish()?;
        Ok(value)
    }

    /// 独立した候補を作成します。確保に失敗しても正規状態を変更しません。
    ///
    /// 候補はstore全体のpayloadを複製する参照実装です。描画や物理用の巨大な
    /// 配列へ無条件に適用せず、将来のページ単位COWも同じcommit契約を守ります。
    pub fn transaction(&mut self) -> EngineResult<Transaction<'_>> {
        if self.revision == u64::MAX {
            return Err(EngineError::new(
                ErrorCode::CapacityExceeded,
                "状態revisionの上限",
            ));
        }
        let candidate = self.try_copy()?;
        Ok(Transaction {
            target: self,
            candidate,
            failed: None,
        })
    }

    /// schema識別とpayloadをキー順に保存します。codecの版は1です。
    pub fn checkpoint(&self) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(self.limits.checkpoint_bytes);
        self.encode_checkpoint(&mut out)?;
        Ok(out.finish())
    }

    /// 実際の保存と同じ順序で、bufferを作らずに長さを求めます。
    pub(crate) fn checkpoint_len(&self) -> EngineResult<usize> {
        let mut out = Encoder::measuring();
        self.encode_checkpoint(&mut out)?;
        Ok(out.written_len())
    }

    fn encode_checkpoint(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHRSTA01")?;
        self.scope.encode(out)?;
        out.u64(self.revision)?;
        out.u32(self.records.len() as u32)?;
        for record in &self.records {
            record.key.encode(out)?;
            record.schema.encode(out)?;
            out.u32(record.version)?;
            out.bytes(&record.payload)?;
        }
        Ok(())
    }

    /// 呼出側が指定したscope・上限・registryで候補を復元します。
    ///
    /// 保存内に容量上限を持たせず、外部データが上限を引き上げることを防ぎます。
    /// migration後のbyte数も再集計し、全recordが成功するまでstoreを返しません。
    pub fn restore(
        bytes: &[u8],
        expected_scope: StableKey,
        registry: SchemaRegistry,
        limits: StoreLimits,
    ) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, limits.checkpoint_bytes)?;
        if input.raw(8)? != b"CHRSTA01" {
            return Err(invalid());
        }
        let scope = StableKey::decode(&mut input)?;
        if scope != expected_scope {
            return Err(EngineError::new(ErrorCode::WrongScope, "状態store復元"));
        }
        let revision = input.u64()?;
        let count = input.u32()?;
        if count > limits.records {
            return Err(EngineError::capacity(
                "復元record数",
                u64::from(count),
                u64::from(limits.records),
            ));
        }
        // 最短のキー二つと版・長さだけでも22byteが必要です。
        if u64::from(count) * 22 > input.remaining() as u64 {
            return Err(invalid());
        }
        let mut candidate = Self::new(scope, registry, limits)?;
        candidate
            .records
            .try_reserve_exact(count as usize)
            .map_err(allocation)?;
        let mut last = None;
        for _ in 0..count {
            let key = StableKey::decode(&mut input)?;
            if last.is_some_and(|previous| previous >= key) {
                return Err(invalid());
            }
            last = Some(key);
            let schema_key = StableKey::decode(&mut input)?;
            let version = input.u32()?;
            let encoded = input.bytes(limits.checkpoint_bytes)?;
            let schema = candidate.registry.get(schema_key)?;
            let payload = schema.upgrade(version, encoded)?;
            candidate.check_total(0, payload.len())?;
            candidate.payload_bytes += payload.len() as u64;
            candidate.records.push(Record {
                key,
                schema: schema_key,
                version: schema.version(),
                payload,
            });
        }
        input.finish()?;
        candidate.revision = revision;
        Ok(candidate)
    }

    fn index(&self, key: StableKey) -> Result<usize, usize> {
        self.records.binary_search_by_key(&key, |record| record.key)
    }

    fn check_total(&self, old_bytes: usize, new_bytes: usize) -> EngineResult<()> {
        let required = self
            .payload_bytes
            .checked_sub(old_bytes as u64)
            .and_then(|remaining| remaining.checked_add(new_bytes as u64))
            .ok_or_else(invalid)?;
        if required > self.limits.payload_bytes {
            return Err(EngineError::capacity(
                "状態payload合計byte数",
                required,
                self.limits.payload_bytes,
            ));
        }
        Ok(())
    }

    fn put<T: StateCodec + 'static>(
        &mut self,
        key: StableKey,
        schema_key: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        let payload = self.encode_value(schema_key, value)?;
        self.put_encoded(key, schema_key, payload)
    }

    /// 型・schema・値域を確認してcommand用payloadを作ります。正規状態は変更しません。
    pub fn encode_value<T: StateCodec + 'static>(
        &self,
        schema_key: StableKey,
        value: &T,
    ) -> EngineResult<Vec<u8>> {
        let schema = self.registry.get(schema_key)?;
        if !schema.is::<T>() {
            return Err(mismatch());
        }
        let mut output = Encoder::new(schema.max_bytes());
        value.encode(&mut output)?;
        let payload = output.finish();
        schema.validate(&payload)?;
        Ok(payload)
    }

    pub(crate) fn put_encoded(
        &mut self,
        key: StableKey,
        schema_key: StableKey,
        payload: Vec<u8>,
    ) -> EngineResult<()> {
        let schema = self.registry.get(schema_key)?;
        schema.validate(&payload)?;
        let version = schema.version();
        let location = self.index(key);
        if let Ok(index) = location
            && self.records[index].schema != schema_key
        {
            return Err(mismatch());
        }
        let old_bytes = location
            .as_ref()
            .map(|index| self.records[*index].payload.len())
            .unwrap_or(0);
        self.check_total(old_bytes, payload.len())?;
        if location.is_err() && self.records.len() >= self.limits.records as usize {
            return Err(EngineError::capacity(
                "状態record数",
                self.records.len() as u64 + 1,
                u64::from(self.limits.records),
            ));
        }
        if location.is_err() {
            self.records.try_reserve(1).map_err(allocation)?;
        }
        self.payload_bytes = self.payload_bytes - old_bytes as u64 + payload.len() as u64;
        let record = Record {
            key,
            schema: schema_key,
            version,
            payload,
        };
        match location {
            Ok(index) => self.records[index] = record,
            Err(index) => self.records.insert(index, record),
        }
        Ok(())
    }

    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        let registry = self.registry.try_copy()?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(self.records.len())
            .map_err(allocation)?;
        for record in &self.records {
            records.push(Record {
                key: record.key,
                schema: record.schema,
                version: record.version,
                payload: copy_bytes(&record.payload)?,
            });
        }
        Ok(Self {
            scope: self.scope,
            revision: self.revision,
            limits: self.limits,
            registry,
            records,
            payload_bytes: self.payload_bytes,
        })
    }
}

impl super::transaction::CandidateState for StateStore {
    fn try_copy(&self) -> EngineResult<Self> {
        StateStore::try_copy(self)
    }
    fn validate(&self) -> EngineResult<()> {
        for record in &self.records {
            self.registry
                .get(record.schema)?
                .validate(&record.payload)?;
        }
        Ok(())
    }
}

/// 一つのstoreに対する候補です。commitせず破棄すると変更を取り消します。
pub struct Transaction<'a> {
    target: &'a mut StateStore,
    candidate: StateStore,
    failed: Option<EngineError>,
}

impl Transaction<'_> {
    pub(crate) fn put_encoded(
        &mut self,
        key: StableKey,
        schema: StableKey,
        payload: Vec<u8>,
    ) -> EngineResult<()> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.candidate.put_encoded(key, schema, payload);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    /// 候補へ型付きrecordを書きます。先行書込はこの候補内だけから見えます。
    pub fn put<T: StateCodec + 'static>(
        &mut self,
        key: StableKey,
        schema: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.candidate.put(key, schema, value);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    /// 自分の候補を読みます。別の未確定候補とは共有しません。
    pub fn get<T: StateCodec + 'static>(&self, key: StableKey) -> EngineResult<T> {
        self.candidate.get(key)
    }
    /// 存在するrecordだけを削除します。欠損では候補を変更しません。
    pub fn remove(&mut self, key: StableKey) -> EngineResult<()> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let index = match self.candidate.index(key) {
            Ok(index) => index,
            Err(_) => {
                let error = EngineError::new(ErrorCode::MissingKey, "状態record削除");
                self.failed = Some(error.clone());
                return Err(error);
            }
        };
        let removed = self.candidate.records.remove(index);
        self.candidate.payload_bytes -= removed.payload.len() as u64;
        Ok(())
    }

    /// 候補がcommit可能かを先に確認します。成功後は追加書込を行わずにcommitできます。
    ///
    /// 外部World候補と複数状態を同じ成功境界で公開する呼出側が、外部候補を確定する前に
    /// transactionの失敗状態を検出するために使います。可変borrowを保持する限り、確認後に
    /// poison状態が変わることはありません。
    pub fn validate_ready(&self) -> EngineResult<()> {
        match &self.failed {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// 全候補を一括公開します。この操作は追加確保・codec・作者callbackを呼びません。
    ///
    /// 可変借用が他の更新を防ぎ、候補作成時にrevision上限も確認済みです。
    /// 書込失敗を一つでも含む候補は、呼出側がエラーを無視しても公開しません。
    /// ここでの失敗確認後は、代入だけで全変更を公開します。
    pub fn commit(mut self) -> EngineResult<u64> {
        self.validate_ready()?;
        self.candidate.revision += 1;
        let revision = self.candidate.revision;
        *self.target = self.candidate;
        Ok(revision)
    }
}
