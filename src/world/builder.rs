//! RuntimeがWorld IDを割り当ててから地形を構築する型付きbuilder。

use super::{BodyPhysicsSettings, Content, World, WorldBounds, WorldLimits};
use crate::foundation::{EngineError, EngineResult, ErrorCode, id::WorldId};
use std::sync::Arc;

type Initializer = Box<dyn FnOnce(&mut World) -> EngineResult<()>>;
/// 作者の世界設定。window設定やYAMLの画面寸法から世界を生成しません。
pub struct WorldBuilder {
    content: Arc<Content>,
    seed: u64,
    bounds: WorldBounds,
    limits: WorldLimits,
    body_physics: Option<BodyPhysicsSettings>,
    initializers: Vec<Initializer>,
    restored: Option<World>,
}
impl WorldBuilder {
    /// 内容定義を共有し、必要なchunkだけ増やすWorldを準備します。
    pub fn new(content: Arc<Content>) -> Self {
        Self {
            content,
            seed: 0,
            bounds: WorldBounds::expandable(),
            limits: WorldLimits::default(),
            body_physics: None,
            initializers: Vec::new(),
            restored: None,
        }
    }
    /// 再現可能な生成seed。
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
    /// 有限境界または表現可能な全域。
    pub fn bounds(mut self, bounds: WorldBounds) -> Self {
        self.bounds = bounds;
        self
    }
    /// resident、作者状態、Job等の容量。
    pub fn limits(mut self, limits: WorldLimits) -> Self {
        self.limits = limits;
        self
    }
    /// 標準Rapier剛体providerの容量と固定step設定。
    pub fn body_physics(mut self, settings: BodyPhysicsSettings) -> Self {
        self.body_physics = Some(settings);
        self
    }
    /// 起動時の初期地形等を設定します。失敗したWorldはRuntimeへ公開しません。
    pub fn initialize(
        mut self,
        callback: impl FnOnce(&mut World) -> EngineResult<()> + 'static,
    ) -> Self {
        self.initializers.push(Box::new(callback));
        self
    }
    /// 検証済みsnapshotの再接続を準備します。ID辞書が異なる場合は黙って変更しません。
    pub fn restored(world: World) -> Self {
        let mut builder = Self::new(Arc::clone(&world.content));
        builder.restored = Some(world);
        builder
    }
    /// 割当済みのWorld IDで構築します。通常はEngineBuilderが呼びます。
    pub fn build(self, id: WorldId) -> EngineResult<World> {
        let was_restored = self.restored.is_some();
        let mut world = match self.restored {
            Some(world) => {
                if world.id() != id {
                    return Err(EngineError::new(
                        ErrorCode::WrongScope,
                        "保存World IDとRuntimeの辞書が不一致",
                    ));
                }
                world
            }
            None => World::new(id, self.seed, self.bounds, self.content, self.limits)?,
        };
        if let Some(settings) = self.body_physics {
            if was_restored {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "復元Worldへ新しいBodyPhysics設定を上書きできません",
                ));
            }
            world.configure_body_physics(settings)?;
        }
        for initialize in self.initializers {
            initialize(&mut world)?;
        }
        Ok(world)
    }
}
