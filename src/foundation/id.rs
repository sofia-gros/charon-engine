//! 型の取り違えと削除済み参照を防ぐ、世代付きIDと値のarena。
//!
//! 空きslotの再利用順も保存するため、復元後に同じ操作から同じIDが得られます。
//! 世代の最大値で削除されたslotは永久に退役し、古いIDへ巻き戻りません。

use super::codec::{Decoder, Encoder, StateCodec, invalid};
use super::error::{EngineError, EngineResult, ErrorCode, allocation};
use std::{fmt, marker::PhantomData};

/// 所属する種類`K`を型で区別するIDです。ID自体は値の寿命を延ばしません。
pub struct Id<K> {
    index: u32,
    generation: u32,
    kind: PhantomData<fn() -> K>,
}

impl<K> Id<K> {
    /// codecや外部参照の候補を作ります。arenaでの生存確認は別途必要です。
    pub fn from_parts(index: u32, generation: u32) -> EngineResult<Self> {
        if generation == 0 {
            return Err(invalid());
        }
        Ok(Self {
            index,
            generation,
            kind: PhantomData,
        })
    }
    /// slot番号を返します。
    pub fn index(self) -> u32 {
        self.index
    }
    /// そのslotを再利用した世代を返します。
    pub fn generation(self) -> u32 {
        self.generation
    }
}
impl<K> Copy for Id<K> {}
impl<K> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        (self.index, self.generation) == (other.index, other.generation)
    }
}
impl<K> Eq for Id<K> {}
impl<K> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<K> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.index, self.generation).cmp(&(other.index, other.generation))
    }
}
impl<K> std::hash::Hash for Id<K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&(self.index, self.generation), state);
    }
}
impl<K> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({}:{})", self.index, self.generation)
    }
}
impl<K> StateCodec for Id<K> {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.index)?;
        out.u32(self.generation)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::from_parts(input.u32()?, input.u32()?)
    }
}

/// World用の型識別子です。
pub enum WorldKind {}
/// Entity用の型識別子です。
pub enum EntityKind {}
/// Scene用の型識別子です。
pub enum SceneKind {}
/// Job用の型識別子です。
pub enum JobKind {}
/// 剛体用の型識別子です。
pub enum BodyKind {}
/// Worldを区別するIDです。
pub type WorldId = Id<WorldKind>;
/// Entityを区別するIDです。Worldへ渡す際は所有scopeも検証します。
pub type EntityId = Id<EntityKind>;
/// Sceneを区別するIDです。
pub type SceneId = Id<SceneKind>;
/// Jobを区別するIDです。
pub type JobId = Id<JobKind>;
/// 剛体を区別するIDです。
pub type BodyId = Id<BodyKind>;

struct Slot<T> {
    generation: u32,
    retired: bool,
    value: Option<T>,
}

/// 容量を先に確保するarenaです。追加と削除時に配列を再確保しません。
pub struct Arena<T, K> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    capacity: u32,
    live: u32,
    kind: PhantomData<fn() -> K>,
}

impl<T, K> Arena<T, K> {
    /// slotとfree-listを予約します。確保失敗ではarenaを公開しません。
    pub fn new(capacity: u32) -> EngineResult<Self> {
        if capacity == 0 {
            return Err(EngineError::new(ErrorCode::InvalidValue, "arena容量"));
        }
        let mut slots = Vec::new();
        let mut free = Vec::new();
        slots
            .try_reserve_exact(capacity as usize)
            .map_err(allocation)?;
        free.try_reserve_exact(capacity as usize)
            .map_err(allocation)?;
        Ok(Self {
            slots,
            free,
            capacity,
            live: 0,
            kind: PhantomData,
        })
    }

    /// 値を追加します。失敗時にallocatorを消費しません。
    pub fn insert(&mut self, value: T) -> EngineResult<Id<K>> {
        let index = if let Some(index) = self.free.pop() {
            self.slots[index as usize].value = Some(value);
            index
        } else {
            if self.slots.len() >= self.capacity as usize {
                return Err(EngineError::capacity(
                    "arena slot数",
                    self.slots.len() as u64 + 1,
                    u64::from(self.capacity),
                ));
            }
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 1,
                retired: false,
                value: Some(value),
            });
            index
        };
        self.live += 1;
        Ok(Id {
            index,
            generation: self.slots[index as usize].generation,
            kind: PhantomData,
        })
    }

    /// 生存と世代が一致する値だけを借ります。
    pub fn get(&self, id: Id<K>) -> EngineResult<&T> {
        self.slots
            .get(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.value.as_ref())
            .ok_or_else(stale)
    }

    /// 一時的な可変参照を返します。構造変更は同じ借用中にはできません。
    pub fn get_mut(&mut self, id: Id<K>) -> EngineResult<&mut T> {
        self.slots
            .get_mut(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.value.as_mut())
            .ok_or_else(stale)
    }

    /// 所有値を取り出して世代を進めます。古いIDでは状態を変更しません。
    pub fn remove(&mut self, id: Id<K>) -> EngineResult<T> {
        let slot = self
            .slots
            .get_mut(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .ok_or_else(stale)?;
        let value = slot.value.take().ok_or_else(stale)?;
        self.live -= 1;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(id.index);
        } else {
            slot.retired = true;
        }
        Ok(value)
    }

    /// 生存している値の数を返します。
    pub fn len(&self) -> usize {
        self.live as usize
    }
    /// 生存値がないかを返します。退役slotは空き容量とは限りません。
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
    /// 設定した最大slot数を返します。
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
    /// index順の安定した走査を提供します。
    pub fn iter(&self) -> impl Iterator<Item = (Id<K>, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.value.as_ref().map(|value| {
                (
                    Id {
                        index: index as u32,
                        generation: slot.generation,
                        kind: PhantomData,
                    },
                    value,
                )
            })
        })
    }
}

