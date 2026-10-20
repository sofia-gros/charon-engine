//! 外部作者コードからAI経路探索jobを保存・再開する結合試験です。

use charon_engine::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    grid::{CellPoint, GridLayout, GridLimits, GridScale},
    id::WorldId,
    random::RandomSource,
    schema::{Schema, SchemaRegistry},
    state::{StateStore, StoreLimits},
};
use charon_engine::game::combat::{
    AttackCatalog, AttackCatalogLimits, AttackContact, AttackDefinition, AttackQueryResult,
    AttackQueryStatus, AttackShape, AttackSpatialQuery, AttackTargetWindowImmunity,
    AttributeCatalog, AttributeChange, AttributeDefinition, AttributeState, CombatError, CombatHit,
    CombatState, CombatTransactionError, HitLimits, HitResolution, HitResolutionProvider,
    WorldAttackQuery, WorldAttackQueryOptions, WorldAttackTargetResolver,
};
use charon_engine::game::effects::{
    EffectCommand, EffectContext, EffectControl, EffectError, EffectEvaluator, EffectJob,
    EffectLimits, EffectNodeCatalog, EffectNodeDefinition, EffectNodeOperation, EffectNodeResult,
    EffectProgram, EffectProgramCatalog, EffectProgress, EffectSchedule, StandardEffectEvaluator,
};
use charon_engine::game::events::{
    TransactionalWorldEventProvider, WorldEvent, WorldEventDelivery, WorldEventProvider,
    WorldEventQueue, WorldEventTransactionResult, WorldEventView,
};
use charon_engine::game::items::{
    CraftCatalog, CraftContext, CraftProgress, CraftRecipe, CraftRequirement, CraftSession,
    EquipmentLimits, EquipmentLoadout, ItemCatalog, ItemContainer, ItemContainerLimits,
    ItemDefinition, ItemError, ItemInstance, ItemInstanceRegistry, ItemInstanceRegistryLimits,
    ItemLot, ItemQuery, MiningDropCatalog, MiningDropRule, MiningTransaction,
    MiningTransactionError, WorldDropLimits, WorldDropStore, mine_cell_with_drop,
};
use charon_engine::game::navigation::{
    NavigationAgentConfig, NavigationAgentProfile, NavigationEdge, NavigationError,
    NavigationExpansion, NavigationHazardCost, NavigationJob, NavigationLimits, NavigationQuery,
    NavigationStatus, NavigationStep,
};
use charon_engine::game::progression::{
    ProgressionCatalog, ProgressionChange, ProgressionDefinition, ProgressionKind, ProgressionState,
};
use charon_engine::game::signals::{
    Signal, SignalDelivery, SignalLimits, SignalProgress, SignalQueue, SignalTiming,
    SignalsProvider, WireDevice, WireDeviceHandler, WireDeviceLimits, WireDeviceResult,
    WireEmission, WireGraph, WireGraphLimits, WirePort, WirePortDirection,
};
use charon_engine::runtime::{
    Engine, StepResult,
    input::TickInput,
    schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
};
use charon_engine::world::{
    ArchiveStorage, BodyDef, BodyPhysics, BodyPose, BreakMode, CellCore, CellQuery,
    ChannelDefinition, ChannelQuery, ChunkCoord, ColliderShape, CollisionQueriesProvider,
    CollisionQueryGeometry, CollisionQueryHit, CollisionQueryRequest, CollisionQueryResult,
    CollisionQueryShapeRef, CollisionQueryStatus, CollisionQueryTarget, ContentLimits,
    ContentRegistry, LayerDef, LayerRole, MaterialBalanceLog, MaterialBase, MaterialBuilder,
    MaterialChangeReason, MaterialHandle, MaterialInventoryLimits, MaterialInventoryProfile,
    MaterialInventoryProvider, MaterialInventorySink, MaterialQuantity, MaterialReceiptKind,
    MixtureRef, Orientation, SaveDurability, World, WorldBounds, WorldLimits,
};
use std::sync::Arc;

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}

fn navigation_profile(
    profile_key: StableKey,
    shape_object: u64,
    footprint_cells: [u32; 2],
) -> NavigationAgentProfile {
    NavigationAgentProfile::new(
        profile_key,
        1,
        CollisionQueryShapeRef {
            provider: key("test:navigation-shape-definitions"),
            version: 1,
            owner: 1,
            object: shape_object,
            part: 0,
        },
        NavigationAgentConfig {
            footprint_cells,
            travel_ticks_per_cell: 1,
            max_step_cells: 1,
            max_jump_height_cells: 2,
            max_jump_range_cells: 3,
            max_fall_cells: 4,
            can_swim: true,
            can_fly: true,
            collision_groups: 1,
            hazard_costs: vec![NavigationHazardCost {
                hazard: key("test:navigation-hazard"),
                additional_ticks: 5,
            }],
        },
        8,
    )
    .unwrap()
}

fn empty_world() -> World {
    let mut content = ContentRegistry::new(ContentLimits::default()).unwrap();
    content
        .layer(LayerDef {
            key: key("test:transaction-layer"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content.freeze().unwrap(),
        WorldLimits::default(),
    )
    .unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthorMaterialBanks {
    source_tokens: u32,
    target_tokens: u32,
    material: StableKey,
    applied_epoch: u64,
}

impl StateCodec for AuthorMaterialBanks {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.source_tokens)?;
        out.u32(self.target_tokens)?;
        self.material.encode(out)?;
        out.u64(self.applied_epoch)
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            source_tokens: input.u32()?,
            target_tokens: input.u32()?,
            material: StableKey::decode(input)?,
            applied_epoch: input.u64()?,
        })
    }
}

struct AuthorMaterialBankProvider {
    key: StableKey,
    state: StableKey,
    layer: StableKey,
}

impl MaterialInventoryProvider for AuthorMaterialBankProvider {
    fn key(&self) -> StableKey {
        self.key
    }

    fn version(&self) -> u32 {
        1
    }

    fn collect_world(
        &self,
        world: &World,
        output: &mut MaterialInventorySink<'_>,
    ) -> EngineResult<()> {
        let banks = world.state::<AuthorMaterialBanks>(self.state)?;
        let tokens = u128::from(banks.source_tokens)
            .checked_add(u128::from(banks.target_tokens))
            .ok_or_else(|| {
                EngineError::new(ErrorCode::CapacityExceeded, "作者material token合計")
            })?;
        if tokens != 0 {
            output.add(
                world.content().layer_id(self.layer)?,
                MaterialHandle(banks.material),
                banks.applied_epoch,
                MixtureRef::NONE,
                MaterialQuantity::new(tokens, 1)?,
            )?;
        }
        Ok(())
    }
}

struct NoAuthorArchives;

impl ArchiveStorage for NoAuthorArchives {
    fn put(&mut self, _: [u8; 32], _: &[u8]) -> EngineResult<SaveDurability> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "予期しない作者archive書込",
        ))
    }

    fn read(&mut self, _: [u8; 32], _: usize) -> EngineResult<Vec<u8>> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "予期しない作者archive読込",
        ))
    }
}

struct AuthorMaterialPump {
    input: StableKey,
    output: StableKey,
}

struct DeathScenarioController {
    action: StableKey,
    dead: bool,
}

impl StateCodec for DeathScenarioController {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.action.encode(out)?;
        out.u8(u8::from(self.dead))
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let action = StableKey::decode(input)?;
        let dead = match input.u8()? {
            0 => false,
            1 => true,
            _ => {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "controller死亡flag",
                ));
            }
        };
        Ok(Self { action, dead })
    }
}

struct DeathScenarioResolver;

impl HitResolutionProvider for DeathScenarioResolver {
    fn key(&self) -> StableKey {
        key("test:death-scenario-resolver")
    }

    fn version(&self) -> u32 {
        1
    }

    fn resolve(
        &self,
        _hit: &CombatHit,
        _attributes: &AttributeState,
        _limits: HitLimits,
    ) -> Result<HitResolution, CombatError> {
        Ok(HitResolution {
            attributes: vec![AttributeChange {
                key: key("test:death-scenario-health"),
                value: 0.0,
            }],
            effects: vec![key("test:death-scenario-event")],
            knockback: None,
        })
    }
}

struct DeathScenarioEvents;

impl TransactionalWorldEventProvider for DeathScenarioEvents {
    fn key(&self) -> StableKey {
        key("test:death-scenario-events")
    }

    fn version(&self) -> u32 {
        1
    }

    fn handle(
        &self,
        event: WorldEventView<'_>,
    ) -> Result<WorldEventTransactionResult, charon_engine::game::signals::SignalError> {
        if event.key != key("test:death-scenario-event") {
            return Err(charon_engine::game::signals::SignalError::InvalidState);
        }
        Ok(WorldEventTransactionResult {
            deliveries: Vec::new(),
            progression_changes: vec![ProgressionChange::SetFlag {
                key: key("test:death-scenario-progress-dead"),
                value: true,
            }],
            has_world_commands: true,
        })
    }
}

impl WireDeviceHandler for AuthorMaterialPump {
    fn key(&self) -> StableKey {
        key("test:material-pump-handler")
    }

    fn version(&self) -> u32 {
        1
    }

    fn validate_state(
        &self,
        state: &[u8],
    ) -> Result<(), charon_engine::game::signals::SignalError> {
        if state.len() == 1 {
            Ok(())
        } else {
            Err(charon_engine::game::signals::SignalError::InvalidState)
        }
    }

    fn handle(
        &self,
        input_port: &WirePort,
        signal: &Signal,
        state: &[u8],
    ) -> Result<WireDeviceResult, charon_engine::game::signals::SignalError> {
        if input_port.key != self.input || signal.payload.len() != 1 {
            return Err(charon_engine::game::signals::SignalError::InvalidPort);
        }
        let revision = state[0]
            .checked_add(1)
            .ok_or(charon_engine::game::signals::SignalError::TickOverflow)?;
        Ok(WireDeviceResult {
            state: vec![revision],
            emissions: vec![WireEmission {
                port: self.output,
                payload: signal.payload.clone(),
                timing: SignalTiming::Deferred,
            }],
        })
    }
}

#[test]
fn author_signal_material_token_bridge_is_accounted_atomic_and_remap_aware() {
    let layer_key = key("test:author-token-layer");
    let banks_key = key("test:author-token-banks");
    let banks_schema = key("test:author-token-banks-schema");
    let balance_key = key("test:author-token-balance");
    let balance_schema = key("test:author-token-balance-schema");
    let provider_key = key("test:author-token-provider");
    let old_material = key("test:author-token-old");
    let new_material = key("test:author-token-new");
    let mut content = ContentRegistry::new(ContentLimits::default()).unwrap();
    content
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content
        .register(MaterialBuilder::new("test:author-token-old", MaterialBase::Liquid).unwrap())
        .unwrap();
    content
        .register(MaterialBuilder::new("test:author-token-new", MaterialBase::Liquid).unwrap())
        .unwrap();
    content
        .schema(Schema::of::<AuthorMaterialBanks>(banks_schema, 1, 128).unwrap())
        .unwrap();
    content
        .schema(Schema::of::<MaterialBalanceLog>(balance_schema, 1, 16_384).unwrap())
        .unwrap();
    let content = content.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content.clone(),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.set_state(
                banks_key,
                banks_schema,
                &AuthorMaterialBanks {
                    source_tokens: 4,
                    target_tokens: 0,
                    material: old_material,
                    applied_epoch: 0,
                },
            )
        })
        .unwrap();
    let profile = MaterialInventoryProfile::new(
        key("test:author-token-profile"),
        1,
        vec![],
        vec![Arc::new(AuthorMaterialBankProvider {
            key: provider_key,
            state: banks_key,
            layer: layer_key,
        })],
        MaterialInventoryLimits {
            atoms: 8,
            components: 4,
        },
    )
    .unwrap();
    let mut archives = NoAuthorArchives;
    let archive_inventory = world
        .load_material_archive_inventory(&profile, &mut archives, 16_384)
        .unwrap();
    world
        .initialize_material_accounting(
            &profile,
            &archive_inventory,
            balance_key,
            balance_schema,
            8,
        )
        .unwrap();
    let baseline = world
        .verify_material_accounting(&profile, &archive_inventory, balance_key)
        .unwrap();
    assert_eq!(baseline.quantities().len(), 1);
    assert_eq!(baseline.quantities()[0].material, old_material);
    assert_eq!(
        baseline.quantities()[0].tokens,
        MaterialQuantity::new(4, 1).unwrap()
    );

    let network = key("test:author-token-network");
    let device_key = key("test:author-token-pump");
    let input = WirePort {
        key: key("test:author-token-input"),
        device: device_key,
        region: key("test:author-token-region"),
        signal_type: key("test:author-token-signal"),
        direction: WirePortDirection::Input,
    };
    let output = WirePort {
        key: key("test:author-token-output"),
        device: device_key,
        region: key("test:author-token-region"),
        signal_type: key("test:author-token-signal"),
        direction: WirePortDirection::Output,
    };
    let target = WirePort {
        key: key("test:author-token-target"),
        device: key("test:author-token-consumer"),
        region: key("test:author-token-consumer-region"),
        signal_type: key("test:author-token-signal"),
        direction: WirePortDirection::Input,
    };
    let graph = WireGraph::from_parts(
        network,
        vec![input, output, target],
        vec![(output.key, target.key)],
        WireGraphLimits {
            ports: 3,
            directed_connections: 2,
            regions: 2,
        },
    )
    .unwrap();
    let handler = AuthorMaterialPump {
        input: input.key,
        output: output.key,
    };
    let device_limits = WireDeviceLimits {
        state_bytes: 1,
        emissions_per_input: 1,
        deliveries_per_input: 1,
        payload_bytes: 8,
        receipts: 4,
    };
    let mut device =
        WireDevice::new(network, device_key, &handler, vec![0], device_limits).unwrap();
    let signals = AuthorSignals;
    let signal_limits = SignalLimits {
        signals_per_tick: 8,
        queued_payload_bytes: 128,
        deliveries_per_signal: 2,
        payload_bytes: 8,
    };
    let mut queue = SignalQueue::new(network, &signals, 0).unwrap();
    let signal = Signal {
        network,
        source: key("test:author-token-controller"),
        sequence: 1,
        target: input.key,
        payload: vec![1],
    };

    let rollback_checkpoint = world.checkpoint(world.checkpoint_limit().unwrap()).unwrap();
    let candidate = device
        .prepare(&graph, &signal, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    let failed = candidate.commit_with(|deliveries, _| {
        let mut discarded_queue = queue.clone();
        for delivery in deliveries {
            discarded_queue
                .enqueue(delivery.signal.clone(), delivery.timing, signal_limits)
                .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "rollback queue候補"))?;
        }
        let mut banks = world.state::<AuthorMaterialBanks>(banks_key)?;
        banks.source_tokens -= u32::from(deliveries[0].signal.payload[0]);
        banks.target_tokens += u32::from(deliveries[0].signal.payload[0]);
        world.edit_material_accounted(
            &profile,
            &archive_inventory,
            balance_key,
            balance_schema,
            None,
            |edit| {
                edit.set_state(banks_key, banks_schema, &banks)?;
                Err::<(), _>(EngineError::new(
                    ErrorCode::InvalidState,
                    "作者token rollback",
                ))
            },
        )
    });
    assert!(failed.is_err());
    assert_eq!(device.revision(), 0);
    assert_eq!(queue.pending(), 0);
    assert_eq!(
        world.checkpoint(world.checkpoint_limit().unwrap()).unwrap(),
        rollback_checkpoint
    );

    let candidate = device
        .prepare(&graph, &signal, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    candidate
        .commit_with(|deliveries, _| {
            let mut next_queue = queue.clone();
            for delivery in deliveries {
                next_queue
                    .enqueue(delivery.signal.clone(), delivery.timing, signal_limits)
                    .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "作者signal queue"))?;
            }
            let mut banks = world.state::<AuthorMaterialBanks>(banks_key)?;
            let amount = u32::from(deliveries[0].signal.payload[0]);
            banks.source_tokens = banks
                .source_tokens
                .checked_sub(amount)
                .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "作者token source"))?;
            banks.target_tokens = banks
                .target_tokens
                .checked_add(amount)
                .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "作者token target"))?;
            world.edit_material_accounted(
                &profile,
                &archive_inventory,
                balance_key,
                balance_schema,
                None,
                |edit| edit.set_state(banks_key, banks_schema, &banks),
            )?;
            queue = next_queue;
            Ok::<_, EngineError>(())
        })
        .unwrap();
    assert_eq!(device.revision(), 1);
    assert_eq!(queue.pending(), 1);
    assert_eq!(
        world.state::<AuthorMaterialBanks>(banks_key).unwrap(),
        AuthorMaterialBanks {
            source_tokens: 3,
            target_tokens: 1,
            material: old_material,
            applied_epoch: 0,
        }
    );
    baseline
        .verify_conserved(
            &world
                .verify_material_accounting(&profile, &archive_inventory, balance_key)
                .unwrap(),
        )
        .unwrap();

    world
        .edit_material_accounted(
            &profile,
            &archive_inventory,
            balance_key,
            balance_schema,
            Some(MaterialChangeReason {
                key: key("test:author-token-remap"),
                version: 1,
                kind: MaterialReceiptKind::Conversion,
            }),
            |edit| {
                edit.remap_materials(charon_engine::world::MaterialRemap {
                    owner: provider_key,
                    layers: vec![],
                    mappings: vec![(MaterialHandle(old_material), MaterialHandle(new_material))],
                })
            },
        )
        .unwrap();
    let remapped = world
        .verify_material_accounting(&profile, &archive_inventory, balance_key)
        .unwrap();
    assert_eq!(remapped.quantities().len(), 1);
    assert_eq!(remapped.quantities()[0].material, new_material);
    assert_eq!(
        remapped.quantities()[0].tokens,
        MaterialQuantity::new(4, 1).unwrap()
    );
    assert_eq!(
        world
            .material_remaps()
            .resolve(provider_key, layer_key, 0, MaterialHandle(old_material),)
            .unwrap(),
        MaterialHandle(new_material)
    );

    let checkpoint = world.checkpoint(world.checkpoint_limit().unwrap()).unwrap();
    let restored = World::restore(
        &checkpoint,
        content.clone(),
        WorldLimits::default(),
        checkpoint.len(),
    )
    .unwrap();
    let restored_archives = restored
        .load_material_archive_inventory(&profile, &mut archives, 16_384)
        .unwrap();
    assert_eq!(
        restored
            .verify_material_accounting(&profile, &restored_archives, balance_key)
            .unwrap(),
        remapped
    );
}

struct AuthorGraph;
impl NavigationQuery for AuthorGraph {
    fn revision(&self) -> u64 {
        11
    }

    fn neighbors(
        &self,
        _: StableKey,
        node: u64,
    ) -> Result<NavigationExpansion, charon_engine::game::navigation::NavigationError> {
        let edges = match node {
            1 => vec![NavigationEdge {
                target: 2,
                cost: 1,
                action: key("test:walk"),
            }],
            2 => vec![NavigationEdge {
                target: 3,
                cost: 2,
                action: key("test:jump"),
            }],
            _ => vec![],
        };
        Ok(NavigationExpansion::Ready(edges))
    }

    fn heuristic(
        &self,
        _: StableKey,
        node: u64,
        goal: u64,
    ) -> Result<u64, charon_engine::game::navigation::NavigationError> {
        Ok(goal.saturating_sub(node))
    }
}

fn registry() -> SchemaRegistry {
    let mut registry = SchemaRegistry::default();
    registry
        .register(Schema::of::<NavigationJob>(key("test:navigation-job"), 1, 32_768).unwrap())
        .unwrap();
    registry
}

fn limits() -> StoreLimits {
    StoreLimits {
        records: 4,
        payload_bytes: 65_536,
        checkpoint_bytes: 131_072,
    }
}

#[test]
fn author_navigation_job_survives_state_store_checkpoint_and_resumes() {
    let query = AuthorGraph;
    let mut job = NavigationJob::new(
        &query,
        key("test:walker"),
        1,
        3,
        NavigationLimits::new(16, 16).unwrap(),
    )
    .unwrap();
    assert_eq!(job.poll(&query, 1).unwrap(), NavigationStatus::Continue);

    let mut store = StateStore::new(key("test:world"), registry(), limits()).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(key("test:route"), key("test:navigation-job"), &job)
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let restored =
        StateStore::restore(&checkpoint, key("test:world"), registry(), limits()).unwrap();
    let mut resumed = restored.get::<NavigationJob>(key("test:route")).unwrap();
    assert_eq!(
        resumed.poll(&query, 16).unwrap(),
        NavigationStatus::Complete(vec![
            NavigationStep {
                node: 1,
                action: None
            },
            NavigationStep {
                node: 2,
                action: Some(key("test:walk"))
            },
            NavigationStep {
                node: 3,
                action: Some(key("test:jump"))
            },
        ])
    );

    let encoded = job.encode(32_768).unwrap();
    let roundtrip = NavigationJob::decode(&encoded, 32_768).unwrap();
    assert_eq!(roundtrip, job);
}

struct WorldGridNavigationQuery<'a> {
    world: &'a World,
    layer: charon_engine::world::LayerId,
    profiles: [NavigationAgentProfile; 2],
}

impl WorldGridNavigationQuery<'_> {
    fn encode_node(x: i32, y: i32) -> u64 {
        (u64::from(y as u32) << 32) | u64::from(x as u32)
    }

    fn decode_node(node: u64) -> (i32, i32) {
        (node as u32 as i32, (node >> 32) as u32 as i32)
    }

    fn region_code(chunk: ChunkCoord) -> u64 {
        (u64::from(chunk.x as u32) << 32) | u64::from(chunk.y as u32)
    }
}

impl NavigationQuery for WorldGridNavigationQuery<'_> {
    fn revision(&self) -> u64 {
        self.world.revision()
    }

    fn agent_profile(
        &self,
        profile_key: StableKey,
    ) -> Result<Option<&NavigationAgentProfile>, charon_engine::game::navigation::NavigationError>
    {
        Ok(self
            .profiles
            .iter()
            .find(|profile| profile.key() == profile_key))
    }

    fn neighbors(
        &self,
        profile: StableKey,
        node: u64,
    ) -> Result<NavigationExpansion, charon_engine::game::navigation::NavigationError> {
        let profile = self
            .agent_profile(profile)?
            .ok_or(charon_engine::game::navigation::NavigationError::InvalidInput)?;
        self.neighbors_with_profile(profile, node)
    }

    fn neighbors_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
    ) -> Result<NavigationExpansion, charon_engine::game::navigation::NavigationError> {
        let (x, y) = Self::decode_node(node);
        let height = i32::try_from(profile.config().footprint_cells[1])
            .map_err(|_| charon_engine::game::navigation::NavigationError::InvalidInput)?;
        let directions = [
            (-1, 0, "test:navigate-west"),
            (1, 0, "test:navigate-east"),
            (0, -1, "test:navigate-south"),
            (0, 1, "test:navigate-north"),
        ];
        let mut edges = Vec::new();
        for (dx, dy, action) in directions {
            let Some(next_x) = x.checked_add(dx) else {
                return Err(charon_engine::game::navigation::NavigationError::InvalidInput);
            };
            let Some(next_y) = y.checked_add(dy) else {
                return Err(charon_engine::game::navigation::NavigationError::InvalidInput);
            };
            let mut blocked = false;
            for offset_y in 0..height {
                let Some(point_y) = next_y.checked_add(offset_y) else {
                    return Err(charon_engine::game::navigation::NavigationError::InvalidInput);
                };
                match self
                    .world
                    .cell(
                        self.layer,
                        CellPoint {
                            x: next_x,
                            y: point_y,
                        },
                    )
                    .map_err(|_| charon_engine::game::navigation::NavigationError::InvalidState)?
                {
                    CellQuery::Unavailable(chunk) => {
                        return Ok(NavigationExpansion::WaitingForRegion(Self::region_code(
                            chunk,
                        )));
                    }
                    CellQuery::WorldBoundary => blocked = true,
                    CellQuery::Loaded(cell) => {
                        let solid = self
                            .world
                            .content()
                            .material(cell.core.material)
                            .map_err(|_| {
                                charon_engine::game::navigation::NavigationError::InvalidState
                            })?
                            .is_some_and(|material| material.base() == MaterialBase::Solid);
                        blocked |= solid;
                    }
                }
            }
            if !blocked {
                edges.push(NavigationEdge {
                    target: Self::encode_node(next_x, next_y),
                    cost: 1,
                    action: key(action),
                });
            }
        }
        Ok(NavigationExpansion::Ready(edges))
    }

    fn heuristic(
        &self,
        _profile: StableKey,
        node: u64,
        goal: u64,
    ) -> Result<u64, charon_engine::game::navigation::NavigationError> {
        let (x, y) = Self::decode_node(node);
        let (goal_x, goal_y) = Self::decode_node(goal);
        Ok(u64::from(x.abs_diff(goal_x)) + u64::from(y.abs_diff(goal_y)))
    }
}

