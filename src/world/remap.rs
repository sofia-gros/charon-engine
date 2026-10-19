//! 定義の変更とは別に、正規素材参照をepochで変換する台帳。
//!
//! resident地形は候補内で変換し、後から生成・読込したchunkにも未適用epochを適用します。
//! 作者のchannel、body等はownerキーを指定して同じ台帳を読み、自身のcodecへ適用できます。
//! 標準cellの形式へ独自stateを丸めるための仕組みではありません。

use super::{
    MaterialHandle, MaterialId, World, WorldEdit, invalid,
    terrain::{copy_slice, local},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
};
use std::sync::Arc;

/// 一つのepochの変換。各mappingは同じepoch内で一回だけ適用します。
#[derive(Clone, Debug)]
pub struct MaterialRemap {
    /// 参照の所有先。engine:terrainは個体に属さない地形、engine:blocksはblock個体です。
    /// 作者のownerキーも使え、独自storeが同じ変換表を解釈できます。
    pub owner: StableKey,
    /// 対象layer。空なら全layer。作者ownerでは独自の領域選択として解釈できます。
    pub layers: Vec<StableKey>,
    /// 旧素材と新素材の組。安定キーで保持し、runtime IDは保存しません。
    pub mappings: Vec<(MaterialHandle, MaterialHandle)>,
}
/// 適用順を含む正規の台帳。epoch=0は変換前です。
pub struct MaterialRemapTable {
    pub(crate) rules: Vec<MaterialRemap>,
}
impl MaterialRemapTable {
    /// 現在までに登録されたepoch数。
    pub fn epoch(&self) -> u64 {
        self.rules.len() as u64
    }
    /// 既適用epoch以降の規則だけを、同じownerへ順に適用します。
    pub fn resolve(
        &self,
        owner: StableKey,
        layer: StableKey,
        applied: u64,
        mut material: MaterialHandle,
    ) -> EngineResult<MaterialHandle> {
        if applied > self.epoch() {
            return Err(invalid("未来epochの素材参照"));
        }
        for rule in self.rules.iter().skip(applied as usize) {
            if rule.owner == owner
                && (rule.layers.is_empty() || rule.layers.contains(&layer))
                && let Some((_, target)) =
                    rule.mappings.iter().find(|(source, _)| *source == material)
            {
                material = *target;
            }
        }
        Ok(material)
    }
    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        let mut rules = Vec::new();
        rules
            .try_reserve_exact(self.rules.len())
            .map_err(crate::foundation::error::allocation)?;
        for rule in &self.rules {
            rules.push(MaterialRemap {
                owner: rule.owner,
                layers: copy_slice(&rule.layers)?,
                mappings: copy_slice(&rule.mappings)?,
            });
        }
        Ok(Self { rules })
    }
}
impl StateCodec for MaterialRemapTable {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(u32::try_from(self.rules.len()).map_err(|_| invalid("remap epoch上限"))?)?;
        for rule in &self.rules {
            rule.owner.encode(out)?;
            out.u32(rule.layers.len() as u32)?;
            for layer in &rule.layers {
                layer.encode(out)?;
            }
            out.u32(rule.mappings.len() as u32)?;
            for (source, target) in &rule.mappings {
                source.0.encode(out)?;
                target.0.encode(out)?;
            }
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let count = input.u32()? as usize;
        if count > input.remaining() / 13 {
            return Err(invalid("remap record数"));
        }
        let mut rules = Vec::new();
        rules
            .try_reserve_exact(count)
            .map_err(crate::foundation::error::allocation)?;
        for _ in 0..count {
            let owner = StableKey::decode(input)?;
            let count = input.u32()? as usize;
            if count > input.remaining() / 7 {
                return Err(invalid("remap layer数"));
            }
            let mut layers = Vec::new();
            layers
                .try_reserve_exact(count)
                .map_err(crate::foundation::error::allocation)?;
            for _ in 0..count {
                layers.push(StableKey::decode(input)?);
            }
            let count = input.u32()? as usize;
            if count > input.remaining() / 14 {
                return Err(invalid("remap mapping数"));
            }
            let mut mappings = Vec::new();
            mappings
                .try_reserve_exact(count)
                .map_err(crate::foundation::error::allocation)?;
            for _ in 0..count {
                mappings.push((
                    MaterialHandle(StableKey::decode(input)?),
                    MaterialHandle(StableKey::decode(input)?),
                ));
            }
            rules.push(MaterialRemap {
                owner,
                layers,
                mappings,
            });
        }
        Ok(Self { rules })
    }
}
impl World {
    /// bodyや作者channelも参照できる、保存対象の変換台帳。
    pub fn material_remaps(&self) -> &MaterialRemapTable {
        &self.remaps
    }
    pub(crate) fn validate_remaps(&self) -> EngineResult<()> {
        if self.remaps.rules.len() > self.limits.remaps as usize {
            return Err(EngineError::capacity(
                "remap epoch数",
                self.remaps.epoch(),
                self.limits.remaps.into(),
            ));
        }
        for rule in &self.remaps.rules {
            if rule.layers.windows(2).any(|pair| pair[0] >= pair[1])
                || rule.mappings.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            {
                return Err(invalid("remap辞書の順序・重複"));
            }
            for layer in &rule.layers {
                self.content.layer_id(*layer)?;
            }
            for (source, target) in &rule.mappings {
                let source = self.content.material_id(*source)?;
                let target = self.content.material_id(*target)?;
                if rule.owner == StableKey::parse("engine:particles")? && target == MaterialId::AIR
                {
                    // 現在のresidentだけで判定すると、退避中または未生成の粒子へ
                    // 不正規則が後から適用され、区域を復帰不能にします。mappingの
                    // 登録と保存復元の両方で拒否し、現在の粒子数に依存させません。
                    return Err(invalid("粒子を空へremapするには明示的な消費ruleが必要"));
                }
                if rule.owner == StableKey::parse("engine:blocks")?
                    && let (Some(old), Some(new)) = (
                        self.content.material(source)?,
                        self.content.material(target)?,
                    )
                    && (old.block_size() != new.block_size() || old.shape() != new.shape())
                {
                    // 未配置・退避中の個体にも後から同じ規則が適用されます。登録時点で
                    // 拒否し、再読込まで誤った形状変換が潜伏することを防ぎます。
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "block寸法・形状を変えるremapには個体変換が必要",
                    ));
                }
            }
        }
        for chunk in self.chunks.values() {
            if chunk.epoch > self.remaps.epoch() {
                return Err(invalid("chunkの未来epoch"));
            }
        }
        Ok(())
    }
    fn remapped(
        &self,
        owner: StableKey,
        layer: super::LayerId,
        epoch: u64,
        id: MaterialId,
    ) -> EngineResult<MaterialId> {
        let material = MaterialHandle(match self.content.material(id)? {
            None => StableKey::parse("engine:air")?,
            Some(def) => def.key(),
        });
        self.content.material_id(self.remaps.resolve(
            owner,
            self.content.layer(layer)?.key,
            epoch,
            material,
        )?)
    }
    pub(crate) fn apply_material_remaps(&mut self) -> EngineResult<()> {
        let terrain = StableKey::parse("engine:terrain")?;
        let blocks = StableKey::parse("engine:blocks")?;
        let current = self.remaps.epoch();
        // blockは複数chunkへ跨れます。個体自身の既適用epochから一回だけ変換します。
        // 標準経路はfootprintを保ちます。形状変更を伴う変換は作者の原子的配置処理で扱います。
        let mut updates = Vec::new();
        updates
            .try_reserve_exact(self.blocks.len())
            .map_err(crate::foundation::error::allocation)?;
        for (id, block) in self.blocks.iter() {
            if block.epoch == current {
                continue;
            }
            let old = self
                .content
                .material(block.material)?
                .ok_or_else(|| invalid("空block素材"))?;
            let epoch = block.epoch;
            let target = self.remapped(blocks, block.layer, epoch, block.material)?;
            if target != block.material {
                if let Some(new) = self.content.material(target)?
                    && (old.block_size() != new.block_size() || old.shape() != new.shape())
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "block寸法・形状を変えるremapには個体変換が必要",
                    ));
                }
                // 休止個体は変換の適法性だけを検査し、archiveのpayloadは復帰時に変換します。
                if block.dormant.is_none() {
                    updates.push((id, target));
                }
            }
        }
        let mut removed = std::collections::BTreeSet::new();
        for (id, target) in updates {
            let block = self.blocks.get(id)?;
            let size = self
                .content
                .material(block.material)?
                .ok_or_else(|| invalid("block素材"))?
                .block_size();
            let positions = block
                .remaining
                .iter()
                .map(|index| block.point(*index, size))
                .collect::<EngineResult<Vec<_>>>()?;
            let layer = block.layer;
            for point in positions {
                let (coord, index) = local(point);
                let chunk = self.chunks.get_mut(&coord).expect("個体参照を検証済み");
                let page = &mut chunk.layers[layer.0 as usize];
                if Arc::strong_count(page) > 1 {
                    *page = Arc::new(page.try_copy()?);
                }
                let page = Arc::get_mut(page).expect("候補専用page");
                page.cells[index].material = target;
                if target == MaterialId::AIR {
                    removed.insert((coord, layer.0, index));
                    page.cells[index] = super::CellCore::AIR;
                    if let Some(blocks) = &mut page.blocks {
                        blocks[index] = None;
                    }
                }
            }
            if target == MaterialId::AIR {
                self.blocks.remove(id)?;
            } else {
                self.blocks.get_mut(id)?.material = target;
            }
        }
        let block_ids = self.blocks.iter().map(|(id, _)| id).collect::<Vec<_>>();
        for id in block_ids {
            if self.blocks.get(id)?.dormant.is_none() {
                self.blocks.get_mut(id)?.epoch = current;
            }
        }
        let coords = self.chunks.keys().copied().collect::<Vec<_>>();
        // 粒子はterrainから独立した所有先です。退避中のpayloadは触らず、復帰時に
        // 同じepoch列を適用します。空気への変更は全payloadの消費ruleが必要なため拒否します。
        let particle_owner = StableKey::parse("engine:particles")?;
        let particle_ids = self
            .particles
            .iter()
            .filter(|(_, p)| !p.is_archived())
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        for id in particle_ids {
            let particle = self.particles.get(id)?;
            let target = self.remapped(
                particle_owner,
                particle.layer,
                particle.epoch,
                particle.core.material,
            )?;
            if target == MaterialId::AIR {
                return Err(invalid("粒子を空へremapするには明示的な消費ruleが必要"));
            }
            let particle = self.particles.get_mut(id)?;
            particle.core.material = target;
            particle.epoch = current;
        }
        for coord in coords {
            let epoch = self.chunks[&coord].epoch;
            if epoch == current {
                continue;
            }
            for layer in 0..self.content.layers.len() {
                let mut page = self.chunks[&coord].layers[layer].try_copy()?;
                let mut changed = false;
                for index in 0..CELL_COUNT {
                    if page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                    {
                        continue;
                    }
                    let target = self.remapped(
                        terrain,
                        super::LayerId(layer as u32),
                        epoch,
                        page.cells[index].material,
                    )?;
                    if target != page.cells[index].material {
                        page.cells[index].material = target;
                        if target == MaterialId::AIR {
                            page.cells[index] = super::CellCore::AIR;
                            removed.insert((coord, layer as u32, index));
                        }
                        changed = true;
                    }
                }
                if changed {
                    self.chunks.get_mut(&coord).expect("列挙したchunk").layers[layer] =
                        Arc::new(page);
                }
            }
            let chunk = self.chunks.get_mut(&coord).expect("列挙したchunk");
            chunk.epoch = current;
            chunk.revision = chunk
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("remapによるchunk revision上限"))?;
            chunk.edited = true;
        }
        // 素材が空へ変わると、そのcellが所有していた組成・作者の追従値も
        // 所有者を失います。素材IDだけを空にすると旧MixtureRefが残り、
        // 次の収支監査や区域の退避・復帰で幽霊の成分を数えてしまいます。
        // 同じWorld編集候補で全追従channelを既定値へ戻します。
        let mut by_layer = std::collections::BTreeMap::<
            super::LayerId,
            Vec<crate::foundation::grid::CellPoint>,
        >::new();
        for (coord, layer_index, index) in removed {
            let origin = coord.origin()?;
            let point = crate::foundation::grid::CellPoint {
                x: origin.x + (index % 64) as i32,
                y: origin.y + (index / 64) as i32,
            };
            let points = by_layer.entry(super::LayerId(layer_index)).or_default();
            points
                .try_reserve(1)
                .map_err(crate::foundation::error::allocation)?;
            points.push(point);
        }
        for (layer, points) in &by_layer {
            let owner = self.content.layer(*layer)?.key;
            for channel in &mut self.channels {
                channel.reset_material(owner, points)?;
            }
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    /// ownerとlayerを指定して新しいepochを記録し、residentの標準参照を同じ候補で変換します。
    pub fn remap_materials(&mut self, mut rule: MaterialRemap) -> EngineResult<u64> {
        self.run(|editor| {
            rule.layers.sort();
            rule.mappings.sort_by_key(|value| value.0);
            editor
                .world
                .remaps
                .rules
                .try_reserve(1)
                .map_err(crate::foundation::error::allocation)?;
            editor.world.remaps.rules.push(rule);
            editor.world.validate_remaps()?;
            editor.world.apply_material_remaps()?;
            Ok(editor.world.remaps.epoch())
        })
    }
}

use super::terrain::CELL_COUNT;