impl<T: StateCodec, K> Arena<T, K> {
    /// slot世代、退役、値、free-listの順序を全て保存します。
    pub fn checkpoint(&self, byte_limit: usize) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(byte_limit);
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
        out.raw(b"CHARNA01")?;
        out.u32(self.capacity)?;
        out.u32(self.slots.len() as u32)?;
        for slot in &self.slots {
            out.u32(slot.generation)?;
            out.u8(u8::from(slot.retired))?;
            out.u8(u8::from(slot.value.is_some()))?;
            if let Some(value) = &slot.value {
                value.encode(out)?;
            }
        }
        out.u32(self.free.len() as u32)?;
        for index in &self.free {
            out.u32(*index)?;
        }
        Ok(())
    }

    /// 外部上限と形式を検証して候補arenaを作ります。
    ///
    /// 保存内のcapacityをそのまま確保に使用しません。呼出側の上限と
    /// 入力の最低必要byte数を先に確認し、不正なfree-listも拒否します。
    pub fn restore(bytes: &[u8], max_slots: u32, byte_limit: usize) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, byte_limit)?;
        if input.raw(8)? != b"CHARNA01" {
            return Err(invalid());
        }
        let capacity = input.u32()?;
        let count = input.u32()?;
        if capacity == 0 || capacity > max_slots || count > capacity {
            return Err(invalid());
        }
        if (count as u64) * 6 + 4 > input.remaining() as u64 {
            return Err(invalid());
        }
        let mut arena = Self::new(capacity)?;
        for _ in 0..count {
            let generation = input.u32()?;
            let retired = input.u8()?;
            let present = input.u8()?;
            if generation == 0
                || retired > 1
                || present > 1
                || (retired == 1 && (generation != u32::MAX || present != 0))
            {
                return Err(invalid());
            }
            let value = if present == 1 {
                arena.live += 1;
                Some(T::decode(&mut input)?)
            } else {
                None
            };
            arena.slots.push(Slot {
                generation,
                retired: retired == 1,
                value,
            });
        }
        let free_count = input.u32()?;
        if free_count > count || (free_count as u64) * 4 > input.remaining() as u64 {
            return Err(invalid());
        }
        let mut seen = Vec::new();
        seen.try_reserve_exact(count as usize).map_err(allocation)?;
        seen.resize(count as usize, false);
        for _ in 0..free_count {
            let index = input.u32()?;
            let slot = arena.slots.get(index as usize).ok_or_else(invalid)?;
            if slot.value.is_some() || slot.retired || seen[index as usize] {
                return Err(invalid());
            }
            seen[index as usize] = true;
            arena.free.push(index);
        }
        if arena
            .slots
            .iter()
            .enumerate()
            .any(|(index, slot)| slot.value.is_none() && !slot.retired && !seen[index])
        {
            return Err(invalid());
        }
        input.finish()?;
        Ok(arena)
    }
}

fn stale() -> EngineError {
    EngineError::new(ErrorCode::StaleId, "arenaの世代付き参照")
}

impl<T: super::transaction::CandidateState, K> super::transaction::CandidateState for Arena<T, K> {
    fn try_copy(&self) -> EngineResult<Self> {
        let mut copy = Self::new(self.capacity)?;
        for slot in &self.slots {
            let value = slot
                .value
                .as_ref()
                .map(|value| value.try_copy())
                .transpose()?;
            copy.slots.push(Slot {
                generation: slot.generation,
                retired: slot.retired,
                value,
            });
        }
        copy.free.extend_from_slice(&self.free);
        copy.live = self.live;
        Ok(copy)
    }
    fn validate(&self) -> EngineResult<()> {
        for (_, value) in self.iter() {
            value.validate()?;
        }
        Ok(())
    }
}