#[test]
fn author_world_navigation_respects_profile_shape_and_resumes_after_region_load() {
    let layer_key = key("test:navigation-terrain");
    let wall_key = key("test:navigation-wall");
    let mut content_registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    content_registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content_registry
        .register(MaterialBuilder::new("test:navigation-wall", MaterialBase::Solid).unwrap())
        .unwrap();
    let content = content_registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let wall = content.material_id(MaterialHandle(wall_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::new(CellPoint { x: 0, y: 0 }, 64, 64).unwrap(),
        content.clone(),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|editor| {
            editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for y in 0..8 {
                if y != 5 {
                    editor.set_cell(
                        layer,
                        CellPoint { x: 3, y },
                        CellCore {
                            material: wall,
                            ..CellCore::AIR
                        },
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
    let one_cell_profile = navigation_profile(key("test:navigation-one-cell"), 1, [1, 1]);
    let wide_profile = navigation_profile(key("test:navigation-two-cell-high"), 2, [1, 2]);
    let query = WorldGridNavigationQuery {
        world: &world,
        layer,
        profiles: [one_cell_profile.clone(), wide_profile.clone()],
    };
    let start = WorldGridNavigationQuery::encode_node(1, 1);
    let goal = WorldGridNavigationQuery::encode_node(5, 1);
    let nav_limits = NavigationLimits::new(256, 4).unwrap();
    let mut one_cell =
        NavigationJob::new(&query, one_cell_profile.key(), start, goal, nav_limits).unwrap();
    assert_eq!(
        one_cell.poll(&query, 1).unwrap(),
        NavigationStatus::Continue
    );

    let mut store = StateStore::new(key("test:world"), registry(), limits()).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(key("test:route"), key("test:navigation-job"), &one_cell)
        .unwrap();
    transaction.commit().unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let restored =
        StateStore::restore(&checkpoint, key("test:world"), registry(), limits()).unwrap();
    let mut one_cell = restored.get::<NavigationJob>(key("test:route")).unwrap();
    let NavigationStatus::Complete(one_cell_path) = one_cell.poll(&query, 256).unwrap() else {
        panic!("一セルprofileのWorld routeが完了しませんでした");
    };
    assert!(
        one_cell_path
            .iter()
            .any(|step| { WorldGridNavigationQuery::decode_node(step.node) == (3, 5) })
    );
    let mut uninterrupted =
        NavigationJob::new(&query, one_cell_profile.key(), start, goal, nav_limits).unwrap();
    let NavigationStatus::Complete(uninterrupted_path) = uninterrupted.poll(&query, 256).unwrap()
    else {
        panic!("連続実行のWorld routeが完了しませんでした");
    };
    assert_eq!(one_cell_path, uninterrupted_path);

    let mut wide = NavigationJob::new(&query, wide_profile.key(), start, goal, nav_limits).unwrap();
    let NavigationStatus::Complete(wide_path) = wide.poll(&query, 256).unwrap() else {
        panic!("二セル高profileのWorld routeが完了しませんでした");
    };
    assert!(wide_path.iter().any(|step| {
        let (x, y) = WorldGridNavigationQuery::decode_node(step.node);
        x == 3 && y >= 8
    }));

    let mut streaming_world = World::new(
        WorldId::from_parts(0, 2).unwrap(),
        0,
        WorldBounds::new(CellPoint { x: 0, y: 0 }, 128, 64).unwrap(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    streaming_world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let streaming_query = WorldGridNavigationQuery {
        world: &streaming_world,
        layer,
        profiles: [one_cell_profile.clone(), wide_profile.clone()],
    };
    let boundary_start = WorldGridNavigationQuery::encode_node(63, 3);
    let boundary_goal = WorldGridNavigationQuery::encode_node(64, 3);
    let mut streaming = NavigationJob::new(
        &streaming_query,
        one_cell_profile.key(),
        boundary_start,
        boundary_goal,
        nav_limits,
    )
    .unwrap();
    assert_eq!(
        streaming.poll(&streaming_query, 1).unwrap(),
        NavigationStatus::WaitingForRegion(WorldGridNavigationQuery::region_code(
            ChunkCoord::new(1, 0).unwrap()
        ))
    );
    streaming_world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(1, 0)?))
        .unwrap();
    let streaming_query = WorldGridNavigationQuery {
        world: &streaming_world,
        layer,
        profiles: [one_cell_profile.clone(), wide_profile.clone()],
    };
    assert_eq!(streaming.revalidate(&streaming_query), Ok(()));
    assert!(matches!(
        streaming.poll(&streaming_query, 4).unwrap(),
        NavigationStatus::Complete(_)
    ));
}

struct NavigationAgentShapeQueries {
    body_radius: f64,
}

impl CollisionQueriesProvider for NavigationAgentShapeQueries {
    fn key(&self) -> StableKey {
        key("test:navigation-agent-shape-queries")
    }

    fn version(&self) -> u32 {
        1
    }

    fn query(
        &self,
        _world: &World,
        physics: Option<&BodyPhysics>,
        request: &CollisionQueryRequest,
    ) -> EngineResult<CollisionQueryResult> {
        let (shape, start, end) = match &request.geometry {
            CollisionQueryGeometry::CustomOverlap { shape, pose } => (*shape, *pose, *pose),
            CollisionQueryGeometry::CustomSweep { shape, start, end } => (*shape, *start, *end),
            _ => {
                return Err(EngineError::new(
                    ErrorCode::InvalidValue,
                    "navigation agent shape query種別",
                ));
            }
        };
        if shape.provider != key("test:navigation-shape-definitions") || shape.version != 1 {
            return Err(EngineError::new(
                ErrorCode::MissingDependency,
                "navigation agent shape定義",
            ));
        }
        let agent_radius = match shape.object {
            1 => 0.2,
            2 => 1.25,
            _ => {
                return Err(EngineError::new(
                    ErrorCode::MissingDependency,
                    "navigation agent shape object",
                ));
            }
        };
        let physics = physics.ok_or_else(|| {
            EngineError::new(
                ErrorCode::InvalidState,
                "navigation physics候補がありません",
            )
        })?;
        let body = physics
            .body_ids()
            .next()
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidState, "navigation obstacleなし"))?;
        let obstacle = physics.state(body)?.pose;
        let delta_x = end.x - start.x;
        let delta_y = end.y - start.y;
        let segment_length_squared = delta_x.mul_add(delta_x, delta_y * delta_y);
        let projection = if segment_length_squared > f64::EPSILON {
            ((obstacle.x - start.x).mul_add(delta_x, (obstacle.y - start.y) * delta_y)
                / segment_length_squared)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let closest_x = delta_x.mul_add(projection, start.x);
        let closest_y = delta_y.mul_add(projection, start.y);
        let dx = obstacle.x - closest_x;
        let dy = obstacle.y - closest_y;
        let distance_squared = dx.mul_add(dx, dy * dy);
        let reach = agent_radius + self.body_radius;
        let hits = if distance_squared < reach * reach {
            let distance = distance_squared.sqrt();
            let normal = if distance > f64::EPSILON {
                [dx / distance, dy / distance]
            } else {
                [1.0, 0.0]
            };
            vec![CollisionQueryHit {
                target: CollisionQueryTarget::Body { body, part: 0 },
                material: None,
                point: [
                    closest_x + normal[0] * agent_radius,
                    closest_y + normal[1] * agent_radius,
                ],
                normal,
                time_of_impact: projection,
                geometry_revision: 1,
                coverage: 1.0,
            }]
        } else {
            Vec::new()
        };
        Ok(CollisionQueryResult {
            status: if hits.is_empty() {
                CollisionQueryStatus::NoHit
            } else {
                CollisionQueryStatus::Hit
            },
            hits,
            truncated: false,
            geometry_revision: 1,
        })
    }
}

struct DynamicBodyNavigationQuery<'a> {
    world: &'a World,
    layer: charon_engine::world::LayerId,
    shapes: NavigationAgentShapeQueries,
    profiles: [NavigationAgentProfile; 2],
}

impl DynamicBodyNavigationQuery<'_> {
    fn encode_node(x: i32, y: i32) -> u64 {
        (u64::from(y as u32) << 32) | u64::from(x as u32)
    }

    fn decode_node(node: u64) -> (i32, i32) {
        (node as u32 as i32, (node >> 32) as u32 as i32)
    }
}

impl NavigationQuery for DynamicBodyNavigationQuery<'_> {
    fn revision(&self) -> u64 {
        self.world.revision()
    }

    fn agent_profile(
        &self,
        profile_key: StableKey,
    ) -> Result<Option<&NavigationAgentProfile>, NavigationError> {
        Ok(self
            .profiles
            .iter()
            .find(|profile| profile.key() == profile_key))
    }

    fn neighbors(
        &self,
        profile: StableKey,
        node: u64,
    ) -> Result<NavigationExpansion, NavigationError> {
        let profile = self
            .agent_profile(profile)?
            .ok_or(NavigationError::InvalidInput)?;
        self.neighbors_with_profile(profile, node)
    }

    fn neighbors_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
    ) -> Result<NavigationExpansion, NavigationError> {
        let (x, y) = Self::decode_node(node);
        let mut edges = Vec::new();
        for (dx, dy, action) in [
            (-1, 0, "test:agent-walk"),
            (1, 0, "test:agent-walk"),
            (0, -1, "test:agent-walk"),
            (0, 1, "test:agent-walk"),
        ] {
            let pose = BodyPose {
                x: f64::from(x + dx) + 0.5,
                y: f64::from(y + dy) + 0.5,
                angle: 0.0,
            };
            let report = self
                .world
                .query_body_collisions(
                    &CollisionQueryRequest {
                        geometry: CollisionQueryGeometry::CustomSweep {
                            shape: profile.shape(),
                            start: BodyPose {
                                x: f64::from(x) + 0.5,
                                y: f64::from(y) + 0.5,
                                angle: 0.0,
                            },
                            end: pose,
                        },
                        layer: self.layer,
                        collision_groups: profile.config().collision_groups,
                        owner: Some(profile.key()),
                        ignore_bodies: Vec::new(),
                        one_way_direction: None,
                        include_sensors: false,
                        include_initial_overlap: true,
                        include_boundary: true,
                        epsilon: 1.0e-9,
                        tick: self.world.tick(),
                        substep: 0,
                        read_latest_candidate: true,
                        first_hit_only: true,
                        stable_order: true,
                        max_hits: 1,
                    },
                    &self.shapes,
                )
                .map_err(|_| NavigationError::InvalidState)?;
            match report.result.status {
                CollisionQueryStatus::NoHit => edges.push(NavigationEdge {
                    target: Self::encode_node(x + dx, y + dy),
                    cost: 1,
                    action: key(action),
                }),
                CollisionQueryStatus::Hit => {}
                CollisionQueryStatus::Incomplete | CollisionQueryStatus::Unsupported => {
                    return Err(NavigationError::InvalidState);
                }
            }
        }
        Ok(NavigationExpansion::Ready(edges))
    }

    fn heuristic(&self, _profile: StableKey, node: u64, goal: u64) -> Result<u64, NavigationError> {
        let (x, y) = Self::decode_node(node);
        let (goal_x, goal_y) = Self::decode_node(goal);
        Ok(u64::from(x.abs_diff(goal_x)) + u64::from(y.abs_diff(goal_y)))
    }

    fn heuristic_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
        goal: u64,
    ) -> Result<u64, NavigationError> {
        self.heuristic(profile.key(), node, goal)
    }
}

struct ProfileMovementQuery {
    profile: NavigationAgentProfile,
    platform_cycle: u32,
    fly_edge_cost: std::cell::Cell<u32>,
}

impl NavigationQuery for ProfileMovementQuery {
    fn revision(&self) -> u64 {
        u64::from(self.platform_cycle)
    }

    fn agent_profile(
        &self,
        profile_key: StableKey,
    ) -> Result<Option<&NavigationAgentProfile>, NavigationError> {
        Ok((profile_key == self.profile.key()).then_some(&self.profile))
    }

    fn neighbors(&self, _: StableKey, _: u64) -> Result<NavigationExpansion, NavigationError> {
        Err(NavigationError::InvalidInput)
    }

    fn neighbors_with_profile(
        &self,
        profile: &NavigationAgentProfile,
        node: u64,
    ) -> Result<NavigationExpansion, NavigationError> {
        let movement = profile.config();
        let step = key("test:step");
        let jump = key("test:jump");
        let fall = key("test:fall");
        let swim = key("test:swim");
        let fly = key("test:fly");
        let platform = key("test:moving-platform-link");
        let edges = match node {
            0 if movement.max_step_cells >= 1 => vec![NavigationEdge {
                target: 1,
                cost: movement.travel_ticks_per_cell,
                action: step,
            }],
            1 if movement.max_jump_height_cells >= 2 && movement.max_jump_range_cells >= 3 => {
                vec![NavigationEdge {
                    target: 2,
                    cost: movement.travel_ticks_per_cell.saturating_mul(3),
                    action: jump,
                }]
            }
            2 if movement.max_fall_cells >= 4 => vec![NavigationEdge {
                target: 3,
                cost: movement.travel_ticks_per_cell,
                action: fall,
            }],
            3 if movement.can_swim => vec![NavigationEdge {
                target: 4,
                cost: movement.travel_ticks_per_cell,
                action: swim,
            }],
            4 if movement.can_fly => {
                let extra = movement
                    .hazard_costs
                    .iter()
                    .find(|cost| cost.hazard == key("test:navigation-hazard"))
                    .map_or(0, |cost| cost.additional_ticks);
                let cost = movement
                    .travel_ticks_per_cell
                    .checked_add(extra)
                    .ok_or(NavigationError::CostOverflow)?;
                self.fly_edge_cost.set(cost);
                vec![NavigationEdge {
                    target: 5,
                    cost,
                    action: fly,
                }]
            }
            5 if self.platform_cycle.is_multiple_of(2) => vec![NavigationEdge {
                target: 6,
                cost: 2,
                action: platform,
            }],
            _ => Vec::new(),
        };
        Ok(NavigationExpansion::Ready(edges))
    }

    fn heuristic(&self, _: StableKey, node: u64, goal: u64) -> Result<u64, NavigationError> {
        Ok(goal.saturating_sub(node))
    }
}

#[test]
fn author_navigation_profile_applies_locomotion_hazards_and_timed_platform_links() {
    let profile = navigation_profile(key("test:full-movement-agent"), 3, [1, 2]);
    let query = ProfileMovementQuery {
        profile: profile.clone(),
        platform_cycle: 2,
        fly_edge_cost: std::cell::Cell::new(0),
    };
    let mut job = NavigationJob::new(
        &query,
        profile.key(),
        0,
        6,
        NavigationLimits::new(16, 1).unwrap(),
    )
    .unwrap();
    let NavigationStatus::Complete(path) = job.poll(&query, 16).unwrap() else {
        panic!("step/jump/fall/swim/fly/platformを通るprofile経路が完了しませんでした");
    };
    assert_eq!(
        path.iter()
            .filter_map(|step| step.action)
            .collect::<Vec<_>>(),
        [
            key("test:step"),
            key("test:jump"),
            key("test:fall"),
            key("test:swim"),
            key("test:fly"),
            key("test:moving-platform-link"),
        ]
    );
    assert_eq!(query.fly_edge_cost.get(), 6);

    let mut in_flight = NavigationJob::new(
        &query,
        profile.key(),
        0,
        6,
        NavigationLimits::new(16, 1).unwrap(),
    )
    .unwrap();
    assert_eq!(
        in_flight.poll(&query, 1).unwrap(),
        NavigationStatus::Continue
    );
    let unavailable_platform = ProfileMovementQuery {
        profile: profile.clone(),
        platform_cycle: 3,
        fly_edge_cost: std::cell::Cell::new(0),
    };
    assert_eq!(
        in_flight.revalidate(&unavailable_platform),
        Err(NavigationError::StaleInput)
    );
    let mut scheduled = NavigationJob::new(
        &unavailable_platform,
        unavailable_platform.profile.key(),
        0,
        6,
        NavigationLimits::new(16, 1).unwrap(),
    )
    .unwrap();
    assert_eq!(
        scheduled.poll(&unavailable_platform, 16).unwrap(),
        NavigationStatus::NoPath
    );
}

#[test]
fn author_navigation_queries_custom_agent_shapes_against_dynamic_bodies() {
    let layer_key = key("test:dynamic-navigation-layer");
    let mut content_registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    content_registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let content = content_registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::new(CellPoint { x: 0, y: 0 }, 16, 16).unwrap(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let obstacle = world
        .edit_bodies(|physics| {
            let body = physics.insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))?;
            physics.set_pose(
                body,
                BodyPose {
                    x: 4.5,
                    y: 2.5,
                    angle: 0.0,
                },
            )?;
            Ok(body)
        })
        .unwrap();

    let small_profile = navigation_profile(key("test:small-navigation-agent"), 1, [1, 1]);
    let large_profile = navigation_profile(key("test:large-navigation-agent"), 2, [2, 2]);
    let query = DynamicBodyNavigationQuery {
        world: &world,
        layer,
        shapes: NavigationAgentShapeQueries { body_radius: 0.5 },
        profiles: [small_profile.clone(), large_profile.clone()],
    };
    let start = DynamicBodyNavigationQuery::encode_node(1, 2);
    let goal = DynamicBodyNavigationQuery::encode_node(7, 2);
    let limits = NavigationLimits::new(256, 4).unwrap();
    let mut small = NavigationJob::new(&query, small_profile.key(), start, goal, limits).unwrap();
    let mut large = NavigationJob::new(&query, large_profile.key(), start, goal, limits).unwrap();
    let NavigationStatus::Complete(small_path) = small.poll(&query, 256).unwrap() else {
        panic!("小shape agentのrouteが完了しませんでした");
    };
    let NavigationStatus::Complete(large_path) = large.poll(&query, 256).unwrap() else {
        panic!("大shape agentのrouteが完了しませんでした");
    };
    assert_ne!(small_path, large_path);
    assert!(small_path.iter().any(|step| {
        DynamicBodyNavigationQuery::decode_node(step.node)
            .1
            .abs_diff(2)
            == 1
    }));
    assert!(large_path.iter().any(|step| {
        DynamicBodyNavigationQuery::decode_node(step.node)
            .1
            .abs_diff(2)
            >= 2
    }));

    let mut stale = NavigationJob::new(&query, small_profile.key(), start, goal, limits).unwrap();
    assert_eq!(stale.poll(&query, 1).unwrap(), NavigationStatus::Continue);
    let old_revision = world.revision();
    world
        .edit_bodies(|physics| {
            physics.set_pose(
                obstacle,
                BodyPose {
                    x: 12.5,
                    y: 10.5,
                    angle: 0.0,
                },
            )
        })
        .unwrap();
    assert!(world.revision() > old_revision);
    let moved_query = DynamicBodyNavigationQuery {
        world: &world,
        layer,
        shapes: NavigationAgentShapeQueries { body_radius: 0.5 },
        profiles: [small_profile.clone(), large_profile.clone()],
    };
    assert_eq!(
        stale.revalidate(&moved_query),
        Err(NavigationError::StaleInput)
    );
    let mut refreshed =
        NavigationJob::new(&moved_query, small_profile.key(), start, goal, limits).unwrap();
    assert!(matches!(
        refreshed.poll(&moved_query, 256).unwrap(),
        NavigationStatus::Complete(_)
    ));
}

struct AuthorEffect;
impl EffectEvaluator for AuthorEffect {
    fn key(&self) -> StableKey {
        key("test:author-effect")
    }
    fn version(&self) -> u32 {
        1
    }
    fn evaluate(
        &self,
        node: StableKey,
        context: EffectContext,
        _: &[u8],
    ) -> Result<EffectNodeResult, charon_engine::game::effects::EffectError> {
        Ok(EffectNodeResult {
            control: EffectControl::Next,
            state: vec![node.as_str().len() as u8],
            commands: vec![EffectCommand {
                key: key("test:effect-command"),
                payload: vec![context.tick as u8],
            }],
            receipts: vec![context.random_cursor + 100],
            spawn_count: 0,
            cell_count: 0,
            random_draws: 1,
            schedule: EffectSchedule::Deferred,
        })
    }
}

#[test]
fn author_effect_state_and_commands_commit_after_store_candidate_and_resume() {
    let program = EffectProgram::new(
        key("test:effect-program"),
        1,
        vec![key("test:node-a"), key("test:node-b")],
    )
    .unwrap();
    let evaluator = AuthorEffect;
    let mut job = EffectJob::new(&program, &evaluator, 4, 8, key("test:effect-random"), 7).unwrap();
    let limits = EffectLimits::new(128, 4, 128, 16, 16, 64, 8).unwrap();

    let mut registry = SchemaRegistry::default();
    registry
        .register(Schema::of::<EffectJob>(key("test:effect-job-schema"), 1, 4096).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 8192,
        checkpoint_bytes: 16384,
    };
    let mut store = StateStore::new(key("test:effect-scope"), registry, store_limits).unwrap();

    let candidate = job.prepare(&program, &evaluator, 12, limits).unwrap();
    assert_eq!(candidate.commands().len(), 1);
    let (_, committed) = candidate
        .commit_with(|commands, candidate_job| {
            assert_eq!(commands.len(), 1);
            let mut transaction = store.transaction()?;
            transaction.put(
                key("test:active-effect"),
                key("test:effect-job-schema"),
                candidate_job,
            )?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(
        committed.progress,
        charon_engine::game::effects::EffectProgress::Deferred
    );

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(key("test:effect-job-schema"), 1, 4096).unwrap())
        .unwrap();
    let restored_store =
        StateStore::restore(&checkpoint, key("test:effect-scope"), schemas, store_limits).unwrap();
    let mut restored = restored_store
        .get::<EffectJob>(key("test:active-effect"))
        .unwrap();
    assert_eq!(restored, job);
    assert!(restored.prepare(&program, &evaluator, 12, limits).is_err());
    assert_eq!(
        restored
            .prepare(&program, &evaluator, 13, limits)
            .unwrap()
            .commit_with(|_, _| Ok::<_, ()>(()))
            .unwrap()
            .1
            .progress,
        charon_engine::game::effects::EffectProgress::Complete
    );
}

struct RuntimeEffectEvaluator;
impl EffectEvaluator for RuntimeEffectEvaluator {
    fn key(&self) -> StableKey {
        key("test:runtime-effect-evaluator")
    }
    fn version(&self) -> u32 {
        1
    }
    fn evaluate(
        &self,
        node: StableKey,
        _: EffectContext,
        _: &[u8],
    ) -> Result<EffectNodeResult, EffectError> {
        let (payload, schedule, control) = if node == key("test:runtime-effect-a") {
            (1, EffectSchedule::SameTick, EffectControl::Next)
        } else if node == key("test:runtime-effect-b") {
            (2, EffectSchedule::Deferred, EffectControl::Next)
        } else if node == key("test:runtime-effect-c") {
            (4, EffectSchedule::Deferred, EffectControl::Complete)
        } else {
            return Err(EffectError::InvalidProgram);
        };
        Ok(EffectNodeResult {
            control,
            state: vec![payload],
            commands: vec![EffectCommand {
                key: key("test:runtime-effect-command"),
                payload: vec![payload],
            }],
            receipts: Vec::new(),
            spawn_count: 0,
            cell_count: 0,
            random_draws: 0,
            schedule,
        })
    }
}

struct RuntimeEffectSystem {
    program: EffectProgram,
    evaluator: RuntimeEffectEvaluator,
    limits: EffectLimits,
}
impl System for RuntimeEffectSystem {
    fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let tick = context.tick();
        let mut worlds = context.worlds();
        let world = worlds.view().id("main")?;
        let mut job = worlds
            .view()
            .get::<EffectJob>(world, key("test:runtime-effect-job"))?;
        if job.is_complete() {
            return Ok(WorkStatus::Complete);
        }
        let candidate = job
            .prepare(&self.program, &self.evaluator, tick, self.limits)
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "Runtime effect候補"))?;
        let progress = candidate.progress();
        candidate.commit_with(|commands, job_state| {
            worlds.set(
                world,
                key("test:runtime-effect-job"),
                key("test:runtime-effect-job-schema"),
                job_state,
            )?;
            let mut total = worlds
                .view()
                .get::<u64>(world, key("test:runtime-effect-command-total"))?;
            for command in commands {
                if command.key != key("test:runtime-effect-command") || command.payload.len() != 1 {
                    return Err(EngineError::new(
                        ErrorCode::InvalidValue,
                        "Runtime effect command",
                    ));
                }
                total = total
                    .checked_add(u64::from(command.payload[0]))
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::CapacityExceeded, "effect command total")
                    })?;
            }
            worlds.set(
                world,
                key("test:runtime-effect-command-total"),
                key("test:runtime-effect-counter-schema"),
                &total,
            )
        })?;
        Ok(match progress {
            EffectProgress::ContinueSameTick => WorkStatus::ContinueSameTick,
            EffectProgress::Deferred | EffectProgress::Complete => WorkStatus::Complete,
        })
    }
}

