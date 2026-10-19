//! E05の混合組成を解決した、resident cellと正規粒子の純成分別集計。
//!
//! carrierの名目massとは別に、PR #8の有理数組成をPR #10の粒子payloadまで
//! 追跡します。archiveの外部payloadを未計上と明示し、Q04の全量証明とはしません。

use super::{
    ChannelQuery, LayerId, MaterialComponentQuantity, MaterialId, MaterialQuantity, MixtureRef,
    MixtureStore, World, invalid,
};
use crate::foundation::{EngineResult, StableKey, error::allocation, grid::CellPoint};

/// 一つの純成分について、混合比率を約分したtokenと換算massの内訳。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentComponentAmount {
    /// 純成分の安定素材キー。
    pub material: StableKey,
    /// 常駐cellが所有する純成分token数。混合物の場合は有理数です。
    pub cell_tokens: MaterialQuantity,
    /// 常駐正規粒子が所有する純成分token数。
    pub particle_tokens: MaterialQuantity,
    /// cellと正規粒子の純成分tokenの合計。
    pub total_tokens: MaterialQuantity,
    /// 成分素材のmass_per_cellを掛けた正確な有理mass。
    pub mass: MaterialQuantity,
}

/// 指定layerの混合channelを使った常駐物質の成分別診断。
/// 外部archiveが存在するときはWorld全量ではありません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentComponentInventory {
    /// 集計したlayerの安定キー。
    pub layer: StableKey,
    /// 解決した物質追従MixtureRef channelのキー。
    pub channel: StableKey,
    /// Contentの素材キー順。混合担体はその成分へ展開し、二重計上しません。
    pub components: Vec<ResidentComponentAmount>,
    /// 計上できていない退避chunk数（全layer共通）。
    pub archived_chunks: usize,
    /// 対象layerの退避粒子数。退避payloadを0成分と誤認しないために返します。
    pub archived_particles: usize,
}

impl World {
    /// PR #8の共有組成を解決し、指定layerのresident cellと粒子の成分量を正確に集計します。
    ///
    /// channelはこのlayerに属し、既定値MixtureRef::NONEでなければなりません。
    /// 混合carrierを二重計上せず、共有定義に質量を持たせません。
    /// レイヤー単位の診断なので、複数layerにわたる全World監査には集計を併合する必要があります。
    /// archiveの外部payloadや後工程の剛体・reservoirは本APIに含みません。
    ///
    /// 全resident cellへのchannel問い合わせと組成展開を行う監査用の低速APIです。
    pub fn resident_mixture_inventory(
        &self,
        layer: LayerId,
        channel: StableKey,
    ) -> EngineResult<ResidentComponentInventory> {
        let layer_key = self.content.layer(layer)?.key;
        if self.channel_material_layer::<MixtureRef>(channel)? != Some(layer_key)
            || self.validate_channel_updates::<MixtureRef>(channel, &[])? != MixtureRef::NONE
        {
            return Err(invalid("集計channelの所属layerまたは既定値"));
        }
        let layout = self.channel_layout::<MixtureRef>(channel)?;
        let store: MixtureStore = self.state(MixtureStore::key())?;
        let mut components = Vec::new();
        components
            .try_reserve_exact(self.content.materials.len())
            .map_err(allocation)?;
        for (_, def) in self.content.materials() {
            components.push(ResidentComponentAmount {
                material: def.key(),
                cell_tokens: MaterialQuantity::ZERO,
                particle_tokens: MaterialQuantity::ZERO,
                total_tokens: MaterialQuantity::ZERO,
                mass: MaterialQuantity::ZERO,
            });
        }
        for (coord, chunk) in &self.chunks {
            let origin = coord.origin()?;
            let page = &chunk.layers[layer.0 as usize];
            for (index, core) in page.cells.iter().enumerate() {
                if core.material == MaterialId::AIR {
                    continue;
                }
                let point = CellPoint {
                    x: origin
                        .x
                        .checked_add((index % 64) as i32)
                        .ok_or_else(|| invalid("組成集計x座標"))?,
                    y: origin
                        .y
                        .checked_add((index / 64) as i32)
                        .ok_or_else(|| invalid("組成集計y座標"))?,
                };
                let reference = match self
                    .channel_value::<MixtureRef>(channel, mixture_point(point, layout.origin)?)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("residentの組成参照coverage")),
                };
                for part in store.quantities(&self.content, core.material, reference)? {
                    add_component(&mut components, part, false)?;
                }
            }
        }
        let mut archived_particles = 0usize;
        for (handle, particle) in self.material_particles() {
            if particle.layer() != layer {
                continue;
            }
            if particle.is_archived() {
                archived_particles = archived_particles
                    .checked_add(1)
                    .ok_or_else(|| invalid("退避混合粒子数overflow"))?;
                continue;
            }
            let reference = self.particle_channel_value::<MixtureRef>(handle, channel)?;
            for part in store.quantities(&self.content, particle.core().material, reference)? {
                add_component(&mut components, part, true)?;
            }
        }
        for amount in &mut components {
            amount.total_tokens = amount.cell_tokens.checked_add(amount.particle_tokens)?;
            let id = self
                .content
                .material_id(super::MaterialHandle(amount.material))?;
            let def = self
                .content
                .material(id)?
                .ok_or_else(|| invalid("成分素材が空"))?;
            amount.mass = amount
                .total_tokens
                .checked_mul(u128::from(def.mass_per_cell()))?;
        }
        Ok(ResidentComponentInventory {
            layer: layer_key,
            channel,
            components,
            archived_chunks: self.archived.len(),
            archived_particles,
        })
    }
}

fn mixture_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("組成channel座標x"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("組成channel座標y"))?,
    })
}

fn add_component(
    records: &mut [ResidentComponentAmount],
    component: MaterialComponentQuantity,
    particle: bool,
) -> EngineResult<()> {
    let record = records
        .binary_search_by_key(&component.material, |record| record.material)
        .map_err(|_| invalid("混合成分の素材キーがContentにない"))?;
    let amount = &mut records[record];
    if particle {
        amount.particle_tokens = amount.particle_tokens.checked_add(component.tokens)?;
    } else {
        amount.cell_tokens = amount.cell_tokens.checked_add(component.tokens)?;
    }
    Ok(())
}