fn runtime_effect_schemas() -> SchemaRegistry {
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(key("test:runtime-effect-job-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<u64>(key("test:runtime-effect-counter-schema"), 1, 8).unwrap())
        .unwrap();
    schemas
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeAgentRouteState {
    node: u64,
    cursor: u32,
    route: Vec<NavigationStep>,
    last_action: Option<StableKey>,
}

impl StateCodec for RuntimeAgentRouteState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self.route.len() > 16 || self.cursor as usize > self.route.len() {
            return Err(EngineError::new(ErrorCode::InvalidValue, "AI route state"));
        }
        out.raw(b"CHRAI001")?;
        out.u64(self.node)?;
        out.u32(self.cursor)?;
        out.u8(u8::try_from(self.route.len())
            .map_err(|_| EngineError::new(ErrorCode::CapacityExceeded, "AI route length"))?)?;
        for step in &self.route {
            out.u64(step.node)?;
            match step.action {
                Some(action) => {
                    out.u8(1)?;
                    action.encode(out)?;
                }
                None => out.u8(0)?,
            }
        }
        match self.last_action {
            Some(action) => {
                out.u8(1)?;
                action.encode(out)?;
            }
            None => out.u8(0)?,
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHRAI001" {
            return Err(EngineError::new(ErrorCode::InvalidValue, "AI route state"));
        }
        let node = input.u64()?;
        let cursor = input.u32()?;
        let count = input.u8()? as usize;
        if count > 16 || cursor as usize > count {
            return Err(EngineError::new(
                ErrorCode::CapacityExceeded,
                "AI route length",
            ));
        }
        let mut route = Vec::new();
        route
            .try_reserve_exact(count)
            .map_err(|_| EngineError::new(ErrorCode::CapacityExceeded, "AI route allocation"))?;
        for _ in 0..count {
            let node = input.u64()?;
            let action = match input.u8()? {
                0 => None,
                1 => Some(StableKey::decode(input)?),
                _ => return Err(EngineError::new(ErrorCode::InvalidValue, "AI route action")),
            };
            route.push(NavigationStep { node, action });
        }
        let last_action = match input.u8()? {
            0 => None,
            1 => Some(StableKey::decode(input)?),
            _ => return Err(EngineError::new(ErrorCode::InvalidValue, "AI action state")),
        };
        Ok(Self {
            node,
            cursor,
            route,
            last_action,
        })
    }
}

struct RuntimeNavigationController {
    profile: StableKey,
}

impl System for RuntimeNavigationController {
    fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = context.worlds();
        let world = worlds.view().id("main")?;
        let mut route_state = worlds
            .view()
            .get::<RuntimeAgentRouteState>(world, key("test:runtime-ai-route"))?;
        if route_state.route.is_empty() {
            let mut job = worlds
                .view()
                .get::<NavigationJob>(world, key("test:runtime-ai-job"))?;
            match job
                .poll(&AuthorGraph, 8)
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "Runtime AI navigation"))?
            {
                NavigationStatus::Complete(route) if route.len() > 1 => {
                    if job.input_revision() != AuthorGraph.revision()
                        || self.profile != key("test:runtime-ai-profile")
                    {
                        return Err(EngineError::new(
                            ErrorCode::Conflict,
                            "Runtime AI profile/revision",
                        ));
                    }
                    worlds.set(
                        world,
                        key("test:runtime-ai-job"),
                        key("test:runtime-ai-job-schema"),
                        &job,
                    )?;
                    route_state.route = route;
                    route_state.cursor = 1;
                }
                NavigationStatus::Continue => {
                    worlds.set(
                        world,
                        key("test:runtime-ai-job"),
                        key("test:runtime-ai-job-schema"),
                        &job,
                    )?;
                    return Ok(WorkStatus::Complete);
                }
                _ => {
                    return Err(EngineError::new(
                        ErrorCode::InvalidState,
                        "Runtime AI route is not available",
                    ));
                }
            }
        }

        if let Some(step) = route_state.route.get(route_state.cursor as usize).copied() {
            route_state.node = step.node;
            route_state.last_action = step.action;
            route_state.cursor += 1;
        } else {
            route_state.last_action = None;
        }
        worlds.set(
            world,
            key("test:runtime-ai-route"),
            key("test:runtime-ai-route-schema"),
            &route_state,
        )?;
        Ok(WorkStatus::Complete)
    }
}

#[test]
fn author_runtime_ai_navigation_routes_actions_into_saved_controller_intent() {
    let query = AuthorGraph;
    let profile = key("test:runtime-ai-profile");
    let job =
        NavigationJob::new(&query, profile, 1, 3, NavigationLimits::new(8, 4).unwrap()).unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<NavigationJob>(key("test:runtime-ai-job-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<RuntimeAgentRouteState>(key("test:runtime-ai-route-schema"), 1, 4096)
                .unwrap(),
        )
        .unwrap();
    let mut state = StateStore::new(key("test:runtime-ai-state"), schemas, limits()).unwrap();
    let mut transaction = state.transaction().unwrap();
    transaction
        .put(
            key("test:runtime-ai-job"),
            key("test:runtime-ai-job-schema"),
            &job,
        )
        .unwrap();
    transaction
        .put(
            key("test:runtime-ai-route"),
            key("test:runtime-ai-route-schema"),
            &RuntimeAgentRouteState {
                node: 1,
                cursor: 0,
                route: Vec::new(),
                last_action: None,
            },
        )
        .unwrap();
    transaction.commit().unwrap();

    let mut engine = Engine::builder()
        .world("main", state)
        .system(
            SystemDescriptor::new(key("test:runtime-ai-controller"), Phase::Intent)
                .write(key("test:runtime-ai-job"))
                .write(key("test:runtime-ai-route"))
                .barrier(),
            RuntimeNavigationController { profile },
        )
        .build()
        .unwrap();
    for tick in 1..=2 {
        assert_eq!(
            engine.step(TickInput::empty(tick)).unwrap(),
            StepResult::Complete(tick)
        );
    }
    let world = engine.worlds().id("main").unwrap();
    let state = engine
        .worlds()
        .get::<RuntimeAgentRouteState>(world, key("test:runtime-ai-route"))
        .unwrap();
    assert_eq!(state.node, 3);
    assert_eq!(state.last_action, Some(key("test:jump")));
    assert_eq!(state.cursor, 3);
}

#[test]
fn author_effect_system_commits_same_tick_and_deferred_steps_through_runtime_barriers() {
    let program = EffectProgram::new(
        key("test:runtime-effect-program"),
        1,
        vec![
            key("test:runtime-effect-a"),
            key("test:runtime-effect-b"),
            key("test:runtime-effect-c"),
        ],
    )
    .unwrap();
    let evaluator = RuntimeEffectEvaluator;
    let job = EffectJob::new(
        &program,
        &evaluator,
        7,
        9,
        key("test:runtime-effect-random"),
        13,
    )
    .unwrap();
    let mut state = StateStore::new(
        key("test:runtime-state"),
        runtime_effect_schemas(),
        limits(),
    )
    .unwrap();
    let mut transaction = state.transaction().unwrap();
    transaction
        .put(
            key("test:runtime-effect-job"),
            key("test:runtime-effect-job-schema"),
            &job,
        )
        .unwrap();
    transaction
        .put(
            key("test:runtime-effect-command-total"),
            key("test:runtime-effect-counter-schema"),
            &0_u64,
        )
        .unwrap();
    transaction.commit().unwrap();

    let descriptor = SystemDescriptor::new(key("test:runtime-effect-system"), Phase::Intent)
        .write(key("test:runtime-effect-job"))
        .write(key("test:runtime-effect-command-total"))
        .barrier();
    let mut engine = Engine::builder()
        .world("main", state)
        .system(
            descriptor,
            RuntimeEffectSystem {
                program,
                evaluator,
                limits: EffectLimits::new(32, 2, 32, 4, 0, 0, 4).unwrap(),
            },
        )
        .build()
        .unwrap();

    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let world = engine.worlds().id("main").unwrap();
    let deferred = engine
        .worlds()
        .get::<EffectJob>(world, key("test:runtime-effect-job"))
        .unwrap();
    assert_eq!(deferred.instruction(), 2);
    assert!(!deferred.is_complete());
    assert_eq!(
        engine
            .worlds()
            .get::<u64>(world, key("test:runtime-effect-command-total"))
            .unwrap(),
        3
    );

    let checkpoint = engine.worlds().checkpoint(world).unwrap();
    let restored = StateStore::restore(
        &checkpoint,
        key("test:runtime-state"),
        runtime_effect_schemas(),
        limits(),
    )
    .unwrap();
    assert_eq!(
        restored
            .get::<EffectJob>(key("test:runtime-effect-job"))
            .unwrap(),
        deferred
    );

    assert_eq!(
        engine.step(TickInput::empty(2)).unwrap(),
        StepResult::Complete(2)
    );
    assert!(
        engine
            .worlds()
            .get::<EffectJob>(world, key("test:runtime-effect-job"))
            .unwrap()
            .is_complete()
    );
    assert_eq!(
        engine
            .worlds()
            .get::<u64>(world, key("test:runtime-effect-command-total"))
            .unwrap(),
        7
    );
}

struct EffectTriggerEvents;
impl TransactionalWorldEventProvider for EffectTriggerEvents {
    fn key(&self) -> StableKey {
        key("test:effect-trigger-events")
    }
    fn version(&self) -> u32 {
        1
    }
    fn handle(
        &self,
        event: WorldEventView<'_>,
    ) -> Result<WorldEventTransactionResult, charon_engine::game::signals::SignalError> {
        let progression_changes = if event.key == key("test:boss-effect-trigger") {
            vec![ProgressionChange::AddCounter {
                key: key("test:effect-trigger-count"),
                delta: 1,
            }]
        } else {
            Vec::new()
        };
        Ok(WorldEventTransactionResult {
            deliveries: Vec::new(),
            progression_changes,
            has_world_commands: true,
        })
    }
}

fn triggered_effect_schemas() -> SchemaRegistry {
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WorldEventQueue>(key("test:trigger-event-schema"), 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<ProgressionState>(key("test:trigger-progression-schema"), 1, 8192)
                .unwrap(),
        )
        .unwrap();
    schemas
        .register(Schema::of::<EffectJob>(key("test:runtime-effect-job-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<u64>(key("test:runtime-effect-counter-schema"), 1, 8).unwrap())
        .unwrap();
    schemas
}

#[test]
fn world_event_receipt_triggers_effect_job_atomically_and_runtime_resumes_it() {
    let scope = key("test:effect-trigger-scope");
    let event_provider = EffectTriggerEvents;
    let signal_limits = SignalLimits {
        signals_per_tick: 8,
        queued_payload_bytes: 256,
        deliveries_per_signal: 2,
        payload_bytes: 32,
    };
    let trigger_event = WorldEvent {
        scope,
        key: key("test:boss-effect-trigger"),
        source: key("test:boss"),
        sequence: 41,
        payload: vec![3],
    };
    let mut events = WorldEventQueue::new_transactional(scope, &event_provider, 1).unwrap();
    events
        .publish(trigger_event.clone(), SignalTiming::SameTick, signal_limits)
        .unwrap();
    assert_eq!(events.pending(), 1);
    assert_eq!(events.ready_pending(), 1);

    let progression_key = key("test:effect-trigger-count");
    let progression_catalog = ProgressionCatalog::new(vec![ProgressionDefinition {
        key: progression_key,
        kind: ProgressionKind::Counter,
        initial: 0,
        minimum: 0,
        maximum: 1,
    }])
    .unwrap();
    let mut progression = progression_catalog
        .create_state(scope, &[progression_key], 8)
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 65_536,
        checkpoint_bytes: 131_072,
    };
    let queue_key = key("test:effect-trigger-queue");
    let queue_schema = key("test:trigger-event-schema");
    let progression_state_key = key("test:effect-trigger-progression");
    let progression_schema = key("test:trigger-progression-schema");
    let job_key = key("test:runtime-effect-job");
    let job_schema = key("test:runtime-effect-job-schema");
    let counter_key = key("test:runtime-effect-command-total");
    let counter_schema = key("test:runtime-effect-counter-schema");
    let mut state = StateStore::new(scope, triggered_effect_schemas(), store_limits).unwrap();
    let mut initial = state.transaction().unwrap();
    initial.put(queue_key, queue_schema, &events).unwrap();
    initial
        .put(progression_state_key, progression_schema, &progression)
        .unwrap();
    initial.put(counter_key, counter_schema, &0_u64).unwrap();
    initial.commit().unwrap();
    let initial_checkpoint = state.checkpoint().unwrap();

    let program = EffectProgram::new(
        key("test:runtime-effect-program"),
        1,
        vec![
            key("test:runtime-effect-a"),
            key("test:runtime-effect-b"),
            key("test:runtime-effect-c"),
        ],
    )
    .unwrap();
    let evaluator = RuntimeEffectEvaluator;
    let mut world = empty_world();
    let world_revision = world.revision();
    let failed = events.poll_transactionally_with_state(
        &mut progression,
        &mut world,
        &mut state,
        queue_key,
        queue_schema,
        progression_state_key,
        progression_schema,
        &event_provider,
        &progression_catalog,
        signal_limits,
        4,
        |actions, transaction| {
            assert_eq!(actions.len(), 1);
            let job = EffectJob::new(
                &program,
                &evaluator,
                7,
                9,
                key("test:runtime-effect-random"),
                actions[0].event.sequence,
            )
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "trigger EffectJob"))?;
            transaction.put(job_key, job_schema, &job)
        },
        |_, editor| {
            editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            Err::<(), _>(EngineError::new(
                ErrorCode::InvalidState,
                "trigger World rollback",
            ))
        },
    );
    assert!(
        matches!(
            &failed,
            Err(charon_engine::game::events::WorldEventTransactionError::World(_))
        ),
        "{failed:?}"
    );
    assert_eq!(world.revision(), world_revision);
    assert_eq!(events.pending(), 1);
    assert_eq!(progression.get(progression_key), Some(0));
    assert_eq!(state.checkpoint().unwrap(), initial_checkpoint);

    let (progress, result) = events
        .poll_transactionally_with_state(
            &mut progression,
            &mut world,
            &mut state,
            queue_key,
            queue_schema,
            progression_state_key,
            progression_schema,
            &event_provider,
            &progression_catalog,
            signal_limits,
            4,
            |actions, transaction| {
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].event.sequence, 41);
                let job = EffectJob::new(
                    &program,
                    &evaluator,
                    7,
                    9,
                    key("test:runtime-effect-random"),
                    actions[0].event.sequence,
                )
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "trigger EffectJob"))?;
                transaction.put(job_key, job_schema, &job)
            },
            |_, editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?),
        )
        .unwrap();
    assert_eq!(progress, SignalProgress::TickComplete);
    assert_eq!(result, Some(()));
    assert_eq!(events.pending(), 0);
    assert_eq!(progression.get(progression_key), Some(1));
    assert_eq!(world.revision(), world_revision + 1);
    assert_eq!(state.get::<WorldEventQueue>(queue_key).unwrap(), events);
    assert_eq!(
        state
            .get::<ProgressionState>(progression_state_key)
            .unwrap(),
        progression
    );
    let triggered_job = state.get::<EffectJob>(job_key).unwrap();
    assert_eq!(triggered_job.instruction(), 0);

    let descriptor = SystemDescriptor::new(key("test:runtime-effect-system"), Phase::Intent)
        .write(job_key)
        .write(counter_key)
        .barrier();
    let mut engine = Engine::builder()
        .world("main", state)
        .system(
            descriptor,
            RuntimeEffectSystem {
                program,
                evaluator,
                limits: EffectLimits::new(32, 2, 32, 4, 0, 0, 4).unwrap(),
            },
        )
        .build()
        .unwrap();
    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let runtime_world = engine.worlds().id("main").unwrap();
    let checkpoint = engine.worlds().checkpoint(runtime_world).unwrap();
    let restored =
        StateStore::restore(&checkpoint, scope, triggered_effect_schemas(), store_limits).unwrap();
    let mut restored_events = restored.get::<WorldEventQueue>(queue_key).unwrap();
    assert!(
        !restored_events
            .publish(trigger_event, SignalTiming::SameTick, signal_limits)
            .unwrap()
    );
    let deferred = restored.get::<EffectJob>(job_key).unwrap();
    assert_eq!(deferred.instruction(), 2);
    assert_eq!(
        restored
            .get::<ProgressionState>(progression_state_key)
            .unwrap(),
        progression
    );
    assert_eq!(restored.get::<u64>(counter_key).unwrap(), 3);

    assert_eq!(
        engine.step(TickInput::empty(2)).unwrap(),
        StepResult::Complete(2)
    );
    assert!(
        engine
            .worlds()
            .get::<EffectJob>(runtime_world, job_key)
            .unwrap()
            .is_complete()
    );
    assert_eq!(
        engine
            .worlds()
            .get::<u64>(runtime_world, counter_key)
            .unwrap(),
        7
    );
}

#[test]
fn author_compiled_effect_timer_and_deferred_commands_resume_after_checkpoint() {
    let limits = EffectLimits::new(128, 4, 128, 8, 4, 16, 8).unwrap();
    let catalog = EffectNodeCatalog::new(
        vec![
            EffectNodeDefinition {
                key: key("test:compiled-timer"),
                operation: EffectNodeOperation::WaitTicks { ticks: 3 },
            },
            EffectNodeDefinition {
                key: key("test:compiled-start"),
                operation: EffectNodeOperation::Emit {
                    commands: vec![EffectCommand {
                        key: key("test:compiled-command-a"),
                        payload: vec![1, 2],
                    }],
                    state: vec![3],
                    receipts: vec![71],
                    control: EffectControl::Next,
                    schedule: EffectSchedule::Deferred,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 2,
                },
            },
            EffectNodeDefinition {
                key: key("test:compiled-finish"),
                operation: EffectNodeOperation::Emit {
                    commands: vec![EffectCommand {
                        key: key("test:compiled-command-b"),
                        payload: vec![4],
                    }],
                    state: vec![5],
                    receipts: vec![72],
                    control: EffectControl::Complete,
                    schedule: EffectSchedule::SameTick,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                },
            },
        ],
        8,
        limits,
    )
    .unwrap();
    let program = catalog
        .compile(
            key("test:compiled-program"),
            1,
            vec![
                key("test:compiled-timer"),
                key("test:compiled-start"),
                key("test:compiled-finish"),
            ],
        )
        .unwrap();
    let evaluator =
        StandardEffectEvaluator::new(key("test:standard-effect-evaluator"), 1, &catalog).unwrap();
    let mut job = EffectJob::new(&program, &evaluator, 7, 9, key("test:random"), 3).unwrap();

    let scope = key("test:compiled-effect-scope");
    let job_key = key("test:compiled-effect-job");
    let schema_key = key("test:compiled-effect-schema");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 16_384,
        checkpoint_bytes: 32_768,
    };
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();

    let timer = job.prepare(&program, &evaluator, 20, limits).unwrap();
    let (_, timer_commit) = timer
        .commit_with(|commands, candidate| {
            assert!(commands.is_empty());
            let mut transaction = store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(timer_commit.progress, EffectProgress::Deferred);
    assert_eq!(job.resume_tick(), 23);

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let mut restored_store =
        StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut resumed = restored_store.get::<EffectJob>(job_key).unwrap().clone();
    assert_eq!(resumed, job);
    assert!(matches!(
        resumed.prepare(&program, &evaluator, 22, limits),
        Err(EffectError::InvalidState)
    ));
    let (_, final_commit) = resumed
        .prepare(&program, &evaluator, 23, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert_eq!(commands[0].key, key("test:compiled-command-a"));
            let mut transaction = restored_store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(final_commit.progress, EffectProgress::Deferred);
    assert_eq!(resumed.resume_tick(), 24);
    let (_, final_commit) = resumed
        .prepare(&program, &evaluator, 24, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert_eq!(commands[0].key, key("test:compiled-command-b"));
            let mut transaction = restored_store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(final_commit.progress, EffectProgress::Complete);
    assert!(resumed.is_complete());
}

#[test]
fn author_effect_program_call_stack_restores_parent_after_checkpoint() {
    let limits = EffectLimits::new(128, 4, 128, 8, 4, 16, 8).unwrap();
    let root_key = key("test:effect-call-root");
    let child_key = key("test:effect-call-child");
    let catalog = EffectNodeCatalog::new(
        vec![
            EffectNodeDefinition {
                key: key("test:effect-call-node"),
                operation: EffectNodeOperation::Emit {
                    commands: vec![],
                    state: vec![11],
                    receipts: vec![],
                    control: EffectControl::CallProgram {
                        key: child_key,
                        version: 3,
                    },
                    schedule: EffectSchedule::Deferred,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                },
            },
            EffectNodeDefinition {
                key: key("test:effect-call-child-node"),
                operation: EffectNodeOperation::Emit {
                    commands: vec![EffectCommand {
                        key: key("test:effect-child-command"),
                        payload: vec![3],
                    }],
                    state: vec![22],
                    receipts: vec![31],
                    control: EffectControl::Next,
                    schedule: EffectSchedule::Deferred,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 1,
                },
            },
            EffectNodeDefinition {
                key: key("test:effect-call-child-return"),
                operation: EffectNodeOperation::Return {
                    schedule: EffectSchedule::Deferred,
                },
            },
            EffectNodeDefinition {
                key: key("test:effect-call-root-after"),
                operation: EffectNodeOperation::Emit {
                    commands: vec![EffectCommand {
                        key: key("test:effect-root-command"),
                        payload: vec![1],
                    }],
                    state: vec![12],
                    receipts: vec![32],
                    control: EffectControl::Complete,
                    schedule: EffectSchedule::SameTick,
                    spawn_count: 0,
                    cell_count: 0,
                    random_draws: 0,
                },
            },
        ],
        8,
        limits,
    )
    .unwrap();
    let root = catalog
        .compile(
            root_key,
            1,
            vec![
                key("test:effect-call-node"),
                key("test:effect-call-root-after"),
            ],
        )
        .unwrap();
    let child = catalog
        .compile(
            child_key,
            3,
            vec![
                key("test:effect-call-child-node"),
                key("test:effect-call-child-return"),
            ],
        )
        .unwrap();
    let programs = EffectProgramCatalog::new(vec![root, child], &catalog, 4).unwrap();
    let evaluator =
        StandardEffectEvaluator::new(key("test:effect-call-evaluator"), 1, &catalog).unwrap();
    let root = programs.get(root_key, 1).unwrap();
    let mut job =
        EffectJob::new(root, &evaluator, 1, 2, key("test:effect-call-random"), 5).unwrap();

    let scope = key("test:effect-call-scope");
    let job_key = key("test:effect-call-job");
    let schema_key = key("test:effect-call-schema");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 2,
        payload_bytes: 16_384,
        checkpoint_bytes: 32_768,
    };
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();

    let (_, call_commit) = job
        .prepare_with_catalog(&programs, &evaluator, 10, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert!(commands.is_empty());
            let mut transaction = store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(call_commit.progress, EffectProgress::Deferred);
    assert_eq!(job.program_key(), child_key);
    assert_eq!(job.call_depth(), 1);
    assert_eq!(job.resume_tick(), 11);

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let mut restored_store =
        StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut resumed = restored_store.get::<EffectJob>(job_key).unwrap().clone();
    assert!(matches!(
        resumed.prepare_with_catalog(&programs, &evaluator, 10, limits),
        Err(EffectError::InvalidState)
    ));
    let (_, child_commit) = resumed
        .prepare_with_catalog(&programs, &evaluator, 11, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert_eq!(commands[0].key, key("test:effect-child-command"));
            let mut transaction = restored_store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(child_commit.progress, EffectProgress::Deferred);
    assert_eq!(resumed.program_key(), child_key);
    assert_eq!(resumed.instruction(), 1);
    assert_eq!(resumed.call_depth(), 1);
    assert_eq!(resumed.resume_tick(), 12);

    let checkpoint = restored_store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let mut restored_store =
        StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut resumed = restored_store.get::<EffectJob>(job_key).unwrap().clone();
    let (_, return_commit) = resumed
        .prepare_with_catalog(&programs, &evaluator, 12, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert!(commands.is_empty());
            let mut transaction = restored_store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(return_commit.progress, EffectProgress::Deferred);
    assert_eq!(resumed.program_key(), root_key);
    assert_eq!(resumed.instruction(), 1);
    assert_eq!(resumed.call_depth(), 0);
    assert_eq!(resumed.resume_tick(), 13);

    let checkpoint = restored_store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(schema_key, 1, 8192).unwrap())
        .unwrap();
    let mut restored_store =
        StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut resumed = restored_store.get::<EffectJob>(job_key).unwrap().clone();
    let (_, final_commit) = resumed
        .prepare_with_catalog(&programs, &evaluator, 13, limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            assert_eq!(commands[0].key, key("test:effect-root-command"));
            let mut transaction = restored_store.transaction()?;
            transaction.put(job_key, schema_key, candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();
    assert_eq!(final_commit.progress, EffectProgress::Complete);
    assert!(resumed.is_complete());
    assert_eq!(resumed.receipts(), &[31, 32]);
}

#[test]
fn author_effect_world_and_state_commit_or_rollback_together() {
    let program = EffectProgram::new(
        key("test:effect-atomic-program"),
        1,
        vec![key("test:effect-atomic-node"), key("test:effect-next-node")],
    )
    .unwrap();
    let evaluator = AuthorEffect;
    let mut job = EffectJob::new(
        &program,
        &evaluator,
        4,
        8,
        key("test:effect-atomic-random"),
        7,
    )
    .unwrap();
    let original_job = job.clone();
    let limits = EffectLimits::new(128, 4, 128, 16, 16, 64, 8).unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<EffectJob>(key("test:effect-job-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<u64>(key("test:effect-counter-schema"), 1, 64).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 8192,
        checkpoint_bytes: 16384,
    };
    let mut store =
        StateStore::new(key("test:effect-atomic-scope"), schemas, store_limits).unwrap();
    let empty_checkpoint = store.checkpoint().unwrap();
    let mut world = empty_world();
    let original_world_revision = world.revision();

    let swallowed_state_error = job
        .prepare(&program, &evaluator, 12, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut store,
            key("test:active-effect"),
            key("test:effect-job-schema"),
            |_, _, _, transaction| {
                let _ = transaction.remove(key("test:missing-effect-state"));
                Ok(())
            },
            |_, _| -> charon_engine::foundation::EngineResult<()> {
                panic!("StateStore候補が失敗したためWorld callbackは呼ばれない")
            },
        );
    assert!(matches!(
        swallowed_state_error,
        Err(charon_engine::game::effects::EffectTransactionError::State(
            _
        ))
    ));
    assert_eq!(job, original_job);
    assert_eq!(store.checkpoint().unwrap(), empty_checkpoint);
    assert_eq!(world.revision(), original_world_revision);

    let failed = job
        .prepare(&program, &evaluator, 12, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut store,
            key("test:active-effect"),
            key("test:effect-job-schema"),
            |commands, receipts, candidate, transaction| {
                assert_eq!(commands.len(), 1);
                assert_eq!(receipts, &[100]);
                transaction.put(
                    key("test:effect-reservation-receipt"),
                    key("test:effect-counter-schema"),
                    &receipts[0],
                )?;
                transaction.put(
                    key("test:effect-counter"),
                    key("test:effect-counter-schema"),
                    &u64::from(candidate.instruction()),
                )?;
                Ok(())
            },
            |_, _| {
                Err::<(), _>(EngineError::new(
                    ErrorCode::InvalidState,
                    "結合rollback試験用World失敗",
                ))
            },
        );
    assert!(matches!(
        failed,
        Err(charon_engine::game::effects::EffectTransactionError::World(
            _
        ))
    ));
    assert_eq!(job, original_job);
    assert_eq!(store.checkpoint().unwrap(), empty_checkpoint);
    assert_eq!(world.revision(), original_world_revision);

    let (_, committed) = job
        .prepare(&program, &evaluator, 12, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut store,
            key("test:active-effect"),
            key("test:effect-job-schema"),
            |commands, receipts, candidate, transaction| {
                assert_eq!(commands.len(), 1);
                assert_eq!(receipts, &[100]);
                transaction.put(
                    key("test:effect-reservation-receipt"),
                    key("test:effect-counter-schema"),
                    &receipts[0],
                )?;
                transaction.put(
                    key("test:effect-counter"),
                    key("test:effect-counter-schema"),
                    &u64::from(candidate.instruction()),
                )
            },
            |commands, _| {
                assert_eq!(commands[0].key, key("test:effect-command"));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(world.revision(), original_world_revision + 1);
    assert_eq!(
        store.get::<EffectJob>(key("test:active-effect")).unwrap(),
        job
    );
    assert_eq!(
        store.get::<u64>(key("test:effect-counter")).unwrap(),
        u64::from(job.instruction())
    );
    assert_eq!(job.receipts(), &[100]);
    assert_eq!(
        store
            .get::<u64>(key("test:effect-reservation-receipt"))
            .unwrap(),
        100
    );
    assert_eq!(
        committed.progress,
        charon_engine::game::effects::EffectProgress::Deferred
    );
}

struct ManaReservationEffect;
impl EffectEvaluator for ManaReservationEffect {
    fn key(&self) -> StableKey {
        key("test:mana-reservation-effect")
    }
    fn version(&self) -> u32 {
        1
    }
    fn evaluate(
        &self,
        _: StableKey,
        _: EffectContext,
        state: &[u8],
    ) -> Result<EffectNodeResult, charon_engine::game::effects::EffectError> {
        let first = state.is_empty();
        Ok(EffectNodeResult {
            control: if first {
                EffectControl::Next
            } else {
                EffectControl::Complete
            },
            state: vec![1],
            commands: if first {
                vec![EffectCommand {
                    key: key("test:reserve-mana"),
                    payload: vec![30],
                }]
            } else {
                Vec::new()
            },
            receipts: if first { vec![777] } else { Vec::new() },
            spawn_count: 0,
            cell_count: 0,
            random_draws: u64::from(first),
            schedule: EffectSchedule::Deferred,
        })
    }
}

struct CreatorPrng(u64);
impl CreatorPrng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
}
impl RandomSource for CreatorPrng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}
impl StateCodec for CreatorPrng {
    fn encode(&self, encoder: &mut Encoder) -> charon_engine::foundation::EngineResult<()> {
        encoder.u64(self.0)
    }
    fn decode(decoder: &mut Decoder<'_>) -> charon_engine::foundation::EngineResult<Self> {
        Ok(Self(decoder.u64()?))
    }
}

struct StatefulPrngEffect;
impl EffectEvaluator for StatefulPrngEffect {
    fn key(&self) -> StableKey {
        key("test:stateful-prng-effect")
    }
    fn version(&self) -> u32 {
        1
    }
    fn evaluate(
        &self,
        _: StableKey,
        context: EffectContext,
        state: &[u8],
    ) -> Result<EffectNodeResult, EffectError> {
        if context.random_provider != key("test:authored-stateful-prng") {
            return Err(EffectError::WrongProvider);
        }
        let mut random = if state.is_empty() {
            CreatorPrng::new(context.random_stream)
        } else {
            let mut decoder = Decoder::new(state, 8).map_err(|_| EffectError::InvalidState)?;
            let random =
                CreatorPrng::decode(&mut decoder).map_err(|_| EffectError::InvalidState)?;
            decoder.finish().map_err(|_| EffectError::InvalidState)?;
            random
        };
        let value = random.next_u64();
        let mut encoder = Encoder::new(8);
        random
            .encode(&mut encoder)
            .map_err(|_| EffectError::CapacityExceeded)?;
        let was_initialized = !state.is_empty();
        Ok(EffectNodeResult {
            control: if was_initialized {
                EffectControl::Complete
            } else {
                EffectControl::Next
            },
            state: encoder.finish(),
            commands: vec![EffectCommand {
                key: key("test:prng-value"),
                payload: value.to_le_bytes().to_vec(),
            }],
            receipts: vec![context.random_cursor.saturating_add(100)],
            spawn_count: 0,
            cell_count: 0,
            random_draws: 1,
            schedule: EffectSchedule::Deferred,
        })
    }
}

#[test]
fn author_effect_seeded_replay_matches_checkpointed_continuation() {
    let provider_key = key("test:authored-stateful-prng");
    let seed = 0xA55A_1020_3344_7788;
    let program = EffectProgram::new(
        key("test:stateful-prng-program"),
        1,
        vec![key("test:draw-random"), key("test:draw-random")],
    )
    .unwrap();
    let evaluator = StatefulPrngEffect;
    let mut job = EffectJob::new(&program, &evaluator, 1, 2, provider_key, seed).unwrap();
    let effect_limits = EffectLimits::new(32, 2, 32, 4, 0, 0, 4).unwrap();
    let scope = key("test:stateful-prng-scope");
    let state_key = key("test:stateful-prng-job");
    let schema_key = key("test:stateful-prng-schema");
    let store_limits = limits();
    let make_schemas = || {
        let mut schemas = SchemaRegistry::default();
        schemas
            .register(Schema::of::<EffectJob>(schema_key, 1, 4096).unwrap())
            .unwrap();
        schemas
    };
    let mut store = StateStore::new(scope, make_schemas(), store_limits).unwrap();
    let mut expected = CreatorPrng::new(seed);
    let expected_first = expected.next_u64();
    let expected_second = expected.next_u64();

    let first = job.prepare(&program, &evaluator, 7, effect_limits).unwrap();
    assert_eq!(first.new_receipts(), &[100]);
    let (first_value, first_commit) = first
        .commit_with(|commands, candidate| {
            let value = u64::from_le_bytes(commands[0].payload.as_slice().try_into().unwrap());
            let mut transaction = store.transaction()?;
            transaction.put(state_key, schema_key, candidate)?;
            transaction.commit()?;
            Ok::<_, EngineError>(value)
        })
        .unwrap();
    assert_eq!(first_value, expected_first);
    assert_eq!(first_commit.progress, EffectProgress::Deferred);
    assert_eq!(job.receipts(), &[100]);

    let checkpoint = store.checkpoint().unwrap();
    let mut reference_store = StateStore::new(scope, make_schemas(), store_limits).unwrap();
    let mut reference_job = EffectJob::new(&program, &evaluator, 1, 2, provider_key, seed).unwrap();
    reference_job
        .prepare(&program, &evaluator, 7, effect_limits)
        .unwrap()
        .commit_with(|_, candidate| {
            let mut transaction = reference_store.transaction()?;
            transaction.put(state_key, schema_key, candidate)?;
            transaction.commit()
        })
        .unwrap();
    assert_eq!(reference_store.checkpoint().unwrap(), checkpoint);

    let mut restored_store =
        StateStore::restore(&checkpoint, scope, make_schemas(), store_limits).unwrap();
    let mut restored_job = restored_store.get::<EffectJob>(state_key).unwrap();
    let (second_value, second_commit) = restored_job
        .prepare(&program, &evaluator, 8, effect_limits)
        .unwrap()
        .commit_with(|commands, candidate| {
            let value = u64::from_le_bytes(commands[0].payload.as_slice().try_into().unwrap());
            let mut transaction = restored_store.transaction()?;
            transaction.put(state_key, schema_key, candidate)?;
            transaction.commit()?;
            Ok::<_, EngineError>(value)
        })
        .unwrap();
    assert_eq!(second_value, expected_second);
    assert_eq!(second_commit.progress, EffectProgress::Complete);
    assert_eq!(restored_job.receipts(), &[100, 101]);

    reference_job
        .prepare(&program, &evaluator, 8, effect_limits)
        .unwrap()
        .commit_with(|_, candidate| {
            let mut transaction = reference_store.transaction()?;
            transaction.put(state_key, schema_key, candidate)?;
            transaction.commit()
        })
        .unwrap();
    assert_eq!(
        reference_store.checkpoint().unwrap(),
        restored_store.checkpoint().unwrap()
    );
}

#[test]
fn effect_mana_reservation_rolls_back_and_does_not_repeat_after_deferred_restore() {
    let program = EffectProgram::new(
        key("test:mana-reservation-program"),
        1,
        vec![key("test:reserve-mana-node"), key("test:finish-mana-node")],
    )
    .unwrap();
    let evaluator = ManaReservationEffect;
    let mut job = EffectJob::new(
        &program,
        &evaluator,
        4,
        8,
        key("test:mana-reservation-random"),
        19,
    )
    .unwrap();
    let original_job = job.clone();
    let limits = EffectLimits::new(32, 2, 32, 4, 0, 0, 4).unwrap();
    let scope = key("test:mana-reservation-scope");
    let job_key = key("test:mana-reservation-job");
    let job_schema = key("test:mana-reservation-job-schema");
    let mana_state_key = key("test:mana-attributes");
    let mana_schema = key("test:mana-attributes-schema");
    let mana_key = key("test:mana");
    let craft_state_key = key("test:mana-item-craft");
    let craft_schema = key("test:mana-item-craft-schema");
    let ore = key("test:mana-item-ore");
    let ingot = key("test:mana-item-ingot");
    let item_catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore, 10, 0, 2, None, None, None, vec![]).unwrap(),
        ItemDefinition::new(ingot, 10, 0, 3, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let mut inventory = ItemContainer::new(
        key("test:mana-item-inventory"),
        ItemContainerLimits {
            slots: 4,
            payload_bytes: 32,
            weight_micros: 100,
        },
    )
    .unwrap();
    inventory
        .deposit(
            ItemLot::Stack {
                item: ore,
                count: 2,
            },
            0,
            &item_catalog,
        )
        .unwrap();
    let initial_craft_session = CraftSession::new(inventory, &item_catalog, 4).unwrap();
    let recipe = CraftRecipe::new(
        key("test:mana-item-recipe"),
        1,
        vec![CraftRequirement {
            item: ore,
            count: 1,
        }],
        vec![],
        ingot,
        1,
        2,
        Some(key("test:mana-item-forge")),
        Some(3),
    )
    .unwrap();
    let recipe_key = recipe.key();
    let recipe_version = recipe.version();
    let recipes = CraftCatalog::new(vec![recipe], &item_catalog).unwrap();
    let craft_context = CraftContext {
        station: Some(key("test:mana-item-forge")),
        world_revision: 3,
    };
    let craft_receipt = key("test:mana-item-receipt");
    let attributes = AttributeCatalog::new(vec![
        AttributeDefinition::new(mana_key, 50.0, 0.0, 100.0).unwrap(),
    ])
    .unwrap();
    let initial_attributes = attributes.initial_state(&[mana_key]).unwrap();
    let make_schemas = || {
        let mut schemas = SchemaRegistry::default();
        schemas
            .register(Schema::of::<EffectJob>(job_schema, 1, 4096).unwrap())
            .unwrap();
        schemas
            .register(Schema::of::<AttributeState>(mana_schema, 1, 1024).unwrap())
            .unwrap();
        schemas
            .register(Schema::of::<CraftSession>(craft_schema, 1, 4096).unwrap())
            .unwrap();
        schemas
    };
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 8192,
        checkpoint_bytes: 16384,
    };
    let mut state = StateStore::new(scope, make_schemas(), store_limits).unwrap();
    let mut initial = state.transaction().unwrap();
    initial
        .put(mana_state_key, mana_schema, &initial_attributes)
        .unwrap();
    initial
        .put(craft_state_key, craft_schema, &initial_craft_session)
        .unwrap();
    initial.commit().unwrap();
    let initial_checkpoint = state.checkpoint().unwrap();
    let mut world = empty_world();
    let world_revision = world.revision();

    let reserve_mana =
        |commands: &[EffectCommand],
         receipts: &[u64],
         _: &EffectJob,
         transaction: &mut charon_engine::foundation::state::Transaction<'_>| {
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].key, key("test:reserve-mana"));
            assert_eq!(commands[0].payload, [30]);
            assert_eq!(receipts, &[777]);
            let current = transaction.get::<AttributeState>(mana_state_key)?;
            let value = current.get(mana_key).unwrap();
            let cost = f32::from(commands[0].payload[0]);
            let candidate = attributes
                .apply_changes(
                    &current,
                    &[AttributeChange {
                        key: mana_key,
                        value: value - cost,
                    }],
                    1,
                )
                .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "mana予約候補"))?;
            transaction.put(mana_state_key, mana_schema, &candidate)?;
            let mut craft = transaction.get::<CraftSession>(craft_state_key)?;
            craft
                .start_registered(
                    &recipes,
                    recipe_key,
                    recipe_version,
                    craft_receipt,
                    5,
                    craft_context,
                    1,
                    &item_catalog,
                )
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "craft材料予約候補"))?;
            transaction.put(craft_state_key, craft_schema, &craft)
        };

    let failed = job
        .prepare(&program, &evaluator, 5, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut state,
            job_key,
            job_schema,
            reserve_mana,
            |_, _| {
                Err::<(), EngineError>(EngineError::new(
                    ErrorCode::InvalidState,
                    "mana予約rollback",
                ))
            },
        );
    assert!(matches!(
        failed,
        Err(charon_engine::game::effects::EffectTransactionError::World(
            _
        ))
    ));
    assert_eq!(state.checkpoint().unwrap(), initial_checkpoint);
    assert_eq!(
        state.get::<AttributeState>(mana_state_key).unwrap(),
        initial_attributes
    );
    assert_eq!(job, original_job);
    assert_eq!(world.revision(), world_revision);
    assert_eq!(
        state.get::<CraftSession>(craft_state_key).unwrap(),
        initial_craft_session
    );

    let (_, first_commit) = job
        .prepare(&program, &evaluator, 5, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut state,
            job_key,
            job_schema,
            reserve_mana,
            |_, _| Ok::<(), EngineError>(()),
        )
        .unwrap();
    assert_eq!(first_commit.progress, EffectProgress::Deferred);
    assert_eq!(job.receipts(), &[777]);
    assert_eq!(state.get::<EffectJob>(job_key).unwrap(), job);
    assert_eq!(
        state
            .get::<AttributeState>(mana_state_key)
            .unwrap()
            .get(mana_key),
        Some(20.0)
    );
    let reserved_craft = state.get::<CraftSession>(craft_state_key).unwrap();
    assert_eq!(
        reserved_craft.progress(),
        Some(CraftProgress::Running { remaining_ticks: 2 })
    );
    assert_eq!(
        reserved_craft.container().slots()[0],
        Some(ItemLot::Stack {
            item: ore,
            count: 1
        })
    );

    let checkpoint = state.checkpoint().unwrap();
    let mut restored =
        StateStore::restore(&checkpoint, scope, make_schemas(), store_limits).unwrap();
    let mut restored_job = restored.get::<EffectJob>(job_key).unwrap();
    let (_, resumed_commit) = restored_job
        .prepare(&program, &evaluator, 6, limits)
        .unwrap()
        .commit_with_world_and_state(
            &mut world,
            &mut restored,
            job_key,
            job_schema,
            |commands, receipts, _, _| {
                assert!(commands.is_empty());
                assert!(receipts.is_empty());
                Ok(())
            },
            |_, _| Ok::<(), EngineError>(()),
        )
        .unwrap();
    assert_eq!(resumed_commit.progress, EffectProgress::Complete);
    assert_eq!(restored_job.receipts(), &[777]);
    assert_eq!(
        restored
            .get::<AttributeState>(mana_state_key)
            .unwrap()
            .get(mana_key),
        Some(20.0)
    );
    assert_eq!(
        restored.get::<CraftSession>(craft_state_key).unwrap(),
        reserved_craft
    );
    assert_eq!(restored.get::<EffectJob>(job_key).unwrap(), restored_job);
}

struct AuthorSignals;
impl SignalsProvider for AuthorSignals {
    fn key(&self) -> StableKey {
        key("test:author-signals")
    }
    fn version(&self) -> u32 {
        2
    }
    fn propagate(
        &self,
        signal: &Signal,
    ) -> Result<Vec<SignalDelivery>, charon_engine::game::signals::SignalError> {
        let target = if signal.target == key("test:wire-a") {
            key("test:wire-b")
        } else {
            key("test:wire-a")
        };
        Ok(vec![SignalDelivery {
            signal: Signal {
                target,
                ..signal.clone()
            },
            timing: SignalTiming::SameTick,
        }])
    }
}

#[test]
fn author_signals_loop_queue_roundtrips_through_state_store() {
    let provider = AuthorSignals;
    let limits = SignalLimits {
        signals_per_tick: 16,
        queued_payload_bytes: 1024,
        deliveries_per_signal: 4,
        payload_bytes: 64,
    };
    let mut queue = SignalQueue::new(key("test:wiring"), &provider, 5).unwrap();
    queue
        .enqueue(
            Signal {
                network: key("test:wiring"),
                source: key("test:device"),
                sequence: 3,
                target: key("test:wire-a"),
                payload: vec![9],
            },
            SignalTiming::SameTick,
            limits,
        )
        .unwrap();
    assert_eq!(
        queue.poll(&provider, limits, 1).unwrap(),
        SignalProgress::ContinueSameTick
    );

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<SignalQueue>(key("test:signal-queue-schema"), 1, 4096).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 2,
        payload_bytes: 8192,
        checkpoint_bytes: 16384,
    };
    let mut store = StateStore::new(key("test:signals-scope"), schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(key("test:signals"), key("test:signal-queue-schema"), &queue)
        .unwrap();
    transaction.commit().unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<SignalQueue>(key("test:signal-queue-schema"), 1, 4096).unwrap())
        .unwrap();
    let restored_store = StateStore::restore(
        &checkpoint,
        key("test:signals-scope"),
        schemas,
        store_limits,
    )
    .unwrap();
    let mut restored = restored_store
        .get::<SignalQueue>(key("test:signals"))
        .unwrap();
    assert_eq!(
        restored.poll(&provider, limits, 8).unwrap(),
        SignalProgress::TickComplete
    );
    assert_eq!(restored.pending(), 0);
}

#[test]
fn author_wire_graph_local_revision_and_connections_restore_from_state_store() {
    let graph_limits = WireGraphLimits {
        ports: 8,
        directed_connections: 8,
        regions: 4,
    };
    let mut graph = WireGraph::new(key("test:wiring-network"), graph_limits).unwrap();
    let source = WirePort {
        key: key("test:generator-output"),
        device: key("test:generator"),
        region: key("test:region-west"),
        signal_type: key("test:power"),
        direction: WirePortDirection::Output,
    };
    let target = WirePort {
        key: key("test:lamp-input"),
        device: key("test:lamp"),
        region: key("test:region-east"),
        signal_type: key("test:power"),
        direction: WirePortDirection::Input,
    };
    graph.register_port(0, source, graph_limits).unwrap();
    graph.register_port(1, target, graph_limits).unwrap();
    graph
        .connect(2, source.key, target.key, graph_limits)
        .unwrap();
    assert_eq!(graph.region_revision(source.region), Some(2));
    assert_eq!(graph.region_revision(target.region), Some(2));

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireGraph>(key("test:wire-graph-schema"), 1, 4096).unwrap())
        .unwrap();
    let mut store = StateStore::new(key("test:wire-scope"), schemas, limits()).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:wire-graph-state"),
            key("test:wire-graph-schema"),
            &graph,
        )
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireGraph>(key("test:wire-graph-schema"), 1, 4096).unwrap())
        .unwrap();
    let restored =
        StateStore::restore(&checkpoint, key("test:wire-scope"), schemas, limits()).unwrap();
    let restored_graph = restored
        .get::<WireGraph>(key("test:wire-graph-state"))
        .unwrap();
    restored_graph.validate_limits(graph_limits).unwrap();
    assert_eq!(restored_graph, graph);
    assert_eq!(
        restored_graph.connections_from(source.key),
        &[charon_engine::game::signals::WireConnection {
            source: source.key,
            target: target.key,
        }]
    );

    struct GraphProvider<'a>(&'a WireGraph);
    impl SignalsProvider for GraphProvider<'_> {
        fn key(&self) -> StableKey {
            key("test:wire-graph-provider")
        }

        fn version(&self) -> u32 {
            1
        }

        fn propagate(
            &self,
            signal: &Signal,
        ) -> Result<Vec<SignalDelivery>, charon_engine::game::signals::SignalError> {
            self.0.deliveries(signal, SignalTiming::Deferred, 8, 64)
        }
    }
    let provider = GraphProvider(&restored_graph);
    let mut queue = SignalQueue::new(restored_graph.network_key(), &provider, 20).unwrap();
    queue
        .enqueue(
            Signal {
                network: restored_graph.network_key(),
                source: source.device,
                sequence: 1,
                target: source.key,
                payload: vec![1, 2, 3],
            },
            SignalTiming::SameTick,
            SignalLimits {
                signals_per_tick: 8,
                queued_payload_bytes: 256,
                deliveries_per_signal: 8,
                payload_bytes: 64,
            },
        )
        .unwrap();
    let signal_limits = SignalLimits {
        signals_per_tick: 8,
        queued_payload_bytes: 256,
        deliveries_per_signal: 8,
        payload_bytes: 64,
    };
    assert_eq!(
        queue.poll(&provider, signal_limits, 8).unwrap(),
        SignalProgress::TickComplete
    );
    assert_eq!(queue.pending(), 1);
    queue.begin_next_tick().unwrap();
    assert_eq!(queue.tick(), 21);
    assert_eq!(
        queue.poll(&provider, signal_limits, 8).unwrap(),
        SignalProgress::TickComplete
    );
    assert_eq!(queue.pending(), 0);
}

struct AuthorCounterDevice {
    input: StableKey,
    output: StableKey,
}

impl WireDeviceHandler for AuthorCounterDevice {
    fn key(&self) -> StableKey {
        key("test:counter-device-handler")
    }

    fn version(&self) -> u32 {
        1
    }

    fn validate_state(
        &self,
        state: &[u8],
    ) -> Result<(), charon_engine::game::signals::SignalError> {
        if state.len() == 1 {
            Ok(())
        } else {
            Err(charon_engine::game::signals::SignalError::InvalidState)
        }
    }

    fn handle(
        &self,
        input_port: &WirePort,
        _signal: &Signal,
        state: &[u8],
    ) -> Result<WireDeviceResult, charon_engine::game::signals::SignalError> {
        if input_port.key != self.input {
            return Err(charon_engine::game::signals::SignalError::InvalidPort);
        }
        let next = state[0]
            .checked_add(1)
            .ok_or(charon_engine::game::signals::SignalError::TickOverflow)?;
        Ok(WireDeviceResult {
            state: vec![next],
            emissions: vec![WireEmission {
                port: self.output,
                payload: vec![next],
                timing: SignalTiming::Deferred,
            }],
        })
    }
}

#[test]
fn author_wire_device_commits_graph_deliveries_with_restorable_device_and_queue_state() {
    let network = key("test:device-network");
    let device_key = key("test:counter-device");
    let input = WirePort {
        key: key("test:counter-input"),
        device: device_key,
        region: key("test:device-region"),
        signal_type: key("test:device-signal"),
        direction: WirePortDirection::Input,
    };
    let output = WirePort {
        key: key("test:counter-output"),
        device: device_key,
        region: key("test:device-region"),
        signal_type: key("test:device-signal"),
        direction: WirePortDirection::Output,
    };
    let target = WirePort {
        key: key("test:device-target"),
        device: key("test:target-device"),
        region: key("test:target-region"),
        signal_type: key("test:device-signal"),
        direction: WirePortDirection::Input,
    };
    let graph = WireGraph::from_parts(
        network,
        vec![input, output, target],
        vec![(output.key, target.key)],
        WireGraphLimits {
            ports: 3,
            directed_connections: 2,
            regions: 2,
        },
    )
    .unwrap();
    let handler = AuthorCounterDevice {
        input: input.key,
        output: output.key,
    };
    let device_limits = WireDeviceLimits {
        state_bytes: 1,
        emissions_per_input: 2,
        deliveries_per_input: 2,
        payload_bytes: 8,
        receipts: 8,
    };
    let device = WireDevice::new(network, device_key, &handler, vec![0], device_limits).unwrap();
    let signals_provider = AuthorSignals;
    let signal_limits = SignalLimits {
        signals_per_tick: 16,
        queued_payload_bytes: 1024,
        deliveries_per_signal: 4,
        payload_bytes: 64,
    };
    let queue = SignalQueue::new(network, &signals_provider, 0).unwrap();

    let scope = key("test:wire-device-scope");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireDevice>(key("test:wire-device-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<SignalQueue>(key("test:wire-queue-schema"), 1, 4096).unwrap())
        .unwrap();
    let mut store = StateStore::new(scope, schemas, limits()).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:wire-device"),
            key("test:wire-device-schema"),
            &device,
        )
        .unwrap();
    transaction
        .put(
            key("test:wire-queue"),
            key("test:wire-queue-schema"),
            &queue,
        )
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireDevice>(key("test:wire-device-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<SignalQueue>(key("test:wire-queue-schema"), 1, 4096).unwrap())
        .unwrap();
    let mut restored_store = StateStore::restore(&checkpoint, scope, schemas, limits()).unwrap();
    let mut device = restored_store
        .get::<WireDevice>(key("test:wire-device"))
        .unwrap()
        .clone();
    let mut queue = restored_store
        .get::<SignalQueue>(key("test:wire-queue"))
        .unwrap()
        .clone();
    let signal = Signal {
        network,
        source: key("test:upstream-device"),
        sequence: 7,
        target: input.key,
        payload: vec![9],
    };
    let candidate = device
        .prepare(&graph, &signal, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    candidate
        .commit_with(|deliveries, next_device| {
            let mut next_queue = queue.clone();
            for delivery in deliveries {
                next_queue.enqueue(delivery.signal.clone(), delivery.timing, signal_limits)?;
            }
            let mut transaction = restored_store.transaction()?;
            transaction.put(
                key("test:wire-device"),
                key("test:wire-device-schema"),
                next_device,
            )?;
            transaction.put(
                key("test:wire-queue"),
                key("test:wire-queue-schema"),
                &next_queue,
            )?;
            transaction.commit()?;
            queue = next_queue;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .unwrap();
    assert_eq!(queue.pending(), 1);
    assert_eq!(device.revision(), 1);
    assert_eq!(device.next_sequence(), 1);

    let checkpoint = restored_store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireDevice>(key("test:wire-device-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<SignalQueue>(key("test:wire-queue-schema"), 1, 4096).unwrap())
        .unwrap();
    let restored_store = StateStore::restore(&checkpoint, scope, schemas, limits()).unwrap();
    let mut restored_device = restored_store
        .get::<WireDevice>(key("test:wire-device"))
        .unwrap()
        .clone();
    let restored_queue = restored_store
        .get::<SignalQueue>(key("test:wire-queue"))
        .unwrap();
    assert_eq!(restored_device.revision(), 1);
    assert_eq!(restored_device.next_sequence(), 1);
    assert_eq!(restored_queue.pending(), 1);
    let resumed = Signal {
        sequence: 8,
        ..signal
    };
    let resumed_candidate = restored_device
        .prepare(&graph, &resumed, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    assert_eq!(resumed_candidate.deliveries()[0].signal.sequence, 1);
    assert_eq!(resumed_candidate.deliveries()[0].signal.payload, vec![2]);
}

#[test]
fn author_wire_device_updates_liquid_channel_with_device_and_queue_atomically() {
    let network = key("test:channel-device-network");
    let device_key = key("test:channel-pump");
    let input = WirePort {
        key: key("test:channel-pump-input"),
        device: device_key,
        region: key("test:channel-device-region"),
        signal_type: key("test:channel-signal"),
        direction: WirePortDirection::Input,
    };
    let output = WirePort {
        key: key("test:channel-pump-output"),
        device: device_key,
        region: key("test:channel-device-region"),
        signal_type: key("test:channel-signal"),
        direction: WirePortDirection::Output,
    };
    let target = WirePort {
        key: key("test:channel-device-target"),
        device: key("test:channel-target"),
        region: key("test:channel-target-region"),
        signal_type: key("test:channel-signal"),
        direction: WirePortDirection::Input,
    };
    let graph = WireGraph::from_parts(
        network,
        vec![input, output, target],
        vec![(output.key, target.key)],
        WireGraphLimits {
            ports: 3,
            directed_connections: 2,
            regions: 2,
        },
    )
    .unwrap();
    let handler = AuthorCounterDevice {
        input: input.key,
        output: output.key,
    };
    let device_limits = WireDeviceLimits {
        state_bytes: 1,
        emissions_per_input: 2,
        deliveries_per_input: 2,
        payload_bytes: 8,
        receipts: 8,
    };
    let mut device =
        WireDevice::new(network, device_key, &handler, vec![0], device_limits).unwrap();
    let signals_provider = AuthorSignals;
    let signal_limits = SignalLimits {
        signals_per_tick: 16,
        queued_payload_bytes: 1024,
        deliveries_per_signal: 4,
        payload_bytes: 64,
    };
    let mut queue = SignalQueue::new(network, &signals_provider, 0).unwrap();

    let layer_key = key("test:channel-world-layer");
    let channel_key = key("test:liquid-volume");
    let channel_schema = key("test:liquid-volume-schema");
    let mut content = ContentRegistry::new(ContentLimits::default()).unwrap();
    content
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content
        .schema(Schema::of::<u8>(channel_schema, 1, 1).unwrap())
        .unwrap();
    content
        .channel(ChannelDefinition {
            key: channel_key,
            version: 1,
            value_schema: channel_schema,
            layout: GridLayout {
                origin: CellPoint { x: 0, y: 0 },
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0u8,
            limits: GridLimits {
                chunks: 4,
                checkpoint_bytes: 4096,
            },
        })
        .unwrap();
    let content = content.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content.clone(),
        WorldLimits::default(),
    )
    .unwrap();
    let source_point = CellPoint { x: 0, y: 1 };
    let channel_point = CellPoint { x: 1, y: 1 };
    world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .edit(|editor| editor.set_channel(channel_key, source_point, 8u8))
        .unwrap();
    let initial_world_revision = world.revision();

    let scope = key("test:channel-device-scope");
    let device_key_in_store = key("test:channel-device-state");
    let device_schema = key("test:channel-device-state-schema");
    let queue_key = key("test:channel-device-queue");
    let queue_schema = key("test:channel-device-queue-schema");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WireDevice>(device_schema, 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<SignalQueue>(queue_schema, 1, 4096).unwrap())
        .unwrap();
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut initial = store.transaction().unwrap();
    initial
        .put(device_key_in_store, device_schema, &device)
        .unwrap();
    initial.put(queue_key, queue_schema, &queue).unwrap();
    initial.commit().unwrap();
    let initial_checkpoint = store.checkpoint().unwrap();
    let signal = Signal {
        network,
        source: key("test:liquid-source"),
        sequence: 3,
        target: input.key,
        payload: vec![9],
    };

    let failed_candidate = device
        .prepare(&graph, &signal, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    let failed = failed_candidate.commit_with(|deliveries, next_device| {
        let mut next_queue = queue.clone();
        for delivery in deliveries {
            next_queue
                .enqueue(delivery.signal.clone(), delivery.timing, signal_limits)
                .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "wire queue候補"))?;
        }
        let mut transaction = store.transaction()?;
        transaction.put(device_key_in_store, device_schema, next_device)?;
        transaction.put(queue_key, queue_schema, &next_queue)?;
        transaction.validate_ready()?;
        let amount = deliveries[0].signal.payload[0];
        let source = world
            .channel_value::<u8>(channel_key, source_point)
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "液量source query"))?;
        let target = world
            .channel_value::<u8>(channel_key, channel_point)
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "液量target query"))?;
        let (ChannelQuery::Loaded(source), ChannelQuery::Loaded(target)) = (source, target) else {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "液量cell未resident",
            ));
        };
        world.edit(|editor| {
            editor.set_channel(
                channel_key,
                source_point,
                source
                    .checked_sub(amount)
                    .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "液量source不足"))?,
            )?;
            editor.set_channel(
                channel_key,
                channel_point,
                target.checked_add(amount).ok_or_else(|| {
                    EngineError::new(ErrorCode::CapacityExceeded, "液量target溢出")
                })?,
            )?;
            Err::<(), EngineError>(EngineError::new(
                ErrorCode::InvalidState,
                "wire channel rollback",
            ))
        })?;
        transaction.commit()?;
        queue = next_queue;
        Ok::<_, EngineError>(())
    });
    assert!(failed.is_err());
    assert_eq!(device.revision(), 0);
    assert_eq!(queue.pending(), 0);
    assert_eq!(world.revision(), initial_world_revision);
    assert_eq!(store.checkpoint().unwrap(), initial_checkpoint);
    assert!(matches!(
        world
            .channel_value::<u8>(channel_key, channel_point)
            .unwrap(),
        ChannelQuery::Loaded(0)
    ));
    assert!(matches!(
        world
            .channel_value::<u8>(channel_key, source_point)
            .unwrap(),
        ChannelQuery::Loaded(8)
    ));

    let candidate = device
        .prepare(&graph, &signal, &handler, device_limits, signal_limits)
        .unwrap()
        .unwrap();
    candidate
        .commit_with(|deliveries, next_device| {
            let mut next_queue = queue.clone();
            for delivery in deliveries {
                next_queue
                    .enqueue(delivery.signal.clone(), delivery.timing, signal_limits)
                    .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "wire queue候補"))?;
            }
            let mut transaction = store.transaction()?;
            transaction.put(device_key_in_store, device_schema, next_device)?;
            transaction.put(queue_key, queue_schema, &next_queue)?;
            transaction.validate_ready()?;
            let amount = deliveries[0].signal.payload[0];
            let source = world
                .channel_value::<u8>(channel_key, source_point)
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "液量source query"))?;
            let target = world
                .channel_value::<u8>(channel_key, channel_point)
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "液量target query"))?;
            let (ChannelQuery::Loaded(source), ChannelQuery::Loaded(target)) = (source, target)
            else {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "液量cell未resident",
                ));
            };
            world.edit(|editor| {
                editor.set_channel(
                    channel_key,
                    source_point,
                    source.checked_sub(amount).ok_or_else(|| {
                        EngineError::new(ErrorCode::InvalidValue, "液量source不足")
                    })?,
                )?;
                editor.set_channel(
                    channel_key,
                    channel_point,
                    target.checked_add(amount).ok_or_else(|| {
                        EngineError::new(ErrorCode::CapacityExceeded, "液量target溢出")
                    })?,
                )
            })?;
            transaction.commit()?;
            queue = next_queue;
            Ok::<_, EngineError>(())
        })
        .unwrap();
    assert_eq!(device.revision(), 1);
    assert_eq!(queue.pending(), 1);
    assert!(matches!(
        world
            .channel_value::<u8>(channel_key, channel_point)
            .unwrap(),
        ChannelQuery::Loaded(1)
    ));
    assert!(matches!(
        world
            .channel_value::<u8>(channel_key, source_point)
            .unwrap(),
        ChannelQuery::Loaded(7)
    ));
    let source_volume = world
        .channel_value::<u8>(channel_key, source_point)
        .unwrap();
    let target_volume = world
        .channel_value::<u8>(channel_key, channel_point)
        .unwrap();
    let (ChannelQuery::Loaded(source_volume), ChannelQuery::Loaded(target_volume)) =
        (source_volume, target_volume)
    else {
        panic!("液量channelの確定値が読めません");
    };
    assert_eq!(u16::from(source_volume) + u16::from(target_volume), 8u16);
    assert_eq!(
        store.get::<WireDevice>(device_key_in_store).unwrap(),
        device
    );
    assert_eq!(store.get::<SignalQueue>(queue_key).unwrap(), queue);
    let world_checkpoint = world.checkpoint(1_000_000).unwrap();
    let restored_world = World::restore(
        &world_checkpoint,
        content,
        WorldLimits::default(),
        1_000_000,
    )
    .unwrap();
    assert!(matches!(
        restored_world
            .channel_value::<u8>(channel_key, channel_point)
            .unwrap(),
        ChannelQuery::Loaded(1)
    ));
    assert!(matches!(
        restored_world
            .channel_value::<u8>(channel_key, source_point)
            .unwrap(),
        ChannelQuery::Loaded(7)
    ));
}

#[test]
fn author_mining_reserves_inventory_or_world_drop_before_breaking_material() {
    let material_key = key("test:minable-ore-rock");
    let block_material_key = key("test:minable-ore-block");
    let ore_item = key("test:mined-ore");
    let filler_item = key("test:inventory-filler");
    let inventory_key = key("test:mining-inventory");
    let drop_key = key("test:world-drop");
    let scope = key("test:mining-world-scope");
    let mut content_registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    content_registry
        .layer(LayerDef {
            key: key("test:mining-layer"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content_registry
        .register(
            MaterialBuilder::new("test:minable-ore-rock", MaterialBase::Solid)
                .unwrap()
                .break_mode(BreakMode::Cell),
        )
        .unwrap();
    content_registry
        .register(
            MaterialBuilder::new("test:minable-ore-block", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1)
                .break_mode(BreakMode::Block),
        )
        .unwrap();
    let content = content_registry.freeze().unwrap();
    let layer = content.layer_id(key("test:mining-layer")).unwrap();
    let material_id = content.material_id(MaterialHandle(material_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        1,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let first = CellPoint { x: 1, y: 1 };
    let second = CellPoint { x: 2, y: 1 };
    let block_anchor = CellPoint { x: 4, y: 1 };
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                first,
                CellCore {
                    material: material_id,
                    ..CellCore::AIR
                },
            )?;
            edit.set_cell(
                layer,
                second,
                CellCore {
                    material: material_id,
                    ..CellCore::AIR
                },
            )?;
            edit.place_block(
                layer,
                block_anchor,
                MaterialHandle(block_material_key),
                Orientation::Original,
            )?;
            Ok(())
        })
        .unwrap();

    let item_catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore_item, 2, 0, 1, None, None, None, vec![]).unwrap(),
        ItemDefinition::new(filler_item, 1, 0, 1, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let drops = MiningDropCatalog::new(
        vec![
            MiningDropRule {
                material: material_key,
                item: ore_item,
                per_cell: 2,
                per_block: 1,
            },
            MiningDropRule {
                material: block_material_key,
                item: ore_item,
                per_cell: 2,
                per_block: 1,
            },
        ],
        &item_catalog,
        4,
    )
    .unwrap();
    let container_limits = ItemContainerLimits {
        slots: 1,
        payload_bytes: 16,
        weight_micros: 16,
    };
    let mut inventory = ItemContainer::new(inventory_key, container_limits).unwrap();
    inventory
        .deposit(
            ItemLot::Stack {
                item: filler_item,
                count: 1,
            },
            0,
            &item_catalog,
        )
        .unwrap();
    let mut drop_container = ItemContainer::new(drop_key, container_limits).unwrap();
    let mut registry = ItemInstanceRegistry::new(scope);
    let registry_limits = ItemInstanceRegistryLimits {
        owners: 4,
        instances: 16,
    };
    registry
        .refresh(
            0,
            &[&inventory, &drop_container],
            &[],
            &item_catalog,
            registry_limits,
        )
        .unwrap();
    let no_other_containers = [];
    let no_equipment = [];
    let expected_world_revision = world.revision();
    let expected_inventory_revision = inventory.revision();
    let expected_drop_revision = drop_container.revision();
    let expected_registry_revision = registry.revision();

    let result = mine_cell_with_drop(
        &mut world,
        &mut inventory,
        &mut drop_container,
        &mut registry,
        &item_catalog,
        &drops,
        MiningTransaction {
            layer,
            point: first,
            expected_world_revision,
            expected_inventory_revision,
            expected_drop_revision,
            registry_scope: scope,
            expected_registry_revision,
            other_containers: &no_other_containers,
            equipment: &no_equipment,
            registry_limits,
        },
    )
    .unwrap();
    assert_eq!(result.material, material_key);
    assert_eq!(result.item, ore_item);
    assert_eq!(result.count, 2);
    assert_eq!(result.destination, Some(drop_key));
    assert_eq!(result.broken_cells, 1);
    assert!(
        matches!(world.cell(layer, first).unwrap(), charon_engine::world::CellQuery::Loaded(cell) if cell.core == CellCore::AIR)
    );
    assert_eq!(drop_container.revision(), 1);
    assert_eq!(registry.revision(), 1);

    let world_revision = world.revision();
    let inventory_revision = inventory.revision();
    let drop_revision = drop_container.revision();
    let registry_revision = registry.revision();
    assert!(matches!(
        mine_cell_with_drop(
            &mut world,
            &mut inventory,
            &mut drop_container,
            &mut registry,
            &item_catalog,
            &drops,
            MiningTransaction {
                layer,
                point: second,
                expected_world_revision: world_revision,
                expected_inventory_revision: inventory_revision,
                expected_drop_revision: drop_revision,
                registry_scope: scope,
                expected_registry_revision: registry_revision,
                other_containers: &no_other_containers,
                equipment: &no_equipment,
                registry_limits,
            },
        ),
        Err(MiningTransactionError::Item(ItemError::CapacityExceeded))
    ));
    assert_eq!(world.revision(), world_revision);
    assert_eq!(inventory.revision(), inventory_revision);
    assert_eq!(drop_container.revision(), drop_revision);
    assert_eq!(registry.revision(), registry_revision);
    assert!(
        matches!(world.cell(layer, second).unwrap(), charon_engine::world::CellQuery::Loaded(cell) if cell.core.material == material_id)
    );

    let block_inventory_key = key("test:block-mining-inventory");
    let block_drop_key = key("test:block-world-drop");
    let mut block_inventory = ItemContainer::new(block_inventory_key, container_limits).unwrap();
    let mut block_drop = ItemContainer::new(block_drop_key, container_limits).unwrap();
    registry
        .refresh(
            registry.revision(),
            &[&inventory, &drop_container, &block_inventory, &block_drop],
            &[],
            &item_catalog,
            registry_limits,
        )
        .unwrap();
    let other_containers = [&inventory, &drop_container];
    let expected_world_revision = world.revision();
    let expected_inventory_revision = block_inventory.revision();
    let expected_drop_revision = block_drop.revision();
    let expected_registry_revision = registry.revision();
    let result = mine_cell_with_drop(
        &mut world,
        &mut block_inventory,
        &mut block_drop,
        &mut registry,
        &item_catalog,
        &drops,
        MiningTransaction {
            layer,
            point: block_anchor,
            expected_world_revision,
            expected_inventory_revision,
            expected_drop_revision,
            registry_scope: scope,
            expected_registry_revision,
            other_containers: &other_containers,
            equipment: &no_equipment,
            registry_limits,
        },
    )
    .unwrap();
    assert_eq!(result.material, block_material_key);
    assert_eq!(result.count, 1);
    assert_eq!(result.destination, Some(block_inventory_key));
    assert_eq!(result.broken_cells, 2);
    for x in block_anchor.x..block_anchor.x + 2 {
        assert!(matches!(
            world.cell(layer, CellPoint { x, y: block_anchor.y }).unwrap(),
            charon_engine::world::CellQuery::Loaded(cell) if cell.core == CellCore::AIR
        ));
    }
    assert_eq!(block_inventory.revision(), 1);
    assert_eq!(registry.revision(), expected_registry_revision);
}

#[test]
fn mined_material_survives_combat_death_drop_and_pickup() {
    let scope = key("test:mining-death-world");
    let layer_key = key("test:mining-death-layer");
    let material_key = key("test:mining-death-ore-rock");
    let ore_item = key("test:mining-death-ore");
    let health_key = key("test:death-scenario-health");
    let mut content = ContentRegistry::new(ContentLimits::default()).unwrap();
    content
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content
        .register(
            MaterialBuilder::new("test:mining-death-ore-rock", MaterialBase::Solid)
                .unwrap()
                .break_mode(BreakMode::Cell),
        )
        .unwrap();
    let content = content.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let material = content.material_id(MaterialHandle(material_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        1,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let mined_cell = CellPoint { x: 2, y: 3 };
    world
        .edit(|editor| {
            editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            editor.set_cell(
                layer,
                mined_cell,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();

    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore_item, 16, 0, 1, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let mining_drops = MiningDropCatalog::new(
        vec![MiningDropRule {
            material: material_key,
            item: ore_item,
            per_cell: 2,
            per_block: 1,
        }],
        &catalog,
        1,
    )
    .unwrap();
    let container_limits = ItemContainerLimits {
        slots: 1,
        payload_bytes: 64,
        weight_micros: 64,
    };
    let inventory_key = key("test:mining-death-inventory");
    let mut inventory = ItemContainer::new(inventory_key, container_limits).unwrap();
    let mut mining_overflow =
        ItemContainer::new(key("test:mining-death-overflow"), container_limits).unwrap();
    let mut registry = ItemInstanceRegistry::new(scope);
    let registry_limits = ItemInstanceRegistryLimits {
        owners: 4,
        instances: 4,
    };
    registry
        .refresh(
            0,
            &[&inventory, &mining_overflow],
            &[],
            &catalog,
            registry_limits,
        )
        .unwrap();
    let expected_world_revision = world.revision();
    let expected_inventory_revision = inventory.revision();
    let expected_overflow_revision = mining_overflow.revision();
    let expected_registry_revision = registry.revision();
    mine_cell_with_drop(
        &mut world,
        &mut inventory,
        &mut mining_overflow,
        &mut registry,
        &catalog,
        &mining_drops,
        MiningTransaction {
            layer,
            point: mined_cell,
            expected_world_revision,
            expected_inventory_revision,
            expected_drop_revision: expected_overflow_revision,
            registry_scope: scope,
            expected_registry_revision,
            other_containers: &[],
            equipment: &[],
            registry_limits,
        },
    )
    .unwrap();
    assert!(matches!(
        inventory.slots()[0],
        Some(ItemLot::Stack { item, count: 2 }) if item == ore_item
    ));
    assert!(matches!(
        world.cell(layer, mined_cell).unwrap(),
        charon_engine::world::CellQuery::Loaded(cell) if cell.core == CellCore::AIR
    ));

    let attributes = AttributeCatalog::new(vec![
        AttributeDefinition::new(health_key, 10.0, 0.0, 10.0).unwrap(),
    ])
    .unwrap();
    let hit_limits = HitLimits {
        immunity_records: 4,
        token_bytes: 128,
        attribute_changes: 2,
        effects: 2,
    };
    let mut combat =
        CombatState::new(attributes.initial_state(&[health_key]).unwrap(), hit_limits).unwrap();
    let hit = CombatHit {
        attack: key("test:mining-death-attack"),
        source: key("test:mining-death-enemy"),
        target: key("test:mining-death-player"),
        hit_group: key("test:mining-death-hit-group"),
        hit_window: 1,
        damage_kind: key("test:mining-death-damage"),
        raw_damage: 10.0,
        impulse: [0.0, 0.0],
        tick: 4,
        substep: 0,
    };
    let drop_position = CellPoint { x: 7, y: 9 };
    let mut death_drops = WorldDropStore::new(scope, WorldDropLimits { containers: 1 });
    let resolver = DeathScenarioResolver;
    let immunity = AttackTargetWindowImmunity;
    combat
        .prepare_hit(hit, &immunity, &resolver, &attributes, hit_limits)
        .unwrap()
        .unwrap()
        .commit_with(&mut combat, |candidate, _| {
            if candidate.attributes().get(health_key) != Some(0.0) {
                return Err(ItemError::InvalidLot);
            }
            let expected_store_revision = death_drops.revision();
            let expected_inventory_revision = inventory.revision();
            let expected_registry_revision = registry.revision();
            death_drops
                .drop_inventory_at(
                    &mut inventory,
                    drop_position,
                    key("test:mining-death-world-drop"),
                    container_limits,
                    expected_store_revision,
                    expected_inventory_revision,
                    expected_registry_revision,
                    &catalog,
                    &mut registry,
                    scope,
                    &[],
                    &[],
                    registry_limits,
                )
                .map_err(|_| ItemError::InvalidLot)?;
            Ok::<(), ItemError>(())
        })
        .unwrap();
    assert_eq!(combat.attributes().get(health_key), Some(0.0));
    assert!(inventory.slots()[0].is_none());
    assert_eq!(
        death_drops
            .get(key("test:mining-death-world-drop"))
            .unwrap()
            .position(),
        drop_position
    );

    let drop_key = key("test:mining-death-world-drop");
    let drop_revision = death_drops.get(drop_key).unwrap().container().revision();
    let expected_store_revision = death_drops.revision();
    let expected_inventory_revision = inventory.revision();
    let expected_registry_revision = registry.revision();
    death_drops
        .pickup(
            drop_key,
            &mut inventory,
            0,
            2,
            expected_store_revision,
            drop_revision,
            expected_inventory_revision,
            &catalog,
            &mut registry,
            scope,
            expected_registry_revision,
            &[],
            &[],
            registry_limits,
        )
        .unwrap();
    assert!(matches!(
        inventory.slots()[0],
        Some(ItemLot::Stack { item, count: 2 }) if item == ore_item
    ));
    assert!(death_drops.get(drop_key).is_none());
}

#[test]
fn world_drop_position_restores_and_pickup_updates_inventory_atomically() {
    let scope = key("test:drop-store-world");
    let item_key = key("test:drop-store-item");
    let instance_item = key("test:drop-store-instance-item");
    let instance_id = key("test:drop-store-instance-id");
    let drop_key = key("test:positioned-drop");
    let instance_drop_key = key("test:positioned-instance-drop");
    let inventory_key = key("test:drop-pickup-inventory");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(item_key, 10, 0, 1, None, None, None, vec![]).unwrap(),
        ItemDefinition::new(instance_item, 1, 4, 1, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let mut inventory = ItemContainer::new(inventory_key, limits).unwrap();
    let mut positioned = ItemContainer::new(drop_key, limits).unwrap();
    positioned
        .deposit(
            ItemLot::Stack {
                item: item_key,
                count: 3,
            },
            0,
            &catalog,
        )
        .unwrap();
    let position = CellPoint { x: -73, y: 128 };
    let mut drops = WorldDropStore::new(scope, WorldDropLimits { containers: 8 });
    drops.insert(position, positioned, 0).unwrap();
    let mut positioned_instance = ItemContainer::new(instance_drop_key, limits).unwrap();
    positioned_instance
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                item: instance_item,
                id: instance_id,
                state: vec![4, 5],
            })),
            0,
            &catalog,
        )
        .unwrap();
    let expected_store_revision = drops.revision();
    drops
        .insert(
            CellPoint { x: 8, y: -4 },
            positioned_instance,
            expected_store_revision,
        )
        .unwrap();

    let mut encoder = Encoder::new(4096);
    drops.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut decoder = Decoder::new(&bytes, 4096).unwrap();
    let mut restored = WorldDropStore::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    assert_eq!(restored.scope(), scope);
    assert_eq!(restored.get(drop_key).unwrap().position(), position);
    assert_eq!(restored.get(drop_key).unwrap().container().revision(), 1);
    assert_eq!(
        restored.get(instance_drop_key).unwrap().position(),
        CellPoint { x: 8, y: -4 }
    );

    let registry_limits = ItemInstanceRegistryLimits {
        owners: 3,
        instances: 8,
    };
    let mut registry = ItemInstanceRegistry::new(scope);
    registry
        .refresh(
            0,
            &[
                &inventory,
                restored.get(drop_key).unwrap().container(),
                restored.get(instance_drop_key).unwrap().container(),
            ],
            &[],
            &catalog,
            registry_limits,
        )
        .unwrap();
    let expected_registry_revision = registry.revision();
    let expected_store_revision = restored.revision();
    let expected_drop_revision = restored.get(drop_key).unwrap().container().revision();
    let expected_inventory_revision = inventory.revision();
    restored
        .pickup(
            drop_key,
            &mut inventory,
            0,
            2,
            expected_store_revision,
            expected_drop_revision,
            expected_inventory_revision,
            &catalog,
            &mut registry,
            scope,
            expected_registry_revision,
            &[],
            &[],
            registry_limits,
        )
        .unwrap();
    assert!(matches!(
        inventory.slots()[0],
        Some(ItemLot::Stack { item, count: 2 }) if item == item_key
    ));
    assert_eq!(restored.get(drop_key).unwrap().position(), position);
    assert!(matches!(
        restored.get(drop_key).unwrap().container().slots()[0],
        Some(ItemLot::Stack { item, count: 1 }) if item == item_key
    ));

    let expected_store_revision = restored.revision();
    let expected_drop_revision = restored.get(drop_key).unwrap().container().revision();
    let expected_inventory_revision = inventory.revision();
    let expected_registry_revision = registry.revision();
    restored
        .pickup(
            drop_key,
            &mut inventory,
            0,
            1,
            expected_store_revision,
            expected_drop_revision,
            expected_inventory_revision,
            &catalog,
            &mut registry,
            scope,
            expected_registry_revision,
            &[],
            &[],
            registry_limits,
        )
        .unwrap();
    assert!(restored.get(drop_key).is_none());
    assert_eq!(restored.iter().count(), 1);
    assert!(matches!(
        inventory.slots()[0],
        Some(ItemLot::Stack { item, count: 3 }) if item == item_key
    ));

    let expected_store_revision = restored.revision();
    let expected_drop_revision = restored
        .get(instance_drop_key)
        .unwrap()
        .container()
        .revision();
    let expected_inventory_revision = inventory.revision();
    let expected_registry_revision = registry.revision();
    restored
        .pickup(
            instance_drop_key,
            &mut inventory,
            0,
            1,
            expected_store_revision,
            expected_drop_revision,
            expected_inventory_revision,
            &catalog,
            &mut registry,
            scope,
            expected_registry_revision,
            &[],
            &[],
            registry_limits,
        )
        .unwrap();
    assert!(restored.get(instance_drop_key).is_none());
    assert_eq!(restored.iter().count(), 0);
    assert_eq!(registry.revision(), expected_registry_revision + 1);
    assert_eq!(
        registry.owners(),
        &[charon_engine::game::items::ItemInstanceOwner {
            instance: instance_id,
            owner: inventory_key,
            slot: charon_engine::game::items::ItemInstanceOwnerSlot::Container(1),
        }]
    );
    assert!(matches!(
        &inventory.slots()[1],
        Some(ItemLot::Instance(instance))
            if instance.item == instance_item && instance.id == instance_id && instance.state.as_slice() == [4, 5]
    ));
}

#[test]
fn death_drop_moves_inventory_stack_and_instance_atomically() {
    let scope = key("test:death-drop-world");
    let inventory_key = key("test:death-drop-inventory");
    let drop_key = key("test:death-drop-container");
    let stack_item = key("test:death-drop-stack-item");
    let instance_item = key("test:death-drop-instance-item");
    let instance_id = key("test:death-drop-instance-id");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(stack_item, 16, 0, 1, None, None, None, vec![]).unwrap(),
        ItemDefinition::new(instance_item, 1, 16, 1, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let inventory_limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let mut inventory = ItemContainer::new(inventory_key, inventory_limits).unwrap();
    inventory
        .deposit(
            ItemLot::Stack {
                item: stack_item,
                count: 5,
            },
            inventory.revision(),
            &catalog,
        )
        .unwrap();
    inventory
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                item: instance_item,
                id: instance_id,
                state: vec![1, 7, 9],
            })),
            inventory.revision(),
            &catalog,
        )
        .unwrap();
    let mut drops = WorldDropStore::new(scope, WorldDropLimits { containers: 1 });
    let mut registry = ItemInstanceRegistry::new(scope);
    let registry_limits = ItemInstanceRegistryLimits {
        owners: 4,
        instances: 8,
    };
    registry
        .refresh(0, &[&inventory], &[], &catalog, registry_limits)
        .unwrap();
    let position = CellPoint { x: -3, y: 17 };
    let before_inventory = inventory.clone();
    let before_drops = drops.clone();
    let before_registry = registry.clone();
    let narrow_drop_limits = ItemContainerLimits {
        slots: 1,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let expected_store_revision = drops.revision();
    let expected_inventory_revision = inventory.revision();
    let expected_registry_revision = registry.revision();
    assert_eq!(
        drops.drop_inventory_at(
            &mut inventory,
            position,
            drop_key,
            narrow_drop_limits,
            expected_store_revision,
            expected_inventory_revision,
            expected_registry_revision,
            &catalog,
            &mut registry,
            scope,
            &[],
            &[],
            registry_limits,
        ),
        Err(ItemError::CapacityExceeded)
    );
    assert_eq!(inventory, before_inventory);
    assert_eq!(drops, before_drops);
    assert_eq!(registry, before_registry);

    let drop_limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let expected_store_revision = drops.revision();
    let expected_inventory_revision = inventory.revision();
    let expected_registry_revision = registry.revision();
    assert!(
        drops
            .drop_inventory_at(
                &mut inventory,
                position,
                drop_key,
                drop_limits,
                expected_store_revision,
                expected_inventory_revision,
                expected_registry_revision,
                &catalog,
                &mut registry,
                scope,
                &[],
                &[],
                registry_limits,
            )
            .unwrap()
    );
    assert!(inventory.slots().iter().all(Option::is_none));
    let drop = drops.get(drop_key).unwrap();
    assert_eq!(drop.position(), position);
    assert!(matches!(
        drop.container().slots()[0],
        Some(ItemLot::Stack { item, count: 5 }) if item == stack_item
    ));
    assert!(matches!(
        &drop.container().slots()[1],
        Some(ItemLot::Instance(instance)) if instance.id == instance_id && instance.state == [1, 7, 9]
    ));
    assert_eq!(registry.owners().len(), 1);
    assert_eq!(registry.owners()[0].instance, instance_id);
    assert_eq!(registry.owners()[0].owner, drop_key);
    assert_eq!(registry.revision(), before_registry.revision() + 1);
}

#[test]
fn author_death_event_commits_drop_controller_progression_and_environment_together() {
    let scope = key("test:death-scenario-scope");
    let layer_key = key("test:death-scenario-layer");
    let channel_key = key("test:death-scenario-environment");
    let channel_schema = key("test:death-scenario-channel-schema");
    let mut content = ContentRegistry::new(ContentLimits::default()).unwrap();
    content
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    content
        .schema(Schema::of::<u8>(channel_schema, 1, 1).unwrap())
        .unwrap();
    content
        .channel(ChannelDefinition {
            key: channel_key,
            version: 1,
            value_schema: channel_schema,
            layout: GridLayout {
                origin: CellPoint { x: 0, y: 0 },
                x_scale: GridScale::new(1, 1).unwrap(),
                y_scale: GridScale::new(1, 1).unwrap(),
            },
            default: 0u8,
            limits: GridLimits {
                chunks: 4,
                checkpoint_bytes: 4096,
            },
        })
        .unwrap();
    let content = content.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content.clone(),
        WorldLimits::default(),
    )
    .unwrap();
    let environment_point = CellPoint { x: 1, y: 1 };
    world
        .edit(|editor| {
            editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            editor.set_channel(channel_key, environment_point, 0u8)
        })
        .unwrap();

    let stack_item = key("test:death-scenario-stack-item");
    let instance_item = key("test:death-scenario-instance-item");
    let instance_id = key("test:death-scenario-instance-id");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(stack_item, 16, 0, 1, None, None, None, vec![]).unwrap(),
        ItemDefinition::new(instance_item, 1, 16, 1, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let inventory_key = key("test:death-scenario-inventory");
    let drop_key = key("test:death-scenario-drop");
    let inventory_limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let mut inventory = ItemContainer::new(inventory_key, inventory_limits).unwrap();
    inventory
        .deposit(
            ItemLot::Stack {
                item: stack_item,
                count: 3,
            },
            inventory.revision(),
            &catalog,
        )
        .unwrap();
    inventory
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                item: instance_item,
                id: instance_id,
                state: vec![2, 5, 8],
            })),
            inventory.revision(),
            &catalog,
        )
        .unwrap();
    let drops = WorldDropStore::new(scope, WorldDropLimits { containers: 1 });
    let mut instance_registry = ItemInstanceRegistry::new(scope);
    let instance_limits = ItemInstanceRegistryLimits {
        owners: 4,
        instances: 8,
    };
    instance_registry
        .refresh(0, &[&inventory], &[], &catalog, instance_limits)
        .unwrap();
    let drop_position = CellPoint { x: -3, y: 17 };
    let controller = DeathScenarioController {
        action: key("test:controller-idle"),
        dead: false,
    };

    let attributes = AttributeCatalog::new(vec![
        AttributeDefinition::new(key("test:death-scenario-health"), 10.0, 0.0, 10.0).unwrap(),
    ])
    .unwrap();
    let hit_limits = HitLimits {
        immunity_records: 8,
        token_bytes: 256,
        attribute_changes: 2,
        effects: 2,
    };
    let mut combat = CombatState::new(
        attributes
            .initial_state(&[key("test:death-scenario-health")])
            .unwrap(),
        hit_limits,
    )
    .unwrap();
    let hit = CombatHit {
        attack: key("test:death-scenario-attack"),
        source: key("test:death-scenario-player"),
        target: key("test:death-scenario-actor"),
        hit_group: key("test:death-scenario-group"),
        hit_window: 1,
        damage_kind: key("test:death-scenario-damage"),
        raw_damage: 10.0,
        impulse: [0.0, 0.0],
        tick: 12,
        substep: 0,
    };
    let resolver = DeathScenarioResolver;
    let immunity = AttackTargetWindowImmunity;
    let mut events = WorldEventQueue::new_transactional(scope, &DeathScenarioEvents, 12).unwrap();
    let combat_key = key("test:death-scenario-combat");
    let combat_schema = key("test:death-scenario-combat-schema");
    let queue_key = key("test:death-scenario-events");
    let queue_schema = key("test:death-scenario-event-schema");
    let progression_key = key("test:death-scenario-progression");
    let progression_schema = key("test:death-scenario-progression-schema");
    let inventory_schema = key("test:death-scenario-inventory-schema");
    let drops_schema = key("test:death-scenario-drops-schema");
    let registry_schema = key("test:death-scenario-registry-schema");
    let controller_schema = key("test:death-scenario-controller-schema");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CombatState>(combat_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<WorldEventQueue>(queue_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ProgressionState>(progression_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ItemContainer>(inventory_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<WorldDropStore>(drops_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ItemInstanceRegistry>(registry_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<DeathScenarioController>(controller_schema, 1, 1024).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 16,
        payload_bytes: 262_144,
        checkpoint_bytes: 524_288,
    };
    let mut state = StateStore::new(scope, schemas, store_limits).unwrap();
    let progression_flag = key("test:death-scenario-progress-dead");
    let progression_catalog = ProgressionCatalog::new(vec![ProgressionDefinition {
        key: progression_flag,
        kind: ProgressionKind::Flag,
        initial: 0,
        minimum: 0,
        maximum: 1,
    }])
    .unwrap();
    let mut progression = progression_catalog
        .create_state(scope, &[progression_flag], 8)
        .unwrap();
    let mut initial = state.transaction().unwrap();
    initial.put(queue_key, queue_schema, &events).unwrap();
    initial
        .put(progression_key, progression_schema, &progression)
        .unwrap();
    initial
        .put(inventory_key, inventory_schema, &inventory)
        .unwrap();
    initial
        .put(key("test:death-scenario-drop-store"), drops_schema, &drops)
        .unwrap();
    initial
        .put(
            key("test:death-scenario-instance-registry"),
            registry_schema,
            &instance_registry,
        )
        .unwrap();
    initial
        .put(
            key("test:death-scenario-controller"),
            controller_schema,
            &controller,
        )
        .unwrap();
    initial.commit().unwrap();

    combat
        .prepare_hit(hit, &immunity, &resolver, &attributes, hit_limits)
        .unwrap()
        .unwrap()
        .commit_with_world_and_state(
            &mut combat,
            &mut world,
            &mut state,
            combat_key,
            combat_schema,
            |candidate, report, transaction| {
                assert_eq!(
                    candidate
                        .attributes()
                        .get(key("test:death-scenario-health")),
                    Some(0.0)
                );
                let mut queue = transaction.get::<WorldEventQueue>(queue_key)?;
                queue
                    .publish(
                        WorldEvent {
                            scope,
                            key: report.resolution.effects[0],
                            source: report.hit.target,
                            sequence: report.hit.tick,
                            payload: Vec::new(),
                        },
                        SignalTiming::SameTick,
                        SignalLimits {
                            signals_per_tick: 8,
                            queued_payload_bytes: 256,
                            deliveries_per_signal: 2,
                            payload_bytes: 32,
                        },
                    )
                    .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "death event発行"))?;
                transaction.put(queue_key, queue_schema, &queue)
            },
            |_, _| Ok(()),
        )
        .unwrap();
    events = state.get(queue_key).unwrap();
    let after_combat_checkpoint = state.checkpoint().unwrap();
    let after_combat_world_checkpoint = world.checkpoint(65_536).unwrap();
    let provider = DeathScenarioEvents;
    let signal_limits = SignalLimits {
        signals_per_tick: 8,
        queued_payload_bytes: 256,
        deliveries_per_signal: 2,
        payload_bytes: 32,
    };
    let drop_limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 32,
    };
    let store_drop_key = key("test:death-scenario-drop-store");
    let store_registry_key = key("test:death-scenario-instance-registry");
    let controller_key = key("test:death-scenario-controller");
    let before_world_revision = world.revision();
    let failed = events.poll_transactionally_with_state(
        &mut progression,
        &mut world,
        &mut state,
        queue_key,
        queue_schema,
        progression_key,
        progression_schema,
        &provider,
        &progression_catalog,
        signal_limits,
        4,
        |actions, transaction| {
            assert_eq!(actions.len(), 1);
            let mut inventory = transaction.get::<ItemContainer>(inventory_key)?;
            let mut stored_drops = transaction.get::<WorldDropStore>(store_drop_key)?;
            let mut stored_registry =
                transaction.get::<ItemInstanceRegistry>(store_registry_key)?;
            let expected_store_revision = stored_drops.revision();
            let expected_inventory_revision = inventory.revision();
            let expected_registry_revision = stored_registry.revision();
            stored_drops
                .drop_inventory_at(
                    &mut inventory,
                    drop_position,
                    drop_key,
                    drop_limits,
                    expected_store_revision,
                    expected_inventory_revision,
                    expected_registry_revision,
                    &catalog,
                    &mut stored_registry,
                    scope,
                    &[],
                    &[],
                    instance_limits,
                )
                .map_err(|_| EngineError::new(ErrorCode::InvalidState, "death drop移動"))?;
            let mut stored_controller =
                transaction.get::<DeathScenarioController>(controller_key)?;
            stored_controller.action = key("test:controller-idle");
            stored_controller.dead = true;
            transaction.put(inventory_key, inventory_schema, &inventory)?;
            transaction.put(store_drop_key, drops_schema, &stored_drops)?;
            transaction.put(store_registry_key, registry_schema, &stored_registry)?;
            transaction.put(controller_key, controller_schema, &stored_controller)
        },
        |actions, editor| {
            assert_eq!(actions.len(), 1);
            editor.set_channel(channel_key, environment_point, 1u8)?;
            Err::<(), _>(EngineError::new(
                ErrorCode::InvalidState,
                "death environment rollback",
            ))
        },
    );
    assert!(matches!(
        failed,
        Err(charon_engine::game::events::WorldEventTransactionError::World(_))
    ));
    assert_eq!(world.revision(), before_world_revision);
    assert_eq!(
        world.checkpoint(65_536).unwrap(),
        after_combat_world_checkpoint
    );
    assert_eq!(state.checkpoint().unwrap(), after_combat_checkpoint);
    assert_eq!(events.pending(), 1);
    assert_eq!(progression.get(progression_flag), Some(0));
    assert_eq!(
        combat.attributes().get(key("test:death-scenario-health")),
        Some(0.0)
    );

    let (progress, result) = events
        .poll_transactionally_with_state(
            &mut progression,
            &mut world,
            &mut state,
            queue_key,
            queue_schema,
            progression_key,
            progression_schema,
            &provider,
            &progression_catalog,
            signal_limits,
            4,
            |actions, transaction| {
                assert_eq!(actions.len(), 1);
                let mut inventory = transaction.get::<ItemContainer>(inventory_key)?;
                let mut stored_drops = transaction.get::<WorldDropStore>(store_drop_key)?;
                let mut stored_registry =
                    transaction.get::<ItemInstanceRegistry>(store_registry_key)?;
                let expected_store_revision = stored_drops.revision();
                let expected_inventory_revision = inventory.revision();
                let expected_registry_revision = stored_registry.revision();
                stored_drops
                    .drop_inventory_at(
                        &mut inventory,
                        drop_position,
                        drop_key,
                        drop_limits,
                        expected_store_revision,
                        expected_inventory_revision,
                        expected_registry_revision,
                        &catalog,
                        &mut stored_registry,
                        scope,
                        &[],
                        &[],
                        instance_limits,
                    )
                    .map_err(|_| EngineError::new(ErrorCode::InvalidState, "death drop移動"))?;
                let mut stored_controller =
                    transaction.get::<DeathScenarioController>(controller_key)?;
                stored_controller.action = key("test:controller-idle");
                stored_controller.dead = true;
                transaction.put(inventory_key, inventory_schema, &inventory)?;
                transaction.put(store_drop_key, drops_schema, &stored_drops)?;
                transaction.put(store_registry_key, registry_schema, &stored_registry)?;
                transaction.put(controller_key, controller_schema, &stored_controller)
            },
            |actions, editor| {
                assert_eq!(actions.len(), 1);
                editor.set_channel(channel_key, environment_point, 1u8)
            },
        )
        .unwrap();
    assert_eq!(progress, SignalProgress::TickComplete);
    assert_eq!(result, Some(()));
    assert_eq!(events.pending(), 0);
    assert_eq!(progression.get(progression_flag), Some(1));
    assert!(
        state
            .get::<ItemContainer>(inventory_key)
            .unwrap()
            .slots()
            .iter()
            .all(Option::is_none)
    );
    let final_drops = state.get::<WorldDropStore>(store_drop_key).unwrap();
    assert_eq!(final_drops.get(drop_key).unwrap().position(), drop_position);
    let final_registry = state
        .get::<ItemInstanceRegistry>(store_registry_key)
        .unwrap();
    assert_eq!(final_registry.owners().len(), 1);
    assert_eq!(final_registry.owners()[0].owner, drop_key);
    let final_controller = state
        .get::<DeathScenarioController>(controller_key)
        .unwrap();
    assert!(final_controller.dead);
    assert_eq!(final_controller.action, key("test:controller-idle"));
    assert!(matches!(
        world
            .channel_value::<u8>(channel_key, environment_point)
            .unwrap(),
        ChannelQuery::Loaded(1)
    ));
    assert_eq!(state.get::<WorldEventQueue>(queue_key).unwrap(), events);
    assert_eq!(
        state.get::<ProgressionState>(progression_key).unwrap(),
        progression
    );

    let final_state_checkpoint = state.checkpoint().unwrap();
    let final_world_checkpoint = world.checkpoint(65_536).unwrap();
    let mut restore_schemas = SchemaRegistry::default();
    restore_schemas
        .register(Schema::of::<CombatState>(combat_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<WorldEventQueue>(queue_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<ProgressionState>(progression_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<ItemContainer>(inventory_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<WorldDropStore>(drops_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<ItemInstanceRegistry>(registry_schema, 1, 8192).unwrap())
        .unwrap();
    restore_schemas
        .register(Schema::of::<DeathScenarioController>(controller_schema, 1, 1024).unwrap())
        .unwrap();
    let restored_state = StateStore::restore(
        &final_state_checkpoint,
        scope,
        restore_schemas,
        store_limits,
    )
    .unwrap();
    let restored_world = World::restore(
        &final_world_checkpoint,
        content,
        WorldLimits::default(),
        final_world_checkpoint.len(),
    )
    .unwrap();
    assert!(
        restored_state
            .get::<DeathScenarioController>(controller_key)
            .unwrap()
            .dead
    );
    assert_eq!(
        restored_state
            .get::<WorldDropStore>(store_drop_key)
            .unwrap()
            .get(drop_key)
            .unwrap()
            .position(),
        drop_position
    );
    assert_eq!(
        restored_state
            .get::<ProgressionState>(progression_key)
            .unwrap()
            .get(progression_flag),
        Some(1)
    );
    assert!(matches!(
        restored_world
            .channel_value::<u8>(channel_key, environment_point)
            .unwrap(),
        ChannelQuery::Loaded(1)
    ));
}

struct AuthorEvents;
impl WorldEventProvider for AuthorEvents {
    fn key(&self) -> StableKey {
        key("test:author-events")
    }
    fn version(&self) -> u32 {
        1
    }
    fn handle(
        &self,
        event: WorldEventView<'_>,
    ) -> Result<Vec<WorldEventDelivery>, charon_engine::game::signals::SignalError> {
        if event.key == key("test:boss-defeated") {
            Ok(vec![WorldEventDelivery {
                event: WorldEvent {
                    scope: event.scope,
                    key: key("test:area-unlocked"),
                    source: event.source,
                    sequence: event.sequence + 1,
                    payload: event.payload.to_vec(),
                },
                timing: SignalTiming::Deferred,
            }])
        } else {
            Ok(Vec::new())
        }
    }
}

struct AtomicAuthorEvents;
impl TransactionalWorldEventProvider for AtomicAuthorEvents {
    fn key(&self) -> StableKey {
        key("test:atomic-world-events")
    }
    fn version(&self) -> u32 {
        1
    }
    fn handle(
        &self,
        event: WorldEventView<'_>,
    ) -> Result<WorldEventTransactionResult, charon_engine::game::signals::SignalError> {
        let (progression_changes, has_world_commands) = match event.key {
            event_key if event_key == key("test:world-command-and-progress") => (
                vec![ProgressionChange::AddCounter {
                    key: key("test:atomic-event-count"),
                    delta: 1,
                }],
                true,
            ),
            event_key if event_key == key("test:progress-only") => (
                vec![ProgressionChange::AddCounter {
                    key: key("test:atomic-event-count"),
                    delta: 1,
                }],
                false,
            ),
            _ => (Vec::new(), false),
        };
        Ok(WorldEventTransactionResult {
            deliveries: Vec::new(),
            progression_changes,
            has_world_commands,
        })
    }
}

#[test]
fn world_event_world_command_and_progression_commit_or_rollback_together() {
    let scope = key("test:atomic-event-scope");
    let provider = AtomicAuthorEvents;
    let signal_limits = SignalLimits {
        signals_per_tick: 8,
        queued_payload_bytes: 256,
        deliveries_per_signal: 2,
        payload_bytes: 32,
    };
    let mut events = WorldEventQueue::new_transactional(scope, &provider, 0).unwrap();
    events
        .publish(
            WorldEvent {
                scope,
                key: key("test:world-command-and-progress"),
                source: key("test:author"),
                sequence: 12,
                payload: vec![7],
            },
            SignalTiming::SameTick,
            signal_limits,
        )
        .unwrap();
    let progression_key = key("test:atomic-event-count");
    let progression_catalog = ProgressionCatalog::new(vec![ProgressionDefinition {
        key: progression_key,
        kind: ProgressionKind::Counter,
        initial: 0,
        minimum: 0,
        maximum: 10,
    }])
    .unwrap();
    let mut progression = progression_catalog
        .create_state(scope, &[progression_key], 8)
        .unwrap();
    let queue_key = key("test:atomic-event-queue");
    let queue_schema = key("test:atomic-event-queue-schema");
    let state_key = key("test:atomic-event-progress");
    let state_schema = key("test:atomic-event-progress-schema");
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WorldEventQueue>(queue_schema, 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ProgressionState>(state_schema, 1, 4096).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 8192,
        checkpoint_bytes: 16384,
    };
    let mut state = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut initial_state = state.transaction().unwrap();
    initial_state.put(queue_key, queue_schema, &events).unwrap();
    initial_state
        .put(state_key, state_schema, &progression)
        .unwrap();
    initial_state.commit().unwrap();
    let initial_checkpoint = state.checkpoint().unwrap();
    let mut world = empty_world();
    let world_revision = world.revision();

    let failed = events.poll_transactionally(
        &mut progression,
        &mut world,
        &mut state,
        queue_key,
        queue_schema,
        state_key,
        state_schema,
        &provider,
        &progression_catalog,
        signal_limits,
        4,
        |actions, editor| {
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0].progression_changes.len(), 1);
            editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            Err::<(), _>(EngineError::new(ErrorCode::InvalidState, "event rollback"))
        },
    );
    assert!(matches!(
        failed,
        Err(charon_engine::game::events::WorldEventTransactionError::World(_))
    ));
    assert_eq!(world.revision(), world_revision);
    assert_eq!(progression.revision(), 0);
    assert_eq!(progression.get(progression_key), Some(0));
    assert_eq!(events.pending(), 1);
    assert_eq!(state.checkpoint().unwrap(), initial_checkpoint);

    let (progress, result) = events
        .poll_transactionally(
            &mut progression,
            &mut world,
            &mut state,
            queue_key,
            queue_schema,
            state_key,
            state_schema,
            &provider,
            &progression_catalog,
            signal_limits,
            4,
            |actions, editor| {
                assert_eq!(actions.len(), 1);
                editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                Ok(actions[0].event.sequence)
            },
        )
        .unwrap();
    assert_eq!(progress, SignalProgress::TickComplete);
    assert_eq!(result, Some(12));
    assert_eq!(events.pending(), 0);
    assert_eq!(progression.revision(), 1);
    assert_eq!(progression.get(progression_key), Some(1));
    assert_eq!(world.revision(), world_revision + 1);
    assert_eq!(state.get::<WorldEventQueue>(queue_key).unwrap(), events);
    assert_eq!(
        state.get::<ProgressionState>(state_key).unwrap(),
        progression
    );
    let committed_world_revision = world.revision();
    let committed_store_revision = state.revision();
    let (progress, result) = events
        .poll_transactionally(
            &mut progression,
            &mut world,
            &mut state,
            queue_key,
            queue_schema,
            state_key,
            state_schema,
            &provider,
            &progression_catalog,
            signal_limits,
            4,
            |_, _| Err::<(), _>(EngineError::new(ErrorCode::InvalidState, "no event")),
        )
        .unwrap();
    assert_eq!(progress, SignalProgress::TickComplete);
    assert_eq!(result, None);
    assert_eq!(world.revision(), committed_world_revision);
    assert_eq!(state.revision(), committed_store_revision);

    events
        .publish(
            WorldEvent {
                scope,
                key: key("test:progress-only"),
                source: key("test:author"),
                sequence: 13,
                payload: Vec::new(),
            },
            SignalTiming::SameTick,
            signal_limits,
        )
        .unwrap();
    let (progress, result) = events
        .poll_transactionally(
            &mut progression,
            &mut world,
            &mut state,
            queue_key,
            queue_schema,
            state_key,
            state_schema,
            &provider,
            &progression_catalog,
            signal_limits,
            4,
            |_, _| Err::<(), _>(EngineError::new(ErrorCode::InvalidState, "progress only")),
        )
        .unwrap();
    assert_eq!(progress, SignalProgress::TickComplete);
    assert_eq!(result, None);
    assert_eq!(progression.get(progression_key), Some(2));
    assert_eq!(world.revision(), committed_world_revision);
}

#[test]
fn author_world_events_replay_deferred_progression_from_checkpoint() {
    let provider = AuthorEvents;
    let scope = key("test:world-events");
    let signal_limits = SignalLimits {
        signals_per_tick: 16,
        queued_payload_bytes: 1024,
        deliveries_per_signal: 4,
        payload_bytes: 64,
    };
    let mut events = WorldEventQueue::new(scope, &provider, 7).unwrap();
    let source_event = WorldEvent {
        scope,
        key: key("test:boss-defeated"),
        source: key("test:player"),
        sequence: 5,
        payload: vec![3, 8],
    };
    events
        .publish(source_event.clone(), SignalTiming::SameTick, signal_limits)
        .unwrap();
    assert_eq!(
        events.poll(&provider, signal_limits, 1).unwrap(),
        SignalProgress::TickComplete
    );
    assert_eq!(events.pending(), 1);

    let progress_key = key("test:progress-flag");
    let progression_catalog = ProgressionCatalog::new(vec![ProgressionDefinition {
        key: progress_key,
        kind: ProgressionKind::Flag,
        initial: 0,
        minimum: 0,
        maximum: 1,
    }])
    .unwrap();
    let mut progression = progression_catalog
        .create_state(scope, &[progress_key], 16)
        .unwrap();
    progression
        .apply_world_event(
            scope,
            0,
            &source_event,
            &[ProgressionChange::SetFlag {
                key: progress_key,
                value: true,
            }],
            &progression_catalog,
        )
        .unwrap();

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WorldEventQueue>(key("test:world-event-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<ProgressionState>(key("test:progress-state-schema"), 1, 4096).unwrap(),
        )
        .unwrap();
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:world-events"),
            key("test:world-event-schema"),
            &events,
        )
        .unwrap();
    transaction
        .put(
            key("test:progress-state"),
            key("test:progress-state-schema"),
            &progression,
        )
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<WorldEventQueue>(key("test:world-event-schema"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<ProgressionState>(key("test:progress-state-schema"), 1, 4096).unwrap(),
        )
        .unwrap();
    let mut restored_store =
        StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut restored = restored_store
        .get::<WorldEventQueue>(key("test:world-events"))
        .unwrap();
    let mut restored_progression = restored_store
        .get::<ProgressionState>(key("test:progress-state"))
        .unwrap();
    restored.validate_scope(scope).unwrap();
    restored.validate_limits(signal_limits).unwrap();
    assert_eq!(restored.tick(), 7);
    restored.begin_next_tick().unwrap();
    assert_eq!(restored.tick(), 8);
    assert_eq!(
        restored.poll(&provider, signal_limits, 1).unwrap(),
        SignalProgress::TickComplete
    );
    assert_eq!(restored.pending(), 0);
    assert!(restored.validate_scope(key("test:other-world")).is_err());
    restored_progression
        .validate(scope, 16, &progression_catalog)
        .unwrap();
    assert_eq!(restored_progression.get(progress_key), Some(1));
    assert!(
        !restored_progression
            .apply_world_event(
                scope,
                0,
                &source_event,
                &[ProgressionChange::SetFlag {
                    key: progress_key,
                    value: false,
                }],
                &progression_catalog,
            )
            .unwrap()
    );
    assert_eq!(restored_progression.get(progress_key), Some(1));

    let mut restored_transaction = restored_store.transaction().unwrap();
    restored_transaction
        .put(
            key("test:world-events"),
            key("test:world-event-schema"),
            &restored,
        )
        .unwrap();
    restored_transaction
        .put(
            key("test:progress-state"),
            key("test:progress-state-schema"),
            &restored_progression,
        )
        .unwrap();
    restored_transaction.commit().unwrap();

    let mut replay_schemas = SchemaRegistry::default();
    replay_schemas
        .register(Schema::of::<WorldEventQueue>(key("test:world-event-schema"), 1, 4096).unwrap())
        .unwrap();
    replay_schemas
        .register(
            Schema::of::<ProgressionState>(key("test:progress-state-schema"), 1, 4096).unwrap(),
        )
        .unwrap();
    let mut replay_store =
        StateStore::restore(&checkpoint, scope, replay_schemas, store_limits).unwrap();
    let mut replayed = replay_store
        .get::<WorldEventQueue>(key("test:world-events"))
        .unwrap();
    let mut replayed_progression = replay_store
        .get::<ProgressionState>(key("test:progress-state"))
        .unwrap();
    replayed.begin_next_tick().unwrap();
    assert_eq!(
        replayed.poll(&provider, signal_limits, 1).unwrap(),
        SignalProgress::TickComplete
    );
    assert!(
        !replayed_progression
            .apply_world_event(
                scope,
                0,
                &source_event,
                &[ProgressionChange::SetFlag {
                    key: progress_key,
                    value: false,
                }],
                &progression_catalog,
            )
            .unwrap()
    );
    let mut replay_transaction = replay_store.transaction().unwrap();
    replay_transaction
        .put(
            key("test:world-events"),
            key("test:world-event-schema"),
            &replayed,
        )
        .unwrap();
    replay_transaction
        .put(
            key("test:progress-state"),
            key("test:progress-state-schema"),
            &replayed_progression,
        )
        .unwrap();
    replay_transaction.commit().unwrap();
    assert_eq!(
        restored_store.checkpoint().unwrap(),
        replay_store.checkpoint().unwrap()
    );
}

#[test]
fn author_item_container_state_restores_through_checkpoint_and_validates_catalog() {
    let ore_key = key("test:ore-item");
    let sword_key = key("test:unique-sword");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(
            ore_key,
            20,
            0,
            2,
            Some(key("test:ore-view")),
            None,
            None,
            vec![key("test:craft-material")],
        )
        .unwrap(),
        ItemDefinition::new(
            sword_key,
            1,
            16,
            8,
            Some(key("test:sword-view")),
            Some(key("test:sword-action")),
            Some(key("test:hand-slot")),
            vec![key("test:weapon")],
        )
        .unwrap(),
    ])
    .unwrap();
    let container_limits = ItemContainerLimits {
        slots: 8,
        payload_bytes: 128,
        weight_micros: 256,
    };
    let mut container = ItemContainer::new(key("test:player-container"), container_limits).unwrap();
    container
        .deposit(
            ItemLot::Stack {
                item: ore_key,
                count: 7,
            },
            0,
            &catalog,
        )
        .unwrap();
    container
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                id: key("test:sword-unique-1"),
                item: sword_key,
                state: vec![1, 4, 9],
            })),
            1,
            &catalog,
        )
        .unwrap();

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:item-container"), 1, 4096).unwrap())
        .unwrap();
    let scope = key("test:player-save");
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:inventory"),
            key("test:item-container"),
            &container,
        )
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:item-container"), 1, 4096).unwrap())
        .unwrap();
    let restored_store = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let restored = restored_store
        .get::<ItemContainer>(key("test:inventory"))
        .unwrap();
    catalog.validate_container(&restored).unwrap();
    restored.validate_limits(container_limits).unwrap();
    assert_eq!(restored, container);
}

#[test]
fn author_item_and_world_updates_commit_or_rollback_together() {
    use charon_engine::game::items::ItemWorldTransactionError;

    let ore = key("test:transaction-ore");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore, 5, 0, 2, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let mut container = ItemContainer::new(
        key("test:transaction-container"),
        ItemContainerLimits {
            slots: 2,
            payload_bytes: 32,
            weight_micros: 100,
        },
    )
    .unwrap();
    let mut world = empty_world();
    let chunk = ChunkCoord::new(0, 0).unwrap();

    container
        .transact_with_world(
            0,
            &catalog,
            &mut world,
            |candidate| {
                candidate.deposit(
                    ItemLot::Stack {
                        item: ore,
                        count: 3,
                    },
                    0,
                    &catalog,
                )?;
                Ok(())
            },
            |edit| edit.insert_empty_chunk(chunk),
        )
        .unwrap();
    assert_eq!(container.revision(), 1);
    assert_eq!(world.revision(), 1);
    let before_container = container.clone();
    let before_world_revision = world.revision();
    let mut world_callback_ran = false;
    let invalid_item = container.transact_with_world(
        container.revision(),
        &catalog,
        &mut world,
        |candidate| {
            candidate.deposit(
                ItemLot::Stack {
                    item: key("test:missing-item"),
                    count: 1,
                },
                candidate.revision(),
                &catalog,
            )?;
            Ok(())
        },
        |_| {
            world_callback_ran = true;
            Ok(())
        },
    );
    assert!(matches!(
        invalid_item,
        Err(ItemWorldTransactionError::Item(_))
    ));
    assert!(!world_callback_ran);
    assert_eq!(container, before_container);
    assert_eq!(world.revision(), before_world_revision);

    let failed = container.transact_with_world(
        container.revision(),
        &catalog,
        &mut world,
        |candidate| {
            candidate.deposit(
                ItemLot::Stack {
                    item: ore,
                    count: 1,
                },
                candidate.revision(),
                &catalog,
            )?;
            Ok(())
        },
        |edit| edit.insert_empty_chunk(chunk),
    );
    assert!(matches!(failed, Err(ItemWorldTransactionError::World(_))));
    assert_eq!(container, before_container);
    assert_eq!(world.revision(), before_world_revision);
}

#[test]
fn author_item_registry_and_world_transaction_preserves_global_instance_ownership() {
    use charon_engine::game::items::ItemWorldTransactionError;

    let sword = key("test:registry-sword");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(sword, 1, 16, 5, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let limits = ItemContainerLimits {
        slots: 4,
        payload_bytes: 64,
        weight_micros: 128,
    };
    let registry_limits = ItemInstanceRegistryLimits {
        owners: 8,
        instances: 8,
    };
    let mut player = ItemContainer::new(key("test:registry-player"), limits).unwrap();
    let mut chest = ItemContainer::new(key("test:registry-chest"), limits).unwrap();
    let existing = ItemInstance {
        id: key("test:registry-sword-a"),
        item: sword,
        state: vec![1],
    };
    chest
        .deposit(ItemLot::Instance(Box::new(existing.clone())), 0, &catalog)
        .unwrap();
    let scope = key("test:registry-world");
    let mut registry = ItemInstanceRegistry::new(scope);
    registry
        .refresh(0, &[&player, &chest], &[], &catalog, registry_limits)
        .unwrap();
    let mut world = empty_world();
    let world_revision = world.revision();
    let mut world_callback_ran = false;
    let duplicate = player.transact_with_world_and_registry(
        0,
        &catalog,
        &mut world,
        &mut registry,
        scope,
        1,
        &[&chest],
        &[],
        registry_limits,
        |candidate| candidate.deposit(ItemLot::Instance(Box::new(existing)), 0, &catalog),
        |_| {
            world_callback_ran = true;
            Ok(())
        },
    );
    assert!(matches!(
        duplicate,
        Err(ItemWorldTransactionError::Item(
            charon_engine::game::items::ItemError::DuplicateInstance(id)
        )) if *id == key("test:registry-sword-a")
    ));
    assert!(!world_callback_ran);
    assert_eq!(player.revision(), 0);
    assert_eq!(registry.revision(), 1);
    assert_eq!(world.revision(), world_revision);

    player
        .transact_with_world_and_registry(
            0,
            &catalog,
            &mut world,
            &mut registry,
            scope,
            1,
            &[&chest],
            &[],
            registry_limits,
            |candidate| {
                candidate.deposit(
                    ItemLot::Instance(Box::new(ItemInstance {
                        id: key("test:registry-sword-b"),
                        item: sword,
                        state: vec![2],
                    })),
                    0,
                    &catalog,
                )
            },
            |edit| edit.insert_empty_chunk(ChunkCoord::new(2, 3).unwrap()),
        )
        .unwrap();
    assert_eq!(player.revision(), 1);
    assert_eq!(registry.revision(), 2);
    assert_eq!(world.revision(), world_revision + 1);
    registry
        .validate_ownership(scope, &[&player, &chest], &[], &catalog, registry_limits)
        .unwrap();
    assert_eq!(registry.owners().len(), 2);

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemInstanceRegistry>(key("test:registry-schema"), 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:registry-container"), 1, 8192).unwrap())
        .unwrap();
    let store_limits = StoreLimits {
        records: 4,
        payload_bytes: 16_384,
        checkpoint_bytes: 32_768,
    };
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:instance-index"),
            key("test:registry-schema"),
            &registry,
        )
        .unwrap();
    transaction
        .put(
            key("test:registry-player"),
            key("test:registry-container"),
            &player,
        )
        .unwrap();
    transaction
        .put(
            key("test:registry-chest"),
            key("test:registry-container"),
            &chest,
        )
        .unwrap();
    transaction.commit().unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemInstanceRegistry>(key("test:registry-schema"), 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:registry-container"), 1, 8192).unwrap())
        .unwrap();
    let restored = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let restored_registry = restored
        .get::<ItemInstanceRegistry>(key("test:instance-index"))
        .unwrap();
    let restored_player = restored
        .get::<ItemContainer>(key("test:registry-player"))
        .unwrap();
    let restored_chest = restored
        .get::<ItemContainer>(key("test:registry-chest"))
        .unwrap();
    restored_registry.validate_limits(registry_limits).unwrap();
    restored_registry
        .validate_ownership(
            scope,
            &[&restored_player, &restored_chest],
            &[],
            &catalog,
            registry_limits,
        )
        .unwrap();
    assert_eq!(restored_registry, registry);
}

#[test]
fn author_equipment_transfer_is_atomic_and_restores_from_checkpoint() {
    use charon_engine::game::items::ItemError;

    let hand = key("test:hand-slot");
    let worn_sword = key("test:worn-sword");
    let stack_sword = key("test:stack-sword");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(worn_sword, 1, 16, 20, None, None, Some(hand), vec![]).unwrap(),
        ItemDefinition::new(stack_sword, 4, 0, 25, None, None, Some(hand), vec![]).unwrap(),
    ])
    .unwrap();
    let container_limits = ItemContainerLimits {
        slots: 1,
        payload_bytes: 32,
        weight_micros: 100,
    };
    let equipment_limits = EquipmentLimits {
        payload_bytes: 32,
        weight_micros: 100,
    };
    let mut inventory = ItemContainer::new(key("test:equip-inventory"), container_limits).unwrap();
    let mut equipment =
        EquipmentLoadout::new(key("test:equip-loadout"), vec![hand], equipment_limits).unwrap();
    inventory
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                id: key("test:worn-sword-instance"),
                item: worn_sword,
                state: vec![4, 2],
            })),
            0,
            &catalog,
        )
        .unwrap();
    equipment
        .equip_from(&mut inventory, hand, 0, 1, 0, &catalog)
        .unwrap();
    assert!(inventory.slots()[0].is_none());
    assert!(
        matches!(equipment.slots().next(), Some((slot, Some(ItemLot::Instance(_)))) if slot == hand)
    );

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:equip-container"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<EquipmentLoadout>(key("test:equip-loadout-schema"), 1, 4096).unwrap(),
        )
        .unwrap();
    let scope = key("test:equip-save");
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(
            key("test:equip-inventory"),
            key("test:equip-container"),
            &inventory,
        )
        .unwrap();
    transaction
        .put(
            key("test:equip-state"),
            key("test:equip-loadout-schema"),
            &equipment,
        )
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<ItemContainer>(key("test:equip-container"), 1, 4096).unwrap())
        .unwrap();
    schemas
        .register(
            Schema::of::<EquipmentLoadout>(key("test:equip-loadout-schema"), 1, 4096).unwrap(),
        )
        .unwrap();
    let restored_store = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut restored_inventory = restored_store
        .get::<ItemContainer>(key("test:equip-inventory"))
        .unwrap();
    let mut restored_equipment = restored_store
        .get::<EquipmentLoadout>(key("test:equip-state"))
        .unwrap();
    restored_inventory.validate(&catalog).unwrap();
    restored_inventory
        .validate_limits(container_limits)
        .unwrap();
    restored_equipment
        .validate_with_inventory(&restored_inventory, &catalog)
        .unwrap();
    restored_equipment
        .validate_limits(equipment_limits)
        .unwrap();
    let mut duplicate_inventory = restored_inventory.clone();
    duplicate_inventory
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                id: key("test:worn-sword-instance"),
                item: worn_sword,
                state: vec![4, 2],
            })),
            duplicate_inventory.revision(),
            &catalog,
        )
        .unwrap();
    assert_eq!(
        restored_equipment.validate_with_inventory(&duplicate_inventory, &catalog),
        Err(ItemError::DuplicateInstance(Box::new(key(
            "test:worn-sword-instance"
        ))))
    );

    let inventory_revision = restored_inventory.revision();
    let equipment_revision = restored_equipment.revision();
    restored_equipment
        .unequip_to(
            &mut restored_inventory,
            hand,
            inventory_revision,
            equipment_revision,
            &catalog,
        )
        .unwrap();
    assert!(matches!(
        restored_inventory.slots()[0],
        Some(ItemLot::Instance(_))
    ));
    let inventory_revision = restored_inventory.revision();
    let equipment_revision = restored_equipment.revision();
    restored_equipment
        .equip_from(
            &mut restored_inventory,
            hand,
            0,
            inventory_revision,
            equipment_revision,
            &catalog,
        )
        .unwrap();

    restored_inventory
        .deposit(
            ItemLot::Stack {
                item: stack_sword,
                count: 2,
            },
            restored_inventory.revision(),
            &catalog,
        )
        .unwrap();
    let before_inventory = restored_inventory.clone();
    let before_equipment = restored_equipment.clone();
    assert_eq!(
        restored_equipment.equip_from(
            &mut restored_inventory,
            hand,
            0,
            before_inventory.revision(),
            before_equipment.revision(),
            &catalog,
        ),
        Err(ItemError::CapacityExceeded)
    );
    assert_eq!(restored_inventory, before_inventory);
    assert_eq!(restored_equipment, before_equipment);
}

#[test]
fn author_equipment_transaction_keeps_world_instance_registry_in_sync() {
    use charon_engine::game::items::ItemError;

    let hand = key("test:registry-hand");
    let blade = key("test:registry-blade");
    let item_id = key("test:registry-blade-instance");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(blade, 1, 16, 12, None, None, Some(hand), vec![]).unwrap(),
    ])
    .unwrap();
    let container_limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 32,
        weight_micros: 64,
    };
    let equipment_limits = EquipmentLimits {
        payload_bytes: 32,
        weight_micros: 64,
    };
    let registry_limits = ItemInstanceRegistryLimits {
        owners: 4,
        instances: 4,
    };
    let mut inventory =
        ItemContainer::new(key("test:registry-equip-bag"), container_limits).unwrap();
    let chest = ItemContainer::new(key("test:registry-equip-chest"), container_limits).unwrap();
    let mut equipment = EquipmentLoadout::new(
        key("test:registry-equip-loadout"),
        vec![hand],
        equipment_limits,
    )
    .unwrap();
    inventory
        .deposit(
            ItemLot::Instance(Box::new(ItemInstance {
                id: item_id,
                item: blade,
                state: vec![9],
            })),
            0,
            &catalog,
        )
        .unwrap();
    let scope = key("test:registry-equip-world");
    let mut registry = ItemInstanceRegistry::new(scope);
    registry
        .refresh(
            0,
            &[&inventory, &chest],
            &[&equipment],
            &catalog,
            registry_limits,
        )
        .unwrap();

    let before_inventory = inventory.clone();
    let before_equipment = equipment.clone();
    let inventory_revision = inventory.revision();
    let equipment_revision = equipment.revision();
    let registry_revision = registry.revision();
    let mut wrong_scope_callback_ran = false;
    assert_eq!(
        equipment.transact_with_registry(
            &mut inventory,
            inventory_revision,
            equipment_revision,
            &catalog,
            &mut registry,
            key("test:wrong-registry-world"),
            registry_revision,
            &[&chest],
            &[],
            registry_limits,
            |_, _| {
                wrong_scope_callback_ran = true;
                Ok(())
            },
        ),
        Err(ItemError::InvalidContainer)
    );
    assert!(!wrong_scope_callback_ran);
    let failed = equipment.transact_with_registry(
        &mut inventory,
        inventory_revision,
        equipment_revision,
        &catalog,
        &mut registry,
        scope,
        registry_revision,
        &[&chest],
        &[],
        registry_limits,
        |candidate_inventory, candidate_equipment| {
            candidate_equipment.equip_from(
                candidate_inventory,
                hand,
                0,
                candidate_inventory.revision(),
                candidate_equipment.revision(),
                &catalog,
            )?;
            Err::<(), _>(ItemError::MissingItem)
        },
    );
    assert_eq!(failed, Err(ItemError::MissingItem));
    assert_eq!(inventory, before_inventory);
    assert_eq!(equipment, before_equipment);
    assert_eq!(registry.revision(), 1);

    let inventory_revision = inventory.revision();
    let equipment_revision = equipment.revision();
    let registry_revision = registry.revision();
    equipment
        .transact_with_registry(
            &mut inventory,
            inventory_revision,
            equipment_revision,
            &catalog,
            &mut registry,
            scope,
            registry_revision,
            &[&chest],
            &[],
            registry_limits,
            |candidate_inventory, candidate_equipment| {
                candidate_equipment.equip_from(
                    candidate_inventory,
                    hand,
                    0,
                    candidate_inventory.revision(),
                    candidate_equipment.revision(),
                    &catalog,
                )
            },
        )
        .unwrap();
    registry
        .validate_ownership(
            scope,
            &[&inventory, &chest],
            &[&equipment],
            &catalog,
            registry_limits,
        )
        .unwrap();
    assert_eq!(registry.revision(), 2);
    assert_eq!(
        registry.owners(),
        &[charon_engine::game::items::ItemInstanceOwner {
            instance: item_id,
            owner: equipment.key(),
            slot: charon_engine::game::items::ItemInstanceOwnerSlot::Equipment(hand),
        }]
    );

    let inventory_revision = inventory.revision();
    let equipment_revision = equipment.revision();
    let registry_revision = registry.revision();
    equipment
        .transact_with_registry(
            &mut inventory,
            inventory_revision,
            equipment_revision,
            &catalog,
            &mut registry,
            scope,
            registry_revision,
            &[&chest],
            &[],
            registry_limits,
            |candidate_inventory, candidate_equipment| {
                candidate_equipment.unequip_to(
                    candidate_inventory,
                    hand,
                    candidate_inventory.revision(),
                    candidate_equipment.revision(),
                    &catalog,
                )
            },
        )
        .unwrap();
    registry
        .validate_ownership(
            scope,
            &[&inventory, &chest],
            &[&equipment],
            &catalog,
            registry_limits,
        )
        .unwrap();
    assert_eq!(registry.revision(), 3);
    assert_eq!(
        registry.owners()[0].slot,
        charon_engine::game::items::ItemInstanceOwnerSlot::Container(0)
    );
}

#[test]
fn author_item_query_is_shared_bounded_and_stably_ordered() {
    let ore = key("test:query-ore");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore, 10, 0, 1, None, None, None, vec![key("test:resource")]).unwrap(),
    ])
    .unwrap();
    let limits = ItemContainerLimits {
        slots: 2,
        payload_bytes: 0,
        weight_micros: 100,
    };
    let mut later = ItemContainer::new(key("test:z-storage"), limits).unwrap();
    let mut earlier = ItemContainer::new(key("test:a-storage"), limits).unwrap();
    later
        .deposit(
            ItemLot::Stack {
                item: ore,
                count: 4,
            },
            0,
            &catalog,
        )
        .unwrap();
    earlier
        .deposit(
            ItemLot::Stack {
                item: ore,
                count: 3,
            },
            0,
            &catalog,
        )
        .unwrap();
    let storages = [&later, &earlier];
    let query = ItemQuery::new(None, Some(key("test:resource"))).unwrap();
    let limited = query.execute(&storages, &catalog, 1).unwrap();
    assert_eq!(limited.matches.len(), 1);
    assert!(limited.truncated);
    assert_eq!(limited.matches[0].container, key("test:a-storage"));
    assert_eq!(limited.matches[0].count, 3);

    let complete = ItemQuery::new(Some(ore), Some(key("test:resource")))
        .unwrap()
        .execute(&storages, &catalog, 2)
        .unwrap();
    assert!(!complete.truncated);
    assert_eq!(complete.matches.len(), 2);
    assert_eq!(complete.matches[1].container, key("test:z-storage"));
    assert_eq!(complete.matches[1].count, 4);
}

#[test]
fn author_craft_job_reservation_and_progress_resume_from_store_checkpoint() {
    let ore = key("test:craft-ore");
    let catalog = ItemCatalog::new(vec![
        ItemDefinition::new(ore, 10, 0, 2, None, None, None, vec![]).unwrap(),
    ])
    .unwrap();
    let mut container = ItemContainer::new(
        key("test:craft-container"),
        ItemContainerLimits {
            slots: 4,
            payload_bytes: 32,
            weight_micros: 100,
        },
    )
    .unwrap();
    container
        .deposit(
            ItemLot::Stack {
                item: ore,
                count: 8,
            },
            0,
            &catalog,
        )
        .unwrap();
    let mut session = CraftSession::new(container, &catalog, 128).unwrap();
    let recipe = CraftRecipe::new(
        key("test:craft-plate"),
        4,
        vec![
            CraftRequirement {
                item: ore,
                count: 2,
            },
            CraftRequirement {
                item: ore,
                count: 3,
            },
        ],
        vec![],
        ore,
        2,
        2,
        Some(key("test:forge")),
        Some(12),
    )
    .unwrap();
    let recipe_key = recipe.key();
    let recipe_version = recipe.version();
    let recipes = CraftCatalog::new(vec![recipe.clone()], &catalog).unwrap();
    let receipt = key("test:craft-job-1");
    let context = CraftContext {
        station: Some(key("test:forge")),
        world_revision: 12,
    };
    assert_eq!(
        session
            .start_registered(
                &recipes,
                recipe_key,
                recipe_version,
                receipt,
                5,
                context,
                1,
                &catalog,
            )
            .unwrap(),
        CraftProgress::Running { remaining_ticks: 2 }
    );
    assert_eq!(
        session.container().slots()[0],
        Some(ItemLot::Stack {
            item: ore,
            count: 3
        })
    );
    session
        .advance_registered(&recipes, 6, context, &catalog)
        .unwrap();

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CraftSession>(key("test:craft-session"), 1, 4096).unwrap())
        .unwrap();
    let scope = key("test:craft-save");
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut transaction = store.transaction().unwrap();
    transaction
        .put(key("test:craft-state"), key("test:craft-session"), &session)
        .unwrap();
    transaction.commit().unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CraftSession>(key("test:craft-session"), 1, 4096).unwrap())
        .unwrap();
    let restored_store = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let mut restored = restored_store
        .get::<CraftSession>(key("test:craft-state"))
        .unwrap();
    restored.validate_receipt_capacity(128).unwrap();
    restored.validate_with_recipe(&recipe, &catalog).unwrap();
    assert_eq!(
        restored
            .advance_registered(&recipes, 7, context, &catalog)
            .unwrap(),
        CraftProgress::Ready
    );
    restored
        .claim_registered(&recipes, receipt, 2, &catalog)
        .unwrap();
    assert!(restored.contains_receipt(receipt));
    assert_eq!(
        restored.container().slots()[0],
        Some(ItemLot::Stack {
            item: ore,
            count: 5
        })
    );
    restored.validate(&catalog).unwrap();
}

struct AuthorHitResolver;
impl HitResolutionProvider for AuthorHitResolver {
    fn key(&self) -> StableKey {
        key("test:author-hit-resolver")
    }
    fn version(&self) -> u32 {
        2
    }
    fn resolve(
        &self,
        hit: &CombatHit,
        attributes: &charon_engine::game::combat::AttributeState,
        _: HitLimits,
    ) -> Result<HitResolution, CombatError> {
        let hp = attributes
            .get(key("test:hp"))
            .ok_or(CombatError::UnknownAttribute(Box::new(key("test:hp"))))?;
        Ok(HitResolution {
            attributes: vec![AttributeChange {
                key: key("test:hp"),
                value: hp - hit.raw_damage,
            }],
            effects: vec![key("test:damaged-event")],
            knockback: Some([0.5, -0.25]),
        })
    }
}

struct AuthorAttackQuery;
impl AttackSpatialQuery for AuthorAttackQuery {
    fn key(&self) -> StableKey {
        key("test:author-attack-query")
    }
    fn version(&self) -> u32 {
        4
    }
    fn query(
        &self,
        shape: &AttackShape,
        source: StableKey,
        origin: [f32; 2],
        tick: u64,
        target_limit: u32,
    ) -> Result<AttackQueryResult, CombatError> {
        assert!(matches!(shape, AttackShape::Capsule { .. }));
        assert_eq!(source, key("test:player"));
        assert_eq!(origin, [4.0, 2.0]);
        assert_eq!(tick, 48);
        assert_eq!(target_limit, 8);
        Ok(AttackQueryResult {
            status: AttackQueryStatus::Hit,
            contacts: vec![
                AttackContact {
                    target: key("test:zombie"),
                    impulse: [1.0, 0.0],
                    substep: 2,
                },
                AttackContact {
                    target: key("test:boss"),
                    impulse: [0.5, -0.25],
                    substep: 1,
                },
            ],
        })
    }
}

struct WorldCombatCollision {
    truncated: bool,
}
impl CollisionQueriesProvider for WorldCombatCollision {
    fn key(&self) -> StableKey {
        key("test:world-combat-collision")
    }
    fn version(&self) -> u32 {
        2
    }
    fn query(
        &self,
        _: &World,
        _: Option<&BodyPhysics>,
        request: &CollisionQueryRequest,
    ) -> charon_engine::foundation::EngineResult<CollisionQueryResult> {
        assert!(matches!(
            request.geometry,
            CollisionQueryGeometry::Overlap {
                shape: charon_engine::world::ColliderShape::Circle { radius },
                pose: charon_engine::world::BodyPose { x: 4.0, y: 2.0, angle: 0.0 },
            } if radius == 0.75
        ));
        assert_eq!(request.owner, Some(key("test:player")));
        assert_eq!(request.tick, 48);
        assert_eq!(request.max_hits, 8);
        let hits = if self.truncated {
            (0..request.max_hits)
                .map(|part| CollisionQueryHit {
                    target: CollisionQueryTarget::Custom {
                        owner: 3,
                        object: 44,
                        part,
                    },
                    material: None,
                    point: [4.0, 2.0],
                    normal: [1.0, 0.0],
                    time_of_impact: 0.0,
                    geometry_revision: 9,
                    coverage: 1.0,
                })
                .collect()
        } else {
            vec![CollisionQueryHit {
                target: CollisionQueryTarget::Custom {
                    owner: 3,
                    object: 44,
                    part: 0,
                },
                material: None,
                point: [4.0, 2.0],
                normal: [1.0, 0.0],
                time_of_impact: 0.0,
                geometry_revision: 9,
                coverage: 1.0,
            }]
        };
        Ok(CollisionQueryResult {
            status: CollisionQueryStatus::Hit,
            hits,
            truncated: self.truncated,
            geometry_revision: 9,
        })
    }
}

struct WorldCombatTargets;
impl WorldAttackTargetResolver for WorldCombatTargets {
    fn resolve(
        &self,
        source: StableKey,
        hit: &CollisionQueryHit,
    ) -> Result<Option<(StableKey, [f32; 2])>, CombatError> {
        assert_eq!(source, key("test:player"));
        Ok(matches!(
            hit.target,
            CollisionQueryTarget::Custom {
                owner: 3,
                object: 44,
                part: 0
            }
        )
        .then_some((key("test:enemy"), [0.5, -0.25])))
    }
}

#[test]
fn world_collision_attack_query_adapts_shape_and_resolves_authored_target() {
    let world = empty_world();
    let layer = world
        .content()
        .layer_id(key("test:transaction-layer"))
        .unwrap();
    let provider = WorldCombatCollision { truncated: false };
    let resolver = WorldCombatTargets;
    let query = WorldAttackQuery::new(
        &world,
        &provider,
        &resolver,
        WorldAttackQueryOptions::new(layer),
    );
    let attack = key("test:world-query-attack");
    let catalog = AttackCatalog::new(
        vec![
            AttackDefinition::new(
                attack,
                1,
                AttackShape::Circle { radius: 0.75 },
                key("test:world-combat-collision"),
                2,
                key("test:world-hit-group"),
                key("test:physical"),
                10.0,
                6,
            )
            .unwrap(),
        ],
        AttackCatalogLimits::new(4, 1024, 16),
    )
    .unwrap();
    let result = catalog
        .query_hits(
            attack,
            key("test:world-attack-instance"),
            key("test:player"),
            [4.0, 2.0],
            48,
            8,
            &query,
        )
        .unwrap();
    assert_eq!(result.status, AttackQueryStatus::Hit);
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].target, key("test:enemy"));
    assert_eq!(result.hits[0].impulse, [0.5, -0.25]);
    assert_eq!(result.hits[0].substep, 0);

    let truncated_provider = WorldCombatCollision { truncated: true };
    let truncated_query = WorldAttackQuery::new(
        &world,
        &truncated_provider,
        &resolver,
        WorldAttackQueryOptions::new(layer),
    );
    let truncated = catalog
        .query_hits(
            attack,
            key("test:world-attack-instance-truncated"),
            key("test:player"),
            [4.0, 2.0],
            48,
            8,
            &truncated_query,
        )
        .unwrap();
    assert_eq!(truncated.status, AttackQueryStatus::Incomplete);
    assert!(truncated.hits.is_empty());
}

#[test]
fn author_attack_query_feeds_registered_hits_into_atomic_combat_resolution() {
    let attack = key("test:slash");
    let catalog = AttackCatalog::new(
        vec![
            AttackDefinition::new(
                attack,
                3,
                AttackShape::Capsule {
                    half_height: 0.75,
                    radius: 0.25,
                },
                key("test:author-attack-query"),
                4,
                key("test:blade"),
                key("test:physical"),
                30.0,
                12,
            )
            .unwrap(),
        ],
        AttackCatalogLimits::new(64, 16 * 1024, 128),
    )
    .unwrap();
    let hits = catalog
        .query_hits(
            attack,
            key("test:slash-instance-1"),
            key("test:player"),
            [4.0, 2.0],
            48,
            8,
            &AuthorAttackQuery,
        )
        .unwrap();
    assert_eq!(hits.status, AttackQueryStatus::Hit);
    assert_eq!(hits.hits.len(), 2);
    assert_eq!(hits.hits[0].target, key("test:boss"));
    assert_eq!(hits.hits[0].substep, 1);
    assert_eq!(hits.hits[1].target, key("test:zombie"));
    assert!(hits.hits.iter().all(|hit| {
        hit.attack == key("test:slash-instance-1")
            && hit.hit_group == key("test:blade")
            && hit.damage_kind == key("test:physical")
            && hit.raw_damage == 30.0
            && hit.hit_window == 12
    }));

    let attributes = AttributeCatalog::new(vec![
        AttributeDefinition::new(key("test:hp"), 100.0, 0.0, 100.0).unwrap(),
    ])
    .unwrap();
    let limits = HitLimits {
        immunity_records: 64,
        token_bytes: 256,
        attribute_changes: 4,
        effects: 4,
    };
    let mut combat =
        CombatState::new(attributes.initial_state(&[key("test:hp")]).unwrap(), limits).unwrap();
    let immunity = AttackTargetWindowImmunity;
    let resolver = AuthorHitResolver;
    let candidate = combat
        .prepare_hit(hits.hits[0], &immunity, &resolver, &attributes, limits)
        .unwrap()
        .unwrap();
    candidate
        .commit_with(&mut combat, |candidate, report| {
            assert_eq!(report.hit.target, key("test:boss"));
            assert_eq!(candidate.attributes().get(key("test:hp")), Some(70.0));
            Ok::<_, CombatError>(())
        })
        .unwrap();
    assert_eq!(combat.attributes().get(key("test:hp")), Some(70.0));
}

#[test]
fn author_combat_attribute_and_hit_immunity_resume_from_store_checkpoint() {
    let catalog = AttributeCatalog::new(vec![
        AttributeDefinition::new(key("test:hp"), 100.0, 0.0, 100.0).unwrap(),
    ])
    .unwrap();
    let combat_limits = HitLimits {
        immunity_records: 64,
        token_bytes: 256,
        attribute_changes: 4,
        effects: 4,
    };
    let attributes = catalog.initial_state(&[key("test:hp")]).unwrap();
    let mut combat = CombatState::new(attributes, combat_limits).unwrap();
    let hit = CombatHit {
        attack: key("test:slash-1"),
        source: key("test:player"),
        target: key("test:boss"),
        hit_group: key("test:blade"),
        hit_window: 6,
        damage_kind: key("test:physical"),
        raw_damage: 30.0,
        impulse: [1.0, 0.0],
        tick: 48,
        substep: 0,
    };
    let immunity = AttackTargetWindowImmunity;
    let resolver = AuthorHitResolver;
    let candidate = combat
        .prepare_hit(hit, &immunity, &resolver, &catalog, combat_limits)
        .unwrap()
        .unwrap();

    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CombatState>(key("test:combat-state"), 1, 8192).unwrap())
        .unwrap();
    let scope = key("test:combat-save");
    let store_limits = limits();
    let mut store = StateStore::new(scope, schemas, store_limits).unwrap();
    candidate
        .commit_with(&mut combat, |candidate, report| {
            assert_eq!(candidate.attributes().get(key("test:hp")), Some(70.0));
            assert_eq!(report.resolution.effects, vec![key("test:damaged-event")]);
            let mut transaction = store.transaction()?;
            transaction.put(key("test:combat"), key("test:combat-state"), candidate)?;
            transaction.commit().map(|_| ())
        })
        .unwrap();

    let checkpoint = store.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CombatState>(key("test:combat-state"), 1, 8192).unwrap())
        .unwrap();
    let restored_store = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let restored = restored_store
        .get::<CombatState>(key("test:combat"))
        .unwrap();
    restored.validate(&catalog, combat_limits).unwrap();
    assert_eq!(
        restored.validate(
            &catalog,
            HitLimits {
                token_bytes: combat_limits.token_bytes + 1,
                ..combat_limits
            }
        ),
        Err(CombatError::InvalidLimits)
    );
    assert_eq!(restored.attributes().get(key("test:hp")), Some(70.0));
    assert_eq!(restored.ledger().len(), 1);
    assert!(
        restored
            .prepare_hit(hit, &immunity, &resolver, &catalog, combat_limits)
            .unwrap()
            .is_none()
    );
}

#[test]
fn author_combat_hit_publishes_effect_event_and_world_command_atomically() {
    let catalog = AttributeCatalog::new(vec![
        AttributeDefinition::new(key("test:hp"), 100.0, 0.0, 100.0).unwrap(),
    ])
    .unwrap();
    let combat_limits = HitLimits {
        immunity_records: 64,
        token_bytes: 256,
        attribute_changes: 4,
        effects: 4,
    };
    let mut combat = CombatState::new(
        catalog.initial_state(&[key("test:hp")]).unwrap(),
        combat_limits,
    )
    .unwrap();
    let hit = CombatHit {
        attack: key("test:transactional-slash"),
        source: key("test:player"),
        target: key("test:boss"),
        hit_group: key("test:blade"),
        hit_window: 4,
        damage_kind: key("test:physical"),
        raw_damage: 30.0,
        impulse: [1.0, 0.0],
        tick: 55,
        substep: 1,
    };
    let immunity = AttackTargetWindowImmunity;
    let resolver = AuthorHitResolver;
    let combat_key = key("test:transactional-combat");
    let combat_schema = key("test:transactional-combat-schema");
    let event_key = key("test:transactional-event-queue");
    let event_schema = key("test:transactional-event-schema");
    let scope = key("test:transactional-combat-scope");
    let signal_limits = SignalLimits {
        signals_per_tick: 16,
        queued_payload_bytes: 1024,
        deliveries_per_signal: 4,
        payload_bytes: 64,
    };
    let event_provider = AuthorEvents;
    let queue = WorldEventQueue::new(scope, &event_provider, 55).unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CombatState>(combat_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<WorldEventQueue>(event_schema, 1, 4096).unwrap())
        .unwrap();
    let store_limits = limits();
    let mut state = StateStore::new(scope, schemas, store_limits).unwrap();
    let mut initial = state.transaction().unwrap();
    initial.put(event_key, event_schema, &queue).unwrap();
    initial.commit().unwrap();
    let initial_checkpoint = state.checkpoint().unwrap();
    let mut world = empty_world();
    let world_revision = world.revision();

    let failed = combat
        .prepare_hit(hit, &immunity, &resolver, &catalog, combat_limits)
        .unwrap()
        .unwrap()
        .commit_with_world_and_state(
            &mut combat,
            &mut world,
            &mut state,
            combat_key,
            combat_schema,
            |candidate, report, transaction| {
                assert_eq!(candidate.attributes().get(key("test:hp")), Some(70.0));
                assert_eq!(report.resolution.effects, vec![key("test:damaged-event")]);
                let mut events = transaction.get::<WorldEventQueue>(event_key)?;
                events
                    .publish(
                        WorldEvent {
                            scope,
                            key: report.resolution.effects[0],
                            source: report.hit.source,
                            sequence: report.hit.tick,
                            payload: report.hit.target.as_str().as_bytes().to_vec(),
                        },
                        SignalTiming::Deferred,
                        signal_limits,
                    )
                    .map_err(|_| {
                        EngineError::new(ErrorCode::InvalidValue, "combat effect event")
                    })?;
                transaction.put(event_key, event_schema, &events)
            },
            |report, editor| {
                assert_eq!(report.hit.target, key("test:boss"));
                editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                Err::<(), EngineError>(EngineError::new(
                    ErrorCode::InvalidState,
                    "combat world rollback",
                ))
            },
        );
    assert!(matches!(failed, Err(CombatTransactionError::World(_))));
    assert_eq!(combat.attributes().get(key("test:hp")), Some(100.0));
    assert_eq!(world.revision(), world_revision);
    assert_eq!(state.checkpoint().unwrap(), initial_checkpoint);

    let (_, report) = combat
        .prepare_hit(hit, &immunity, &resolver, &catalog, combat_limits)
        .unwrap()
        .unwrap()
        .commit_with_world_and_state(
            &mut combat,
            &mut world,
            &mut state,
            combat_key,
            combat_schema,
            |candidate, report, transaction| {
                assert_eq!(candidate.attributes().get(key("test:hp")), Some(70.0));
                let mut events = transaction.get::<WorldEventQueue>(event_key)?;
                events
                    .publish(
                        WorldEvent {
                            scope,
                            key: report.resolution.effects[0],
                            source: report.hit.source,
                            sequence: report.hit.tick,
                            payload: report.hit.target.as_str().as_bytes().to_vec(),
                        },
                        SignalTiming::Deferred,
                        signal_limits,
                    )
                    .map_err(|_| {
                        EngineError::new(ErrorCode::InvalidValue, "combat effect event")
                    })?;
                transaction.put(event_key, event_schema, &events)
            },
            |report, editor| {
                editor.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
                Ok(report.hit.target)
            },
        )
        .unwrap();
    assert_eq!(report.hit.target, key("test:boss"));
    assert_eq!(combat.attributes().get(key("test:hp")), Some(70.0));
    assert_eq!(world.revision(), world_revision + 1);
    assert_eq!(state.get::<CombatState>(combat_key).unwrap(), combat);
    assert_eq!(
        state.get::<WorldEventQueue>(event_key).unwrap().pending(),
        1
    );

    let checkpoint = state.checkpoint().unwrap();
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<CombatState>(combat_schema, 1, 8192).unwrap())
        .unwrap();
    schemas
        .register(Schema::of::<WorldEventQueue>(event_schema, 1, 4096).unwrap())
        .unwrap();
    let restored = StateStore::restore(&checkpoint, scope, schemas, store_limits).unwrap();
    let restored_combat = restored.get::<CombatState>(combat_key).unwrap();
    assert_eq!(restored_combat, combat);
    assert_eq!(
        restored
            .get::<WorldEventQueue>(event_key)
            .unwrap()
            .pending(),
        1
    );
    assert!(
        restored_combat
            .prepare_hit(hit, &immunity, &resolver, &catalog, combat_limits)
            .unwrap()
            .is_none()
    );
}
