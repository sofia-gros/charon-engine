//! E07剛体APIを外部crate相当の公開面から検査します。

use charon_engine::foundation::{
    StableKey,
    codec::{Decoder, Encoder, StateCodec},
    grid::{CellPoint, GridLayout, GridLimits, GridScale},
    id::WorldId,
    schema::Schema,
};
use charon_engine::world::{
    BodyBlockAttachmentParticleization, BodyBlockAttachmentParticleizationProvider,
    BodyBlockAttachmentSplitProvider, BodyBoundaryContact, BodyBoundarySegment,
    BodyCellParticleTransfer, BodyCellReturn, BodyCellReturnConfig, BodyCellSidecar,
    BodyCellTransfer, BodyCellTransferConfig, BodyComponentSearchJob, BodyComponentSearchStatus,
    BodyComponentTransferOutcome, BodyContactEvent, BodyContactExitReason, BodyContactPhase,
    BodyContactTarget, BodyCouplingContext, BodyCouplingProviderStamp, BodyCouplingResponse,
    BodyDamageConfig, BodyDef, BodyFluidContactSample, BodyFluidOccupancySample,
    BodyLocalBondProvider, BodyLocalCell, BodyLocalGrid, BodyLocalGridInventoryProvider,
    BodyLocalGridLimits, BodyMaterialComponent, BodyMotionType, BodyParticleReattachment,
    BodyPhysics, BodyPhysicsSettings, BodyPhysicsStepContext, BodyPhysicsStepProvider, BodyPose,
    BodyReactionParticleOutputConfig, BodyReactionSchedule, BodyReactionScheduleState, BodyState,
    BodyStencil, BodyTeleportOptions, BodyTeleportOverlapPolicy, BodyTeleportPathPolicy,
    BodyTeleportQueryOutcome, BodyTeleportQueryProvider, BodyThermalBoundaryEdgeState,
    BodyThermalBoundaryFace, BodyThermalBoundaryProvider, BodyThermalReactionBoundary,
    BodyVelocity, CellCore, ChannelDefinition, ChunkCoord, ChunkRegion, ColliderShape,
    CollisionQueriesProvider, CollisionQueryGeometry, CollisionQueryHit, CollisionQueryRequest,
    CollisionQueryResult, CollisionQueryShapeRef, CollisionQueryStatus, CollisionQueryTarget,
    ContentRegistry, CouplingProvider, EnthalpyBand, EnthalpyFamily, JointDef, JointKind, LayerDef,
    LayerId, LayerRole, LeaseMode, MaterialBase, MaterialBuilder, MaterialHandle,
    MaterialInventoryLimits, MaterialInventoryProfile, MaterialMotion, MaterialMotionProperties,
    MaterialParticleHandle, MaterialQuantity, MaterialRemap, MixtureLimits, MixtureRef,
    MixtureStore, ReactionChainPolicy, ReactionJob, ReactionRule, ReactionRuleSet, ReactionSidecar,
    ReactionStateTransfer, RegionLease, SameMaterialBodyBondProvider,
    SameMaterialBodyLocalBondProvider, SaveDurability, ThermalCellState, ThermalStepConfig, World,
    WorldBounds, WorldLimits,
};
use std::collections::BTreeMap;
use std::sync::Arc;

struct EmptyArchives;

struct SeparateCellBonds;

struct TransformBlockPropertiesOnSplit;
struct InvalidBlockMaskOnSplit;
struct ParticleizeBlockState;
struct PortalTeleportQueries;
struct StableCollisionQueries {
    malformed_result: bool,
    status: CollisionQueryStatus,
    truncated: bool,
}
struct DeterministicBodyStep {
    fail_after_edit: bool,
    version: u32,
}
struct CollisionDrivenBodyStep {
    layer: LayerId,
}

struct ContactReportingBodyStep {
    exceed_capacity: bool,
    event_substep: u32,
    custom_target: bool,
}
struct CandidateAwareCollisionQueries;
struct ConservingFluidCoupling {
    fail_after_edit: bool,
    invalid_balance: bool,
    version: u32,
    custom_contact: Option<BodyFluidContactSample>,
    custom_occupancy: Option<Vec<BodyFluidOccupancySample>>,
}

struct CustomThermalBoundary {
    invalid_local_cell: bool,
}

impl BodyThermalBoundaryProvider for CustomThermalBoundary {
    fn boundary_contacts(
        &self,
        _body: charon_engine::foundation::id::BodyId,
        _grid: &BodyLocalGrid,
        _pose: BodyPose,
        _world_units_per_cell: f64,
        _max_contacts: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<BodyBoundaryContact>> {
        Ok(vec![BodyBoundaryContact {
            local_cell: if self.invalid_local_cell {
                [u32::MAX, u32::MAX]
            } else {
                [0, 0]
            },
            face: BodyThermalBoundaryFace::Right,
            world_cell: point(5, 4),
            world_point: [5.0, 4.5],
            covered_length: 0.5,
        }])
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ProviderCounter(u64);

impl StateCodec for ProviderCounter {
    fn encode(&self, out: &mut Encoder) -> charon_engine::foundation::EngineResult<()> {
        out.u64(self.0)
    }

    fn decode(input: &mut Decoder<'_>) -> charon_engine::foundation::EngineResult<Self> {
        Ok(Self(input.u64()?))
    }
}

impl BodyPhysicsStepProvider for DeterministicBodyStep {
    fn key(&self) -> StableKey {
        StableKey::parse("test:deterministic-body-step").unwrap()
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn step(
        &self,
        physics_candidate: &mut BodyPhysics,
        _world: &World,
        context: BodyPhysicsStepContext,
    ) -> charon_engine::foundation::EngineResult<u32> {
        assert_eq!(context.fixed_dt_seconds, 1.0 / 60.0);
        assert_eq!(context.step_index, physics_candidate.step_index() + 1);
        assert!(context.max_substeps > 0);
        assert_eq!(context.world_units_per_cell, 1.0);
        assert!(context.provider_state_byte_limit > 0);
        let counter = physics_candidate
            .provider_state::<ProviderCounter>()?
            .unwrap_or_default();
        physics_candidate.set_provider_state(&ProviderCounter(counter.0 + 1))?;
        let body = physics_candidate.body_ids().next().ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "test body missing",
            )
        })?;
        let mut state = physics_candidate.state(body)?;
        state.pose.x += 1.0;
        physics_candidate.set_pose(body, state.pose)?;
        if self.fail_after_edit {
            return Err(charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "test provider failure",
            ));
        }
        Ok(1)
    }
}

impl BodyPhysicsStepProvider for CollisionDrivenBodyStep {
    fn key(&self) -> StableKey {
        StableKey::parse("test:collision-driven-step").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn step(
        &self,
        physics_candidate: &mut BodyPhysics,
        world: &World,
        context: BodyPhysicsStepContext,
    ) -> charon_engine::foundation::EngineResult<u32> {
        let body = physics_candidate.body_ids().next().ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "query step body missing",
            )
        })?;
        let mut state = physics_candidate.state(body)?;
        state.pose.x += 1.0;
        physics_candidate.set_pose(body, state.pose)?;
        let start = physics_candidate.state(body)?.pose;
        let end = BodyPose {
            x: start.x + 2.0,
            ..start
        };
        let request = CollisionQueryRequest {
            geometry: CollisionQueryGeometry::CustomSweep {
                shape: CollisionQueryShapeRef {
                    provider: StableKey::parse("test:custom-beam-shape")?,
                    version: 1,
                    owner: 2,
                    object: 5,
                    part: 1,
                },
                start,
                end,
            },
            layer: self.layer,
            collision_groups: u32::MAX,
            owner: None,
            ignore_bodies: vec![body],
            one_way_direction: Some([1.0, 0.0]),
            include_sensors: false,
            include_initial_overlap: true,
            include_boundary: true,
            epsilon: 0.0,
            tick: context.step_index,
            substep: 0,
            read_latest_candidate: true,
            first_hit_only: true,
            stable_order: true,
            max_hits: 1,
        };
        let report =
            physics_candidate.query_collisions(world, &request, &CandidateAwareCollisionQueries)?;
        let hit = report.result.hits.first().ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "candidate sweep did not return a hit",
            )
        })?;
        state.pose.x = start.x + (end.x - start.x) * hit.time_of_impact;
        physics_candidate.set_pose(body, state.pose)?;
        let pre_velocity = physics_candidate.state(body)?.velocity;
        let mass = physics_candidate.mass_properties(body)?.mass;
        let impulse = [hit.normal[0] * mass * pre_velocity.x, 0.0];
        let post_velocity = BodyVelocity {
            x: pre_velocity.x + impulse[0] / mass,
            y: pre_velocity.y,
            angular: pre_velocity.angular,
        };
        physics_candidate.set_velocity(body, post_velocity)?;
        physics_candidate.set_provider_contact_events(&[BodyContactEvent {
            first: BodyContactTarget::Body(body),
            second: BodyContactTarget::Custom {
                owner: 9,
                object: 3,
                part: 0,
            },
            phase: BodyContactPhase::Enter,
            sensor: false,
            exit_reason: None,
            point: hit.point,
            normal: [-hit.normal[0], -hit.normal[1]],
            impulse,
            first_pre_velocity: Some(pre_velocity),
            first_post_velocity: Some(post_velocity),
            second_pre_velocity: None,
            second_post_velocity: None,
            substep: 1,
        }])?;
        Ok(1)
    }
}

impl BodyPhysicsStepProvider for ContactReportingBodyStep {
    fn key(&self) -> StableKey {
        StableKey::parse("test:contact-reporting-step").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn step(
        &self,
        physics_candidate: &mut BodyPhysics,
        _world: &World,
        _context: BodyPhysicsStepContext,
    ) -> charon_engine::foundation::EngineResult<u32> {
        let (first, second) = {
            let mut bodies = physics_candidate.body_ids();
            let first = bodies.next().ok_or_else(|| {
                charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::InvalidState,
                    "contact testのfirst body missing",
                )
            })?;
            let second = bodies.next().ok_or_else(|| {
                charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::InvalidState,
                    "contact testのsecond body missing",
                )
            })?;
            (first, second)
        };
        let mut second_pose = physics_candidate.state(second)?.pose;
        second_pose.x = physics_candidate.state(first)?.pose.x + 1.0;
        physics_candidate.set_pose(second, second_pose)?;
        let first_velocity = BodyVelocity {
            x: 1.0,
            y: 0.0,
            angular: 0.0,
        };
        let second_velocity = BodyVelocity {
            x: -1.0,
            y: 0.0,
            angular: 0.0,
        };
        physics_candidate.set_velocity(first, first_velocity)?;
        physics_candidate.set_velocity(second, second_velocity)?;
        let second_target = if self.custom_target {
            BodyContactTarget::Custom {
                owner: 7,
                object: 4,
                part: 2,
            }
        } else {
            BodyContactTarget::Body(second)
        };
        let event = BodyContactEvent {
            first: BodyContactTarget::Body(first),
            second: second_target,
            phase: BodyContactPhase::Enter,
            sensor: false,
            exit_reason: None,
            point: [physics_candidate.state(first)?.pose.x + 0.5, 10.0],
            normal: [1.0, 0.0],
            impulse: [1.0, 0.0],
            first_pre_velocity: Some(BodyVelocity::default()),
            first_post_velocity: Some(first_velocity),
            second_pre_velocity: (!self.custom_target).then_some(BodyVelocity::default()),
            second_post_velocity: (!self.custom_target).then_some(second_velocity),
            substep: self.event_substep,
        };
        if self.exceed_capacity {
            let terrain_event = BodyContactEvent {
                first: BodyContactTarget::Body(first),
                second: BodyContactTarget::Terrain(ChunkCoord::new(0, 0)?),
                phase: BodyContactPhase::Enter,
                sensor: false,
                exit_reason: None,
                point: [physics_candidate.state(first)?.pose.x, 10.0],
                normal: [0.0, 1.0],
                impulse: [0.0, 1.0],
                first_pre_velocity: Some(BodyVelocity::default()),
                first_post_velocity: Some(first_velocity),
                second_pre_velocity: None,
                second_post_velocity: None,
                substep: self.event_substep,
            };
            physics_candidate.set_provider_contact_events(&[event, terrain_event])?;
        } else {
            physics_candidate.set_provider_contact_events(&[event])?;
        }
        Ok(1)
    }
}

impl CollisionQueriesProvider for CandidateAwareCollisionQueries {
    fn key(&self) -> StableKey {
        StableKey::parse("test:candidate-aware-collision-query").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn query(
        &self,
        _world: &World,
        physics: Option<&BodyPhysics>,
        request: &CollisionQueryRequest,
    ) -> charon_engine::foundation::EngineResult<CollisionQueryResult> {
        let CollisionQueryGeometry::CustomSweep { shape, start, .. } = request.geometry else {
            return Err(charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidValue,
                "candidate query expected custom sweep",
            ));
        };
        assert_eq!(
            shape.provider,
            StableKey::parse("test:custom-beam-shape").unwrap()
        );
        assert_eq!(
            (shape.version, shape.owner, shape.object, shape.part),
            (1, 2, 5, 1)
        );
        let candidate = physics.ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "candidate physics missing",
            )
        })?;
        let body = candidate.body_ids().next().ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "candidate body missing",
            )
        })?;
        assert_eq!(candidate.state(body)?.pose, start);
        assert_eq!(request.ignore_bodies, [body]);
        assert_eq!(request.one_way_direction, Some([1.0, 0.0]));
        assert!(request.include_initial_overlap);
        assert!(request.first_hit_only);
        assert!(request.stable_order);
        assert_eq!((request.tick, request.substep), (1, 0));
        assert_eq!(request.max_hits, 1);
        Ok(CollisionQueryResult {
            status: CollisionQueryStatus::Hit,
            hits: vec![CollisionQueryHit {
                target: CollisionQueryTarget::Custom {
                    owner: 9,
                    object: 3,
                    part: 0,
                },
                material: None,
                point: [start.x + 1.0, start.y],
                normal: [-1.0, 0.0],
                time_of_impact: 0.5,
                geometry_revision: 5,
                coverage: 1.0,
            }],
            truncated: false,
            geometry_revision: 5,
        })
    }
}

impl CouplingProvider for ConservingFluidCoupling {
    fn key(&self) -> StableKey {
        StableKey::parse("test:conserving-fluid-coupling").unwrap()
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn fluid_contacts(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_contacts: usize,
    ) -> charon_engine::foundation::EngineResult<Option<Vec<BodyFluidContactSample>>> {
        Ok(self.custom_contact.map(|contact| vec![contact]))
    }

    fn fluid_occupancy(
        &self,
        _physics_candidate: &BodyPhysics,
        _world: &World,
        _layer: LayerId,
        _world_units_per_cell: f64,
        _max_cells: usize,
    ) -> charon_engine::foundation::EngineResult<Option<Vec<BodyFluidOccupancySample>>> {
        Ok(self.custom_occupancy.clone())
    }

    fn couple(
        &self,
        physics_candidate: &mut BodyPhysics,
        _world: &World,
        context: BodyCouplingContext,
    ) -> charon_engine::foundation::EngineResult<BodyCouplingResponse> {
        assert!(context.fixed_dt_seconds > 0.0);
        assert!(context.coverage > 0.0);
        let counter = physics_candidate
            .coupling_provider_state::<ProviderCounter>()?
            .unwrap_or_default();
        physics_candidate.set_coupling_provider_state(&ProviderCounter(counter.0 + 1))?;
        if self.fail_after_edit {
            return Err(charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::InvalidState,
                "test coupling provider failure",
            ));
        }
        let delta = [1024_i64, 0_i64];
        let fluid_scale = f64::from(context.fluid_mass) * context.world_units_per_cell
            / (context.fixed_dt_seconds * 65536.0);
        Ok(BodyCouplingResponse {
            fluid_velocity_delta_q16: delta,
            body_impulse: if self.invalid_balance {
                [0.0, 0.0]
            } else {
                [-fluid_scale * delta[0] as f64, 0.0]
            },
            external_impulse: [0.0, 0.0],
        })
    }
}

impl BodyTeleportQueryProvider for PortalTeleportQueries {
    fn key(&self) -> StableKey {
        StableKey::parse("test:portal-teleport-queries").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn query(
        &self,
        _world: &World,
        physics_candidate: &BodyPhysics,
        body: charon_engine::foundation::id::BodyId,
        previous: BodyPose,
        destination: BodyPose,
        options: BodyTeleportOptions,
    ) -> charon_engine::foundation::EngineResult<BodyTeleportQueryOutcome> {
        let candidate = physics_candidate.state(body)?;
        if options.path == BodyTeleportPathPolicy::Check
            && previous.x < 16.0
            && destination.x > 19.0
            && candidate.pose == destination
        {
            // このgameのportalは途中にあるsolid wallを別空間へ抜けて到着させます。
            Ok(BodyTeleportQueryOutcome::Clear)
        } else {
            Ok(BodyTeleportQueryOutcome::Blocked)
        }
    }
}

impl CollisionQueriesProvider for StableCollisionQueries {
    fn key(&self) -> StableKey {
        StableKey::parse("test:stable-collision-queries").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn query(
        &self,
        world: &World,
        physics: Option<&BodyPhysics>,
        request: &CollisionQueryRequest,
    ) -> charon_engine::foundation::EngineResult<CollisionQueryResult> {
        assert_eq!(
            physics.is_some(),
            matches!(
                &request.geometry,
                CollisionQueryGeometry::Sweep { .. }
                    | CollisionQueryGeometry::CustomSweep { .. }
                    | CollisionQueryGeometry::CustomOverlap { .. }
            )
        );
        assert!(request.read_latest_candidate);
        assert!(matches!(request.max_hits, 3 | 4));
        if let CollisionQueryGeometry::CustomOverlap { shape, .. } = &request.geometry {
            assert_eq!(
                shape.provider,
                StableKey::parse("test:custom-beam-shape").unwrap()
            );
            assert_eq!(
                (shape.version, shape.owner, shape.object, shape.part),
                (1, 2, 5, 1)
            );
        }
        if matches!(
            &request.geometry,
            CollisionQueryGeometry::Sweep { .. } | CollisionQueryGeometry::CustomSweep { .. }
        ) {
            assert_eq!(request.one_way_direction, Some([0.0, 1.0]));
            assert!(request.include_initial_overlap);
            assert!(request.include_boundary);
            assert_eq!((request.tick, request.substep), (21, 2));
            assert!(!request.first_hit_only);
            assert!(request.stable_order);
        }
        let stone = Some(
            world
                .content()
                .material_id(MaterialHandle(StableKey::parse("test:stone")?))?,
        );
        let time_of_impact = if matches!(
            request.geometry,
            CollisionQueryGeometry::Overlap { .. } | CollisionQueryGeometry::CustomOverlap { .. }
        ) {
            0.0
        } else {
            0.25
        };
        let hits = vec![
            CollisionQueryHit {
                target: CollisionQueryTarget::Terrain {
                    cell: CellPoint { x: 9, y: 4 },
                },
                material: None,
                point: [2.0, 3.0],
                normal: [0.0, 1.0],
                time_of_impact: time_of_impact * 3.0,
                geometry_revision: 7,
                coverage: 1.0,
            },
            CollisionQueryHit {
                target: CollisionQueryTarget::Terrain {
                    cell: CellPoint { x: 2, y: 4 },
                },
                material: stone,
                point: [1.0, 3.0],
                normal: [1.0, 0.0],
                time_of_impact,
                geometry_revision: 7,
                coverage: 0.5,
            },
            CollisionQueryHit {
                target: CollisionQueryTarget::Custom {
                    owner: 3,
                    object: 8,
                    part: 1,
                },
                material: None,
                point: [1.5, 3.0],
                normal: [1.0, 0.0],
                time_of_impact,
                geometry_revision: 7,
                coverage: 1.0,
            },
        ];
        let status = if self.malformed_result {
            CollisionQueryStatus::NoHit
        } else {
            self.status
        };
        let hits = match status {
            CollisionQueryStatus::Hit => hits,
            CollisionQueryStatus::Incomplete => hits.into_iter().take(1).collect(),
            CollisionQueryStatus::NoHit | CollisionQueryStatus::Unsupported => Vec::new(),
        };
        let hits = if self.malformed_result {
            vec![CollisionQueryHit {
                target: CollisionQueryTarget::Terrain {
                    cell: CellPoint { x: 2, y: 4 },
                },
                material: None,
                point: [1.0, 3.0],
                normal: [1.0, 0.0],
                time_of_impact,
                geometry_revision: 7,
                coverage: 1.0,
            }]
        } else {
            hits
        };
        Ok(CollisionQueryResult {
            status,
            hits,
            truncated: self.truncated,
            geometry_revision: 7,
        })
    }
}

impl BodyBlockAttachmentParticleizationProvider for ParticleizeBlockState {
    fn key(&self) -> StableKey {
        StableKey::parse("test:particleize-block-state").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn particleize(
        &self,
        original: &charon_engine::world::BlockInstance,
        selected_mask_indices: &[u32],
        remaining_mask_indices: &[u32],
        _max_encoded_bytes: usize,
    ) -> charon_engine::foundation::EngineResult<BodyBlockAttachmentParticleization> {
        let value = original
            .properties()
            .ok_or_else(|| {
                charon_engine::foundation::EngineError::new(
                    charon_engine::foundation::ErrorCode::MissingKey,
                    "block properties",
                )
            })?
            .get::<u8>(StableKey::parse("test:block-state").unwrap())?;
        let selected_sidecars = selected_mask_indices
            .iter()
            .map(|mask| {
                Ok((
                    *mask,
                    vec![BodyCellSidecar::new(
                        StableKey::parse("test:heat").unwrap(),
                        1,
                        vec![value.saturating_add(*mask as u8)],
                    )?],
                ))
            })
            .collect::<charon_engine::foundation::EngineResult<Vec<_>>>()?;
        let remaining = if remaining_mask_indices.is_empty() {
            None
        } else {
            let mut properties = original
                .properties()
                .ok_or_else(|| {
                    charon_engine::foundation::EngineError::new(
                        charon_engine::foundation::ErrorCode::MissingKey,
                        "block properties",
                    )
                })?
                .fork()?;
            let state_key = StableKey::parse("test:block-state").unwrap();
            let mut transaction = properties.transaction()?;
            transaction.put(state_key, state_key, &99_u8)?;
            transaction.commit()?;
            Some(original.with_fragment_state(remaining_mask_indices.to_vec(), Some(properties))?)
        };
        BodyBlockAttachmentParticleization::new(selected_sidecars, remaining)
    }
}

impl BodyLocalBondProvider for SeparateCellBonds {
    fn joins(
        &self,
        _from: [u32; 2],
        _to: [u32; 2],
        _from_cell: &BodyLocalCell,
        _to_cell: &BodyLocalCell,
    ) -> charon_engine::foundation::EngineResult<bool> {
        Ok(false)
    }
}

impl BodyBlockAttachmentSplitProvider for TransformBlockPropertiesOnSplit {
    fn key(&self) -> StableKey {
        StableKey::parse("test:transform-block-properties-on-split").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn split(
        &self,
        original: &charon_engine::world::BlockInstance,
        fragment_masks: &[Vec<u32>],
        _max_encoded_bytes: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<charon_engine::world::BlockInstance>> {
        fragment_masks
            .iter()
            .enumerate()
            .map(|(index, mask)| {
                let state_key = StableKey::parse("test:block-state").unwrap();
                let mut properties = original
                    .properties()
                    .ok_or_else(|| {
                        charon_engine::foundation::EngineError::new(
                            charon_engine::foundation::ErrorCode::MissingKey,
                            "block properties",
                        )
                    })?
                    .fork()?;
                let mut transaction = properties.transaction()?;
                transaction.put(state_key, state_key, &(73_u8 + index as u8))?;
                transaction.commit()?;
                original.with_fragment_state(mask.clone(), Some(properties))
            })
            .collect()
    }
}

impl BodyBlockAttachmentSplitProvider for InvalidBlockMaskOnSplit {
    fn key(&self) -> StableKey {
        StableKey::parse("test:invalid-block-mask-on-split").unwrap()
    }

    fn version(&self) -> u32 {
        1
    }

    fn split(
        &self,
        original: &charon_engine::world::BlockInstance,
        fragment_masks: &[Vec<u32>],
        _max_encoded_bytes: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<charon_engine::world::BlockInstance>> {
        fragment_masks
            .iter()
            .map(|_| original.with_fragment_state(original.remaining_mask().to_vec(), None))
            .collect()
    }
}

impl charon_engine::world::ArchiveStorage for EmptyArchives {
    fn put(
        &mut self,
        _hash: [u8; 32],
        _bytes: &[u8],
    ) -> charon_engine::foundation::EngineResult<SaveDurability> {
        Ok(SaveDurability::FilesAndDirectories)
    }

    fn read(
        &mut self,
        _hash: [u8; 32],
        _limit: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<u8>> {
        Err(charon_engine::foundation::EngineError::new(
            charon_engine::foundation::ErrorCode::MissingKey,
            "unexpected archive read",
        ))
    }
}

#[derive(Default)]
struct MemoryArchives(BTreeMap<[u8; 32], Vec<u8>>);

impl charon_engine::world::ArchiveStorage for MemoryArchives {
    fn put(
        &mut self,
        hash: [u8; 32],
        bytes: &[u8],
    ) -> charon_engine::foundation::EngineResult<SaveDurability> {
        self.0.insert(hash, bytes.to_vec());
        Ok(SaveDurability::FilesAndDirectories)
    }

    fn read(
        &mut self,
        hash: [u8; 32],
        limit: usize,
    ) -> charon_engine::foundation::EngineResult<Vec<u8>> {
        let bytes = self.0.get(&hash).ok_or_else(|| {
            charon_engine::foundation::EngineError::new(
                charon_engine::foundation::ErrorCode::MissingKey,
                "archive hashがありません",
            )
        })?;
        if bytes.len() > limit {
            return Err(charon_engine::foundation::EngineError::capacity(
                "archive read bytes",
                bytes.len() as u64,
                limit as u64,
            ));
        }
        Ok(bytes.clone())
    }
}

fn terrain_world() -> (World, LayerId, MaterialHandle) {
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("test:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let stone = MaterialHandle(StableKey::parse("test:stone").unwrap());
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:dense", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:grain", MaterialBase::Solid).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    (world, layer, stone)
}

#[test]
fn body_stencil_boundary_segments_keep_outer_and_hole_faces_exact_after_rotation() {
    let stencil = BodyStencil::new(
        3,
        3,
        2.0,
        [0.0, 0.0],
        1,
        vec![true, true, true, true, false, true, true, true, true],
    )
    .unwrap();
    let pose = BodyPose {
        x: 10.0,
        y: 20.0,
        angle: std::f64::consts::FRAC_PI_2,
    };

    let segments: Vec<BodyBoundarySegment> = stencil.boundary_segments(pose).unwrap();
    assert_eq!(segments.len(), 16);
    assert_eq!(segments, stencil.boundary_segments(pose).unwrap());
    assert_eq!(segments[0].local_cell, [0, 0]);
    assert_eq!(segments[0].face, BodyThermalBoundaryFace::Left);
    assert_eq!(segments[0].start, [10.0, 20.0]);
    assert_eq!(segments[0].end, [8.0, 20.0]);
    assert!(segments.iter().all(|segment| {
        let dx = segment.end[0] - segment.start[0];
        let dy = segment.end[1] - segment.start[1];
        (dx.hypot(dy) - 2.0).abs() < 1e-12
    }));
    assert!(segments.iter().any(|segment| {
        segment.local_cell == [0, 1] && segment.face == BodyThermalBoundaryFace::Right
    }));
    assert!(!segments.iter().any(|segment| {
        (segment.local_cell == [0, 0] && segment.face == BodyThermalBoundaryFace::Right)
            || (segment.local_cell == [1, 0] && segment.face == BodyThermalBoundaryFace::Left)
    }));
}

#[test]
fn body_stencil_boundary_contacts_partition_coverage_without_losing_length() {
    let stencil = BodyStencil::new(1, 1, 2.0, [0.0, 0.0], 1, vec![true]).unwrap();
    let aligned = stencil
        .boundary_contacts(BodyPose::default(), 1.0, 8)
        .unwrap();
    assert_eq!(aligned.len(), 8);
    assert_eq!(
        aligned
            .iter()
            .map(|contact| contact.covered_length)
            .sum::<f64>(),
        8.0
    );
    assert!(aligned.iter().any(|contact| {
        contact.face == BodyThermalBoundaryFace::Left
            && contact.world_cell == CellPoint { x: -1, y: 0 }
    }));
    assert!(aligned.iter().any(|contact| {
        contact.face == BodyThermalBoundaryFace::Right
            && contact.world_cell == CellPoint { x: 2, y: 1 }
    }));
    let pose = BodyPose {
        x: 1.0,
        y: 1.0,
        angle: std::f64::consts::FRAC_PI_4,
    };
    let contacts: Vec<BodyBoundaryContact> = stencil.boundary_contacts(pose, 1.0, 32).unwrap();
    assert!(contacts.len() > 4);
    assert_eq!(contacts, stencil.boundary_contacts(pose, 1.0, 32).unwrap());
    assert!(
        (contacts
            .iter()
            .map(|contact| contact.covered_length)
            .sum::<f64>()
            - 8.0)
            .abs()
            < 1e-10
    );
    assert!(contacts.iter().all(|contact| {
        contact.covered_length.is_finite()
            && contact.covered_length > 0.0
            && contact.world_cell.x.abs() < 10
            && contact.world_cell.y.abs() < 10
    }));
    assert!(
        stencil
            .boundary_contacts(pose, 1.0, contacts.len() - 1)
            .is_err()
    );
    assert!(stencil.boundary_contacts(pose, 0.0, 32).is_err());
}

#[test]
fn body_stencil_volume_contacts_preserve_exact_area_after_rotation() {
    let stencil = BodyStencil::new(2, 1, 2.0, [0.0, 0.0], 1, vec![true, true]).unwrap();
    let aligned = stencil
        .volume_contacts(BodyPose::default(), 1.0, 8)
        .unwrap();
    assert_eq!(aligned.len(), 8);
    assert!(
        (aligned
            .iter()
            .map(|contact| contact.covered_area)
            .sum::<f64>()
            - 8.0)
            .abs()
            < 1e-12
    );
    assert!(aligned.iter().all(|contact| {
        (contact.covered_area - 1.0).abs() < 1e-12
            && contact.world_point.iter().all(|value| value.is_finite())
    }));
    assert_eq!(
        aligned,
        stencil
            .volume_contacts(BodyPose::default(), 1.0, 8)
            .unwrap()
    );

    let rotated = stencil
        .volume_contacts(
            BodyPose {
                x: 4.25,
                y: -1.75,
                angle: std::f64::consts::FRAC_PI_4,
            },
            1.0,
            32,
        )
        .unwrap();
    assert!(rotated.len() > aligned.len());
    assert!(
        (rotated
            .iter()
            .map(|contact| contact.covered_area)
            .sum::<f64>()
            - 8.0)
            .abs()
            < 1e-10
    );
    assert!(rotated.iter().all(|contact| {
        contact.covered_area > 0.0
            && contact.covered_area <= 1.0 + 1e-12
            && contact.world_point.iter().all(|value| value.is_finite())
    }));
    assert_eq!(
        rotated,
        stencil
            .volume_contacts(
                BodyPose {
                    x: 4.25,
                    y: -1.75,
                    angle: std::f64::consts::FRAC_PI_4,
                },
                1.0,
                32,
            )
            .unwrap()
    );
    assert!(
        stencil
            .volume_contacts(BodyPose::default(), 1.0, 7)
            .is_err()
    );
}

fn world_with_heat_channel() -> World {
    world_with_heat_channel_limits(WorldLimits::default())
}

fn world_with_heat_channel_limits(limits: WorldLimits) -> World {
    world_with_heat_channel_and_motion_limits(limits, false)
}

fn world_with_heat_channel_and_motion_limits(limits: WorldLimits, with_motion: bool) -> World {
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("test:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    let mut water = MaterialBuilder::new("test:water", MaterialBase::Liquid).unwrap();
    if with_motion {
        water = water.motion_properties(charon_engine::world::MaterialMotionProperties {
            drag_q16: 65536,
            ..charon_engine::world::MaterialMotionProperties::for_base(MaterialBase::Liquid)
        });
    }
    registry.register(water).unwrap();
    let heat = StableKey::parse("test:heat").unwrap();
    let thermal = StableKey::parse("test:thermal").unwrap();
    registry
        .schema(Schema::of::<u8>(heat, 1, 1).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<charon_engine::world::ThermalCellState>(thermal, 1, 8).unwrap())
        .unwrap();
    registry
        .material_channel(
            StableKey::parse("test:terrain").unwrap(),
            ChannelDefinition {
                key: heat,
                version: 1,
                value_schema: heat,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0u8,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    if with_motion {
        let motion = StableKey::parse("test:motion").unwrap();
        registry
            .schema(Schema::of::<charon_engine::world::MaterialMotion>(motion, 1, 24).unwrap())
            .unwrap();
        registry
            .material_channel(
                StableKey::parse("test:terrain").unwrap(),
                ChannelDefinition {
                    key: motion,
                    version: 1,
                    value_schema: motion,
                    layout: GridLayout {
                        origin: point(0, 0),
                        x_scale: GridScale::new(1, 1).unwrap(),
                        y_scale: GridScale::new(1, 1).unwrap(),
                    },
                    default: charon_engine::world::MaterialMotion::default(),
                    limits: GridLimits {
                        chunks: 8,
                        checkpoint_bytes: 1024,
                    },
                },
            )
            .unwrap();
    }
    registry
        .material_channel(
            StableKey::parse("test:terrain").unwrap(),
            ChannelDefinition {
                key: thermal,
                version: 1,
                value_schema: thermal,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: charon_engine::world::ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        limits,
    )
    .unwrap()
}

fn stationary_particle_body_fixture(
    velocity_q16: [i64; 2],
) -> (
    World,
    LayerId,
    charon_engine::foundation::id::BodyId,
    MaterialParticleHandle,
    CellPoint,
) {
    let mut world = world_with_heat_channel();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(stone_key))
        .unwrap();
    let point = point(4, 4);
    let particle = world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point,
                CellCore {
                    material: stone,
                    flags: 2,
                    variation: 9,
                },
            )?;
            edit.set_channel(StableKey::parse("test:heat")?, point, 42_u8)?;
            edit.emit_material_particle(layer, point, velocity_q16)
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    (world, layer, body, particle, point)
}

fn fluid_exclusion_body_fixture(
    reservoir_capacity: u32,
    two_occupied_cells: bool,
) -> (
    World,
    LayerId,
    charon_engine::foundation::id::BodyId,
    CellPoint,
) {
    fluid_exclusion_body_fixture_with_limits(
        reservoir_capacity,
        two_occupied_cells,
        WorldLimits::default(),
    )
}

fn fluid_exclusion_body_fixture_with_limits(
    reservoir_capacity: u32,
    two_occupied_cells: bool,
    limits: WorldLimits,
) -> (
    World,
    LayerId,
    charon_engine::foundation::id::BodyId,
    CellPoint,
) {
    fluid_exclusion_body_fixture_with_settings(
        reservoir_capacity,
        two_occupied_cells,
        limits,
        BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        },
    )
}

fn fluid_exclusion_body_fixture_with_settings(
    reservoir_capacity: u32,
    two_occupied_cells: bool,
    limits: WorldLimits,
    physics_settings: BodyPhysicsSettings,
) -> (
    World,
    LayerId,
    charon_engine::foundation::id::BodyId,
    CellPoint,
) {
    fluid_exclusion_body_fixture_with_angle(
        reservoir_capacity,
        two_occupied_cells,
        limits,
        physics_settings,
        0.0,
    )
}

fn fluid_exclusion_body_fixture_with_angle(
    reservoir_capacity: u32,
    two_occupied_cells: bool,
    limits: WorldLimits,
    physics_settings: BodyPhysicsSettings,
    angle: f64,
) -> (
    World,
    LayerId,
    charon_engine::foundation::id::BodyId,
    CellPoint,
) {
    let mut world = world_with_heat_channel_and_motion_limits(limits, true);
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let source = point(4, 4);
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                source,
                CellCore {
                    material: water,
                    flags: 4,
                    variation: 5,
                },
            )?;
            edit.set_channel(StableKey::parse("test:heat")?, source, 42_u8)?;
            if two_occupied_cells {
                edit.set_cell(
                    layer,
                    point(5, 4),
                    CellCore {
                        material: water,
                        flags: 6,
                        variation: 7,
                    },
                )?;
                edit.set_channel(StableKey::parse("test:heat")?, point(5, 4), 43_u8)?;
            }
            Ok(())
        })
        .unwrap();
    world.configure_body_physics(physics_settings).unwrap();
    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: reservoir_capacity,
            geometry_cells: 2,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
    )
    .unwrap();
    if two_occupied_cells {
        grid.set_cell(
            1,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.gravity_scale = 0.0;
    definition.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle,
    };
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    (world, layer, body, source)
}

#[test]
fn body_exclusion_reservoir_roundtrips_payload_and_releases_atomically() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    world
        .edit(|edit| {
            edit.store_body_exclusion_reservoir_cell(
                layer,
                body,
                source,
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.cell_count(), 1);
    assert_eq!(grid.owned_cell_count(), 2);
    assert_eq!(grid.exclusion_reservoir().len(), 1);
    assert_eq!(
        grid.exclusion_reservoir()[0].cell().material().unwrap(),
        StableKey::parse("test:water").unwrap()
    );
    assert_eq!(grid.exclusion_reservoir()[0].cell().flags(), 4);
    assert_eq!(grid.exclusion_reservoir()[0].cell().variation(), 5);
    assert_eq!(
        grid.exclusion_reservoir()[0].cell().sidecars()[0].payload(),
        &[42]
    );

    let content = world.content().clone();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .exclusion_reservoir()
            .len(),
        1
    );
    let destination = point(5, 4);
    let report = restored
        .edit(|edit| {
            edit.release_body_exclusion_reservoir_cells(
                layer,
                body,
                &[charon_engine::world::BodyExclusionReservoirReturn {
                    origin: source,
                    destination,
                }],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(report.cells, 1);
    let water = restored
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    assert!(matches!(
        restored.cell(layer, destination).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == water && view.core.flags == 4 && view.core.variation == 5
    ));
    assert!(matches!(
        restored
            .channel_value::<u8>(StableKey::parse("test:heat").unwrap(), destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(42)
    ));
    assert!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .exclusion_reservoir()
            .is_empty()
    );
}

#[test]
fn body_step_automatically_releases_exclusion_reservoir_to_nearby_empty_cell() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    let thermal = StableKey::parse("test:thermal").unwrap();
    world
        .edit(|edit| {
            edit.set_channel(
                thermal,
                source,
                charon_engine::world::ThermalCellState { energy: 81 },
            )?;
            edit.store_body_exclusion_reservoir_cell(
                layer,
                body,
                source,
                BodyCellTransferConfig {
                    thermal_channel: Some(thermal),
                    ..BodyCellTransferConfig::default()
                },
            )
        })
        .unwrap();

    let content = world.content().clone();
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    world = World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();

    let report = world.step_bodies(layer, 1.0).unwrap();
    assert_eq!(report.step, 1);
    let destination = point(source.x, source.y + 1);
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    assert!(matches!(
        world.cell(layer, destination).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == water && view.core.flags == 4 && view.core.variation == 5
    ));
    assert!(matches!(
        world
            .channel_value::<u8>(StableKey::parse("test:heat").unwrap(), destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(42)
    ));
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::ThermalCellState>(thermal, destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(state) if state.energy == 81
    ));
    assert!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .exclusion_reservoir()
            .is_empty()
    );
}

#[test]
fn body_exclusion_reservoir_capacity_failure_keeps_world_and_body_unchanged() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(1, false);
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.store_body_exclusion_reservoir_cell(
                layer,
                body,
                source,
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

fn body_grid(
    width: u32,
    height: u32,
    pivot: [f64; 2],
    limits: BodyLocalGridLimits,
) -> BodyLocalGrid {
    BodyLocalGrid::new(width, height, 1.0, pivot, 0, limits).unwrap()
}

fn legacy_body_snapshot(current: &[u8], version: u32) -> Vec<u8> {
    fn remove_key(json: &mut String, key: &str) {
        let needle = format!("\"{key}\":");
        while let Some(start) = json.find(&needle) {
            let value_start = start + needle.len();
            let mut in_string = false;
            let mut escaped = false;
            let mut depth = 0_u32;
            let mut value_end = json.len();
            for (offset, byte) in json.as_bytes()[value_start..].iter().copied().enumerate() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                    continue;
                }
                match byte {
                    b'"' => in_string = true,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' if depth > 0 => depth -= 1,
                    b',' | b'}' if depth == 0 => {
                        value_end = value_start + offset;
                        break;
                    }
                    _ => {}
                }
            }
            if json.as_bytes().get(value_end) == Some(&b',') {
                json.replace_range(start..=value_end, "");
            } else {
                let comma = json[..start].rfind(',').expect("JSON field separator");
                json.replace_range(comma..value_end, "");
            }
        }
    }

    let mut payload_value: serde_json::Value = serde_json::from_slice(&current[16..]).unwrap();
    if payload_value["version"] == 12 {
        let backend_hex: String = serde_json::from_value(payload_value["backend"].clone()).unwrap();
        let (pairs, remainder) = backend_hex.as_bytes().as_chunks::<2>();
        assert!(remainder.is_empty());
        let backend_bytes: Vec<u8> = pairs
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let backend: rapier2d_f64::prelude::PhysicsWorld =
            bincode::deserialize(&backend_bytes).unwrap();
        payload_value["backend"] = serde_json::to_value(backend).unwrap();
    }
    if version < 12 {
        payload_value["backend_compatibility"] = serde_json::Value::from(
            "rapier2d-f64:0.36.1:dim2,f64,serde-serialize,enhanced-determinism",
        );
    }
    let mut payload = serde_json::to_string(&payload_value).unwrap();
    let version_start = payload.find("\"version\":").unwrap() + "\"version\":".len();
    let version_end = payload[version_start..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(payload.len(), |offset| version_start + offset);
    payload.replace_range(version_start..version_end, &version.to_string());
    for key in [
        "block_attachments",
        "next_block_attachment",
        "block_attachment",
        "block_mask_index",
    ] {
        remove_key(&mut payload, key);
    }
    if version < 11 {
        remove_key(&mut payload, "pending_teleport_exits");
    }
    if version < 6 {
        remove_key(&mut payload, "thermal_edges");
    }
    if version < 8 {
        remove_key(&mut payload, "thermal_boundary_edges");
    }
    if version < 9 {
        remove_key(&mut payload, "step_provider");
        remove_key(&mut payload, "provider_state");
    }
    if version < 10 {
        remove_key(&mut payload, "coupling_provider");
        remove_key(&mut payload, "coupling_state");
    }
    if version < 5 {
        remove_key(&mut payload, "exclusion_reservoir");
    }
    if version < 4 {
        remove_key(&mut payload, "remap_epoch");
    }
    let payload = payload.into_bytes();
    let mut snapshot = Vec::with_capacity(16 + payload.len());
    snapshot.extend_from_slice(format!("CHBODY{version:02}").as_bytes());
    snapshot.extend_from_slice(&version.to_le_bytes());
    snapshot.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    snapshot.extend_from_slice(&payload);
    snapshot
}

#[test]
fn body_exclusion_reservoir_batches_multiple_cells_in_one_transaction() {
    let (mut world, layer, body, first) = fluid_exclusion_body_fixture(4, true);
    let second = point(5, 4);
    let stored = world
        .edit(|edit| {
            edit.store_body_exclusion_reservoir_cells(
                layer,
                body,
                &[second, first],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(stored, 2);
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.exclusion_reservoir().len(), 2);
    assert_eq!(grid.exclusion_reservoir()[0].origin(), first);
    assert_eq!(grid.exclusion_reservoir()[1].origin(), second);

    let restored = world
        .edit(|edit| {
            edit.release_body_exclusion_reservoir_cells(
                layer,
                body,
                &[
                    charon_engine::world::BodyExclusionReservoirReturn {
                        origin: second,
                        destination: point(7, 4),
                    },
                    charon_engine::world::BodyExclusionReservoirReturn {
                        origin: first,
                        destination: point(6, 4),
                    },
                ],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(restored.cells, 2);
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    for (destination, flags, variation, heat) in [(point(6, 4), 4, 5, 42), (point(7, 4), 6, 7, 43)]
    {
        assert!(matches!(
            world.cell(layer, destination).unwrap(),
            charon_engine::world::CellQuery::Loaded(view)
                if view.core.material == water
                    && view.core.flags == flags
                    && view.core.variation == variation
        ));
        assert!(matches!(
            world
                .channel_value::<u8>(StableKey::parse("test:heat").unwrap(), destination)
                .unwrap(),
            charon_engine::world::ChannelQuery::Loaded(value) if value == heat
        ));
    }
    assert!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .exclusion_reservoir()
            .is_empty()
    );
}

#[test]
fn body_exclusion_reservoir_batch_capacity_failure_rolls_back_all_sources() {
    let (mut world, layer, body, first) = fluid_exclusion_body_fixture(3, true);
    let second = point(5, 4);
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.store_body_exclusion_reservoir_cells(
                layer,
                body,
                &[first, second],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::error::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        2
    );
    for source in [first, second] {
        assert!(matches!(
            world.cell(layer, source).unwrap(),
            charon_engine::world::CellQuery::Loaded(view)
                if view.core.material != charon_engine::world::MaterialId::AIR
        ));
    }
}

#[test]
fn body_step_pushes_overlapping_fluid_to_nearest_empty_cell_with_channels() {
    let (mut world, layer, _body, source) = fluid_exclusion_body_fixture(2, false);
    let thermal = StableKey::parse("test:thermal").unwrap();
    world
        .edit(|edit| {
            edit.set_channel(
                thermal,
                source,
                charon_engine::world::ThermalCellState { energy: 81 },
            )
        })
        .unwrap();
    let revision = world.revision();
    let report = world.step_bodies(layer, 1.0).unwrap();
    assert_eq!(report.step, 1);
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == charon_engine::world::MaterialId::AIR
    ));
    let destination = point(source.x, source.y + 1);
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    assert!(matches!(
        world.cell(layer, destination).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == water && view.core.flags == 4 && view.core.variation == 5
    ));
    assert!(matches!(
        world
            .channel_value::<u8>(StableKey::parse("test:heat").unwrap(), destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(42)
    ));
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::ThermalCellState>(thermal, destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(state) if state.energy == 81
    ));
    assert_eq!(world.revision(), revision + 1);
}

#[test]
fn body_step_exchanges_equal_and_opposite_fluid_drag_impulse() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    let motion_key = StableKey::parse("test:motion").unwrap();
    world
        .edit(|edit| {
            edit.set_channel(
                motion_key,
                source,
                charon_engine::world::MaterialMotion {
                    velocity_q16: [0, -65536],
                    remainder_q16: [0, 0],
                },
            )
        })
        .unwrap();

    world.step_bodies(layer, 1.0).unwrap();

    let destination = point(source.x, source.y + 1);
    assert!(matches!(
        world.channel_value::<charon_engine::world::MaterialMotion>(motion_key, destination).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(value) if value.velocity_q16 == [0, 0]
    ));
    let body_state = world.body_physics().unwrap().state(body).unwrap();
    let velocity = body_state.velocity;
    assert!(
        velocity.y < 0.0,
        "fluid drag should push the body upward: {velocity:?}"
    );
    let after_motion = match world
        .channel_value::<charon_engine::world::MaterialMotion>(motion_key, destination)
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(value) => value,
        _ => panic!("moved fluid motion must be loaded"),
    };
    let fluid = match world.cell(layer, destination).unwrap() {
        charon_engine::world::CellQuery::Loaded(view) => view.core,
        _ => panic!("moved fluid cell must be loaded"),
    };
    let fluid_mass = f64::from(
        world
            .content()
            .material(fluid.material)
            .unwrap()
            .unwrap()
            .mass_per_cell(),
    );
    let body_mass = world
        .body_physics()
        .unwrap()
        .mass_properties(body)
        .unwrap()
        .mass;
    let fluid_impulse = fluid_mass
        * f64::from(BodyPhysicsSettings::default().step_hz)
        * (after_motion.velocity_q16[1] - (-65536)) as f64
        / 65536.0;
    assert!(
        (body_mass * velocity.y + fluid_impulse).abs() < 1.0e-9,
        "body_mass={body_mass}, body_velocity={}, fluid_impulse={fluid_impulse}, fluid_q16={}",
        velocity.y,
        after_motion.velocity_q16[1]
    );
}

#[test]
fn standard_body_contacts_publish_enter_stay_and_exit_with_substep_data() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut fixed = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    fixed.motion = BodyMotionType::Fixed;
    let fixed_id = physics.insert(fixed).unwrap();
    let mut moving = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    moving.pose.x = 1.5;
    let moving_id = physics.insert(moving).unwrap();

    physics.step().unwrap();
    let entered = physics.contact_events();
    assert_eq!(entered.len(), 1);
    assert_eq!(entered[0].first, BodyContactTarget::Body(fixed_id));
    assert_eq!(entered[0].second, BodyContactTarget::Body(moving_id));
    assert_eq!(entered[0].phase, BodyContactPhase::Enter);
    assert_eq!(entered[0].substep, 1);
    assert!(entered[0].first_pre_velocity.is_some());
    assert!(entered[0].second_pre_velocity.is_some());
    assert!(entered[0].point.iter().all(|value| value.is_finite()));
    assert!(entered[0].normal.iter().all(|value| value.is_finite()));
    assert!(entered[0].impulse.iter().all(|value| value.is_finite()));

    physics.step().unwrap();
    assert_eq!(physics.contact_events().len(), 1);
    assert_eq!(physics.contact_events()[0].phase, BodyContactPhase::Stay);

    physics
        .set_pose(
            moving_id,
            BodyPose {
                x: 8.0,
                y: 0.0,
                angle: 0.0,
            },
        )
        .unwrap();
    physics.step().unwrap();
    assert_eq!(physics.contact_events().len(), 1);
    assert_eq!(physics.contact_events()[0].phase, BodyContactPhase::Exit);
    assert_eq!(
        physics.contact_events()[0].exit_reason,
        Some(BodyContactExitReason::Separated)
    );
    assert!(physics.contact_events()[0].second_post_velocity.is_some());
}

#[test]
fn standard_body_sensor_intersections_publish_enter_stay_and_exit() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut fixed = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    fixed.motion = BodyMotionType::Fixed;
    let fixed_id = physics.insert(fixed).unwrap();
    let mut sensor = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    sensor.pose.x = 1.5;
    sensor.collider.sensor = true;
    let sensor_id = physics.insert(sensor).unwrap();

    physics.step().unwrap();
    let entered = physics.contact_events();
    assert_eq!(entered.len(), 1);
    assert_eq!(entered[0].first, BodyContactTarget::Body(fixed_id));
    assert_eq!(entered[0].second, BodyContactTarget::Body(sensor_id));
    assert_eq!(entered[0].phase, BodyContactPhase::Enter);
    assert!(entered[0].sensor);
    assert_eq!(entered[0].impulse, [0.0, 0.0]);
    assert!(entered[0].point.iter().all(|value| value.is_finite()));
    assert!((entered[0].normal[0].hypot(entered[0].normal[1]) - 1.0).abs() < 1.0e-12);

    physics.step().unwrap();
    assert_eq!(physics.contact_events().len(), 1);
    assert_eq!(physics.contact_events()[0].phase, BodyContactPhase::Stay);
    assert!(physics.contact_events()[0].sensor);

    physics
        .set_pose(
            sensor_id,
            BodyPose {
                x: 8.0,
                y: 0.0,
                angle: 0.0,
            },
        )
        .unwrap();
    physics.step().unwrap();
    assert_eq!(physics.contact_events().len(), 1);
    assert_eq!(physics.contact_events()[0].phase, BodyContactPhase::Exit);
    assert_eq!(
        physics.contact_events()[0].exit_reason,
        Some(BodyContactExitReason::Separated)
    );
    assert!(physics.contact_events()[0].sensor);
    assert_eq!(physics.contact_events()[0].impulse, [0.0, 0.0]);
}

#[test]
fn removing_a_body_publishes_a_cause_tagged_contact_exit() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut fixed = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    fixed.motion = BodyMotionType::Fixed;
    fixed.pose.x = 8.5;
    fixed.pose.y = 8.5;
    let mut moving = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    moving.pose = fixed.pose;
    let (fixed_id, moving_id) = world
        .edit_bodies(|physics| Ok((physics.insert(fixed)?, physics.insert(moving)?)))
        .unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    assert_eq!(
        world.body_physics().unwrap().contact_events()[0].phase,
        BodyContactPhase::Enter
    );

    world
        .edit_bodies(|physics| physics.remove(moving_id))
        .unwrap();

    let events = world.body_physics().unwrap().contact_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].first, BodyContactTarget::Body(fixed_id));
    assert_eq!(events[0].second, BodyContactTarget::Body(moving_id));
    assert_eq!(events[0].phase, BodyContactPhase::Exit);
    assert_eq!(
        events[0].exit_reason,
        Some(BodyContactExitReason::BodyRemoved)
    );
    assert_eq!(events[0].substep, 0);
    assert!(events[0].first_post_velocity.is_some());
    assert_eq!(events[0].second_post_velocity, None);
}

#[test]
fn teleport_publishes_one_cause_tagged_contact_exit() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut fixed = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    fixed.motion = BodyMotionType::Fixed;
    fixed.pose = BodyPose {
        x: 8.5,
        y: 8.5,
        angle: 0.0,
    };
    let mut moving = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    moving.pose = fixed.pose;
    let (fixed_id, moving_id) = world
        .edit_bodies(|physics| Ok((physics.insert(fixed)?, physics.insert(moving)?)))
        .unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    assert_eq!(
        world.body_physics().unwrap().contact_events()[0].phase,
        BodyContactPhase::Enter
    );

    world
        .teleport_body(
            moving_id,
            BodyPose {
                x: 16.5,
                y: 8.5,
                angle: 0.0,
            },
            layer,
            1.0,
            BodyTeleportOptions {
                path: BodyTeleportPathPolicy::Ignore,
                overlap: BodyTeleportOverlapPolicy::Reject,
            },
        )
        .unwrap();

    let events = world.body_physics().unwrap().contact_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].first, BodyContactTarget::Body(fixed_id));
    assert_eq!(events[0].second, BodyContactTarget::Body(moving_id));
    assert_eq!(events[0].phase, BodyContactPhase::Exit);
    assert_eq!(
        events[0].exit_reason,
        Some(BodyContactExitReason::Teleported)
    );
    assert_eq!(events[0].substep, 0);

    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    restored.step_bodies(layer, 1.0).unwrap();
    assert!(world.body_physics().unwrap().contact_events().is_empty());
    assert!(restored.body_physics().unwrap().contact_events().is_empty());
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        restored.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn terrain_sync_reports_update_and_unload_exit_without_replaying_it() {
    let (mut world, layer, stone) = terrain_world();
    let chunk = ChunkCoord::new(0, 0).unwrap();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(chunk)?;
            edit.set_cell(
                layer,
                point(4, 4),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    assert!(physics.sync_solid_terrain(&world, layer, 1.0).unwrap());
    let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.75 });
    body.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle: 0.0,
    };
    physics.insert(body).unwrap();
    physics.step().unwrap();
    assert!(physics.contact_events().iter().any(|event| {
        event.second == BodyContactTarget::Terrain(chunk) && event.phase == BodyContactPhase::Enter
    }));

    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(4, 4),
                CellCore {
                    material: stone,
                    flags: 1,
                    variation: 0,
                },
            )
        })
        .unwrap();
    assert!(physics.sync_solid_terrain(&world, layer, 1.0).unwrap());
    assert!(
        physics.contact_events().iter().any(|event| {
            event.second == BodyContactTarget::Terrain(chunk)
                && event.phase == BodyContactPhase::Exit
                && event.exit_reason == Some(BodyContactExitReason::TerrainChanged)
        }),
        "events after terrain update: {:?}",
        physics.contact_events()
    );
    physics.step().unwrap();
    assert!(!physics.contact_events().iter().any(|event| {
        event.second == BodyContactTarget::Terrain(chunk)
            && event.exit_reason == Some(BodyContactExitReason::TerrainChanged)
    }));

    let mut archives = MemoryArchives::default();
    let stored = world
        .prepare_eviction(ChunkRegion::single(chunk).unwrap(), 1_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();
    physics.sync_solid_terrain(&world, layer, 1.0).unwrap();
    assert!(physics.contact_events().iter().any(|event| {
        event.second == BodyContactTarget::Terrain(chunk)
            && event.phase == BodyContactPhase::Exit
            && event.exit_reason == Some(BodyContactExitReason::TerrainUnloaded)
            && event.substep == 0
    }));

    physics.step().unwrap();
    assert!(!physics.contact_events().iter().any(|event| {
        event.second == BodyContactTarget::Terrain(chunk)
            && event.exit_reason == Some(BodyContactExitReason::TerrainUnloaded)
    }));
}

#[test]
fn world_fixed_step_includes_terrain_update_exit_from_its_sync_candidate() {
    let (mut world, layer, stone) = terrain_world();
    let chunk = ChunkCoord::new(0, 0).unwrap();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(chunk)?;
            edit.set_cell(
                layer,
                point(4, 4),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.75 });
    body.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle: 0.0,
    };
    world.edit_bodies(|physics| physics.insert(body)).unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    assert!(
        world
            .body_physics()
            .unwrap()
            .contact_events()
            .iter()
            .any(|event| event.phase == BodyContactPhase::Enter)
    );

    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point(4, 4),
                CellCore {
                    material: stone,
                    flags: 1,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    assert!(
        world
            .body_physics()
            .unwrap()
            .contact_events()
            .iter()
            .any(|event| {
                event.second == BodyContactTarget::Terrain(chunk)
                    && event.phase == BodyContactPhase::Exit
                    && event.exit_reason == Some(BodyContactExitReason::TerrainChanged)
            })
    );
}

#[test]
fn standard_contact_event_capacity_failure_rolls_back_the_candidate_step() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        contact_event_capacity: 1,
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut left = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    left.motion = BodyMotionType::Fixed;
    left.pose.x = -0.5;
    physics.insert(left).unwrap();
    let mut right = BodyDef::dynamic(ColliderShape::Circle { radius: 1.0 });
    right.motion = BodyMotionType::Fixed;
    right.pose.x = 0.5;
    physics.insert(right).unwrap();
    let dynamic = physics
        .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.75 }))
        .unwrap();
    let before = physics.state(dynamic).unwrap();
    let before_snapshot = physics.snapshot().unwrap();

    assert!(physics.step().is_err());
    assert_eq!(physics.step_index(), 0);
    assert_eq!(physics.state(dynamic).unwrap(), before);
    assert_eq!(physics.snapshot().unwrap(), before_snapshot);
    assert!(physics.contact_events().is_empty());
}

#[test]
fn coupling_provider_is_conservative_checkpointed_and_cannot_be_silently_bypassed() {
    let (mut world, layer, _body, source) = fluid_exclusion_body_fixture(2, false);
    let coupling = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: false,
        version: 1,
        custom_contact: None,
        custom_occupancy: None,
    };
    let report = world
        .step_bodies_with_coupling_provider(layer, 1.0, &coupling)
        .unwrap();
    assert_eq!(
        report.coupling_provider,
        Some(BodyCouplingProviderStamp {
            key: StableKey::parse("test:conserving-fluid-coupling").unwrap(),
            version: 1,
        })
    );
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::MaterialMotion>(
                StableKey::parse("test:motion").unwrap(),
                point(source.x, source.y + 1),
            )
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(value)
            if value.velocity_q16[0] == 1024
    ));
    assert!(
        world
            .body_physics()
            .unwrap()
            .coupling_provider_state::<ProviderCounter>()
            .unwrap()
            .is_some_and(|counter| counter.0 > 0)
    );

    let snapshot = world.body_physics().unwrap().snapshot().unwrap();
    let restored = BodyPhysics::restore(&snapshot).unwrap();
    assert_eq!(restored.snapshot().unwrap(), snapshot);
    assert!(
        restored
            .coupling_provider_state::<ProviderCounter>()
            .unwrap()
            .is_some()
    );
    let before = world.body_physics().unwrap().snapshot().unwrap();
    assert!(world.step_bodies(layer, 1.0).is_err());
    assert_eq!(world.body_physics().unwrap().snapshot().unwrap(), before);

    let other_version = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: false,
        version: 2,
        custom_contact: None,
        custom_occupancy: None,
    };
    assert!(
        world
            .step_bodies_with_coupling_provider(layer, 1.0, &other_version)
            .is_err()
    );
    assert_eq!(world.body_physics().unwrap().snapshot().unwrap(), before);
}

#[test]
fn coupling_provider_callback_failure_rolls_back_physics_fluid_and_codec_state() {
    let (mut world, layer, _body, _source) = fluid_exclusion_body_fixture(2, false);
    let before = world.body_physics().unwrap().snapshot().unwrap();
    let failing = ConservingFluidCoupling {
        fail_after_edit: true,
        invalid_balance: false,
        version: 1,
        custom_contact: None,
        custom_occupancy: None,
    };
    assert!(
        world
            .step_bodies_with_coupling_provider(layer, 1.0, &failing)
            .is_err()
    );
    assert_eq!(world.body_physics().unwrap().snapshot().unwrap(), before);
}

#[test]
fn coupling_provider_custom_fluid_contact_geometry_is_validated_and_used() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    let displaced = point(source.x + 2, source.y);
    let contact = point(source.x + 3, source.y);
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            for point in [displaced, contact] {
                edit.set_cell(
                    layer,
                    point,
                    CellCore {
                        material: water,
                        flags: 0,
                        variation: 0,
                    },
                )?;
                edit.set_channel(
                    StableKey::parse("test:motion")?,
                    point,
                    MaterialMotion::default(),
                )?;
            }
            Ok(())
        })
        .unwrap();

    let invalid = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: false,
        version: 1,
        custom_contact: Some(BodyFluidContactSample {
            body,
            fluid_cell: contact,
            surface_point: [7.0, 4.5],
            reaction_point: [7.0, 4.5],
            coverage: 1.5,
        }),
        custom_occupancy: None,
    };
    let before_invalid = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .step_bodies_with_coupling_provider(layer, 1.0, &invalid)
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_invalid);

    let duplicate_occupancy = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: false,
        version: 1,
        custom_contact: None,
        custom_occupancy: Some(vec![
            BodyFluidOccupancySample {
                body,
                world_cell: displaced,
            },
            BodyFluidOccupancySample {
                body,
                world_cell: displaced,
            },
        ]),
    };
    let before_duplicate_occupancy = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .step_bodies_with_coupling_provider(layer, 1.0, &duplicate_occupancy)
            .is_err()
    );
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        before_duplicate_occupancy
    );

    let custom = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: false,
        version: 1,
        custom_contact: Some(BodyFluidContactSample {
            body,
            fluid_cell: contact,
            surface_point: [7.0, 4.5],
            reaction_point: [7.0, 4.5],
            coverage: 0.5,
        }),
        custom_occupancy: Some(vec![
            BodyFluidOccupancySample {
                body,
                world_cell: source,
            },
            BodyFluidOccupancySample {
                body,
                world_cell: displaced,
            },
        ]),
    };
    world
        .step_bodies_with_coupling_provider(layer, 1.0, &custom)
        .unwrap();
    assert!(matches!(
        world.cell(layer, displaced).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == charon_engine::world::MaterialId::AIR
    ));
    assert!(matches!(
        world
            .channel_value::<MaterialMotion>(StableKey::parse("test:motion").unwrap(), contact)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(value)
            if value.velocity_q16[0] == 1024
    ));
}

#[test]
fn parallel_world_body_candidates_match_the_sequential_checkpoint_timeline() {
    const RESTORE_LIMIT: usize = 4_000_000;
    const TICKS: usize = 4;
    let (mut reference, layer, _body, _source) = fluid_exclusion_body_fixture(2, true);
    let initial = reference.checkpoint(RESTORE_LIMIT).unwrap();
    let content = Arc::clone(reference.content());
    let mut sequential_timeline = Vec::with_capacity(TICKS);
    for _ in 0..TICKS {
        reference.step_bodies(layer, 1.0).unwrap();
        sequential_timeline.push(reference.checkpoint(RESTORE_LIMIT).unwrap());
    }

    for worker_count in [1_usize, 2, 4] {
        let worker_timelines = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(worker_count);
            for worker_index in 0..worker_count {
                let initial = initial.clone();
                let content = Arc::clone(&content);
                workers.push(scope.spawn(move || {
                    let mut candidate =
                        World::restore(&initial, content, WorldLimits::default(), RESTORE_LIMIT)
                            .unwrap();
                    let mut timeline = Vec::with_capacity(TICKS);
                    for tick in 0..TICKS {
                        let delay = (worker_index + tick * 2) % 3;
                        std::thread::sleep(std::time::Duration::from_millis(delay as u64));
                        candidate.step_bodies(layer, 1.0).unwrap();
                        timeline.push(candidate.checkpoint(RESTORE_LIMIT).unwrap());
                    }
                    timeline
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        for timeline in worker_timelines {
            assert_eq!(timeline, sequential_timeline, "worker_count={worker_count}");
        }
    }
}

#[test]
fn world_candidate_matches_reusable_body_physics_reference_across_workers() {
    const RESTORE_LIMIT: usize = 4_000_000;
    const TICKS: usize = 8;
    let (mut initial_world, layer, _) = terrain_world();
    initial_world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    let settings = BodyPhysicsSettings {
        gravity: [0.0, 9.81],
        ..BodyPhysicsSettings::default()
    };
    initial_world.configure_body_physics(settings).unwrap();
    initial_world
        .edit_bodies(|physics| {
            let mut first = BodyDef::dynamic(ColliderShape::Circle { radius: 0.4 });
            first.motion = BodyMotionType::Fixed;
            first.pose = BodyPose {
                x: 8.0,
                y: 8.0,
                angle: 0.15,
            };
            first.velocity.x = 1.0;
            first.velocity.angular = 0.2;
            physics.insert(first)?;
            let mut second = BodyDef::dynamic(ColliderShape::Cuboid {
                half_extents: [0.4, 0.3],
            });
            second.pose = BodyPose {
                x: 8.7,
                y: 8.0,
                angle: -0.1,
            };
            second.velocity.x = -0.5;
            second.velocity.y = 0.25;
            physics.insert(second)?;
            Ok(())
        })
        .unwrap();

    let initial_checkpoint = initial_world.checkpoint(RESTORE_LIMIT).unwrap();
    let initial_physics = initial_world.body_physics().unwrap().snapshot().unwrap();
    let content = Arc::clone(initial_world.content());
    let mut reference = BodyPhysics::restore(&initial_physics).unwrap();
    let mut reference_timeline = Vec::with_capacity(TICKS);
    for _ in 0..TICKS {
        reference.step().unwrap();
        let states = reference
            .body_ids()
            .map(|body| (body, reference.state(body).unwrap()))
            .collect::<Vec<_>>();
        let events = reference.contact_events().to_vec();
        assert!(
            !events.is_empty(),
            "reference fixture must exercise contacts"
        );
        reference_timeline.push((states, events));
    }

    for worker_count in [1_usize, 2, 4] {
        let worker_timelines = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(worker_count);
            for worker_index in 0..worker_count {
                let initial_checkpoint = initial_checkpoint.clone();
                let content = Arc::clone(&content);
                workers.push(scope.spawn(move || {
                    let mut world = World::restore(
                        &initial_checkpoint,
                        content,
                        WorldLimits::default(),
                        RESTORE_LIMIT,
                    )
                    .unwrap();
                    let mut timeline = Vec::with_capacity(TICKS);
                    for tick in 0..TICKS {
                        let delay = (worker_index * 3 + tick * 2) % 4;
                        std::thread::sleep(std::time::Duration::from_millis(delay as u64));
                        world.step_bodies(layer, 1.0).unwrap();
                        let physics = world.body_physics().unwrap();
                        let states = physics
                            .body_ids()
                            .map(|body| (body, physics.state(body).unwrap()))
                            .collect::<Vec<_>>();
                        timeline.push((
                            states,
                            physics.contact_events().to_vec(),
                            physics.step_index(),
                            world.checkpoint(RESTORE_LIMIT).unwrap(),
                        ));
                    }
                    timeline
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });

        let mut first_timeline = None;
        for timeline in worker_timelines {
            for (tick, (states, events, step, _world_checkpoint)) in timeline.iter().enumerate() {
                assert_eq!(states, &reference_timeline[tick].0);
                assert_eq!(events, &reference_timeline[tick].1);
                assert_eq!(*step, tick as u64 + 1);
            }
            if let Some(first_timeline) = &first_timeline {
                assert_eq!(timeline, *first_timeline, "worker_count={worker_count}");
            } else {
                first_timeline = Some(timeline);
            }
        }
    }
}

#[test]
fn coupling_provider_rejects_nonconserving_quantized_impulse_atomically() {
    let (mut world, layer, _body, _source) = fluid_exclusion_body_fixture(2, false);
    let before = world.body_physics().unwrap().snapshot().unwrap();
    let invalid = ConservingFluidCoupling {
        fail_after_edit: false,
        invalid_balance: true,
        version: 1,
        custom_contact: None,
        custom_occupancy: None,
    };
    let error = world
        .step_bodies_with_coupling_provider(layer, 1.0, &invalid)
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.body_physics().unwrap().snapshot().unwrap(), before);
}

#[test]
fn body_step_exchanges_drag_impulse_with_adjacent_fluid_surface() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    let contact = point(source.x + 1, source.y);
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    let motion_key = StableKey::parse("test:motion").unwrap();
    world
        .edit(|edit| {
            edit.set_cell(layer, source, CellCore::AIR)?;
            edit.set_cell(
                layer,
                contact,
                CellCore {
                    material: water,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(
                motion_key,
                contact,
                charon_engine::world::MaterialMotion {
                    velocity_q16: [0, -65536],
                    remainder_q16: [0, 0],
                },
            )
        })
        .unwrap();

    world.step_bodies(layer, 1.0).unwrap();

    let after_motion = match world
        .channel_value::<charon_engine::world::MaterialMotion>(motion_key, contact)
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(value) => value,
        _ => panic!("surface fluid motion must remain loaded"),
    };
    assert!(after_motion.velocity_q16[1] > -65536);
    let body_state = world.body_physics().unwrap().state(body).unwrap();
    let velocity = body_state.velocity;
    assert!(
        velocity.y < 0.0,
        "surface drag should move the body: {velocity:?}"
    );
    assert!(
        velocity.angular < 0.0,
        "off-center surface drag should rotate the body: {velocity:?}"
    );
    let fluid_mass = f64::from(
        world
            .content()
            .material(water)
            .unwrap()
            .unwrap()
            .mass_per_cell(),
    );
    let mass_properties = world.body_physics().unwrap().mass_properties(body).unwrap();
    let body_mass = mass_properties.mass;
    let velocity_scale = f64::from(BodyPhysicsSettings::default().step_hz) / 65536.0;
    let fluid_delta = [
        fluid_mass * after_motion.velocity_q16[0] as f64 * velocity_scale,
        fluid_mass * (after_motion.velocity_q16[1] + 65536) as f64 * velocity_scale,
    ];
    let fluid_impulse = fluid_delta[1];
    assert!(
        (body_mass * velocity.y + fluid_impulse).abs() < 1.0e-9,
        "body_mass={body_mass}, body_velocity={}, fluid_impulse={fluid_impulse}, fluid_q16={}",
        velocity.y,
        after_motion.velocity_q16[1]
    );
    let (sin, cos) = body_state.pose.angle.sin_cos();
    let body_com = [
        body_state.pose.x + cos * mass_properties.center_of_mass[0]
            - sin * mass_properties.center_of_mass[1],
        body_state.pose.y
            + sin * mass_properties.center_of_mass[0]
            + cos * mass_properties.center_of_mass[1],
    ];
    let body_angular_momentum = mass_properties.angular_inertia * velocity.angular
        + body_com[0] * (body_mass * velocity.y)
        - body_com[1] * (body_mass * velocity.x);
    let fluid_position = [f64::from(contact.x) + 0.5, f64::from(contact.y) + 0.5];
    let fluid_angular_momentum =
        fluid_position[0] * fluid_delta[1] - fluid_position[1] * fluid_delta[0];
    assert!(
        (body_angular_momentum + fluid_angular_momentum).abs() < 1.0e-9,
        "body_angular={body_angular_momentum}, fluid_angular={fluid_angular_momentum}"
    );
}

#[test]
fn body_step_buoyancy_applies_opposite_impulses_to_body_and_fluid() {
    let (mut world, layer, body, _source) = fluid_exclusion_body_fixture_with_settings(
        2,
        false,
        WorldLimits::default(),
        BodyPhysicsSettings::default(),
    );
    let motion_key = StableKey::parse("test:motion").unwrap();

    world.step_bodies(layer, 1.0).unwrap();

    let mut fluid_state = None;
    for y in 0..8 {
        for x in 0..8 {
            let point = point(x, y);
            let charon_engine::world::CellQuery::Loaded(view) = world.cell(layer, point).unwrap()
            else {
                continue;
            };
            if view.core.material == charon_engine::world::MaterialId::AIR {
                continue;
            }
            let motion = world
                .channel_value::<charon_engine::world::MaterialMotion>(motion_key, point)
                .unwrap();
            if let charon_engine::world::ChannelQuery::Loaded(motion) = motion {
                fluid_state = Some((view.core, motion));
            }
        }
    }
    let (fluid, fluid_motion) = fluid_state.expect("fluid cell should remain in the World");
    let velocity_q16 = fluid_motion.velocity_q16;
    assert!(
        velocity_q16[1] > 0,
        "motion={velocity_q16:?}, pose={:?}",
        world.body_physics().unwrap().state(body).unwrap()
    );
    let velocity = world.body_physics().unwrap().state(body).unwrap().velocity;
    let fluid_mass = f64::from(
        world
            .content()
            .material(fluid.material)
            .unwrap()
            .unwrap()
            .mass_per_cell(),
    );
    let body_mass = world
        .body_physics()
        .unwrap()
        .mass_properties(body)
        .unwrap()
        .mass;
    let fluid_impulse =
        fluid_mass * f64::from(BodyPhysicsSettings::default().step_hz) * velocity_q16[1] as f64
            / 65536.0;
    assert!(
        (body_mass * velocity.y + fluid_impulse).abs() < 1.0e-9,
        "body_mass={body_mass}, body_velocity={}, fluid_impulse={fluid_impulse}, fluid_q16={}",
        velocity.y,
        velocity_q16[1]
    );
}

#[test]
fn body_step_uses_material_particle_pool_before_exclusion_reservoir() {
    let (mut world, layer, body, source) = fluid_exclusion_body_fixture(2, false);
    let stone = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:stone").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            for point in [
                point(source.x, source.y - 1),
                point(source.x - 1, source.y),
                point(source.x + 1, source.y),
                point(source.x, source.y + 1),
            ] {
                edit.set_cell(
                    layer,
                    point,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    world.edit_and_step_bodies(layer, 1.0, |_| Ok(())).unwrap();
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == charon_engine::world::MaterialId::AIR
    ));
    assert!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .exclusion_reservoir()
            .is_empty()
    );
    let (handle, particle) = world.material_particles().next().unwrap();
    assert_eq!(particle.cell(), source);
    assert_eq!(
        particle.core().material,
        world
            .content()
            .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
            .unwrap()
    );
    assert_eq!(
        world
            .particle_channel_value::<u8>(handle, StableKey::parse("test:heat").unwrap())
            .unwrap(),
        42
    );
}

#[test]
fn body_step_uses_exclusion_reservoir_when_particle_pool_is_full() {
    let limits = WorldLimits {
        particles: 1,
        ..WorldLimits::default()
    };
    let (mut world, layer, body, source) =
        fluid_exclusion_body_fixture_with_limits(2, false, limits);
    let thermal = StableKey::parse("test:thermal").unwrap();
    world
        .edit(|edit| {
            edit.set_channel(
                thermal,
                source,
                charon_engine::world::ThermalCellState { energy: 83 },
            )
        })
        .unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:stone").unwrap()))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            let particle_source = point(source.x + 8, source.y + 8);
            edit.set_cell(
                layer,
                particle_source,
                CellCore {
                    material: water,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.emit_material_particle(layer, particle_source, [0, 0])?;
            for occupied in [
                point(source.x, source.y - 1),
                point(source.x - 1, source.y),
                point(source.x + 1, source.y),
                point(source.x, source.y + 1),
            ] {
                edit.set_cell(
                    layer,
                    occupied,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == charon_engine::world::MaterialId::AIR
    ));
    assert_eq!(world.material_particles().count(), 1);
    let reservoir = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .exclusion_reservoir();
    assert_eq!(reservoir.len(), 1);
    assert_eq!(reservoir[0].origin(), source);
    assert_eq!(reservoir[0].cell().flags(), 4);
    assert_eq!(reservoir[0].cell().variation(), 5);
    assert_eq!(reservoir[0].cell().sidecars()[0].payload(), &[42]);
    assert_eq!(reservoir[0].cell().energy(), 83);
}

#[test]
fn body_step_fluid_reservoir_capacity_failure_rolls_back_world_and_physics() {
    let limits = WorldLimits {
        particles: 1,
        ..WorldLimits::default()
    };
    let (mut world, layer, _body, source) =
        fluid_exclusion_body_fixture_with_limits(1, false, limits);
    let stone = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:stone").unwrap()))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:water").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            let particle_source = point(source.x + 8, source.y + 8);
            edit.set_cell(
                layer,
                particle_source,
                CellCore {
                    material: water,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.emit_material_particle(layer, particle_source, [0, 0])?;
            for occupied in [
                point(source.x, source.y - 1),
                point(source.x - 1, source.y),
                point(source.x + 1, source.y),
                point(source.x, source.y + 1),
            ] {
                edit.set_cell(
                    layer,
                    occupied,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world.step_bodies(layer, 1.0).unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::error::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

#[test]
fn body_local_connected_components_are_stable_and_provider_driven() {
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let dense_key = StableKey::parse("test:dense").unwrap();
    let mut grid = body_grid(
        5,
        3,
        [0.0, 0.0],
        BodyLocalGridLimits {
            cells: 8,
            geometry_cells: 15,
            sidecar_bytes: 1024,
        },
    );
    for (x, y, material) in [
        (0, 0, stone_key),
        (1, 0, stone_key),
        (3, 0, stone_key),
        (4, 0, stone_key),
        (3, 1, dense_key),
        (4, 1, dense_key),
        (1, 1, dense_key),
        (0, 2, dense_key),
    ] {
        grid.set_cell(
            x,
            y,
            BodyLocalCell::new(layer_key, material, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
    }
    let revision = grid.revision();
    let components = grid
        .connected_components(&SameMaterialBodyLocalBondProvider)
        .unwrap();
    assert_eq!(
        components,
        vec![
            vec![[0, 0], [1, 0]],
            vec![[3, 0], [4, 0]],
            vec![[1, 1]],
            vec![[3, 1], [4, 1]],
            vec![[0, 2]],
        ]
    );
    assert_eq!(grid.revision(), revision);
    assert_eq!(grid.cell_count(), 8);

    struct JoinAllLocalCells;
    impl BodyLocalBondProvider for JoinAllLocalCells {
        fn joins(
            &self,
            _from: [u32; 2],
            _to: [u32; 2],
            _from_cell: &BodyLocalCell,
            _to_cell: &BodyLocalCell,
        ) -> charon_engine::foundation::EngineResult<bool> {
            Ok(true)
        }
    }
    assert_eq!(
        grid.connected_components(&JoinAllLocalCells).unwrap(),
        vec![
            vec![[0, 0], [1, 0], [1, 1]],
            vec![[3, 0], [4, 0], [3, 1], [4, 1]],
            vec![[0, 2]],
        ]
    );
}

fn disconnected_dynamic_body(body_capacity: u32) -> (World, charon_engine::foundation::id::BodyId) {
    let (mut world, _layer, stone) = terrain_world();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [0.0, 0.0],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 3,
            sidecar_bytes: 64,
        },
    );
    for x in [0, 2] {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                stone.0,
                0,
                0,
                i64::from(x) * 11,
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = BodyPose {
        x: 10.0,
        y: 20.0,
        angle: 0.0,
    };
    definition.velocity = BodyVelocity {
        x: 2.0,
        y: 3.0,
        angular: 0.5,
    };
    definition.ccd = true;
    definition.can_sleep = false;
    definition.collider.friction = 0.3;
    definition.collider.restitution = 0.2;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    (world, body)
}

#[test]
fn fracture_splits_local_components_and_conserves_linear_and_angular_momentum() {
    let (mut world, body) = disconnected_dynamic_body(4);
    let source_mass = world.body_physics().unwrap().mass_properties(body).unwrap();
    let source_state = world.body_physics().unwrap().state(body).unwrap();
    let report = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap();
    assert_eq!(report.fragments.len(), 2);
    assert_eq!(report.fragments[0], body);
    assert_eq!(world.body_physics().unwrap().body_count(), 2);

    let physics = world.body_physics().unwrap();
    let fragments = report
        .fragments
        .iter()
        .map(|fragment| {
            (
                physics.mass_properties(*fragment).unwrap(),
                physics.state(*fragment).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        fragments
            .iter()
            .all(|(mass, _)| (mass.mass - 1.0).abs() < 1e-10)
    );
    assert!((fragments[0].1.velocity.x - 2.0).abs() < 1e-10);
    assert!((fragments[0].1.velocity.y - 2.5).abs() < 1e-10);
    assert!((fragments[1].1.velocity.x - 2.0).abs() < 1e-10);
    assert!((fragments[1].1.velocity.y - 3.5).abs() < 1e-10);
    assert!(fragments.iter().all(|(_, state)| {
        (state.velocity.angular - source_state.velocity.angular).abs() < 1e-10
    }));

    let linear_momentum = fragments.iter().fold([0.0, 0.0], |sum, (mass, state)| {
        [
            sum[0] + mass.mass * state.velocity.x,
            sum[1] + mass.mass * state.velocity.y,
        ]
    });
    assert!((linear_momentum[0] - source_mass.mass * source_state.velocity.x).abs() < 1e-10);
    assert!((linear_momentum[1] - source_mass.mass * source_state.velocity.y).abs() < 1e-10);

    let angular_momentum = fragments
        .iter()
        .map(|(mass, state)| {
            let rx = mass.center_of_mass[0] - source_mass.center_of_mass[0];
            let ry = mass.center_of_mass[1] - source_mass.center_of_mass[1];
            let dvx = state.velocity.x - source_state.velocity.x;
            let dvy = state.velocity.y - source_state.velocity.y;
            mass.angular_inertia * state.velocity.angular + mass.mass * (rx * dvy - ry * dvx)
        })
        .sum::<f64>();
    let expected_angular = source_mass.angular_inertia * source_state.velocity.angular;
    assert!((angular_momentum - expected_angular).abs() < 1e-10);
    for fragment in report.fragments {
        let grid = physics.local_grid(fragment).unwrap().unwrap();
        assert_eq!(grid.cell_count(), 1);
    }
}

#[test]
fn fracture_moves_world_boundary_thermal_edge_state_with_its_local_cell() {
    let (mut world, _layer, stone) = terrain_world();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 4,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [1.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 3,
            sidecar_bytes: 64,
        },
    );
    for x in [0, 2] {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                stone.0,
                0,
                0,
                0,
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let left_state = BodyThermalBoundaryEdgeState {
        residual: 2,
        equilibrium_credit: 1,
    };
    let right_state = BodyThermalBoundaryEdgeState {
        residual: -3,
        equilibrium_credit: 4,
    };
    grid.set_thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Left, left_state)
        .unwrap();
    grid.set_thermal_boundary_edge_state(2, 0, BodyThermalBoundaryFace::Right, right_state)
        .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();

    let report = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap();
    assert_eq!(report.fragments.len(), 2);
    let physics = world.body_physics().unwrap();
    assert_eq!(
        physics
            .local_grid(report.fragments[0])
            .unwrap()
            .unwrap()
            .thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Left),
        Some(left_state)
    );
    assert_eq!(
        physics
            .local_grid(report.fragments[1])
            .unwrap()
            .unwrap()
            .thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        Some(right_state)
    );
}

#[test]
fn damage_particleization_and_fracture_preserve_total_momentum_atomically() {
    let (mut world, layer, stone) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 4,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [1.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 64,
        },
    );
    for x in 0..3 {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                stone.0,
                0,
                0,
                0,
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 1.5 });
    definition.pose = BodyPose {
        x: 10.5,
        y: 10.5,
        angle: 0.0,
    };
    definition.velocity = BodyVelocity {
        x: 60.0,
        y: 0.0,
        angular: 60.0,
    };
    definition.can_sleep = false;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let old_properties = world.body_physics().unwrap().mass_properties(body).unwrap();
    let old_state = world.body_physics().unwrap().state(body).unwrap();
    let old_com = [
        old_state.pose.x + old_properties.center_of_mass[0],
        old_state.pose.y + old_properties.center_of_mass[1],
    ];
    let old_momentum = [
        old_properties.mass * old_state.velocity.x,
        old_properties.mass * old_state.velocity.y,
    ];
    let old_angular_momentum = old_properties.angular_inertia * old_state.velocity.angular
        + old_com[0] * old_momentum[1]
        - old_com[1] * old_momentum[0];

    let report = world
        .edit(|edit| {
            edit.apply_body_damage_cells_to_particles(
                layer,
                body,
                &[BodyCellParticleTransfer {
                    source: [1, 0],
                    destination: point(10, 10),
                    velocity_q16: [65_536, 0],
                }],
                1.0,
                BodyCellReturnConfig::default(),
                &SameMaterialBodyLocalBondProvider,
            )
        })
        .unwrap();

    assert_eq!(report.particle_transfer.cells, 1);
    assert!(!report.particle_transfer.body_removed);
    assert_eq!(report.particle_transfer.particles.len(), 1);
    assert_eq!(report.fracture.as_ref().unwrap().fragments.len(), 2);
    let particle = world
        .material_particle(report.particle_transfer.particles[0])
        .unwrap();
    assert_eq!(particle.velocity_q16(), [65_536, 0]);

    let mut total_momentum = [60.0, 0.0];
    let mut total_angular_momentum = 10.5 * 0.0 - 10.5 * 60.0;
    for fragment in &report.fracture.unwrap().fragments {
        let properties = world
            .body_physics()
            .unwrap()
            .mass_properties(*fragment)
            .unwrap();
        let state = world.body_physics().unwrap().state(*fragment).unwrap();
        let center = [
            state.pose.x + properties.center_of_mass[0],
            state.pose.y + properties.center_of_mass[1],
        ];
        let momentum = [
            properties.mass * state.velocity.x,
            properties.mass * state.velocity.y,
        ];
        total_momentum[0] += momentum[0];
        total_momentum[1] += momentum[1];
        total_angular_momentum += properties.angular_inertia * state.velocity.angular
            + center[0] * momentum[1]
            - center[1] * momentum[0];
    }
    assert!((total_momentum[0] - old_momentum[0]).abs() < 1e-9);
    assert!((total_momentum[1] - old_momentum[1]).abs() < 1e-9);
    assert!((total_angular_momentum - old_angular_momentum).abs() < 1e-8);
}

#[test]
fn damage_particleization_rolls_back_when_fragment_capacity_is_full() {
    let (mut world, layer, stone) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [1.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 64,
        },
    );
    for x in 0..3 {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                stone.0,
                0,
                0,
                0,
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 1.5 });
    definition.pose = BodyPose {
        x: 10.5,
        y: 10.5,
        angle: 0.0,
    };
    definition.velocity = BodyVelocity {
        x: 3.0,
        y: -2.0,
        angular: 0.75,
    };
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();

    let error = world
        .edit(|edit| {
            edit.apply_body_damage_cells_to_particles(
                layer,
                body,
                &[BodyCellParticleTransfer {
                    source: [1, 0],
                    destination: point(10, 10),
                    velocity_q16: [0, 0],
                }],
                1.0,
                BodyCellReturnConfig::default(),
                &SameMaterialBodyLocalBondProvider,
            )
        })
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(world.material_particles().count(), 0);
    assert_eq!(world.body_physics().unwrap().body_count(), 1);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        3
    );
}

#[test]
fn fracture_capacity_failure_rolls_back_all_fragments() {
    let (mut world, body) = disconnected_dynamic_body(1);
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(world.body_physics().unwrap().body_count(), 1);
}

#[test]
fn fracture_migrates_joint_to_the_component_owning_its_anchor() {
    let (mut world, body) = disconnected_dynamic_body(4);
    let other = world
        .edit_bodies(|physics| {
            physics.insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))
        })
        .unwrap();
    let joint = world
        .edit_bodies(|physics| {
            physics.insert_joint(JointDef {
                body1: body,
                body2: other,
                local_anchor1: [2.5, 0.5],
                local_anchor2: [0.0, 0.0],
                kind: JointKind::Fixed,
            })
        })
        .unwrap();
    let report = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap();
    assert_eq!(report.fragments.len(), 2);
    assert!(report.removed_joints.is_empty());
    assert_eq!(world.body_physics().unwrap().body_count(), 3);
    assert_eq!(world.body_physics().unwrap().joint_count(), 1);
    assert_eq!(
        world.body_physics().unwrap().joint_bodies(joint).unwrap(),
        (report.fragments[1], other)
    );
    assert!(world.body_physics().unwrap().state(other).is_ok());
}

#[test]
fn fracture_reports_and_removes_joint_whose_anchor_is_in_a_gap() {
    let (mut world, body) = disconnected_dynamic_body(4);
    let other = world
        .edit_bodies(|physics| {
            physics.insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))
        })
        .unwrap();
    let joint = world
        .edit_bodies(|physics| {
            physics.insert_joint(JointDef {
                body1: body,
                body2: other,
                local_anchor1: [1.5, 0.5],
                local_anchor2: [0.0, 0.0],
                kind: JointKind::Fixed,
            })
        })
        .unwrap();
    let report = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap();
    assert_eq!(report.removed_joints, vec![joint]);
    assert_eq!(world.body_physics().unwrap().joint_count(), 0);
    assert_eq!(world.body_physics().unwrap().body_count(), 3);
}

#[test]
fn terrain_material_cell_transfer_batches_sidecars_and_rolls_back_on_capacity() {
    let mut world = world_with_heat_channel();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("test:stone").unwrap()))
        .unwrap();
    let heat = StableKey::parse("test:heat").unwrap();
    let source = point(3, 4);
    let source2 = point(4, 4);
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0).unwrap())?;
            edit.set_cell(
                layer,
                source,
                CellCore {
                    material: stone,
                    flags: 2,
                    variation: 9,
                },
            )?;
            edit.set_channel(heat, source, 17_u8)?;
            edit.set_cell(
                layer,
                source2,
                CellCore {
                    material: stone,
                    flags: 1,
                    variation: 11,
                },
            )?;
            edit.set_channel(heat, source2, 19_u8)
        })
        .unwrap();

    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut initial = body_grid(
        3,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 3,
            sidecar_bytes: 16,
        },
    );
    initial
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, initial))
        .unwrap();

    let error = world
        .edit(|edit| {
            edit.transfer_material_cell_to_body(
                layer,
                source,
                body,
                [1, 0],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    assert!(matches!(
        world.channel_value::<u8>(heat, source).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(17)
    ));
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        1
    );

    let mut expanded = body_grid(
        3,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 16,
        },
    );
    expanded
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    world
        .edit_bodies(|physics| physics.replace_local_grid(body, expanded))
        .unwrap();
    let batch_capacity_error = world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    charon_engine::world::BodyCellTransfer {
                        source,
                        destination: [1, 0],
                    },
                    charon_engine::world::BodyCellTransfer {
                        source: source2,
                        destination: [2, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(
        batch_capacity_error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    assert!(matches!(
        world.cell(layer, source2).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        1
    );
    let mut expanded = body_grid(
        3,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 32,
        },
    );
    expanded
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    world
        .edit_bodies(|physics| physics.replace_local_grid(body, expanded))
        .unwrap();
    let duplicate_error = world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    charon_engine::world::BodyCellTransfer {
                        source,
                        destination: [1, 0],
                    },
                    charon_engine::world::BodyCellTransfer {
                        source,
                        destination: [2, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(
        duplicate_error.code,
        charon_engine::foundation::ErrorCode::Conflict
    );
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    charon_engine::world::BodyCellTransfer {
                        source,
                        destination: [1, 0],
                    },
                    charon_engine::world::BodyCellTransfer {
                        source: source2,
                        destination: [2, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));
    assert!(matches!(
        world.channel_value::<u8>(heat, source).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(0)
    ));
    assert!(matches!(
        world.cell(layer, source2).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));
    assert!(matches!(
        world.channel_value::<u8>(heat, source2).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(0)
    ));
    let transferred = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(1, 0)
        .unwrap();
    assert_eq!(
        transferred.material().unwrap(),
        StableKey::parse("test:stone").unwrap()
    );
    assert_eq!(transferred.flags(), 2);
    assert_eq!(transferred.variation(), 9);
    assert_eq!(transferred.sidecars().len(), 1);
    assert_eq!(transferred.sidecars()[0].key().unwrap(), heat);
    assert_eq!(transferred.sidecars()[0].payload(), &[17]);
    let transferred2 = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(2, 0)
        .unwrap();
    assert_eq!(transferred2.flags(), 1);
    assert_eq!(transferred2.variation(), 11);
    assert_eq!(transferred2.sidecars()[0].payload(), &[19]);
}

#[test]
fn complete_block_instance_moves_into_body_grid_and_survives_checkpoint() {
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("test:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let block_material = MaterialHandle(StableKey::parse("test:block").unwrap());
    let block_state_key = StableKey::parse("test:block-state").unwrap();
    registry
        .schema(Schema::of::<u8>(block_state_key, 1, 1).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:block", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1),
        )
        .unwrap();
    let stone_material = MaterialHandle(StableKey::parse("test:stone").unwrap());
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        17,
        WorldBounds::expandable(),
        Arc::clone(&content),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [1.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 4096,
        },
    );
    grid.set_cell(
        2,
        0,
        BodyLocalCell::new(
            content.layer(layer).unwrap().key,
            content
                .material(content.material_id(stone_material).unwrap())
                .unwrap()
                .unwrap()
                .key(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let block = world
        .edit(|edit| {
            edit.place_block(
                layer,
                point(0, 0),
                block_material,
                charon_engine::world::Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| edit.set_block_state(block, block_state_key, block_state_key, &73_u8))
        .unwrap();

    let before_partial = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[BodyCellTransfer {
                    source: point(0, 0),
                    destination: [0, 0],
                }],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_partial);
    assert_eq!(world.block(block).unwrap().remaining_cells(), 2);

    world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    BodyCellTransfer {
                        source: point(0, 0),
                        destination: [0, 0],
                    },
                    BodyCellTransfer {
                        source: point(1, 0),
                        destination: [1, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(world.block_count(), 0);
    assert!(
        matches!(world.cell(layer, point(0, 0)).unwrap(), charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR && view.block.is_none())
    );

    let attached_checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored_attached = World::restore(
        &attached_checkpoint,
        Arc::clone(&content),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let grid = restored_attached
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.cell_count(), 3);
    assert_eq!(
        grid.clone().remove_cell(0, 0).unwrap_err().code,
        charon_engine::foundation::ErrorCode::Conflict
    );

    let before_split_block = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SeparateCellBonds))
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_split_block);

    let before_partial_return = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(5, 5),
                }],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_partial_return);
    let error = world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[
                    BodyCellReturn {
                        source: [0, 0],
                        destination: point(5, 5),
                    },
                    BodyCellReturn {
                        source: [1, 0],
                        destination: point(6, 6),
                    },
                ],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_partial_return);

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[
                    BodyCellReturn {
                        source: [0, 0],
                        destination: point(5, 5),
                    },
                    BodyCellReturn {
                        source: [1, 0],
                        destination: point(6, 5),
                    },
                ],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(world.block_count(), 1);
    let returned = match world.cell(layer, point(5, 5)).unwrap() {
        charon_engine::world::CellQuery::Loaded(view) => view.block.unwrap(),
        _ => panic!("復帰したblockのcellが読込済みである必要があります"),
    };
    let returned_block = world.block(returned).unwrap();
    assert_eq!(returned_block.remaining_cells(), 2);
    assert_eq!(returned_block.anchor(), point(5, 5));
    assert_eq!(
        world.block_state::<u8>(returned, block_state_key).unwrap(),
        73
    );
    assert_eq!(
        returned_block.orientation(),
        charon_engine::world::Orientation::Original
    );
    world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    BodyCellTransfer {
                        source: point(5, 5),
                        destination: [0, 0],
                    },
                    BodyCellTransfer {
                        source: point(6, 5),
                        destination: [1, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[
                    BodyCellReturn {
                        source: [0, 0],
                        destination: point(10, 10),
                    },
                    BodyCellReturn {
                        source: [1, 0],
                        destination: point(10, 11),
                    },
                ],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();
    let rotated = match world.cell(layer, point(10, 10)).unwrap() {
        charon_engine::world::CellQuery::Loaded(view) => view.block.unwrap(),
        _ => panic!("回転復帰blockのcellが読込済みである必要があります"),
    };
    assert_eq!(
        world.block(rotated).unwrap().orientation(),
        charon_engine::world::Orientation::Right
    );
    assert_eq!(world.block(rotated).unwrap().anchor(), point(10, 10));
    assert_eq!(
        world.block_state::<u8>(rotated, block_state_key).unwrap(),
        73
    );
    world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    BodyCellTransfer {
                        source: point(10, 10),
                        destination: [0, 0],
                    },
                    BodyCellTransfer {
                        source: point(10, 11),
                        destination: [1, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    let fracture = world
        .edit(|edit| edit.fracture_body_local_grid(body, &SameMaterialBodyLocalBondProvider))
        .unwrap();
    assert_eq!(fracture.fragments.len(), 2);
    let retained_grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(retained_grid.cell_count(), 2);
    assert_eq!(
        retained_grid.clone().remove_cell(0, 0).unwrap_err().code,
        charon_engine::foundation::ErrorCode::Conflict
    );
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(restored.block_count(), 0);
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .clone()
            .remove_cell(0, 0)
            .unwrap_err()
            .code,
        charon_engine::foundation::ErrorCode::Conflict
    );
    let before_invalid_split = restored.checkpoint(4_000_000).unwrap();
    assert!(
        restored
            .edit(|edit| {
                edit.fracture_body_local_grid_with_attachment_provider(
                    body,
                    &SeparateCellBonds,
                    &InvalidBlockMaskOnSplit,
                )
            })
            .is_err()
    );
    assert_eq!(
        restored.checkpoint(4_000_000).unwrap(),
        before_invalid_split
    );
    let split = restored
        .edit(|edit| {
            edit.fracture_body_local_grid_with_attachment_provider(
                body,
                &SeparateCellBonds,
                &TransformBlockPropertiesOnSplit,
            )
        })
        .unwrap();
    assert_eq!(split.fragments.len(), 2);
    assert_eq!(
        split.attachment_provider,
        Some(charon_engine::world::BodyAttachmentProviderStamp {
            key: StableKey::parse("test:transform-block-properties-on-split").unwrap(),
            version: 1,
        })
    );
    let mut block_fragments = Vec::new();
    for fragment in split.fragments {
        let grid = restored
            .body_physics()
            .unwrap()
            .local_grid(fragment)
            .unwrap()
            .unwrap();
        if grid.cell(0, 0).unwrap().material().unwrap() == StableKey::parse("test:block").unwrap() {
            block_fragments.push(fragment);
        }
    }
    assert_eq!(block_fragments.len(), 2);
    let mut returned_blocks = Vec::new();
    for (fragment, destination) in block_fragments
        .into_iter()
        .zip([point(10, 10), point(12, 10)])
    {
        restored
            .edit(|edit| {
                edit.transfer_body_cells_to_material_cells(
                    layer,
                    fragment,
                    &[BodyCellReturn {
                        source: [0, 0],
                        destination,
                    }],
                    BodyCellReturnConfig {
                        remove_empty_body: true,
                        ..BodyCellReturnConfig::default()
                    },
                )
            })
            .unwrap();
        returned_blocks.push(match restored.cell(layer, destination).unwrap() {
            charon_engine::world::CellQuery::Loaded(view) => view.block.unwrap(),
            _ => panic!("split attachment復帰先はloadedである必要があります"),
        });
    }
    assert_eq!(restored.block_count(), 2);
    assert_eq!(
        returned_blocks
            .iter()
            .map(|block| restored.block_state::<u8>(*block, block_state_key).unwrap())
            .collect::<Vec<_>>(),
        vec![73, 74]
    );
}

fn body_reaction_fixture(
    flags: u16,
    include_opaque_sidecar: bool,
) -> (
    World,
    charon_engine::foundation::id::BodyId,
    EnthalpyFamily,
    ReactionRuleSet,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
) {
    body_reaction_fixture_with_capacity(flags, include_opaque_sidecar, 1)
}

fn body_reaction_fixture_with_capacity(
    flags: u16,
    include_opaque_sidecar: bool,
    body_capacity: u32,
) -> (
    World,
    charon_engine::foundation::id::BodyId,
    EnthalpyFamily,
    ReactionRuleSet,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
    StableKey,
) {
    let layer_key = StableKey::parse("reaction:terrain").unwrap();
    let ice_key = StableKey::parse("reaction:ice").unwrap();
    let ash_key = StableKey::parse("reaction:ash").unwrap();
    let water_key = StableKey::parse("reaction:water").unwrap();
    let sidecar_key = StableKey::parse("reaction:state").unwrap();
    let sidecar_schema = StableKey::parse("reaction:state-schema").unwrap();
    let thermal_key = StableKey::parse("reaction:thermal").unwrap();
    let thermal_schema = StableKey::parse("reaction:thermal-schema").unwrap();
    let motion_key = StableKey::parse("reaction:motion").unwrap();
    let quantity_key = StableKey::parse("reaction:quantity").unwrap();
    let quantity_schema = StableKey::parse("reaction:quantity-schema").unwrap();
    let job_key = StableKey::parse("reaction:job-state").unwrap();
    let job_schema = StableKey::parse("reaction:job-schema").unwrap();
    let schedule_state_key = StableKey::parse("reaction:schedule-state").unwrap();
    let schedule_state_schema = StableKey::parse("reaction:schedule-state-schema").unwrap();
    let opaque_key = StableKey::parse("reaction:opaque").unwrap();
    let opaque_schema = StableKey::parse("reaction:opaque-schema").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new(ice_key.as_str(), MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new(ash_key.as_str(), MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new(water_key.as_str(), MaterialBase::Liquid)
                .unwrap()
                .motion_properties(MaterialMotionProperties {
                    drag_q16: 65_536,
                    ..MaterialMotionProperties::for_base(MaterialBase::Liquid)
                }),
        )
        .unwrap();
    registry
        .schema(Schema::of::<ReactionSidecar>(sidecar_schema, 1, 14).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ThermalCellState>(thermal_schema, 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotion>(motion_key, 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(quantity_schema, 1, 8).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<ReactionJob>(job_schema, 1, 4096).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<BodyReactionScheduleState>(schedule_state_schema, 1, 65_536).unwrap())
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: sidecar_key,
                version: 1,
                value_schema: sidecar_schema,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ReactionSidecar::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: motion_key,
                version: 1,
                value_schema: motion_key,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MaterialMotion::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: thermal_key,
                version: 1,
                value_schema: thermal_schema,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: quantity_key,
                version: 1,
                value_schema: quantity_schema,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0_u64,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    let mut sidecars = Vec::new();
    let mut state = Encoder::new(14);
    ReactionSidecar {
        fuel: 4,
        timer: 2,
        ..ReactionSidecar::default()
    }
    .encode(&mut state)
    .unwrap();
    sidecars.push(BodyCellSidecar::new(sidecar_key, 1, state.finish()).unwrap());
    if include_opaque_sidecar {
        registry
            .schema(Schema::of::<u8>(opaque_schema, 1, 1).unwrap())
            .unwrap();
        registry
            .material_channel(
                layer_key,
                ChannelDefinition {
                    key: opaque_key,
                    version: 1,
                    value_schema: opaque_schema,
                    layout: GridLayout {
                        origin: point(0, 0),
                        x_scale: GridScale::new(1, 1).unwrap(),
                        y_scale: GridScale::new(1, 1).unwrap(),
                    },
                    default: 0_u8,
                    limits: GridLimits {
                        chunks: 8,
                        checkpoint_bytes: 1024,
                    },
                },
            )
            .unwrap();
        sidecars.push(BodyCellSidecar::new(opaque_key, 1, vec![7]).unwrap());
    }
    sidecars.sort_by_key(|sidecar| sidecar.key().unwrap());
    let content = registry.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = BodyLocalGrid::new(
        4,
        1,
        1.0,
        [2.0, 0.5],
        0,
        BodyLocalGridLimits {
            cells: 4,
            geometry_cells: 4,
            sidecar_bytes: 128,
        },
    )
    .unwrap();
    let second_sidecars = sidecars.clone();
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, ice_key, flags, 0, 90, sidecars).unwrap(),
    )
    .unwrap();
    grid.set_cell(
        2,
        0,
        BodyLocalCell::new(layer_key, ice_key, flags, 0, 90, second_sidecars).unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Cuboid {
                    half_extents: [2.0, 0.5],
                }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("reaction:family").unwrap(),
        vec![
            EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 1000,
                material: ice_key,
            },
            EnthalpyBand::Latent {
                energy_start: 100,
                energy_end: 101,
                temperature_mk: 1000,
                from: ice_key,
                to: ash_key,
            },
            EnthalpyBand::Sensible {
                energy_start: 101,
                energy_end: 200,
                temperature_start_mk: 1000,
                temperature_end_mk: 2000,
                material: ash_key,
            },
        ],
    )
    .unwrap();
    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:burn").unwrap(),
            input: ice_key,
            contact: None,
            priority: 0,
            consume: 1,
            output: ash_key,
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 0,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 20,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::MigrateToOutput,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    (
        world,
        body,
        family,
        rules,
        layer_key,
        sidecar_key,
        job_key,
        job_schema,
        schedule_state_key,
        schedule_state_schema,
    )
}

fn body_particle_reaction_rules(family: &EnthalpyFamily) -> ReactionRuleSet {
    ReactionRuleSet::new(
        family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:burn-particle").unwrap(),
            input: StableKey::parse("reaction:ice").unwrap(),
            contact: None,
            priority: 0,
            consume: 1,
            output: StableKey::parse("reaction:ash").unwrap(),
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: Some([0, 0]),
            minimum_temperature_mk: 0,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 20,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::MigrateToOutput,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap()
}

#[test]
fn body_local_reaction_schedule_resumes_after_restore_and_skips_unchanged_grid() {
    let (
        mut world,
        body,
        _family,
        rules,
        layer_key,
        sidecar,
        _job_key,
        _job_schema,
        schedule_state_key,
        schedule_state_schema,
    ) = body_reaction_fixture(0, false);
    let collision_layer = world.content().layer_id(layer_key).unwrap();
    world
        .edit(|edit| {
            for y in -1..=1 {
                for x in -1..=1 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            Ok(())
        })
        .unwrap();
    let schedule = BodyReactionSchedule {
        schedule_key: StableKey::parse("reaction:scheduled-burn").unwrap(),
        version: 1,
        state_key: schedule_state_key,
        state_schema: schedule_state_schema,
        body,
        layer: layer_key,
        reaction_sidecar: sidecar,
        sidecar_version: 1,
        rules: &rules,
        candidate_limit: 16,
        budget: 1,
        particle_output: None,
    };

    world
        .edit_bodies(|physics| {
            physics.set_velocity(
                body,
                BodyVelocity {
                    x: 10_000.0,
                    y: 0.0,
                    angular: 0.0,
                },
            )
        })
        .unwrap();
    let before_failed_step = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .step_bodies_with_reactions(collision_layer, 1.0, std::slice::from_ref(&schedule))
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_failed_step);

    world
        .edit_bodies(|physics| physics.set_velocity(body, BodyVelocity::default()))
        .unwrap();
    let before_post_step_failure = world.checkpoint(4_000_000).unwrap();
    let invalid_schedule = BodyReactionSchedule {
        state_schema: StableKey::parse("reaction:missing-schedule-schema").unwrap(),
        ..schedule
    };
    assert!(
        world
            .step_bodies_with_reactions(
                collision_layer,
                1.0,
                std::slice::from_ref(&invalid_schedule)
            )
            .is_err()
    );
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        before_post_step_failure
    );
    let before_successful_step = world.revision();
    let (first, _) = world
        .step_bodies_with_reactions(collision_layer, 1.0, std::slice::from_ref(&schedule))
        .unwrap();
    assert!(world.revision() > before_successful_step);
    assert_eq!(first[0].reaction.committed, 1);
    assert_eq!(
        first[0].status,
        charon_engine::runtime::schedule::WorkStatus::Deferred
    );

    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let content = Arc::clone(world.content());
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    let (second, _) = restored
        .step_bodies_with_reactions(collision_layer, 1.0, std::slice::from_ref(&schedule))
        .unwrap();
    assert_eq!(second[0].reaction.committed, 1);
    assert_eq!(
        second[0].status,
        charon_engine::runtime::schedule::WorkStatus::Complete
    );
    let (unchanged, _) = restored
        .step_bodies_with_reactions(collision_layer, 1.0, std::slice::from_ref(&schedule))
        .unwrap();
    assert_eq!(
        unchanged[0].status,
        charon_engine::runtime::schedule::WorkStatus::Complete
    );
    assert_eq!(unchanged[0].reaction, Default::default());
}

#[test]
fn body_surface_heat_precedes_reaction_schedule_in_fixed_step() {
    let (
        mut world,
        body,
        family,
        _,
        layer_key,
        sidecar,
        _job_key,
        _job_schema,
        schedule_state_key,
        schedule_state_schema,
    ) = body_reaction_fixture_with_capacity(0, false, 4);
    let layer = world.content().layer_id(layer_key).unwrap();
    let mut heated_grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .clone();
    let source = heated_grid.cell(0, 0).unwrap();
    heated_grid
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                layer_key,
                source.material().unwrap(),
                source.flags(),
                source.variation(),
                94,
                source.sidecars().to_vec(),
            )
            .unwrap(),
        )
        .unwrap();
    world
        .edit_bodies(|physics| physics.replace_local_grid(body, heated_grid))
        .unwrap();
    let hot_cell = point(-1, 0);
    let ash = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("reaction:ash").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            for y in -1..=1 {
                for x in -1..=1 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            edit.set_cell(
                layer,
                hot_cell,
                CellCore {
                    material: ash,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(
                StableKey::parse("reaction:thermal")?,
                hot_cell,
                ThermalCellState { energy: 180 },
            )?;
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("reaction:hot-region")?,
                region: ChunkRegion::new(ChunkCoord::new(-1, -1)?, 2, 2)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:heated-burn").unwrap(),
            input: StableKey::parse("reaction:ice").unwrap(),
            contact: None,
            priority: 0,
            consume: 1,
            output: StableKey::parse("reaction:ash").unwrap(),
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 950,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 20,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::MigrateToOutput,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    let schedule = BodyReactionSchedule {
        schedule_key: StableKey::parse("reaction:heated-schedule").unwrap(),
        version: 1,
        state_key: schedule_state_key,
        state_schema: schedule_state_schema,
        body,
        layer: layer_key,
        reaction_sidecar: sidecar,
        sidecar_version: 1,
        rules: &rules,
        candidate_limit: 16,
        budget: 1,
        particle_output: None,
    };
    let before_heat = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .energy();
    let (unheated, _) = world
        .step_bodies_with_reactions(layer, 1.0, std::slice::from_ref(&schedule))
        .unwrap();
    assert_eq!(unheated[0].reaction.committed, 0);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .energy(),
        before_heat
    );

    let boundary = BodyThermalReactionBoundary {
        body,
        layer: layer_key,
        energy_channel: StableKey::parse("reaction:thermal").unwrap(),
        family: &family,
        config: ThermalStepConfig {
            conductance_numerator: 1000,
            conductance_denominator: 1,
        },
        conduct_body_interior: false,
    };
    let (heated, _) = world
        .step_bodies_with_thermal_reactions(
            layer,
            1.0,
            std::slice::from_ref(&boundary),
            std::slice::from_ref(&schedule),
        )
        .unwrap();
    assert_eq!(heated[0].reaction.committed, 1);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell(1, 0)
            .unwrap()
            .material()
            .unwrap(),
        StableKey::parse("reaction:ash").unwrap()
    );
}

fn q06_acceptance_material_tokens(world: &World, layer: LayerId) -> usize {
    let mut world_cells = 0;
    for y in -16..16 {
        for x in -16..16 {
            if let charon_engine::world::CellQuery::Loaded(view) =
                world.cell(layer, point(x, y)).unwrap()
                && view.core.material != charon_engine::world::MaterialId::AIR
            {
                world_cells += 1;
            }
        }
    }
    let body_cells = world
        .body_physics()
        .unwrap()
        .body_ids()
        .map(|body| {
            world
                .body_physics()
                .unwrap()
                .local_grid(body)
                .unwrap()
                .map_or(0, BodyLocalGrid::owned_cell_count)
        })
        .sum::<usize>();
    world_cells + body_cells + world.material_particles().count()
}

fn q06_acceptance_total_enthalpy(world: &World, layer: LayerId, thermal: StableKey) -> i64 {
    let mut total = 0_i64;
    for y in -16..16 {
        for x in -16..16 {
            let point = point(x, y);
            if matches!(
                world.cell(layer, point).unwrap(),
                charon_engine::world::CellQuery::Loaded(view)
                    if view.core.material != charon_engine::world::MaterialId::AIR
            ) && let charon_engine::world::ChannelQuery::Loaded(value) = world
                .channel_value::<ThermalCellState>(thermal, point)
                .unwrap()
            {
                total = total.checked_add(value.energy).unwrap();
            }
        }
    }
    for body in world.body_physics().unwrap().body_ids() {
        let Some(grid) = world.body_physics().unwrap().local_grid(body).unwrap() else {
            continue;
        };
        for y in 0..1 {
            for x in 0..4 {
                if let Some(cell) = grid.cell(x, y) {
                    total = total.checked_add(cell.energy()).unwrap();
                }
            }
        }
    }
    for (particle, _) in world.material_particles() {
        let value = world
            .particle_channel_value::<ThermalCellState>(particle, thermal)
            .unwrap();
        total = total.checked_add(value.energy).unwrap();
    }
    total
}

#[test]
fn q06_acceptance_rotating_body_fluid_heat_fracture_particle_checkpoint_chain() {
    let (
        mut world,
        body,
        family,
        _default_rules,
        layer_key,
        sidecar,
        _job_key,
        _job_schema,
        schedule_state_key,
        schedule_state_schema,
    ) = body_reaction_fixture_with_capacity(0, false, 4);
    let layer = world.content().layer_id(layer_key).unwrap();
    let thermal = StableKey::parse("reaction:thermal").unwrap();
    let motion = StableKey::parse("reaction:motion").unwrap();
    let ash = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("reaction:ash").unwrap()))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("reaction:water").unwrap()))
        .unwrap();
    let pose = BodyPose::default();
    let original = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .clone();
    let mut grid = BodyLocalGrid::new(
        4,
        1,
        1.0,
        [2.0, 0.5],
        0,
        BodyLocalGridLimits {
            cells: 4,
            geometry_cells: 4,
            sidecar_bytes: 256,
        },
    )
    .unwrap();
    for (x, energy) in [(0, 90), (2, 94)] {
        let source = original.cell(x, 0).unwrap();
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(
                layer_key,
                source.material().unwrap(),
                source.flags(),
                source.variation(),
                energy,
                source.sidecars().to_vec(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    world
        .edit_bodies(|physics| {
            physics.replace_local_grid(body, grid)?;
            physics.set_pose(body, pose)?;
            physics.set_velocity(
                body,
                BodyVelocity {
                    x: 1.0,
                    y: 0.0,
                    angular: 0.5,
                },
            )
        })
        .unwrap();
    let hot_point = point(-1, 0);
    let fluid_point = point(2, 0);
    world
        .edit(|edit| {
            for y in -1..=0 {
                for x in -1..=0 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            edit.set_cell(
                layer,
                hot_point,
                CellCore {
                    material: ash,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(thermal, hot_point, ThermalCellState { energy: 180 })?;
            edit.set_cell(
                layer,
                fluid_point,
                CellCore {
                    material: water,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(
                motion,
                fluid_point,
                MaterialMotion {
                    velocity_q16: [0, -65_536],
                    remainder_q16: [0, 0],
                },
            )?;
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("reaction:acceptance-region")?,
                region: ChunkRegion::new(ChunkCoord::new(-1, -1)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let initial_tokens = q06_acceptance_material_tokens(&world, layer);
    assert_eq!(initial_tokens, 4);
    let initial_enthalpy = q06_acceptance_total_enthalpy(&world, layer, thermal);

    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:acceptance-heated-burn").unwrap(),
            input: StableKey::parse("reaction:ice").unwrap(),
            contact: None,
            priority: 0,
            consume: 1,
            output: StableKey::parse("reaction:ash").unwrap(),
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 950,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 20,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::MigrateToOutput,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    let schedule = BodyReactionSchedule {
        schedule_key: StableKey::parse("reaction:acceptance-schedule").unwrap(),
        version: 1,
        state_key: schedule_state_key,
        state_schema: schedule_state_schema,
        body,
        layer: layer_key,
        reaction_sidecar: sidecar,
        sidecar_version: 1,
        rules: &rules,
        candidate_limit: 16,
        budget: 1,
        particle_output: None,
    };
    let boundary = BodyThermalReactionBoundary {
        body,
        layer: layer_key,
        energy_channel: thermal,
        family: &family,
        config: ThermalStepConfig {
            conductance_numerator: 1000,
            conductance_denominator: 1,
        },
        conduct_body_interior: false,
    };

    let (first_reports, first_step) = world
        .step_bodies_with_thermal_reactions(
            layer,
            1.0,
            std::slice::from_ref(&boundary),
            std::slice::from_ref(&schedule),
        )
        .unwrap();

    assert_eq!(first_step.step, 1);
    assert_ne!(
        world
            .body_physics()
            .unwrap()
            .state(body)
            .unwrap()
            .pose
            .angle,
        pose.angle
    );
    assert_eq!(first_reports[0].reaction.committed, 1);
    assert_eq!(first_reports[0].reaction.declared_energy_delta, 20);
    let reacted_enthalpy = q06_acceptance_total_enthalpy(&world, layer, thermal);
    assert_eq!(reacted_enthalpy, initial_enthalpy + 20);
    assert_eq!(world.material_particles().count(), 0);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell(3, 0)
            .unwrap()
            .material()
            .unwrap(),
        StableKey::parse("reaction:ash").unwrap()
    );
    assert_eq!(
        q06_acceptance_material_tokens(&world, layer),
        initial_tokens
    );
    let fluid_motion = world
        .channel_value::<MaterialMotion>(motion, fluid_point)
        .unwrap();
    assert!(
        matches!(fluid_motion, charon_engine::world::ChannelQuery::Loaded(value) if value.velocity_q16 != [0, 0])
    );

    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let content = Arc::clone(world.content());
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
    let (resumed_reports, _) = restored
        .step_bodies_with_thermal_reactions(
            layer,
            1.0,
            std::slice::from_ref(&boundary),
            std::slice::from_ref(&schedule),
        )
        .unwrap();
    assert_eq!(resumed_reports[0].reaction.committed, 0);
    assert_eq!(
        q06_acceptance_material_tokens(&restored, layer),
        initial_tokens
    );
    assert_eq!(
        q06_acceptance_total_enthalpy(&restored, layer, thermal),
        reacted_enthalpy
    );

    let before_fracture_body = restored.body_physics().unwrap().body_ids().next().unwrap();
    let before_fracture_properties = restored
        .body_physics()
        .unwrap()
        .mass_properties(before_fracture_body)
        .unwrap();
    let before_fracture_state = restored
        .body_physics()
        .unwrap()
        .state(before_fracture_body)
        .unwrap();
    let (before_sin, before_cos) = before_fracture_state.pose.angle.sin_cos();
    let before_fracture_center = [
        before_fracture_state.pose.x + before_cos * before_fracture_properties.center_of_mass[0]
            - before_sin * before_fracture_properties.center_of_mass[1],
        before_fracture_state.pose.y
            + before_sin * before_fracture_properties.center_of_mass[0]
            + before_cos * before_fracture_properties.center_of_mass[1],
    ];
    let before_linear = [
        before_fracture_properties.mass * before_fracture_state.velocity.x,
        before_fracture_properties.mass * before_fracture_state.velocity.y,
    ];
    let before_angular = before_fracture_properties.angular_inertia
        * before_fracture_state.velocity.angular
        + before_fracture_center[0] * before_linear[1]
        - before_fracture_center[1] * before_linear[0];
    let fracture = restored
        .edit(|edit| {
            edit.fracture_body_local_grid(before_fracture_body, &SameMaterialBodyLocalBondProvider)
        })
        .unwrap();
    assert_eq!(fracture.fragments.len(), 2);
    let mut after_linear = [0.0, 0.0];
    let mut after_angular = 0.0;
    for fragment in &fracture.fragments {
        let physics = restored.body_physics().unwrap();
        let properties = physics.mass_properties(*fragment).unwrap();
        let state = physics.state(*fragment).unwrap();
        let (sin, cos) = state.pose.angle.sin_cos();
        let center = [
            state.pose.x + cos * properties.center_of_mass[0] - sin * properties.center_of_mass[1],
            state.pose.y + sin * properties.center_of_mass[0] + cos * properties.center_of_mass[1],
        ];
        let momentum = [
            properties.mass * state.velocity.x,
            properties.mass * state.velocity.y,
        ];
        after_linear[0] += momentum[0];
        after_linear[1] += momentum[1];
        after_angular += properties.angular_inertia * state.velocity.angular
            + center[0] * momentum[1]
            - center[1] * momentum[0];
    }
    assert!((after_linear[0] - before_linear[0]).abs() < 1.0e-9);
    assert!((after_linear[1] - before_linear[1]).abs() < 1.0e-9);
    assert!((after_angular - before_angular).abs() < 1.0e-8);
    assert_eq!(
        q06_acceptance_material_tokens(&restored, layer),
        initial_tokens
    );
    assert_eq!(
        q06_acceptance_total_enthalpy(&restored, layer, thermal),
        reacted_enthalpy
    );

    let particleized_body = fracture.fragments[0];
    let transfer = restored
        .edit(|edit| {
            edit.transfer_body_cells_to_material_particles(
                layer,
                particleized_body,
                &[BodyCellParticleTransfer {
                    source: [0, 0],
                    destination: point(13, 13),
                    velocity_q16: [0, 0],
                }],
                BodyCellReturnConfig {
                    thermal_channel: Some(thermal),
                    remove_empty_body: true,
                    ..BodyCellReturnConfig::default()
                },
            )
        })
        .unwrap();
    assert_eq!(transfer.cells, 1);
    assert!(transfer.body_removed);
    assert_eq!(
        q06_acceptance_material_tokens(&restored, layer),
        initial_tokens
    );
    assert_eq!(
        q06_acceptance_total_enthalpy(&restored, layer, thermal),
        reacted_enthalpy
    );
    let final_checkpoint = restored.checkpoint(4_000_000).unwrap();
    let content = Arc::clone(restored.content());
    let continued = World::restore(
        &final_checkpoint,
        content,
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(continued.checkpoint(4_000_000).unwrap(), final_checkpoint);
}

#[test]
fn author_convex_polygon_shape_is_validated_and_used_for_mass_properties() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let body = physics
        .insert(BodyDef::dynamic(ColliderShape::ConvexPolygon {
            vertices: vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]],
        }))
        .unwrap();
    let mass = physics.mass_properties(body).unwrap();
    assert!((mass.mass - 2.0).abs() < 1.0e-10);
    let snapshot = physics.snapshot().unwrap();
    let restored = BodyPhysics::restore(&snapshot).unwrap();
    assert_eq!(restored.state(body).unwrap(), physics.state(body).unwrap());
    assert_eq!(restored.mass_properties(body).unwrap(), mass);

    let invalid = BodyDef::dynamic(ColliderShape::ConvexPolygon {
        vertices: vec![[0.0, 0.0], [2.0, 0.0], [1.0, 1.0], [2.0, 2.0], [0.0, 2.0]],
    });
    assert!(physics.insert(invalid).is_err());

    let self_intersecting = BodyDef::dynamic(ColliderShape::ConvexPolygon {
        vertices: vec![
            [0.0, 3.0],
            [2.0, -3.0],
            [-3.0, 1.0],
            [3.0, 1.0],
            [-2.0, -3.0],
        ],
    });
    assert!(physics.insert(self_intersecting).is_err());
}

#[test]
fn body_local_reaction_particle_output_moves_payload_and_energy_atomically() {
    let (mut world, body, family, _rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(1, true);
    let rules = body_particle_reaction_rules(&family);
    let thermal = StableKey::parse("reaction:thermal").unwrap();
    let quantity = StableKey::parse("reaction:quantity").unwrap();
    world
        .edit(|edit| {
            for y in -1..=1 {
                for x in -1..=1 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("reaction:particle-output")?,
                region: ChunkRegion::new(ChunkCoord::new(-1, -1)?, 2, 2)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let config = BodyReactionParticleOutputConfig {
        thermal_channel: thermal,
        quantity_channel: quantity,
        reaction_sidecar: sidecar,
        world_units_per_cell: 1.0,
    };
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions_with_particle_output(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                1,
                Some(job),
                job_key,
                job_schema,
                config,
            )
        })
        .unwrap();
    assert_eq!(report.committed, 1);
    assert!(!job.is_complete());
    assert_eq!(world.material_particles().count(), 1);
    let (_, particle) = world.material_particles().next().unwrap();
    assert_eq!(particle.cell(), point(-1, 0));
    assert_eq!(particle.velocity_q16(), [0, 0]);
    assert_eq!(particle.core().flags, 1);
    assert_eq!(
        world
            .content()
            .material(particle.core().material)
            .unwrap()
            .unwrap()
            .key(),
        StableKey::parse("reaction:ash").unwrap()
    );
    let particle_handle = world
        .material_particles()
        .next()
        .map(|(handle, _)| handle)
        .unwrap();
    assert_eq!(
        world
            .particle_channel_value::<ThermalCellState>(particle_handle, thermal)
            .unwrap(),
        ThermalCellState { energy: 110 }
    );
    assert_eq!(
        world
            .particle_channel_value::<u64>(particle_handle, quantity)
            .unwrap(),
        1
    );
    assert_eq!(
        world
            .particle_channel_value::<ReactionSidecar>(particle_handle, sidecar)
            .unwrap(),
        ReactionSidecar {
            fuel: 3,
            timer: 1,
            corrosion: 2,
            ..ReactionSidecar::default()
        }
    );
    assert_eq!(
        world
            .particle_channel_value::<u8>(
                particle_handle,
                StableKey::parse("reaction:opaque").unwrap(),
            )
            .unwrap(),
        7
    );
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        1
    );
    let loaded = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(
        (0..4)
            .filter_map(|x| loaded.cell(x, 0))
            .map(BodyLocalCell::energy)
            .sum::<i64>(),
        90
    );
}

#[test]
fn body_local_reaction_particle_output_defers_on_occupied_world_destination() {
    let (mut world, body, family, _rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(0, false);
    let rules = body_particle_reaction_rules(&family);
    let thermal = StableKey::parse("reaction:thermal").unwrap();
    let quantity = StableKey::parse("reaction:quantity").unwrap();
    let layer_id = world.content().layer_id(layer).unwrap();
    let obstacle = world
        .content()
        .material_id(MaterialHandle(StableKey::parse("reaction:ash").unwrap()))
        .unwrap();
    world
        .edit(|edit| {
            for y in -1..=1 {
                for x in -1..=1 {
                    edit.insert_empty_chunk(ChunkCoord::new(x, y)?)?;
                }
            }
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("reaction:particle-output-blocked")?,
                region: ChunkRegion::new(ChunkCoord::new(-1, -1)?, 2, 2)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer_id,
                point(-1, 0),
                CellCore {
                    material: obstacle,
                    flags: 0,
                    variation: 0,
                },
            )?;
            Ok(())
        })
        .unwrap();
    let config = BodyReactionParticleOutputConfig {
        thermal_channel: thermal,
        quantity_channel: quantity,
        reaction_sidecar: sidecar,
        world_units_per_cell: 1.0,
    };
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions_with_particle_output(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                1,
                Some(job),
                job_key,
                job_schema,
                config,
            )
        })
        .unwrap();
    assert_eq!(report.committed, 0);
    assert_eq!(report.deferred, 1);
    assert_eq!(job.remaining(), 2);
    assert_eq!(world.material_particles().count(), 0);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        2
    );
}

#[test]
fn body_local_reaction_moves_energy_state_material_and_mass_atomically() {
    let (mut world, body, _family, rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(0, false);
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                1,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();
    assert!(!job.is_complete());
    assert_eq!(report.committed, 1);
    assert_eq!(report.declared_energy_delta, 20);
    assert_eq!(world.state::<ReactionJob>(job_key).unwrap(), job);
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                1,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();
    assert!(job.is_complete());
    assert_eq!(report.committed, 1);
    assert_eq!(report.declared_energy_delta, 20);
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert!(grid.cell(0, 0).is_none());
    assert!(grid.cell(2, 0).is_none());
    let output = grid.cell(1, 0).unwrap();
    let second_output = grid.cell(3, 0).unwrap();
    assert_eq!(
        output.material().unwrap(),
        StableKey::parse("reaction:ash").unwrap()
    );
    assert_eq!(output.energy(), 110);
    assert_eq!(output.tokens(), MaterialQuantity::ONE);
    assert_eq!(
        second_output.material().unwrap(),
        output.material().unwrap()
    );
    assert_eq!(second_output.energy(), 110);
    let output_state = output
        .sidecars()
        .iter()
        .find(|value| value.key().unwrap() == sidecar)
        .unwrap();
    let mut decoder = Decoder::new(output_state.payload(), 14).unwrap();
    let state = ReactionSidecar::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    assert_eq!(state.fuel, 3);
    assert_eq!(state.timer, 1);
    assert_eq!(state.corrosion, 2);
    assert_eq!(world.state::<ReactionJob>(job_key).unwrap(), job);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .mass_properties(body)
            .unwrap()
            .mass,
        4.0
    );
}

#[test]
fn body_local_reaction_migrates_opaque_sidecar_to_full_consumption_outputs() {
    let (mut world, body, _family, rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(0, true);
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                8,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();
    assert_eq!(report.committed, 2);
    assert!(job.is_complete());
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    for x in [1, 3] {
        let output = grid.cell(x, 0).unwrap();
        assert_eq!(output.flags(), 0);
        assert_eq!(
            output
                .sidecars()
                .iter()
                .find(|sidecar| sidecar.key().unwrap()
                    == StableKey::parse("reaction:opaque").unwrap())
                .unwrap()
                .payload(),
            &[7]
        );
    }
}

#[test]
fn body_local_reaction_explicit_drop_discards_author_state() {
    let (mut world, body, family, _, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(1, true);
    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:burn").unwrap(),
            input: StableKey::parse("reaction:ice").unwrap(),
            contact: None,
            priority: 0,
            consume: 1,
            output: StableKey::parse("reaction:ash").unwrap(),
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 0,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 20,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::Drop,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();

    let (_, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                8,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();

    assert_eq!(report.committed, 2);
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    for x in [1, 3] {
        let output = grid.cell(x, 0).unwrap();
        assert_eq!((output.flags(), output.variation()), (0, 0));
        assert!(output.sidecars().is_empty());
    }
}

#[test]
fn body_local_reaction_migrates_nondefault_flags_to_full_consumption_outputs() {
    let (mut world, body, _family, rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(1, false);
    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (_, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                8,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();
    assert_eq!(report.committed, 2);
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.cell(1, 0).unwrap().flags(), 1);
    assert_eq!(grid.cell(3, 0).unwrap().flags(), 1);
}

#[test]
fn body_local_reaction_keeps_author_state_on_a_partially_consumed_source() {
    let (mut world, body, family, _rules, layer, sidecar, job_key, job_schema, _, _) =
        body_reaction_fixture(1, true);
    let ice = StableKey::parse("reaction:ice").unwrap();
    let ash = StableKey::parse("reaction:ash").unwrap();
    let rules = ReactionRuleSet::new(
        &family,
        vec![ReactionRule {
            key: StableKey::parse("reaction:burn").unwrap(),
            input: ice,
            contact: None,
            priority: 0,
            consume: 1,
            output: ash,
            produce: 1,
            output_offset: [1, 0],
            particle_velocity_q16: None,
            minimum_temperature_mk: 0,
            fuel_cost: 1,
            timer_cost: 1,
            energy_delta: 60,
            maximum_wetness: u16::MAX,
            corrosion_delta: 2,
            state_transfer: ReactionStateTransfer::RetainSource,
            chain_policy: ReactionChainPolicy::Deferred,
        }],
    )
    .unwrap();
    let opaque = StableKey::parse("reaction:opaque").unwrap();
    let mut grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .clone();
    let first = grid.cell(0, 0).unwrap().clone();
    grid.set_cell(
        0,
        0,
        first
            .with_tokens(MaterialQuantity::new(2, 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    grid.remove_cell(2, 0).unwrap();
    world
        .edit_bodies(|physics| physics.replace_local_grid(body, grid))
        .unwrap();

    let job = world
        .edit(|edit| edit.build_body_local_reaction_job(body, layer, sidecar, 1, &rules, 16))
        .unwrap();
    let (job, report) = world
        .edit(|edit| {
            edit.step_body_local_reactions(
                body,
                layer,
                sidecar,
                1,
                &rules,
                16,
                1,
                Some(job),
                job_key,
                job_schema,
            )
        })
        .unwrap();

    assert_eq!(report.committed, 1);
    assert!(job.is_complete());
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    let source = grid.cell(0, 0).unwrap();
    assert_eq!(source.tokens(), MaterialQuantity::ONE);
    assert_eq!(source.flags(), 1);
    assert_eq!(
        source
            .sidecars()
            .iter()
            .find(|value| value.key().unwrap() == opaque)
            .unwrap()
            .payload(),
        &[7]
    );
    assert_eq!(grid.cell(1, 0).unwrap().flags(), 0);
    assert!(
        grid.cell(1, 0)
            .unwrap()
            .sidecars()
            .iter()
            .all(|value| { value.key().unwrap() != opaque })
    );
}

#[test]
fn body_local_cells_return_to_world_atomically() {
    let (mut world, layer, stone) = terrain_world();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = stone.0;
    let stone_id = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut local = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    for x in 0..2 {
        local
            .set_cell(
                x,
                0,
                BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
            )
            .unwrap();
    }
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, local))
        .unwrap();

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(5, 5),
                }],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();

    assert!(matches!(
        world.cell(layer, point(5, 5)).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone_id
    ));
    let remaining = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(remaining.cell_count(), 1);
    assert!(remaining.cell(1, 0).is_some());
}

#[test]
fn body_cell_particle_transfer_preserves_energy_and_material_sidecars() {
    let mut world = world_with_heat_channel();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone = StableKey::parse("test:stone").unwrap();
    let heat = StableKey::parse("test:heat").unwrap();
    let thermal = StableKey::parse("test:thermal").unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();

    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            layer_key,
            stone,
            3,
            11,
            77,
            vec![BodyCellSidecar::new(heat, 1, vec![42]).unwrap()],
        )
        .unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    let destination = point(5, 5);
    let report = world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_particles(
                layer,
                body,
                &[BodyCellParticleTransfer {
                    source: [0, 0],
                    destination,
                    velocity_q16: [3 * 65_536, -2 * 65_536],
                }],
                BodyCellReturnConfig {
                    thermal_channel: Some(thermal),
                    remove_empty_body: true,
                    ..BodyCellReturnConfig::default()
                },
            )
        })
        .unwrap();

    assert_eq!(report.cells, 1);
    assert!(report.body_removed);
    let particle = report.particles[0];
    let state = world.material_particle(particle).unwrap();
    assert_eq!(state.core().flags, 3);
    assert_eq!(state.core().variation, 11);
    assert_eq!(state.velocity_q16(), [3 * 65_536, -2 * 65_536]);
    assert_eq!(
        world.particle_channel_value::<u8>(particle, heat).unwrap(),
        42
    );
    assert_eq!(
        world
            .particle_channel_value::<charon_engine::world::ThermalCellState>(particle, thermal)
            .unwrap()
            .energy,
        77
    );
    assert!(world.body_physics().unwrap().local_grid(body).is_err());
    assert!(matches!(
        world.cell(layer, destination).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));

    world
        .edit(|edit| edit.land_material_particle(particle))
        .unwrap();
    assert!(matches!(
        world.cell(layer, destination).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == world.content().material_id(MaterialHandle(stone)).unwrap()
                && view.core.flags == 3
                && view.core.variation == 11
    ));
    assert!(matches!(
        world.channel_value::<u8>(heat, destination).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(42)
    ));
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::ThermalCellState>(thermal, destination)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(value) if value.energy == 77
    ));
}

#[test]
fn body_damage_particleizes_block_attachment_state_and_keeps_remaining_block() {
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let heat = StableKey::parse("test:heat").unwrap();
    let block_state = StableKey::parse("test:block-state").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .schema(Schema::of::<u8>(heat, 1, 1).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u8>(block_state, 1, 1).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:block", MaterialBase::Solid)
                .unwrap()
                .block_size(2, 1),
        )
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: heat,
                version: 1,
                value_schema: heat,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: 0_u8,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let block_material = MaterialHandle(StableKey::parse("test:block").unwrap());
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        73,
        WorldBounds::expandable(),
        Arc::clone(&content),
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let mut grid = body_grid(
        3,
        1,
        [1.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 4096,
        },
    );
    grid.set_cell(
        2,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, Vec::new()).unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let block = world
        .edit(|edit| {
            edit.place_block(
                layer,
                point(0, 0),
                block_material,
                charon_engine::world::Orientation::Original,
            )
        })
        .unwrap();
    world
        .edit(|edit| edit.set_block_state(block, block_state, block_state, &73_u8))
        .unwrap();
    world
        .edit(|edit| {
            edit.transfer_material_cells_to_body(
                layer,
                body,
                &[
                    BodyCellTransfer {
                        source: point(0, 0),
                        destination: [0, 0],
                    },
                    BodyCellTransfer {
                        source: point(1, 0),
                        destination: [1, 0],
                    },
                ],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();

    let transfer = [BodyCellParticleTransfer {
        source: [0, 0],
        destination: point(5, 5),
        velocity_q16: [0, 0],
    }];
    let before_missing_provider = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.apply_body_damage_cells_to_particles(
                layer,
                body,
                &transfer,
                1.0,
                BodyCellReturnConfig::default(),
                &SameMaterialBodyLocalBondProvider,
            )
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        before_missing_provider
    );

    let report = world
        .edit(|edit| {
            edit.apply_body_damage_cells_to_particles_with_attachment_provider(
                layer,
                body,
                &transfer,
                BodyDamageConfig {
                    world_units_per_cell: 1.0,
                    return_config: BodyCellReturnConfig::default(),
                },
                &SameMaterialBodyLocalBondProvider,
                &ParticleizeBlockState,
            )
        })
        .unwrap();
    assert_eq!(report.particle_transfer.cells, 1);
    assert_eq!(report.attachment_provider.unwrap().version, 1);
    assert_eq!(
        world
            .particle_channel_value::<u8>(report.particle_transfer.particles[0], heat)
            .unwrap(),
        73
    );

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(4, 4),
                }],
                BodyCellReturnConfig {
                    remove_empty_body: true,
                    ..BodyCellReturnConfig::default()
                },
            )
        })
        .unwrap();
    let remaining = match world.cell(layer, point(4, 4)).unwrap() {
        charon_engine::world::CellQuery::Loaded(view) => view.block.unwrap(),
        _ => panic!("remaining attachmentの復帰先が読込済みである必要があります"),
    };
    assert_eq!(world.block_state::<u8>(remaining, block_state).unwrap(), 99);
}

#[test]
fn stationary_material_particle_reattachment_preserves_world_channels_and_body_ownership() {
    let (mut world, layer, body, particle, point) = stationary_particle_body_fixture([0, 0]);
    let report = world
        .edit(|edit| {
            edit.reattach_stationary_material_particles_to_body(
                layer,
                body,
                &[BodyParticleReattachment {
                    particle,
                    destination: [1, 0],
                }],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(report.cells, 1);
    assert!(world.material_particle(particle).is_err());
    assert!(matches!(
        world.cell(layer, point).unwrap(),
        charon_engine::world::CellQuery::Loaded(view)
            if view.core.material == charon_engine::world::MaterialId::AIR
    ));
    let heat = StableKey::parse("test:heat").unwrap();
    assert!(matches!(
        world.channel_value::<u8>(heat, point).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(0)
    ));
    let cell = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(1, 0)
        .unwrap();
    assert_eq!(
        cell.material().unwrap(),
        StableKey::parse("test:stone").unwrap()
    );
    assert_eq!(cell.flags(), 2);
    assert_eq!(cell.variation(), 9);
    assert_eq!(cell.sidecars().len(), 1);
    assert_eq!(cell.sidecars()[0].key().unwrap(), heat);
    assert_eq!(cell.sidecars()[0].payload(), &[42]);
}

#[test]
fn moving_particle_reattachment_preserves_linear_and_angular_momentum() {
    let (mut world, layer, body, particle, _) = stationary_particle_body_fixture([65_536, 0]);
    world
        .edit_bodies(|physics| {
            physics.set_velocity(
                body,
                BodyVelocity {
                    x: 2.0,
                    y: -3.0,
                    angular: 0.5,
                },
            )
        })
        .unwrap();
    let particle_state = world.material_particle(particle).unwrap();
    let position_q16 = particle_state.position_q16();
    let velocity_q16 = particle_state.velocity_q16();
    let material = world
        .content()
        .material(particle_state.core().material)
        .unwrap()
        .unwrap();
    let particle_mass = f64::from(material.mass_per_cell());
    let old_state = world.body_physics().unwrap().state(body).unwrap();
    let old_mass = world.body_physics().unwrap().mass_properties(body).unwrap();
    let old_center = [
        old_state.pose.x + old_mass.center_of_mass[0],
        old_state.pose.y + old_mass.center_of_mass[1],
    ];
    let particle_position = [
        position_q16[0] as f64 / 65_536.0,
        position_q16[1] as f64 / 65_536.0,
    ];
    let particle_velocity = [
        velocity_q16[0] as f64 * 60.0 / 65_536.0,
        velocity_q16[1] as f64 * 60.0 / 65_536.0,
    ];
    let expected_momentum = [
        old_mass.mass * old_state.velocity.x + particle_mass * particle_velocity[0],
        old_mass.mass * old_state.velocity.y + particle_mass * particle_velocity[1],
    ];
    let expected_angular_momentum = old_mass.angular_inertia * old_state.velocity.angular
        + old_center[0] * old_mass.mass * old_state.velocity.y
        - old_center[1] * old_mass.mass * old_state.velocity.x
        + particle_position[0] * particle_mass * particle_velocity[1]
        - particle_position[1] * particle_mass * particle_velocity[0];

    let report = world
        .edit(|edit| {
            edit.reattach_material_particles_to_body(
                layer,
                body,
                &[BodyParticleReattachment {
                    particle,
                    destination: [1, 0],
                }],
                1.0,
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(report.cells, 1);
    assert!(world.material_particle(particle).is_err());

    let final_state = world.body_physics().unwrap().state(body).unwrap();
    let final_mass = world.body_physics().unwrap().mass_properties(body).unwrap();
    let sin = final_state.pose.angle.sin();
    let cos = final_state.pose.angle.cos();
    let final_center = [
        final_state.pose.x + cos * final_mass.center_of_mass[0]
            - sin * final_mass.center_of_mass[1],
        final_state.pose.y
            + sin * final_mass.center_of_mass[0]
            + cos * final_mass.center_of_mass[1],
    ];
    let final_momentum = [
        final_mass.mass * final_state.velocity.x,
        final_mass.mass * final_state.velocity.y,
    ];
    let final_angular_momentum = final_mass.angular_inertia * final_state.velocity.angular
        + final_center[0] * final_momentum[1]
        - final_center[1] * final_momentum[0];
    assert!((expected_momentum[0] - final_momentum[0]).abs() < 1e-9);
    assert!((expected_momentum[1] - final_momentum[1]).abs() < 1e-9);
    assert!((expected_angular_momentum - final_angular_momentum).abs() < 1e-8);
}

#[test]
fn stationary_reattachment_compatibility_entry_rejects_moving_particles_atomically() {
    let (mut world, layer, body, particle, _) = stationary_particle_body_fixture([65_536, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.reattach_stationary_material_particles_to_body(
                layer,
                body,
                &[BodyParticleReattachment {
                    particle,
                    destination: [1, 0],
                }],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(
        world.material_particle(particle).unwrap().velocity_q16(),
        [65_536, 0]
    );
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        1
    );
}

#[test]
fn body_particle_capacity_failure_rolls_back_body_and_world_cell() {
    let (source_world, layer, stone) = terrain_world();
    let limits = WorldLimits {
        particles: 1,
        ..WorldLimits::default()
    };
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        source_world.content().clone(),
        limits,
    )
    .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 16,
        },
    );
    for x in 0..2 {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(layer_key, stone.0, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
    }
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let result = world.edit(|edit| {
        edit.transfer_body_cells_to_material_particles(
            layer,
            body,
            &[
                BodyCellParticleTransfer {
                    source: [0, 0],
                    destination: point(5, 5),
                    velocity_q16: [0, 0],
                },
                BodyCellParticleTransfer {
                    source: [1, 0],
                    destination: point(6, 5),
                    velocity_q16: [0, 0],
                },
            ],
            BodyCellReturnConfig {
                remove_empty_body: true,
                ..BodyCellReturnConfig::default()
            },
        )
    });

    assert!(result.is_err());
    assert!(world.material_particles().next().is_none());
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        2
    );
}

#[test]
fn body_particle_batch_handles_keep_the_input_mapping_after_coordinate_sorting() {
    let (mut world, layer, stone) = terrain_world();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 16,
        },
    );
    for x in 0..2 {
        grid.set_cell(
            x,
            0,
            BodyLocalCell::new(layer_key, stone.0, 0, 0, 0, Vec::new()).unwrap(),
        )
        .unwrap();
    }
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();

    let report = world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_particles(
                layer,
                body,
                &[
                    BodyCellParticleTransfer {
                        source: [0, 0],
                        destination: point(6, 5),
                        velocity_q16: [7, 0],
                    },
                    BodyCellParticleTransfer {
                        source: [1, 0],
                        destination: point(5, 5),
                        velocity_q16: [-3, 0],
                    },
                ],
                BodyCellReturnConfig {
                    remove_empty_body: true,
                    ..BodyCellReturnConfig::default()
                },
            )
        })
        .unwrap();

    assert_eq!(report.particles.len(), 2);
    assert_eq!(
        world
            .material_particle(report.particles[0])
            .unwrap()
            .position_q16(),
        [6 * 65_536 + 32_768, 5 * 65_536 + 32_768]
    );
    assert_eq!(
        world
            .material_particle(report.particles[0])
            .unwrap()
            .velocity_q16(),
        [7, 0]
    );
    assert_eq!(
        world
            .material_particle(report.particles[1])
            .unwrap()
            .position_q16(),
        [5 * 65_536 + 32_768, 5 * 65_536 + 32_768]
    );
    assert_eq!(
        world
            .material_particle(report.particles[1])
            .unwrap()
            .velocity_q16(),
        [-3, 0]
    );
}

#[test]
fn returning_the_last_cell_requires_explicit_body_removal() {
    let (mut world, layer, stone) = terrain_world();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = stone.0;
    let stone_id = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut local = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 64,
        },
    );
    local
        .set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, local))
        .unwrap();
    let transfer = [BodyCellReturn {
        source: [0, 0],
        destination: point(5, 5),
    }];

    assert!(
        world
            .edit(|edit| {
                edit.transfer_body_cells_to_material_cells(
                    layer,
                    body,
                    &transfer,
                    BodyCellReturnConfig::default(),
                )
            })
            .is_err()
    );
    assert!(matches!(
        world.cell(layer, point(5, 5)).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));

    let report = world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &transfer,
                BodyCellReturnConfig {
                    remove_empty_body: true,
                    ..BodyCellReturnConfig::default()
                },
            )
        })
        .unwrap();
    assert_eq!(
        report,
        charon_engine::world::BodyCellReturnReport {
            cells: 1,
            body_removed: true
        }
    );
    assert!(matches!(
        world.cell(layer, point(5, 5)).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone_id
    ));
    assert!(world.body_physics().unwrap().local_grid(body).is_err());
}

#[test]
fn body_local_cell_sidecar_returns_to_its_material_channel() {
    let mut world = world_with_heat_channel();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let heat_key = StableKey::parse("test:heat").unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(stone_key))
        .unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut local = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    local
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                layer_key,
                stone_key,
                0,
                0,
                0,
                vec![BodyCellSidecar::new(heat_key, 1, vec![42]).unwrap()],
            )
            .unwrap(),
        )
        .unwrap();
    local
        .set_cell(
            1,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, local))
        .unwrap();

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(5, 5),
                }],
                BodyCellReturnConfig::default(),
            )
        })
        .unwrap();

    assert!(matches!(
        world.cell(layer, point(5, 5)).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    assert!(matches!(
        world.channel_value::<u8>(heat_key, point(5, 5)).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(42)
    ));
}

#[test]
fn body_local_cell_energy_returns_to_thermal_channel() {
    let mut world = world_with_heat_channel();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let thermal_key = StableKey::parse("test:thermal").unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut local = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    local
        .set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 321, vec![]).unwrap(),
        )
        .unwrap();
    local
        .set_cell(
            1,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, local))
        .unwrap();

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(5, 5),
                }],
                BodyCellReturnConfig {
                    mixture_channel: None,
                    mixture_schema: None,
                    thermal_channel: Some(thermal_key),
                    remove_empty_body: false,
                },
            )
        })
        .unwrap();
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::ThermalCellState>(thermal_key, point(5, 5))
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(charon_engine::world::ThermalCellState {
            energy: 321
        })
    ));
}

#[test]
fn body_mixture_returns_to_world_with_exact_rational_components() {
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let dense_key = StableKey::parse("test:dense").unwrap();
    let mixture_key = StableKey::parse("test:composition").unwrap();
    let mixture_schema = StableKey::parse("test:mixture-store").unwrap();
    let mixture_value_schema = StableKey::parse("test:mixture-ref").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:stone", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(1),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:dense", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    registry
        .schema(Schema::of::<MixtureRef>(mixture_value_schema, 1, 32).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MixtureStore>(mixture_schema, 1, 65_536).unwrap())
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: mixture_key,
                version: 1,
                value_schema: mixture_value_schema,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MixtureRef::NONE,
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 65_536,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let dense_id = content.material_id(MaterialHandle(dense_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 8,
            components: 8,
            receipts: 8,
        },
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_state(MixtureStore::key(), mixture_schema, &store)
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut local = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    local
        .set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, dense_key, 0, 0, 0, vec![])
                .unwrap()
                .with_mixture(vec![
                    BodyMaterialComponent::new(stone_key, MaterialQuantity::ONE).unwrap(),
                    BodyMaterialComponent::new(dense_key, MaterialQuantity::new(1, 2).unwrap())
                        .unwrap(),
                ])
                .unwrap(),
        )
        .unwrap();
    local
        .set_cell(
            1,
            0,
            BodyLocalCell::new(layer_key, dense_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, local))
        .unwrap();

    world
        .edit(|edit| {
            edit.transfer_body_cells_to_material_cells(
                layer,
                body,
                &[BodyCellReturn {
                    source: [0, 0],
                    destination: point(5, 5),
                }],
                BodyCellReturnConfig {
                    mixture_channel: Some(mixture_key),
                    mixture_schema: Some(mixture_schema),
                    thermal_channel: None,
                    remove_empty_body: false,
                },
            )
        })
        .unwrap();

    let reference = match world
        .channel_value::<MixtureRef>(mixture_key, point(5, 5))
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(reference) => reference,
        _ => panic!("混合参照がWorld cellへ戻っていない"),
    };
    let store: MixtureStore = world.state(MixtureStore::key()).unwrap();
    let quantities = store
        .quantities(world.content(), dense_id, reference)
        .unwrap();
    assert_eq!(quantities.len(), 2);
    assert_eq!(quantities[0].material, dense_key);
    assert_eq!(quantities[0].tokens, MaterialQuantity::new(1, 2).unwrap());
    assert_eq!(quantities[1].material, stone_key);
    assert_eq!(quantities[1].tokens, MaterialQuantity::ONE);
}

#[test]
fn terrain_material_cell_transfer_expands_mixture_store_into_exact_body_amounts() {
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let dense_key = StableKey::parse("test:dense").unwrap();
    let mixture_key = StableKey::parse("test:composition").unwrap();
    let mixture_schema = StableKey::parse("test:mixtures").unwrap();
    let mixture_value_schema = StableKey::parse("test:mixture-ref").unwrap();
    let thermal_key = StableKey::parse("test:thermal").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:dense", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    registry
        .schema(
            Schema::of::<charon_engine::world::MixtureRef>(mixture_value_schema, 1, 32).unwrap(),
        )
        .unwrap();
    registry
        .schema(Schema::of::<charon_engine::world::MixtureStore>(mixture_schema, 1, 4096).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<charon_engine::world::ThermalCellState>(thermal_key, 1, 32).unwrap())
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: mixture_key,
                version: 1,
                value_schema: mixture_value_schema,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: charon_engine::world::MixtureRef::NONE,
                limits: GridLimits {
                    chunks: 4,
                    checkpoint_bytes: 4096,
                },
            },
        )
        .unwrap();
    registry
        .material_channel(
            layer_key,
            ChannelDefinition {
                key: thermal_key,
                version: 1,
                value_schema: thermal_key,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: charon_engine::world::ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 4,
                    checkpoint_bytes: 4096,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let dense = content.material_id(MaterialHandle(dense_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let store = charon_engine::world::MixtureStore::new(
        &world,
        charon_engine::world::MixtureLimits {
            entries: 4,
            components: 4,
            receipts: 4,
        },
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_state(
                charon_engine::world::MixtureStore::key(),
                mixture_schema,
                &store,
            )
        })
        .unwrap();
    let reference = world
        .edit(|edit| {
            edit.register_mixture(
                mixture_schema,
                &charon_engine::world::MixtureState::new(&[
                    charon_engine::world::MixturePart {
                        material: stone_key,
                        ratio: 1,
                    },
                    charon_engine::world::MixturePart {
                        material: dense_key,
                        ratio: 1,
                    },
                ])?,
            )
        })
        .unwrap();
    let source = point(3, 4);
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                source,
                CellCore {
                    material: dense,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(mixture_key, source, reference)?;
            edit.set_channel(
                thermal_key,
                source,
                charon_engine::world::ThermalCellState { energy: 321 },
            )
        })
        .unwrap();

    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut initial = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 32,
        },
    );
    initial
        .set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, initial))
        .unwrap();

    let error = world
        .edit(|edit| {
            edit.transfer_material_cell_to_body(
                layer,
                source,
                body,
                [1, 0],
                BodyCellTransferConfig {
                    mixture_channel: Some(mixture_key),
                    thermal_channel: None,
                },
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == dense
    ));

    world
        .edit(|edit| {
            edit.transfer_material_cell_to_body(
                layer,
                source,
                body,
                [1, 0],
                BodyCellTransferConfig {
                    mixture_channel: Some(mixture_key),
                    thermal_channel: Some(thermal_key),
                },
            )
        })
        .unwrap();

    assert!(matches!(
        world.cell(layer, source).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
    ));
    assert!(matches!(
        world.channel_value::<charon_engine::world::MixtureRef>(mixture_key, source).unwrap(),
        charon_engine::world::ChannelQuery::Loaded(value) if value.is_none()
    ));
    assert!(matches!(
        world
            .channel_value::<charon_engine::world::ThermalCellState>(thermal_key, source)
            .unwrap(),
        charon_engine::world::ChannelQuery::Loaded(charon_engine::world::ThermalCellState {
            energy: 0
        })
    ));
    let transferred = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(1, 0)
        .unwrap();
    assert_eq!(transferred.material().unwrap(), dense_key);
    assert_eq!(transferred.energy(), 321);
    assert_eq!(transferred.mixture().len(), 2);
    assert_eq!(transferred.mixture()[0].material().unwrap(), dense_key);
    assert_eq!(
        transferred.mixture()[0].tokens(),
        MaterialQuantity::new(2, 3).unwrap()
    );
    assert_eq!(transferred.mixture()[1].material().unwrap(), stone_key);
    assert_eq!(
        transferred.mixture()[1].tokens(),
        MaterialQuantity::new(2, 3).unwrap()
    );
}

#[test]
fn body_component_search_resumes_from_codec_and_transfers_atomically() {
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let dense_key = StableKey::parse("test:dense").unwrap();
    let job_record = StableKey::parse("test:body-component-job").unwrap();
    let job_schema = StableKey::parse("test:body-component-job-schema").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer_key,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:stone", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:dense", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    registry
        .schema(Schema::of::<BodyComponentSearchJob>(job_schema, 1, 65_536).unwrap())
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(layer_key).unwrap();
    let stone = content.material_id(MaterialHandle(stone_key)).unwrap();
    let dense = content.material_id(MaterialHandle(dense_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    let seed = point(10, 10);
    let component_points = [seed, point(11, 10), point(11, 11)];
    let disconnected = point(13, 11);
    let boundary_material = point(12, 10);
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0).unwrap())?;
            for point in component_points {
                edit.set_cell(
                    layer,
                    point,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
            }
            edit.set_cell(
                layer,
                disconnected,
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_cell(
                layer,
                boundary_material,
                CellCore {
                    material: dense,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();

    world
        .edit(|edit| {
            edit.begin_body_component_search(
                job_record,
                job_schema,
                layer,
                seed,
                16,
                &SameMaterialBodyBondProvider,
            )
        })
        .unwrap();
    let first_status = world
        .edit(|edit| {
            edit.step_body_component_search(
                job_record,
                job_schema,
                1,
                &SameMaterialBodyBondProvider,
            )
        })
        .unwrap();
    assert_eq!(first_status, BodyComponentSearchStatus::Searching);
    let saved_job: BodyComponentSearchJob = world.state(job_record).unwrap();
    let mut encoder = Encoder::new(65_536);
    saved_job.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut decoder = Decoder::new(&bytes, 65_536).unwrap();
    let restored_job = BodyComponentSearchJob::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    world
        .edit(|edit| edit.set_state(job_record, job_schema, &restored_job))
        .unwrap();

    for _ in 0..32 {
        let status = world
            .edit(|edit| {
                edit.step_body_component_search(
                    job_record,
                    job_schema,
                    2,
                    &SameMaterialBodyBondProvider,
                )
            })
            .unwrap();
        if status == BodyComponentSearchStatus::Ready {
            break;
        }
    }
    let ready: BodyComponentSearchJob = world.state(job_record).unwrap();
    assert_eq!(ready.status(), BodyComponentSearchStatus::Ready);
    assert_eq!(ready.component_cells(), component_points.len());

    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut initial = body_grid(
        4,
        4,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 8,
            geometry_cells: 16,
            sidecar_bytes: 128,
        },
    );
    initial
        .set_cell(
            0,
            0,
            BodyLocalCell::new(layer_key, stone_key, 0, 0, 0, vec![]).unwrap(),
        )
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 30.0;
    definition.pose.y = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, initial))
        .unwrap();

    let outcome = world
        .edit(|edit| {
            edit.transfer_body_component_job(
                job_record,
                job_schema,
                body,
                [1, 1],
                BodyCellTransferConfig::default(),
            )
        })
        .unwrap();
    assert_eq!(
        outcome,
        BodyComponentTransferOutcome::Transferred {
            cells: component_points.len()
        }
    );
    let transferred: BodyComponentSearchJob = world.state(job_record).unwrap();
    assert_eq!(transferred.status(), BodyComponentSearchStatus::Transferred);
    for point in component_points {
        assert!(matches!(
            world.cell(layer, point).unwrap(),
            charon_engine::world::CellQuery::Loaded(view) if view.core == CellCore::AIR
        ));
    }
    assert!(matches!(
        world.cell(layer, disconnected).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == stone
    ));
    assert!(matches!(
        world.cell(layer, boundary_material).unwrap(),
        charon_engine::world::CellQuery::Loaded(view) if view.core.material == dense
    ));
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.cell_count(), 4);
    assert!(grid.cell(1, 1).is_some());
    assert!(grid.cell(2, 1).is_some());
    assert!(grid.cell(2, 2).is_some());
}

#[test]
fn body_component_search_waits_for_unavailable_chunks_and_resumes() {
    let (mut world, layer, stone) = terrain_world();
    let seed = point(63, 10);
    let stone_id = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                seed,
                CellCore {
                    material: stone_id,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    let mut job =
        BodyComponentSearchJob::new(&world, layer, seed, 8, &SameMaterialBodyBondProvider).unwrap();

    assert_eq!(
        job.step(&world, 16, &SameMaterialBodyBondProvider).unwrap(),
        BodyComponentSearchStatus::WaitingForRegion
    );
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(1, 0)?))
        .unwrap();
    assert_eq!(
        job.step(&world, 16, &SameMaterialBodyBondProvider).unwrap(),
        BodyComponentSearchStatus::Ready
    );
    assert_eq!(job.component_cells(), 1);
}

#[test]
fn body_component_search_invalidates_when_an_input_chunk_changes() {
    let (mut world, layer, stone) = terrain_world();
    let seed = point(5, 5);
    let stone_id = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                seed,
                CellCore {
                    material: stone_id,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    let mut job =
        BodyComponentSearchJob::new(&world, layer, seed, 8, &SameMaterialBodyBondProvider).unwrap();

    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                seed,
                CellCore {
                    material: charon_engine::world::MaterialId::AIR,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    assert_eq!(
        job.step(&world, 16, &SameMaterialBodyBondProvider).unwrap(),
        BodyComponentSearchStatus::Invalidated
    );
}

#[test]
fn body_component_search_waits_when_a_previously_seen_chunk_is_evicted() {
    let (mut world, layer, stone) = terrain_world();
    let seed = point(5, 5);
    let stone_id = world.content().material_id(stone).unwrap();
    let chunk = ChunkCoord::new(0, 0).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(chunk)?;
            edit.set_cell(
                layer,
                seed,
                CellCore {
                    material: stone_id,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    let mut job =
        BodyComponentSearchJob::new(&world, layer, seed, 8, &SameMaterialBodyBondProvider).unwrap();
    assert_eq!(
        job.step(&world, 1, &SameMaterialBodyBondProvider).unwrap(),
        BodyComponentSearchStatus::Searching
    );

    let mut archives = MemoryArchives::default();
    let stored = world
        .prepare_eviction(ChunkRegion::single(chunk).unwrap(), 1_000_000)
        .unwrap()
        .store(&mut archives)
        .unwrap();
    world.edit(|edit| edit.finish_eviction(stored)).unwrap();

    assert_eq!(
        job.step(&world, 1, &SameMaterialBodyBondProvider).unwrap(),
        BodyComponentSearchStatus::WaitingForRegion
    );
}

#[test]
fn public_body_ids_stay_stale_after_removal_and_snapshot_restore() {
    let mut world = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        body_capacity: 2,
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let first = world
        .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))
        .unwrap();
    world.remove(first).unwrap();
    let second = world
        .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }))
        .unwrap();
    assert_ne!(first, second);

    let mut restored = BodyPhysics::restore(&world.snapshot().unwrap()).unwrap();
    assert!(restored.state(first).is_err());
    assert_eq!(restored.state(second), world.state(second));
    restored.remove(second).unwrap();
    assert_eq!(restored.body_count(), 0);
}

#[test]
fn body_local_grid_owns_material_energy_and_sidecars_across_snapshot_restore() {
    let mut grid = body_grid(
        2,
        1,
        [1.0, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    let sidecar =
        BodyCellSidecar::new(StableKey::parse("test:heat").unwrap(), 3, vec![7, 8, 9]).unwrap();
    let cell = BodyLocalCell::new(
        StableKey::parse("test:terrain").unwrap(),
        StableKey::parse("test:stone").unwrap(),
        2,
        17,
        1234,
        vec![sidecar],
    )
    .unwrap()
    .with_tokens(MaterialQuantity::new(3, 2).unwrap())
    .unwrap()
    .with_mixture(vec![
        BodyMaterialComponent::new(
            StableKey::parse("test:stone").unwrap(),
            MaterialQuantity::ONE,
        )
        .unwrap(),
        BodyMaterialComponent::new(
            StableKey::parse("test:sand").unwrap(),
            MaterialQuantity::new(1, 2).unwrap(),
        )
        .unwrap(),
    ])
    .unwrap();
    grid.set_cell(0, 0, cell.clone()).unwrap();
    let error = grid
        .set_cell(
            1,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(grid.cell_count(), 1);
    assert_eq!(grid.cell(0, 0), Some(&cell));

    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 10.0;
    definition.pose.y = 10.0;
    let id = physics.insert_with_local_grid(definition, grid).unwrap();
    assert_eq!(
        physics.local_grid(id).unwrap().unwrap().cell(0, 0),
        Some(&cell)
    );

    let before_rejected_replacement = physics.snapshot().unwrap();
    let empty = body_grid(
        2,
        1,
        [1.0, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    assert!(physics.replace_local_grid(id, empty).is_err());
    assert_eq!(physics.snapshot().unwrap(), before_rejected_replacement);

    let mut expanded = body_grid(
        2,
        1,
        [1.0, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    expanded.set_cell(0, 0, cell.clone()).unwrap();
    expanded.set_cell(1, 0, cell.clone()).unwrap();
    physics.replace_local_grid(id, expanded).unwrap();
    assert_eq!(physics.local_grid(id).unwrap().unwrap().cell_count(), 2);
    let bytes = physics.snapshot().unwrap();
    let restored = BodyPhysics::restore(&bytes).unwrap();
    assert_eq!(restored.local_grid(id).unwrap().unwrap().cell_count(), 2);
    assert_eq!(
        restored.local_grid(id).unwrap().unwrap().cell(0, 0),
        Some(&cell)
    );
    assert_eq!(restored.snapshot().unwrap(), bytes);
    assert_eq!(
        restored
            .local_grid(id)
            .unwrap()
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .tokens(),
        MaterialQuantity::new(3, 2).unwrap()
    );
    assert_eq!(
        restored
            .local_grid(id)
            .unwrap()
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .mixture()
            .len(),
        2
    );
}

#[test]
fn world_body_mass_center_and_inertia_follow_exact_material_quantities() {
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: StableKey::parse("test:terrain").unwrap(),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    let stone = StableKey::parse("test:stone").unwrap();
    let dense = StableKey::parse("test:dense").unwrap();
    registry
        .register(
            MaterialBuilder::new("test:stone", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(4),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:dense", MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(8),
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content.clone(),
        WorldLimits::default(),
    )
    .unwrap();
    let mut grid = body_grid(
        3,
        1,
        [100_000_000.0, 0.0],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 3,
            sidecar_bytes: 1,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            stone,
            0,
            0,
            0,
            vec![],
        )
        .unwrap(),
    )
    .unwrap();
    grid.set_cell(
        2,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            dense,
            0,
            0,
            0,
            vec![],
        )
        .unwrap()
        .with_tokens(MaterialQuantity::new(3, 2).unwrap())
        .unwrap()
        .with_mixture(vec![
            BodyMaterialComponent::new(stone, MaterialQuantity::ONE).unwrap(),
            BodyMaterialComponent::new(dense, MaterialQuantity::ONE).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let id = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let properties = world.body_physics().unwrap().mass_properties(id).unwrap();
    assert_eq!(properties.mass, 16.0);
    assert_eq!(properties.center_of_mass[0], -99_999_998.0);
    assert_eq!(properties.center_of_mass[1], 0.5);
    assert!((properties.angular_inertia - 44.0 / 3.0).abs() < 1e-12);
    world
        .edit_bodies(|physics| physics.apply_impulse(id, [12.0, 0.0]))
        .unwrap();
    assert!((world.body_physics().unwrap().state(id).unwrap().velocity.x - 0.75).abs() < 1e-12);

    let before_invalid_mixture = world.checkpoint(4_000_000).unwrap();
    let mut invalid_grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 1,
        },
    );
    invalid_grid
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                stone,
                0,
                0,
                0,
                vec![],
            )
            .unwrap()
            .with_mixture(vec![
                BodyMaterialComponent::new(stone, MaterialQuantity::ONE).unwrap(),
                BodyMaterialComponent::new(dense, MaterialQuantity::ONE).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    let error = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                invalid_grid,
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_invalid_mixture);

    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .mass_properties(id)
            .unwrap(),
        properties
    );
}

#[test]
fn body_local_grid_provider_contributes_exact_material_amounts_to_world_inventory() {
    let (mut world, _layer, stone) = terrain_world();
    let material = world.content().material_id(stone).unwrap();
    let mass_per_cell = world
        .content()
        .material(material)
        .unwrap()
        .unwrap()
        .mass_per_cell();
    let dense = StableKey::parse("test:dense").unwrap();
    let dense_id = world.content().material_id(MaterialHandle(dense)).unwrap();
    let dense_mass_per_cell = world
        .content()
        .material(dense_id)
        .unwrap()
        .unwrap()
        .mass_per_cell();
    let profile = MaterialInventoryProfile::new(
        StableKey::parse("test:inventory").unwrap(),
        1,
        Vec::new(),
        vec![Arc::new(BodyLocalGridInventoryProvider::new().unwrap())],
        MaterialInventoryLimits {
            atoms: 32,
            components: 8,
        },
    )
    .unwrap();
    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.25],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap()
        .with_tokens(MaterialQuantity::new(3, 2).unwrap())
        .unwrap(),
    )
    .unwrap();
    grid.set_cell(
        1,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            dense,
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap()
        .with_tokens(MaterialQuantity::new(3, 2).unwrap())
        .unwrap()
        .with_mixture(vec![
            BodyMaterialComponent::new(
                StableKey::parse("test:stone").unwrap(),
                MaterialQuantity::ONE,
            )
            .unwrap(),
            BodyMaterialComponent::new(dense, MaterialQuantity::ONE).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )?;
            Ok(())
        })
        .unwrap();

    let archives = world
        .load_material_archive_inventory(&profile, &mut EmptyArchives, 1_000_000)
        .unwrap();
    let inventory = world.material_inventory(&profile, &archives).unwrap();
    let amount = inventory
        .quantities()
        .iter()
        .find(|amount| amount.material == StableKey::parse("test:stone").unwrap())
        .unwrap();

    assert_eq!(amount.tokens, MaterialQuantity::new(5, 2).unwrap());
    assert_eq!(
        amount.mass,
        MaterialQuantity::new(u128::from(mass_per_cell) * 5, 2).unwrap()
    );
    let dense_amount = inventory
        .quantities()
        .iter()
        .find(|amount| amount.material == dense)
        .unwrap();
    assert_eq!(dense_amount.tokens, MaterialQuantity::ONE);
    assert_eq!(
        dense_amount.mass,
        MaterialQuantity::new(u128::from(dense_mass_per_cell), 1).unwrap()
    );
}

#[test]
fn body_local_cells_apply_saved_material_remaps_and_update_inventory_epoch_and_mass() {
    let (mut world, _layer, stone) = terrain_world();
    let grain = StableKey::parse("test:grain").unwrap();
    let dense = StableKey::parse("test:dense").unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 256,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            stone.0,
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap()
        .with_tokens(MaterialQuantity::new(2, 1).unwrap())
        .unwrap()
        .with_mixture(vec![
            BodyMaterialComponent::new(stone.0, MaterialQuantity::ONE).unwrap(),
            BodyMaterialComponent::new(grain, MaterialQuantity::ONE).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();

    let before = world.body_physics().unwrap().mass_properties(body).unwrap();
    world
        .edit(|edit| {
            edit.remap_materials(MaterialRemap {
                owner: StableKey::parse("engine:body-local-grid-inventory")?,
                layers: Vec::new(),
                mappings: vec![
                    (MaterialHandle(grain), MaterialHandle(dense)),
                    (stone, MaterialHandle(dense)),
                ],
            })
        })
        .unwrap();

    let cell = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.material().unwrap(), dense);
    assert_eq!(cell.applied_remap_epoch(), 1);
    assert!(cell.mixture().is_empty());
    assert_eq!(cell.tokens(), MaterialQuantity::new(2, 1).unwrap());
    let after = world.body_physics().unwrap().mass_properties(body).unwrap();
    assert_eq!(before.mass, 2.0);
    assert_eq!(after.mass, 4.0);

    let bytes = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &bytes,
        world.content().clone(),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .applied_remap_epoch(),
        1
    );
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), bytes);
}

#[test]
fn body_grid_added_after_a_remap_is_normalized_at_the_world_edit_boundary() {
    let (mut world, _layer, stone) = terrain_world();
    let dense = StableKey::parse("test:dense").unwrap();
    world
        .edit(|edit| {
            edit.remap_materials(MaterialRemap {
                owner: StableKey::parse("engine:body-local-grid-inventory")?,
                layers: Vec::new(),
                mappings: vec![(stone, MaterialHandle(dense))],
            })
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 16,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            stone.0,
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    let cell = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.material().unwrap(), dense);
    assert_eq!(cell.applied_remap_epoch(), 1);
}

#[test]
fn a_late_stale_cell_invalidates_the_grid_epoch_cache() {
    let (mut world, _layer, stone) = terrain_world();
    let dense = StableKey::parse("test:dense").unwrap();
    world
        .edit(|edit| {
            edit.remap_materials(MaterialRemap {
                owner: StableKey::parse("engine:body-local-grid-inventory")?,
                layers: Vec::new(),
                mappings: vec![(stone, MaterialHandle(dense))],
            })
        })
        .unwrap();

    let mut grid = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 2,
            geometry_cells: 2,
            sidecar_bytes: 16,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            dense,
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap()
        .with_applied_remap_epoch(1),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            let mut updated = physics
                .local_grid(body)?
                .cloned()
                .expect("初期化したbody local grid");
            updated.set_cell(
                1,
                0,
                BodyLocalCell::new(
                    StableKey::parse("test:terrain")?,
                    stone.0,
                    0,
                    0,
                    0,
                    Vec::new(),
                )?,
            )?;
            physics.replace_local_grid(body, updated)
        })
        .unwrap();

    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(grid.cell(1, 0).unwrap().material().unwrap(), dense);
    assert_eq!(grid.cell(1, 0).unwrap().applied_remap_epoch(), 1);
}

#[test]
fn local_grid_aggregate_capacity_rejects_a_second_body_without_partial_insert() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        body_local_cell_capacity: 1,
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let make_grid = || {
        let mut grid = body_grid(
            1,
            1,
            [0.5, 0.5],
            BodyLocalGridLimits {
                cells: 1,
                geometry_cells: 1,
                sidecar_bytes: 64,
            },
        );
        grid.set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
        grid
    };
    physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
            make_grid(),
        )
        .unwrap();
    let before = physics.snapshot().unwrap();
    let error = physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
            make_grid(),
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(physics.body_count(), 1);
    assert_eq!(physics.snapshot().unwrap(), before);

    let mut geometry_limited = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        body_local_cell_capacity: 4,
        body_local_geometry_cell_capacity: 1,
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut sparse_but_wide = body_grid(
        2,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 2,
            sidecar_bytes: 64,
        },
    );
    sparse_but_wide
        .set_cell(
            0,
            0,
            BodyLocalCell::new(
                StableKey::parse("test:terrain").unwrap(),
                StableKey::parse("test:stone").unwrap(),
                0,
                0,
                0,
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
    let error = geometry_limited
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
            sparse_but_wide,
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(geometry_limited.body_count(), 0);
}

#[test]
fn world_rejects_local_grid_sidecars_without_a_matching_material_channel() {
    let (mut world, _, _) = terrain_world();
    let before_revision = world.revision();
    let error = world
        .edit_bodies(|physics| {
            let mut grid = body_grid(
                1,
                1,
                [0.5, 0.5],
                BodyLocalGridLimits {
                    cells: 1,
                    geometry_cells: 1,
                    sidecar_bytes: 64,
                },
            );
            grid.set_cell(
                0,
                0,
                BodyLocalCell::new(
                    StableKey::parse("test:terrain")?,
                    StableKey::parse("test:stone")?,
                    0,
                    0,
                    0,
                    vec![BodyCellSidecar::new(
                        StableKey::parse("test:missing-channel")?,
                        1,
                        vec![1],
                    )?],
                )?,
            )?;
            let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            definition.pose.x = 10.0;
            definition.pose.y = 10.0;
            physics.insert_with_local_grid(definition, grid)?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::MissingKey);
    assert_eq!(world.revision(), before_revision);
    assert!(world.body_physics().is_none());
}

#[test]
fn world_checkpoint_restores_valid_local_grid_sidecars_with_content_validation() {
    let mut world = world_with_heat_channel();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 64,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            500,
            vec![
                BodyCellSidecar::new(StableKey::parse("test:heat").unwrap(), 1, vec![42]).unwrap(),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose.x = 10.0;
    definition.pose.y = 10.0;
    let id = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let bytes = world.checkpoint(4_000_000).unwrap();
    let content = world.content().clone();
    let restored = World::restore(&bytes, content, WorldLimits::default(), 4_000_000).unwrap();
    let cell = restored
        .body_physics()
        .unwrap()
        .local_grid(id)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.energy(), 500);
    assert_eq!(cell.sidecars()[0].payload(), &[42]);
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), bytes);
}

#[test]
fn kinematic_pose_and_dynamic_velocity_have_explicit_operations() {
    let mut world = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut kinematic = BodyDef::dynamic(ColliderShape::Cuboid {
        half_extents: [0.5, 0.5],
    });
    kinematic.motion = BodyMotionType::KinematicPosition;
    let kinematic_id = world.insert(kinematic).unwrap();
    world
        .set_kinematic_target(
            kinematic_id,
            BodyPose {
                x: 2.0,
                y: -1.0,
                angle: 0.25,
            },
        )
        .unwrap();
    assert!(
        world
            .set_velocity(
                kinematic_id,
                BodyVelocity {
                    x: 1.0,
                    y: 0.0,
                    angular: 0.0,
                },
            )
            .is_err()
    );

    let dynamic_id = world
        .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }))
        .unwrap();
    world
        .set_velocity(
            dynamic_id,
            BodyVelocity {
                x: 3.0,
                y: 0.0,
                angular: 0.5,
            },
        )
        .unwrap();
    world.step().unwrap();
    let kinematic_state: BodyState = world.state(kinematic_id).unwrap();
    assert!((kinematic_state.pose.x - 2.0).abs() < 1e-9);
    assert!((kinematic_state.pose.y + 1.0).abs() < 1e-9);
    assert!(world.state(dynamic_id).unwrap().pose.x > 0.0);
}

#[test]
fn fixed_body_rejects_impulse_without_changing_its_state() {
    let mut world = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut fixed = BodyDef::dynamic(ColliderShape::Cuboid {
        half_extents: [2.0, 0.25],
    });
    fixed.motion = BodyMotionType::Fixed;
    let id = world.insert(fixed).unwrap();
    let before = world.state(id).unwrap();
    assert!(world.apply_impulse(id, [5.0, 0.0]).is_err());
    assert_eq!(world.state(id).unwrap(), before);
}

#[test]
fn public_stencil_compound_and_joint_round_trip_through_snapshot() {
    let mut world = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let stencil = BodyStencil::new(2, 1, 1.0, [1.0, 0.5], 3, vec![true, true]).unwrap();
    let mut first = BodyDef::dynamic(stencil.collider_shape().unwrap());
    first.pose.x = -0.5;
    let first_id = world.insert(first).unwrap();
    let mut second = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    second.pose.x = 1.0;
    let second_id = world.insert(second).unwrap();
    let joint = world
        .insert_joint(JointDef {
            body1: first_id,
            body2: second_id,
            local_anchor1: [1.0, 0.0],
            local_anchor2: [-0.25, 0.0],
            kind: JointKind::Revolute { limits: None },
        })
        .unwrap();
    assert!(joint.index() < u32::MAX);
    world.step().unwrap();

    let bytes = world.snapshot().unwrap();
    let restored = BodyPhysics::restore(&bytes).unwrap();
    assert_eq!(restored.joint_count(), 1);
    assert_eq!(restored.snapshot().unwrap(), bytes);
}

#[test]
fn body_material_remap_to_air_rolls_back_the_whole_world_edit() {
    let (mut world, _layer, stone) = terrain_world();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 16,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            stone.0,
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
                grid,
            )
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();

    let result = world.edit(|edit| {
        edit.remap_materials(MaterialRemap {
            owner: StableKey::parse("engine:body-local-grid-inventory")?,
            layers: Vec::new(),
            mappings: vec![(stone, MaterialHandle(StableKey::parse("engine:air")?))],
        })
    });

    assert!(result.is_err());
    assert_eq!(world.material_remaps().epoch(), 0);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    let cell = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.material().unwrap(), stone.0);
    assert_eq!(cell.applied_remap_epoch(), 0);
}

#[test]
fn body_snapshot_v3_restores_cells_with_epoch_zero() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 8,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
            grid,
        )
        .unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 3);
    let restored = BodyPhysics::restore(&legacy).unwrap();
    let cell = restored
        .local_grid(charon_engine::foundation::id::BodyId::from_parts(0, 1).unwrap())
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.applied_remap_epoch(), 0);
}

#[test]
fn body_snapshot_v4_restores_with_empty_exclusion_reservoir() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 8,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
            grid,
        )
        .unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 4);

    let restored = BodyPhysics::restore(&legacy).unwrap();
    let grid = restored.local_grid(body).unwrap().unwrap();
    assert_eq!(grid.cell_count(), 1);
    assert!(grid.exclusion_reservoir().is_empty());
}

#[test]
fn resident_solid_terrain_is_cached_as_chunk_compound_colliders() {
    let (mut world, layer, stone) = terrain_world();
    let material = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point(10, 10),
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap();

    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 100.0],
            body_capacity: 8,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    assert_eq!(world.body_physics().unwrap().terrain_chunk_count(), 0);
    let mut falling = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    falling.pose.x = 10.5;
    falling.pose.y = 2.0;
    let body = world
        .edit_bodies(|physics| physics.insert(falling))
        .unwrap();
    for _ in 0..90 {
        world.step_bodies(layer, 1.0).unwrap();
    }
    let resting_y = world.body_physics().unwrap().state(body).unwrap().pose.y;
    assert!(resting_y > 9.0 && resting_y < 10.0, "y={resting_y}");
    assert!(
        world
            .body_physics()
            .unwrap()
            .contact_events()
            .iter()
            .any(|event| {
                event.first == BodyContactTarget::Body(body)
                    && event.second == BodyContactTarget::Terrain(ChunkCoord::new(0, 0).unwrap())
                    && event.phase == BodyContactPhase::Stay
            })
    );

    let world_snapshot = world.checkpoint(8_000_000).unwrap();
    assert_eq!(world.checkpoint_limit().unwrap(), world_snapshot.len());
    let mut resumed = World::restore(
        &world_snapshot,
        std::sync::Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap();
    assert_eq!(
        resumed.body_physics().unwrap().state(body).unwrap(),
        world.body_physics().unwrap().state(body).unwrap()
    );
    assert_eq!(
        resumed.body_physics().unwrap().snapshot().unwrap(),
        world.body_physics().unwrap().snapshot().unwrap()
    );
    for _ in 0..8 {
        world.step_bodies(layer, 1.0).unwrap();
        resumed.step_bodies(layer, 1.0).unwrap();
        assert_eq!(
            resumed.body_physics().unwrap().contact_events(),
            world.body_physics().unwrap().contact_events()
        );
    }
    assert_eq!(
        resumed.body_physics().unwrap().state(body).unwrap(),
        world.body_physics().unwrap().state(body).unwrap()
    );
    assert_eq!(
        resumed.body_physics().unwrap().snapshot().unwrap(),
        world.body_physics().unwrap().snapshot().unwrap()
    );

    world
        .edit(|edit| edit.set_cell(layer, point(10, 10), CellCore::AIR))
        .unwrap();
    for _ in 0..30 {
        world.step_bodies(layer, 1.0).unwrap();
    }
    assert_eq!(world.body_physics().unwrap().terrain_chunk_count(), 0);
    assert!(world.body_physics().unwrap().state(body).unwrap().pose.y > 10.0);
}

#[test]
fn world_edit_and_rigid_step_rollback_together_on_terrain_capacity_failure() {
    let (mut world, layer, stone) = terrain_world();
    let material = world.content().material_id(stone).unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            physics
                .insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }))
                .map(|_| ())
        })
        .unwrap();
    let before_snapshot = world.checkpoint(4_000_000).unwrap();
    let before_revision = world.revision();
    let error = world
        .edit_and_step_bodies(layer, 1.0, |edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point(1, 1),
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.revision(), before_revision);
    assert_eq!(world.resident_chunks(), 0);
    assert_eq!(world.body_physics().unwrap().step_index(), 0);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_snapshot);
}

#[test]
fn world_edit_can_atomically_edit_body_local_grid_and_terrain() {
    let (mut world, layer, stone) = terrain_world();
    let material = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            body_capacity: 2,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();

    let mut grid = body_grid(
        1,
        1,
        [0.25, 0.25],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();

    let (body, report) = world
        .edit_and_step_bodies(layer, 1.0, |edit| {
            edit.set_cell(
                layer,
                point(2, 2),
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )?;
            edit.edit_bodies(|physics| {
                let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
                definition.pose.x = 10.0;
                definition.pose.y = 10.0;
                physics.insert_with_local_grid(definition, grid)
            })
        })
        .unwrap();

    assert_eq!(report.step, 1);
    assert!(matches!(
        world.cell(layer, point(2, 2)).unwrap(),
        charon_engine::world::CellQuery::Loaded(value) if value.core.material == material
    ));
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell_count(),
        1
    );

    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit_and_step_bodies(layer, 1.0, |edit| {
            edit.set_cell(layer, point(3, 3), CellCore::AIR)?;
            edit.edit_bodies(|physics| {
                physics.insert(BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }))?;
                Ok(())
            })
        })
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(world.body_physics().unwrap().body_count(), 1);
    assert!(matches!(
        world.cell(layer, point(3, 3)).unwrap(),
        charon_engine::world::CellQuery::Loaded(value) if value.core == CellCore::AIR
    ));
}

#[test]
fn world_step_rejects_candidate_pose_that_enters_unavailable_chunk() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(2, 0)?)
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            max_substeps: 512,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    body.pose.x = 63.5;
    body.pose.y = 10.5;
    body.velocity.x = 7_680.0;
    let id = world.edit_bodies(|physics| physics.insert(body)).unwrap();
    let before_state = world.body_physics().unwrap().state(id).unwrap();
    let before_snapshot = world.checkpoint(4_000_000).unwrap();
    let before_revision = world.revision();

    let error = world
        .edit_and_step_bodies(layer, 1.0, |_| Ok(()))
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidState
    );
    assert_eq!(world.body_physics().unwrap().step_index(), 0);
    assert_eq!(
        world.body_physics().unwrap().state(id).unwrap(),
        before_state
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_snapshot);
    assert_eq!(world.revision(), before_revision);
}

#[test]
fn newly_inserted_body_sweep_cannot_cross_an_unavailable_chunk() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(2, 0)?)
        })
        .unwrap();
    assert_eq!(world.resident_chunks(), 2);
    world
        .configure_body_physics(BodyPhysicsSettings {
            step_hz: 1,
            gravity: [0.0, 0.0],
            max_substeps: 512,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let revision = world.revision();

    let error = world
        .edit_and_step_bodies(layer, 1.0, |edit| {
            edit.edit_bodies(|physics| {
                let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
                body.pose.x = 20.0;
                body.pose.y = 10.0;
                body.velocity.x = 150.0;
                physics.insert(body).map(|_| ())
            })
        })
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidState
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.body_physics().unwrap().body_count(), 0);
    assert_eq!(world.body_physics().unwrap().step_index(), 0);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

#[test]
fn moving_kinematic_platform_transfers_motion_to_dynamic_body() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 100.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let (platform, rider) = world
        .edit_bodies(|physics| {
            let mut platform = BodyDef::dynamic(ColliderShape::Cuboid {
                half_extents: [2.0, 0.5],
            });
            platform.motion = BodyMotionType::KinematicPosition;
            platform.pose = BodyPose {
                x: 10.0,
                y: 10.0,
                angle: 0.0,
            };
            platform.collider.friction = 1.0;
            let platform = physics.insert(platform)?;

            let mut rider = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            rider.pose = BodyPose {
                x: 10.0,
                y: 9.4,
                angle: 0.0,
            };
            rider.collider.friction = 1.0;
            let rider = physics.insert(rider)?;
            Ok((platform, rider))
        })
        .unwrap();
    let rider_x_before = world.body_physics().unwrap().state(rider).unwrap().pose.x;

    for step in 1..=8 {
        world
            .edit_bodies(|physics| {
                physics.set_kinematic_target(
                    platform,
                    BodyPose {
                        x: 10.0 + f64::from(step) * 0.5,
                        y: 10.0,
                        angle: 0.0,
                    },
                )
            })
            .unwrap();
        world.step_bodies(layer, 1.0).unwrap();
    }

    let physics = world.body_physics().unwrap();
    assert_eq!(physics.state(platform).unwrap().pose.x, 14.0);
    assert!(
        physics.state(rider).unwrap().pose.x > rider_x_before + 0.1,
        "rider x={} platform x={}",
        physics.state(rider).unwrap().pose.x,
        physics.state(platform).unwrap().pose.x
    );
}

#[test]
fn world_teleport_policy_checks_path_or_destination_atomically() {
    let (mut world, layer, stone) = terrain_world();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point(15, 10),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose = BodyPose {
        x: 10.5,
        y: 10.5,
        angle: 0.0,
    };
    definition.velocity = BodyVelocity {
        x: 3.0,
        y: -2.0,
        angular: 0.75,
    };
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let revision = world.revision();
    let velocity_before = world.body_physics().unwrap().state(body).unwrap().velocity;

    let error = world
        .teleport_body(
            body,
            BodyPose {
                x: 20.5,
                y: 10.5,
                angle: 0.0,
            },
            layer,
            1.0,
            BodyTeleportOptions::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);

    let error = world
        .teleport_body(
            body,
            BodyPose {
                x: 20.5,
                y: 10.5,
                angle: 0.0,
            },
            layer,
            1.0,
            BodyTeleportOptions {
                path: BodyTeleportPathPolicy::Check,
                overlap: BodyTeleportOverlapPolicy::Allow,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.revision(), revision);

    let error = world
        .teleport_body(
            body,
            BodyPose {
                x: 15.5,
                y: 10.5,
                angle: 0.0,
            },
            layer,
            1.0,
            BodyTeleportOptions {
                path: BodyTeleportPathPolicy::Ignore,
                overlap: BodyTeleportOverlapPolicy::Reject,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(world.revision(), revision);

    let report = world
        .teleport_body(
            body,
            BodyPose {
                x: 15.5,
                y: 10.5,
                angle: 0.0,
            },
            layer,
            1.0,
            BodyTeleportOptions {
                path: BodyTeleportPathPolicy::Ignore,
                overlap: BodyTeleportOverlapPolicy::Allow,
            },
        )
        .unwrap();
    assert!(!report.path_checked);
    assert!(!report.overlap_checked);
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().pose,
        report.destination
    );
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().velocity,
        velocity_before
    );

    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut resumed =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    world.step_bodies(layer, 1.0).unwrap();
    resumed.step_bodies(layer, 1.0).unwrap();
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        resumed.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn world_teleport_path_shape_cast_checks_rotation_arc() {
    let (mut world, layer, stone) = terrain_world();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point(11, 11),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Cuboid {
        half_extents: [2.5, 0.1],
    });
    definition.pose = BodyPose {
        x: 10.5,
        y: 10.5,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();

    let error = world
        .teleport_body(
            body,
            BodyPose {
                x: 10.5,
                y: 10.5,
                angle: std::f64::consts::FRAC_PI_2,
            },
            layer,
            1.0,
            BodyTeleportOptions::default(),
        )
        .unwrap_err();

    assert_eq!(error.code, charon_engine::foundation::ErrorCode::Conflict);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .state(body)
            .unwrap()
            .pose
            .angle,
        0.0
    );
}

#[test]
fn world_collision_queries_provider_validates_and_stably_orders_hits() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings::default())
        .unwrap();
    let request = CollisionQueryRequest {
        geometry: CollisionQueryGeometry::Sweep {
            shape: ColliderShape::Circle { radius: 0.25 },
            start: BodyPose {
                x: 2.0,
                y: 3.0,
                angle: 0.0,
            },
            end: BodyPose {
                x: 5.0,
                y: 3.0,
                angle: 0.25,
            },
        },
        layer,
        collision_groups: u32::MAX,
        owner: Some(StableKey::parse("test:query-owner").unwrap()),
        ignore_bodies: Vec::new(),
        one_way_direction: Some([0.0, 1.0]),
        include_sensors: false,
        include_initial_overlap: true,
        include_boundary: true,
        epsilon: 1.0e-9,
        tick: 21,
        substep: 2,
        read_latest_candidate: true,
        first_hit_only: false,
        stable_order: true,
        max_hits: 4,
    };
    let revision = world.revision();

    let report = world
        .query_body_collisions(
            &request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Hit,
                truncated: false,
            },
        )
        .unwrap();

    assert_eq!(report.world_revision, revision);
    assert_eq!(report.body_step, 0);
    assert_eq!(report.provider.version, 1);
    assert_eq!(report.result.geometry_revision, 7);
    assert_eq!(report.result.status, CollisionQueryStatus::Hit);
    assert_eq!(report.result.hits.len(), 3);
    assert_eq!(report.result.hits[0].time_of_impact, 0.25);
    assert_eq!(
        report.result.hits[0].target,
        CollisionQueryTarget::Terrain {
            cell: CellPoint { x: 2, y: 4 }
        }
    );
    assert_eq!(
        report.result.hits[0].material,
        Some(
            world
                .content()
                .material_id(MaterialHandle(StableKey::parse("test:stone").unwrap()))
                .unwrap()
        )
    );
    assert_eq!(
        report.result.hits[1].target,
        CollisionQueryTarget::Custom {
            owner: 3,
            object: 8,
            part: 1
        }
    );

    let mut custom_overlap_request = request.clone();
    custom_overlap_request.geometry = CollisionQueryGeometry::CustomOverlap {
        shape: CollisionQueryShapeRef {
            provider: StableKey::parse("test:custom-beam-shape").unwrap(),
            version: 1,
            owner: 2,
            object: 5,
            part: 1,
        },
        pose: BodyPose {
            x: 3.0,
            y: 4.0,
            angle: 0.5,
        },
    };
    let custom_overlap_report = world
        .query_body_collisions(
            &custom_overlap_request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Hit,
                truncated: false,
            },
        )
        .unwrap();
    assert_eq!(
        custom_overlap_report.result.status,
        CollisionQueryStatus::Hit
    );
    assert_eq!(custom_overlap_report.result.hits[0].time_of_impact, 0.0);

    let incomplete = world
        .query_body_collisions(
            &request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Incomplete,
                truncated: false,
            },
        )
        .unwrap();
    assert_eq!(incomplete.result.status, CollisionQueryStatus::Incomplete);
    assert_eq!(incomplete.result.hits.len(), 1);
    let unsupported = world
        .query_body_collisions(
            &request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Unsupported,
                truncated: false,
            },
        )
        .unwrap();
    assert_eq!(unsupported.result.status, CollisionQueryStatus::Unsupported);
    assert!(unsupported.result.hits.is_empty());

    let mut limited_request = request.clone();
    limited_request.max_hits = 3;
    let truncated = world
        .query_body_collisions(
            &limited_request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Hit,
                truncated: true,
            },
        )
        .unwrap();
    assert_eq!(truncated.result.hits.len(), 3);
    assert!(truncated.result.truncated);
    assert_eq!(world.revision(), revision);
}

#[test]
fn world_collision_queries_provider_rejects_inconsistent_or_invalid_results() {
    let (world, layer, _) = terrain_world();
    let request = CollisionQueryRequest {
        geometry: CollisionQueryGeometry::Ray {
            origin: [0.0, 0.0],
            direction: [1.0, 0.0],
            max_distance: 20.0,
        },
        layer,
        collision_groups: u32::MAX,
        owner: None,
        ignore_bodies: Vec::new(),
        one_way_direction: None,
        include_sensors: true,
        include_initial_overlap: false,
        include_boundary: true,
        epsilon: 0.0,
        tick: 0,
        substep: 0,
        read_latest_candidate: true,
        first_hit_only: false,
        stable_order: false,
        max_hits: 4,
    };
    let revision = world.revision();

    let error = world
        .query_body_collisions(
            &request,
            &StableCollisionQueries {
                malformed_result: true,
                status: CollisionQueryStatus::Hit,
                truncated: false,
            },
        )
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
    assert_eq!(world.revision(), revision);
    let mut invalid_request = request.clone();
    invalid_request.geometry = CollisionQueryGeometry::Ray {
        origin: [0.0, 0.0],
        direction: [2.0, 0.0],
        max_distance: 20.0,
    };
    let invalid_error = world
        .query_body_collisions(
            &invalid_request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Hit,
                truncated: false,
            },
        )
        .unwrap_err();
    assert_eq!(
        invalid_error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );

    let mut invalid_custom_shape_request = request;
    invalid_custom_shape_request.geometry = CollisionQueryGeometry::CustomOverlap {
        shape: CollisionQueryShapeRef {
            provider: StableKey::parse("test:custom-beam-shape").unwrap(),
            version: 0,
            owner: 2,
            object: 5,
            part: 1,
        },
        pose: BodyPose {
            x: 3.0,
            y: 4.0,
            angle: 0.5,
        },
    };
    let invalid_custom_shape_error = world
        .query_body_collisions(
            &invalid_custom_shape_request,
            &StableCollisionQueries {
                malformed_result: false,
                status: CollisionQueryStatus::Hit,
                truncated: false,
            },
        )
        .unwrap_err();
    assert_eq!(
        invalid_custom_shape_error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
}

#[test]
fn world_teleport_can_replace_collision_queries_with_provider() {
    let (mut world, layer, stone) = terrain_world();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                point(15, 10),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose = BodyPose {
        x: 10.5,
        y: 10.5,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();
    let destination = BodyPose {
        x: 20.5,
        y: 10.5,
        angle: 0.0,
    };
    let before = world.checkpoint(4_000_000).unwrap();
    let options = BodyTeleportOptions {
        path: BodyTeleportPathPolicy::Check,
        overlap: BodyTeleportOverlapPolicy::Reject,
    };

    assert!(
        world
            .teleport_body(body, destination, layer, 1.0, options)
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);

    let report = world
        .teleport_body_with_provider(
            body,
            destination,
            layer,
            1.0,
            options,
            &PortalTeleportQueries,
        )
        .unwrap();
    assert_eq!(
        report
            .query_provider
            .map(|stamp| (stamp.key, stamp.version)),
        Some((StableKey::parse("test:portal-teleport-queries").unwrap(), 1))
    );
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().pose,
        destination
    );
}

#[test]
fn empty_world_edit_and_physics_step_preserve_world_revision() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    body.pose.x = 10.0;
    body.pose.y = 10.0;
    body.velocity.x = 3.0;
    let id = world.edit_bodies(|physics| physics.insert(body)).unwrap();
    let revision = world.revision();

    let report = world.step_bodies(layer, 1.0).unwrap();
    assert_eq!(report.step, 1);
    assert_eq!(world.revision(), revision);
    assert!(world.body_physics().unwrap().state(id).unwrap().pose.x > 10.0);

    let ((), report) = world.edit_and_step_bodies(layer, 1.0, |_| Ok(())).unwrap();

    assert_eq!(report.step, 2);
    assert_eq!(world.revision(), revision);
    assert!(world.body_physics().unwrap().state(id).unwrap().pose.x > 10.0);
}

#[test]
fn author_body_physics_provider_replaces_rapier_step_and_reports_identity() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 100.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose = BodyPose {
        x: 10.0,
        y: 10.0,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();

    let report = world
        .step_bodies_with_provider(
            layer,
            1.0,
            &DeterministicBodyStep {
                fail_after_edit: false,
                version: 1,
            },
        )
        .unwrap();

    assert_eq!(report.step, 1);
    assert_eq!(report.substeps, 1);
    assert_eq!(
        report.provider.map(|stamp| (stamp.key, stamp.version)),
        Some((StableKey::parse("test:deterministic-body-step").unwrap(), 1))
    );
    let state = world.body_physics().unwrap().state(body).unwrap();
    assert_eq!(state.pose.x, 11.0);
    assert_eq!(state.pose.y, 10.0);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .provider_state::<ProviderCounter>()
            .unwrap(),
        Some(ProviderCounter(1))
    );

    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let content = world.content().clone();
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert!(
        restored
            .step_bodies_with_provider(
                layer,
                1.0,
                &DeterministicBodyStep {
                    fail_after_edit: false,
                    version: 2,
                },
            )
            .is_err()
    );
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
    assert!(restored.step_bodies(layer, 1.0).is_err());
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
    restored
        .step_bodies_with_provider(
            layer,
            1.0,
            &DeterministicBodyStep {
                fail_after_edit: false,
                version: 1,
            },
        )
        .unwrap();
    assert_eq!(
        restored.body_physics().unwrap().state(body).unwrap().pose.x,
        12.0
    );
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .provider_state::<ProviderCounter>()
            .unwrap(),
        Some(ProviderCounter(2))
    );
}

#[test]
fn author_body_physics_provider_publishes_contact_events_with_the_candidate_step() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            contact_event_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let (first, second) = world
        .edit_bodies(|physics| {
            let mut first = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            first.pose = BodyPose {
                x: 10.0,
                y: 10.0,
                angle: 0.0,
            };
            let mut second = first.clone();
            second.pose.x = 12.0;
            Ok((physics.insert(first)?, physics.insert(second)?))
        })
        .unwrap();

    let report = world
        .step_bodies_with_provider(
            layer,
            1.0,
            &ContactReportingBodyStep {
                exceed_capacity: false,
                event_substep: 1,
                custom_target: false,
            },
        )
        .unwrap();

    assert_eq!(report.substeps, 1);
    assert_eq!(
        world.body_physics().unwrap().contact_events(),
        &[BodyContactEvent {
            first: BodyContactTarget::Body(first),
            second: BodyContactTarget::Body(second),
            phase: BodyContactPhase::Enter,
            sensor: false,
            exit_reason: None,
            point: [10.5, 10.0],
            normal: [1.0, 0.0],
            impulse: [1.0, 0.0],
            first_pre_velocity: Some(BodyVelocity::default()),
            first_post_velocity: Some(BodyVelocity {
                x: 1.0,
                y: 0.0,
                angular: 0.0,
            }),
            second_pre_velocity: Some(BodyVelocity::default()),
            second_post_velocity: Some(BodyVelocity {
                x: -1.0,
                y: 0.0,
                angular: 0.0,
            }),
            substep: 1,
        }]
    );
}

#[test]
fn body_crosses_preleased_chunk_boundary_and_world_checkpoint_restores_both() {
    let (mut world, layer, _) = terrain_world();
    let first_chunk = ChunkCoord::new(0, 0).unwrap();
    let second_chunk = ChunkCoord::new(1, 0).unwrap();
    let region = ChunkRegion::new(first_chunk, 2, 1).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(first_chunk)?;
            edit.insert_empty_chunk(second_chunk)?;
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("test:rigid-body-crossing").unwrap(),
                region,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let body = world
        .edit_bodies(|physics| {
            let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
            definition.pose = BodyPose {
                x: 60.0,
                y: 10.5,
                angle: 0.0,
            };
            definition.velocity.x = 50.0;
            physics.insert(definition)
        })
        .unwrap();

    for _ in 0..16 {
        world.step_bodies(layer, 1.0).unwrap();
    }

    assert!(world.body_physics().unwrap().state(body).unwrap().pose.x > 64.0);
    assert_eq!(world.active_chunks().count(), 2);
    let leased_region_error =
        match world.prepare_eviction(ChunkRegion::single(second_chunk).unwrap(), 4_000_000) {
            Err(error) => error,
            Ok(_) => panic!("lease対象regionは退避できない"),
        };
    assert_eq!(
        leased_region_error.code,
        charon_engine::foundation::ErrorCode::Conflict
    );

    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let content = Arc::clone(world.content());
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
    assert_eq!(restored.active_chunks().count(), 2);
    let restored_x = restored.body_physics().unwrap().state(body).unwrap().pose.x;
    assert!(restored_x > 64.0);
    for _ in 0..16 {
        world.step_bodies(layer, 1.0).unwrap();
        restored.step_bodies(layer, 1.0).unwrap();
    }
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        restored.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn author_contact_event_can_name_a_stable_custom_target() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            contact_event_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            let mut body = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            body.pose = BodyPose {
                x: 10.0,
                y: 10.0,
                angle: 0.0,
            };
            physics.insert(body)?;
            let mut other = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            other.pose.x = 12.0;
            other.pose.y = 10.0;
            physics.insert(other)?;
            Ok(())
        })
        .unwrap();

    world
        .step_bodies_with_provider(
            layer,
            1.0,
            &ContactReportingBodyStep {
                exceed_capacity: false,
                event_substep: 1,
                custom_target: true,
            },
        )
        .unwrap();

    let event = &world.body_physics().unwrap().contact_events()[0];
    assert_eq!(
        event.second,
        BodyContactTarget::Custom {
            owner: 7,
            object: 4,
            part: 2,
        }
    );
    assert!(event.second_pre_velocity.is_none());
    assert!(event.second_post_velocity.is_none());
}

#[test]
fn author_contact_event_capacity_failure_rolls_back_the_fixed_step() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            contact_event_capacity: 1,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    world
        .edit_bodies(|physics| {
            let mut first = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
            first.pose = BodyPose {
                x: 10.0,
                y: 10.0,
                angle: 0.0,
            };
            let mut second = first.clone();
            second.pose.x = 12.0;
            physics.insert(first)?;
            physics.insert(second)?;
            Ok(())
        })
        .unwrap();
    let checkpoint = world.checkpoint(4_000_000).unwrap();

    let error = world
        .step_bodies_with_provider(
            layer,
            1.0,
            &ContactReportingBodyStep {
                exceed_capacity: true,
                event_substep: 1,
                custom_target: false,
            },
        )
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), checkpoint);
    assert!(world.body_physics().unwrap().contact_events().is_empty());

    let error = world
        .step_bodies_with_provider(
            layer,
            1.0,
            &ContactReportingBodyStep {
                exceed_capacity: false,
                event_substep: 2,
                custom_target: false,
            },
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), checkpoint);
    assert!(world.body_physics().unwrap().contact_events().is_empty());
}

#[test]
fn custom_step_queries_its_updated_candidate_and_uses_provider_toi() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose = BodyPose {
        x: 4.0,
        y: 4.0,
        angle: 0.0,
    };
    definition.velocity.x = 120.0;
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();

    let report = world
        .step_bodies_with_provider(layer, 1.0, &CollisionDrivenBodyStep { layer })
        .unwrap();

    assert_eq!(report.provider.unwrap().version, 1);
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().pose.x,
        6.0
    );
    let physics = world.body_physics().unwrap();
    let mass = physics.mass_properties(body).unwrap().mass;
    assert!(physics.state(body).unwrap().velocity.x.abs() < 1.0e-10);
    let event = &world.body_physics().unwrap().contact_events()[0];
    assert_eq!(event.first, BodyContactTarget::Body(body));
    assert_eq!(
        event.second,
        BodyContactTarget::Custom {
            owner: 9,
            object: 3,
            part: 0,
        }
    );
    assert_eq!(event.point, [6.0, 4.0]);
    assert_eq!(event.normal, [1.0, 0.0]);
    assert_eq!(event.impulse, [-mass * 120.0, 0.0]);
    assert_eq!(event.first_pre_velocity.unwrap().x, 120.0);
    assert!(event.first_post_velocity.unwrap().x.abs() < 1.0e-10);
    assert_eq!(event.substep, 1);
}

#[test]
fn author_body_physics_provider_failure_rolls_back_world_and_physics() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose = BodyPose {
        x: 10.0,
        y: 10.0,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();
    let checkpoint = world.checkpoint(4_000_000).unwrap();

    assert!(
        world
            .step_bodies_with_provider(
                layer,
                1.0,
                &DeterministicBodyStep {
                    fail_after_edit: true,
                    version: 1,
                }
            )
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), checkpoint);
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().pose.x,
        10.0
    );
}

#[test]
fn world_physics_uses_reported_substeps_to_bound_body_motion() {
    let mut legacy_settings = serde_json::to_value(BodyPhysicsSettings::default()).unwrap();
    legacy_settings
        .as_object_mut()
        .unwrap()
        .remove("max_substep_displacement_cells");
    let restored_legacy_settings: BodyPhysicsSettings =
        serde_json::from_value(legacy_settings).unwrap();
    assert_eq!(restored_legacy_settings.max_substep_displacement_cells, 0.5);
    assert!(
        BodyPhysics::new(BodyPhysicsSettings {
            max_substep_displacement_cells: 0.0,
            ..BodyPhysicsSettings::default()
        })
        .is_err()
    );
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            max_substep_displacement_cells: 0.25,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose.x = 10.0;
    definition.pose.y = 10.0;
    definition.velocity.x = 100.0;
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let content = Arc::clone(world.content());
    let mut world =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();

    let report = world.step_bodies(layer, 1.0).unwrap();

    assert_eq!(report.substeps, 7);
    let state = world.body_physics().unwrap().state(body).unwrap();
    assert!((state.pose.x - (10.0 + 100.0 / 60.0)).abs() < 1e-9);
}

#[test]
fn world_fixed_step_does_not_tunnel_through_a_thin_terrain_wall_at_high_speed() {
    let (mut world, layer, stone) = terrain_world();
    let chunk = ChunkCoord::new(0, 0).unwrap();
    let stone = world.content().material_id(stone).unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(chunk)?;
            edit.set_cell(
                layer,
                point(20, 10),
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )
        })
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            max_substeps: 64,
            max_substep_displacement_cells: 0.5,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut projectile = BodyDef::dynamic(ColliderShape::Circle { radius: 0.1 });
    projectile.pose.x = 10.5;
    projectile.pose.y = 10.5;
    projectile.velocity.x = 1_200.0;
    projectile.ccd = true;
    let body = world
        .edit_bodies(|physics| physics.insert(projectile))
        .unwrap();

    let report = world.step_bodies(layer, 1.0).unwrap();

    assert_eq!(report.substeps, 40);
    let state = world.body_physics().unwrap().state(body).unwrap();
    assert!(state.pose.x < 19.9, "高速bodyが薄壁を通り抜けた: {state:?}");
}

#[test]
fn world_physics_substeps_large_rotation_by_collider_radius() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 2.0 });
    definition.pose.x = 10.0;
    definition.pose.y = 10.0;
    definition.velocity.angular = 30.0;
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();

    let report = world.step_bodies(layer, 1.0).unwrap();

    assert_eq!(report.substeps, 2);
    let state = world.body_physics().unwrap().state(body).unwrap();
    // Rapier標準の角減衰を含むため、step前後で回転上限が働くことを確認します。
    assert!((state.pose.angle - 0.5).abs() < 0.001);
}

#[test]
fn world_physics_rejects_substep_overflow_without_publishing_candidate() {
    let (mut world, layer, _) = terrain_world();
    world
        .edit(|edit| edit.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            max_substeps: 2,
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 });
    definition.pose.x = 10.0;
    definition.pose.y = 10.0;
    definition.velocity.x = 100.0;
    let body = world
        .edit_bodies(|physics| physics.insert(definition))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let revision = world.revision();

    let error = world.step_bodies(layer, 1.0).unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.body_physics().unwrap().step_index(), 0);
    assert_eq!(
        world.body_physics().unwrap().state(body).unwrap().pose.x,
        10.0
    );
}

#[test]
fn body_local_thermal_step_conserves_energy_and_persists_edge_credit() {
    let mut world = world_with_heat_channel();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let mut grid = body_grid(
        3,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 3,
            geometry_cells: 3,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 900, Vec::new()).unwrap(),
    )
    .unwrap();
    grid.set_cell(
        1,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 300, Vec::new()).unwrap(),
    )
    .unwrap();
    let boundary_edge = BodyThermalBoundaryEdgeState {
        residual: -1,
        equilibrium_credit: 3,
    };
    grid.set_thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right, boundary_edge)
        .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let config = ThermalStepConfig {
        conductance_numerator: 1,
        conductance_denominator: 1,
    };

    let report = world
        .edit(|edit| edit.step_body_local_thermal(body, &family, config))
        .unwrap();
    assert_eq!(report.total_energy, 1200);
    let grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    let hot = grid.cell(0, 0).unwrap().energy();
    let cold = grid.cell(1, 0).unwrap().energy();
    assert!(hot < 900);
    assert!(cold > 300);
    assert_eq!(hot + cold, 1200);
    let edge = grid.thermal_edge_state(0, 0).unwrap();
    assert!(edge.equilibrium_credit_x > 0);

    let mismatched_family = EnthalpyFamily::new(
        StableKey::parse("test:water-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: StableKey::parse("test:water").unwrap(),
        }],
    )
    .unwrap();
    let before_failure = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.step_body_local_thermal(body, &mismatched_family, config))
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_failure);

    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut restored =
        World::restore(&checkpoint, content, WorldLimits::default(), 4_000_000).unwrap();
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .thermal_edge_state(0, 0),
        Some(edge)
    );
    assert_eq!(
        restored
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        Some(boundary_edge)
    );
    let body_snapshot = world.body_physics().unwrap().snapshot().unwrap();
    let legacy_v7 = legacy_body_snapshot(&body_snapshot, 7);
    let legacy_v8 = legacy_body_snapshot(&body_snapshot, 8);
    let restored_v7 = BodyPhysics::restore(&legacy_v7).unwrap();
    let restored_v8 = BodyPhysics::restore(&legacy_v8).unwrap();
    assert_eq!(
        restored_v7
            .local_grid(body)
            .unwrap()
            .unwrap()
            .thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        None
    );
    assert_eq!(
        restored_v8
            .local_grid(body)
            .unwrap()
            .unwrap()
            .thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        Some(boundary_edge)
    );
    for _ in 0..4 {
        world
            .edit(|edit| edit.step_body_local_thermal(body, &family, config))
            .unwrap();
        restored
            .edit(|edit| edit.step_body_local_thermal(body, &family, config))
            .unwrap();
    }
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        restored.checkpoint(4_000_000).unwrap()
    );
    let mut fractured_grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .clone();
    fractured_grid.remove_cell(1, 0).unwrap();
    world
        .edit_bodies(|physics| physics.replace_local_grid(body, fractured_grid))
        .unwrap();
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .thermal_edge_state(0, 0),
        None
    );
}

#[test]
fn body_local_thermal_phase_change_preserves_an_exact_mass_mixture() {
    let mut world = world_with_heat_channel();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer = StableKey::parse("test:terrain").unwrap();
    let stone = StableKey::parse("test:stone").unwrap();
    let water = StableKey::parse("test:water").unwrap();
    let mixed = BodyLocalCell::new(layer, stone, 0, 0, 150, Vec::new())
        .unwrap()
        .with_tokens(MaterialQuantity::new(2, 1).unwrap())
        .unwrap()
        .with_mixture(vec![
            BodyMaterialComponent::new(stone, MaterialQuantity::ONE).unwrap(),
            BodyMaterialComponent::new(water, MaterialQuantity::ONE).unwrap(),
        ])
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(0, 0, mixed).unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:mixture-phase-family").unwrap(),
        vec![
            EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 1000,
                material: stone,
            },
            EnthalpyBand::Latent {
                energy_start: 100,
                energy_end: 200,
                temperature_mk: 1000,
                from: stone,
                to: water,
            },
            EnthalpyBand::Sensible {
                energy_start: 200,
                energy_end: 300,
                temperature_start_mk: 1000,
                temperature_end_mk: 2000,
                material: water,
            },
        ],
    )
    .unwrap();
    let before_mass = world
        .body_physics()
        .unwrap()
        .mass_properties(body)
        .unwrap()
        .mass;

    world
        .edit(|edit| {
            edit.step_body_local_thermal(
                body,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
            )
        })
        .unwrap();

    let physics = world.body_physics().unwrap();
    let cell = physics
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap();
    assert_eq!(cell.material().unwrap(), water);
    assert_eq!(cell.energy(), 150);
    assert_eq!(cell.tokens(), MaterialQuantity::new(2, 1).unwrap());
    assert_eq!(
        cell.mixture(),
        &[
            BodyMaterialComponent::new(stone, MaterialQuantity::ONE).unwrap(),
            BodyMaterialComponent::new(water, MaterialQuantity::ONE).unwrap(),
        ]
    );
    assert_eq!(physics.mass_properties(body).unwrap().mass, before_mass);
}

#[test]
fn body_local_thermal_phase_change_rejects_mixture_mass_mismatch_atomically() {
    let layer = StableKey::parse("test:terrain").unwrap();
    let stone = StableKey::parse("test:stone").unwrap();
    let water = StableKey::parse("test:water").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    registry
        .layer(LayerDef {
            key: layer,
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(
            MaterialBuilder::new(stone.as_str(), MaterialBase::Solid)
                .unwrap()
                .mass_per_cell(1),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new(water.as_str(), MaterialBase::Liquid)
                .unwrap()
                .mass_per_cell(2),
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        0,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let cell = BodyLocalCell::new(layer, stone, 0, 0, 150, Vec::new())
        .unwrap()
        .with_mixture(vec![
            BodyMaterialComponent::new(stone, MaterialQuantity::new(1, 2).unwrap()).unwrap(),
            BodyMaterialComponent::new(water, MaterialQuantity::new(1, 4).unwrap()).unwrap(),
        ])
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(0, 0, cell).unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:mixture-mass-family").unwrap(),
        vec![
            EnthalpyBand::Sensible {
                energy_start: 0,
                energy_end: 100,
                temperature_start_mk: 0,
                temperature_end_mk: 1000,
                material: stone,
            },
            EnthalpyBand::Latent {
                energy_start: 100,
                energy_end: 200,
                temperature_mk: 1000,
                from: stone,
                to: water,
            },
            EnthalpyBand::Sensible {
                energy_start: 200,
                energy_end: 300,
                temperature_start_mk: 1000,
                temperature_end_mk: 2000,
                material: water,
            },
        ],
    )
    .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();

    let error = world
        .edit(|edit| {
            edit.step_body_local_thermal(
                body,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
            )
        })
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::InvalidValue
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    assert_eq!(
        world
            .body_physics()
            .unwrap()
            .local_grid(body)
            .unwrap()
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .material()
            .unwrap(),
        stone
    );
}

#[test]
fn body_world_thermal_boundary_conserves_energy_and_restores_fractional_flux() {
    // body energy、World channel、edge stateを一度ずつ数えれば境界値3で成立する。
    let limits = WorldLimits {
        edit_cells: 3,
        ..WorldLimits::default()
    };
    let mut world = world_with_heat_channel_limits(limits);
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(stone_key))
        .unwrap();
    let contact = point(5, 4);
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                contact,
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(
                StableKey::parse("test:thermal")?,
                contact,
                charon_engine::world::ThermalCellState { energy: 300 },
            )?;
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("test:thermal-active")?,
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 256,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 900, Vec::new()).unwrap(),
    )
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let mismatched_family = EnthalpyFamily::new(
        StableKey::parse("test:water-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: StableKey::parse("test:water").unwrap(),
        }],
    )
    .unwrap();
    let config = ThermalStepConfig {
        conductance_numerator: 1,
        conductance_denominator: 1000,
    };
    let total_before = 900 + 300;

    let before_invalid_edit = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.step_body_world_thermal_boundary(
                    body,
                    layer,
                    StableKey::parse("test:thermal")?,
                    &mismatched_family,
                    config,
                    1.0,
                )
            })
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before_invalid_edit);

    let first_report = world
        .edit(|edit| {
            edit.step_body_world_thermal_boundary(
                body,
                layer,
                StableKey::parse("test:thermal")?,
                &family,
                config,
                1.0,
            )
        })
        .unwrap();
    assert_eq!(first_report.total_energy, i128::from(total_before));
    assert_eq!(first_report.edges, 1);
    let body_grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(body_grid.cell(0, 0).unwrap().energy(), 900);
    assert_eq!(
        body_grid.thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        Some(BodyThermalBoundaryEdgeState {
            residual: 600_i64 * (1_i64 << 32) / 1000,
            equilibrium_credit: 4,
        })
    );

    let content = Arc::clone(world.content());
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(&checkpoint, content, limits, 4_000_000).unwrap();
    for candidate in [&mut world, &mut restored] {
        candidate
            .edit(|edit| {
                edit.step_body_world_thermal_boundary(
                    body,
                    layer,
                    StableKey::parse("test:thermal")?,
                    &family,
                    ThermalStepConfig {
                        conductance_numerator: 1,
                        conductance_denominator: 1,
                    },
                    1.0,
                )
            })
            .unwrap();
    }
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        restored.checkpoint(4_000_000).unwrap()
    );
    let before_invalid_custom_boundary = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.step_body_world_thermal_boundary_with_provider(
                    body,
                    layer,
                    StableKey::parse("test:thermal")?,
                    &family,
                    config,
                    1.0,
                    &CustomThermalBoundary {
                        invalid_local_cell: true,
                    },
                )
            })
            .is_err()
    );
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        before_invalid_custom_boundary
    );
    let custom_boundary_report = world
        .edit(|edit| {
            edit.step_body_world_thermal_boundary_with_provider(
                body,
                layer,
                StableKey::parse("test:thermal")?,
                &family,
                config,
                1.0,
                &CustomThermalBoundary {
                    invalid_local_cell: false,
                },
            )
        })
        .unwrap();
    assert_eq!(
        custom_boundary_report.total_energy,
        i128::from(total_before)
    );
    assert_eq!(custom_boundary_report.edges, 1);
    let pre_integrated_body_energy = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .energy();
    let pre_integrated_world_energy = match world
        .channel_value::<charon_engine::world::ThermalCellState>(
            StableKey::parse("test:thermal").unwrap(),
            contact,
        )
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(value) => value.energy,
        _ => panic!("unexpected thermal channel query"),
    };
    let thermal_profile = BodyThermalReactionBoundary {
        body,
        layer: layer_key,
        energy_channel: StableKey::parse("test:thermal").unwrap(),
        family: &family,
        config: ThermalStepConfig {
            conductance_numerator: 1,
            conductance_denominator: 1,
        },
        conduct_body_interior: false,
    };
    let before_duplicate_profile = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .step_bodies_with_thermal_reactions(
                layer,
                1.0,
                &[thermal_profile, thermal_profile],
                &[],
            )
            .is_err()
    );
    assert_eq!(
        world.checkpoint(4_000_000).unwrap(),
        before_duplicate_profile
    );
    let (_, body_step) = world
        .step_bodies_with_thermal_reactions(layer, 1.0, &[thermal_profile], &[])
        .unwrap();
    assert!(body_step.substeps > 0);
    let body_energy = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .energy();
    let world_energy = match world
        .channel_value::<charon_engine::world::ThermalCellState>(
            StableKey::parse("test:thermal").unwrap(),
            contact,
        )
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(value) => value.energy,
        _ => panic!("unexpected thermal channel query"),
    };
    assert_eq!(
        body_energy + world_energy,
        pre_integrated_body_energy + pre_integrated_world_energy
    );
    assert_eq!(body_energy + world_energy, total_before);
    assert!(body_energy < 900);
    assert!(world_energy > 300);
}

#[test]
fn rotated_body_world_thermal_boundary_conserves_energy_across_cells() {
    let mut world = world_with_heat_channel();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer = world
        .content()
        .layer_id(StableKey::parse("test:terrain").unwrap())
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(stone_key))
        .unwrap();
    let points: Vec<_> = (5..12)
        .flat_map(|y| (5..12).map(move |x| point(x, y)))
        .collect();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for &point in &points {
                edit.set_cell(
                    layer,
                    point,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
                edit.set_channel(
                    StableKey::parse("test:thermal")?,
                    point,
                    charon_engine::world::ThermalCellState { energy: 300 },
                )?;
            }
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("test:thermal-active")?,
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 256,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 900, Vec::new()).unwrap(),
    )
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = BodyPose {
        x: 8.5,
        y: 8.5,
        angle: std::f64::consts::FRAC_PI_4,
    };
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let report = world
        .edit(|edit| {
            edit.step_body_world_thermal_boundary(
                body,
                layer,
                StableKey::parse("test:thermal")?,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 4,
                },
                1.0,
            )
        })
        .unwrap();
    assert!(report.edges > 1);
    let world_energy = points
        .iter()
        .try_fold(0_i128, |sum, point| {
            let energy = match world
                .channel_value::<charon_engine::world::ThermalCellState>(
                    StableKey::parse("test:thermal").unwrap(),
                    *point,
                )
                .unwrap()
            {
                charon_engine::world::ChannelQuery::Loaded(value) => value.energy,
                _ => panic!("thermal channel coverage must be loaded"),
            };
            Some(sum + i128::from(energy))
        })
        .unwrap();
    let body_energy = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .energy();
    assert_eq!(world_energy + i128::from(body_energy), 900 + 49 * 300);
    assert!(body_energy < 900);
}

#[test]
fn rotated_body_world_thermal_flux_scales_with_boundary_coverage() {
    let pose = BodyPose {
        x: 8.3,
        y: 8.4,
        angle: std::f64::consts::FRAC_PI_4,
    };
    let stencil = BodyStencil::new(1, 1, 1.0, [0.5, 0.5], 0, vec![true]).unwrap();
    let contacts = stencil.boundary_contacts(pose, 1.0, 32).unwrap();
    let mut world_points: Vec<_> = contacts.iter().map(|contact| contact.world_cell).collect();
    world_points.sort_unstable_by_key(|point| (point.y, point.x));
    world_points.dedup();
    let coverage_for = |point: CellPoint| {
        contacts
            .iter()
            .filter(|contact| contact.world_cell == point)
            .map(|contact| contact.covered_length)
            .sum::<f64>()
    };
    let (first, first_coverage, second, second_coverage) = world_points
        .iter()
        .enumerate()
        .find_map(|(index, first)| {
            world_points[index + 1..]
                .iter()
                .find(|second| {
                    coverage_for(*first) > 0.2
                        && coverage_for(**second) > 0.2
                        && (coverage_for(*first) - coverage_for(**second)).abs() > 0.1
                })
                .map(|second| (*first, coverage_for(*first), *second, coverage_for(*second)))
        })
        .expect("回転bodyが異なるcoverageのWorld cellを横切る");
    let mut world = world_with_heat_channel();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let layer = world.content().layer_id(layer_key).unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let stone = world
        .content()
        .material_id(MaterialHandle(stone_key))
        .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            for contact in [first, second] {
                edit.set_cell(
                    layer,
                    contact,
                    CellCore {
                        material: stone,
                        flags: 0,
                        variation: 0,
                    },
                )?;
                edit.set_channel(
                    StableKey::parse("test:thermal")?,
                    contact,
                    charon_engine::world::ThermalCellState { energy: 300 },
                )?;
            }
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("test:coverage-active")?,
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 256,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 900, Vec::new()).unwrap(),
    )
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = pose;
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:coverage-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let report = world
        .edit(|edit| {
            edit.step_body_world_thermal_boundary(
                body,
                layer,
                StableKey::parse("test:thermal")?,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
                1.0,
            )
        })
        .unwrap();
    assert!(report.edges >= 2);
    let after = |point| match world
        .channel_value::<charon_engine::world::ThermalCellState>(
            StableKey::parse("test:thermal").unwrap(),
            point,
        )
        .unwrap()
    {
        charon_engine::world::ChannelQuery::Loaded(value) => value.energy,
        _ => panic!("thermal channel must be loaded"),
    };
    let first_delta = after(first) - 300;
    let second_delta = after(second) - 300;
    assert!(
        first_delta > 0 && second_delta > 0,
        "deltas={first_delta}/{second_delta}, coverage={first_coverage}/{second_coverage}"
    );
    let scaled_difference =
        (first_delta as f64 * second_coverage - second_delta as f64 * first_coverage).abs();
    assert!(
        scaled_difference <= 2.0,
        "energy deltas {first_delta}/{second_delta} do not follow coverage {}/{}",
        first_coverage,
        second_coverage
    );
    let body_energy = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .energy();
    assert_eq!(body_energy + after(first) + after(second), 1500);
}

#[test]
fn body_world_thermal_boundary_ignores_other_local_layers() {
    let terrain_key = StableKey::parse("test:terrain").unwrap();
    let other_layer_key = StableKey::parse("test:other-layer").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let thermal_key = StableKey::parse("test:thermal").unwrap();
    let mut registry = ContentRegistry::new(Default::default()).unwrap();
    for (key, render_order) in [(terrain_key, 0), (other_layer_key, 1)] {
        registry
            .layer(LayerDef {
                key,
                role: LayerRole::Terrain,
                render_order,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
    }
    registry
        .register(MaterialBuilder::new(stone_key.as_str(), MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<charon_engine::world::ThermalCellState>(thermal_key, 1, 8).unwrap())
        .unwrap();
    registry
        .material_channel(
            terrain_key,
            ChannelDefinition {
                key: thermal_key,
                version: 1,
                value_schema: thermal_key,
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: charon_engine::world::ThermalCellState::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1024,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(terrain_key).unwrap();
    let stone = content.material_id(MaterialHandle(stone_key)).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let contact = point(5, 4);
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.set_cell(
                layer,
                contact,
                CellCore {
                    material: stone,
                    flags: 0,
                    variation: 0,
                },
            )?;
            edit.set_channel(
                thermal_key,
                contact,
                charon_engine::world::ThermalCellState { energy: 300 },
            )?;
            edit.acquire_lease(RegionLease {
                source: StableKey::parse("test:thermal-active")?,
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 1, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 32,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(other_layer_key, stone_key, 0, 0, 900, Vec::new()).unwrap(),
    )
    .unwrap();
    let mut definition = BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 });
    definition.pose = BodyPose {
        x: 4.5,
        y: 4.5,
        angle: 0.0,
    };
    let body = world
        .edit_bodies(|physics| physics.insert_with_local_grid(definition, grid))
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 100,
            energy_end: 1100,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let report = world
        .edit(|edit| {
            edit.step_body_world_thermal_boundary(
                body,
                layer,
                thermal_key,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
                1.0,
            )
        })
        .unwrap();
    assert_eq!(report.edges, 0);
    let body_grid = world
        .body_physics()
        .unwrap()
        .local_grid(body)
        .unwrap()
        .unwrap();
    assert_eq!(body_grid.cell(0, 0).unwrap().energy(), 900);
    assert_eq!(
        body_grid.thermal_boundary_edge_state(0, 0, BodyThermalBoundaryFace::Right),
        None
    );
    assert!(matches!(
        world.channel_value::<charon_engine::world::ThermalCellState>(thermal_key, contact),
        Ok(charon_engine::world::ChannelQuery::Loaded(
            charon_engine::world::ThermalCellState { energy: 300 }
        ))
    ));
}

#[test]
fn body_local_thermal_capacity_failure_keeps_candidate_unchanged() {
    let limits = WorldLimits {
        job_cells: 2,
        ..WorldLimits::default()
    };
    let mut world = world_with_heat_channel_limits(limits);
    world
        .configure_body_physics(BodyPhysicsSettings {
            gravity: [0.0, 0.0],
            ..BodyPhysicsSettings::default()
        })
        .unwrap();
    let layer_key = StableKey::parse("test:terrain").unwrap();
    let stone_key = StableKey::parse("test:stone").unwrap();
    let mut grid = body_grid(
        3,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 3,
            sidecar_bytes: 16,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(layer_key, stone_key, 0, 0, 100, Vec::new()).unwrap(),
    )
    .unwrap();
    let body = world
        .edit_bodies(|physics| {
            physics.insert_with_local_grid(
                BodyDef::dynamic(ColliderShape::Circle { radius: 0.5 }),
                grid,
            )
        })
        .unwrap();
    let family = EnthalpyFamily::new(
        StableKey::parse("test:stone-enthalpy").unwrap(),
        vec![EnthalpyBand::Sensible {
            energy_start: 0,
            energy_end: 1000,
            temperature_start_mk: 0,
            temperature_end_mk: 1000,
            material: stone_key,
        }],
    )
    .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    let error = world
        .edit(|edit| {
            edit.step_body_local_thermal(
                body,
                &family,
                ThermalStepConfig {
                    conductance_numerator: 1,
                    conductance_denominator: 1,
                },
            )
        })
        .unwrap_err();

    assert_eq!(
        error.code,
        charon_engine::foundation::ErrorCode::CapacityExceeded
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
}

#[test]
fn body_snapshot_v6_restores_with_empty_block_attachment_state() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 8,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
            grid,
        )
        .unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 6);
    let restored = BodyPhysics::restore(&legacy).unwrap();
    let grid = restored.local_grid(body).unwrap().unwrap();
    assert_eq!(grid.cell_count(), 1);
    assert_eq!(grid.thermal_edge_state(0, 0), None);
}

#[test]
fn body_snapshot_v9_restores_without_coupling_provider_state() {
    let physics = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 9);

    let restored = BodyPhysics::restore(&legacy).unwrap();
    assert_eq!(restored.step_index(), 0);
    let migrated = restored.snapshot().unwrap();
    assert_eq!(&migrated[..8], b"CHBODY12");
}

#[test]
fn body_snapshot_v11_restores_legacy_json_backend() {
    let physics = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 11);

    let restored = BodyPhysics::restore(&legacy).unwrap();

    assert_eq!(restored.step_index(), 0);
    assert_eq!(&restored.snapshot().unwrap()[..8], b"CHBODY12");
}

#[test]
fn body_snapshot_v10_restores_without_pending_teleport_events() {
    let physics = BodyPhysics::new(BodyPhysicsSettings::default()).unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 10);

    let restored = BodyPhysics::restore(&legacy).unwrap();

    assert_eq!(restored.step_index(), 0);
    let current = restored.snapshot().unwrap();
    assert_eq!(&current[..8], b"CHBODY12");
    let payload: serde_json::Value = serde_json::from_slice(&current[16..]).unwrap();
    assert_eq!(
        payload["pending_teleport_exits"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn body_snapshot_v5_restores_without_thermal_edge_state() {
    let mut physics = BodyPhysics::new(BodyPhysicsSettings {
        gravity: [0.0, 0.0],
        ..BodyPhysicsSettings::default()
    })
    .unwrap();
    let mut grid = body_grid(
        1,
        1,
        [0.5, 0.5],
        BodyLocalGridLimits {
            cells: 1,
            geometry_cells: 1,
            sidecar_bytes: 8,
        },
    );
    grid.set_cell(
        0,
        0,
        BodyLocalCell::new(
            StableKey::parse("test:terrain").unwrap(),
            StableKey::parse("test:stone").unwrap(),
            0,
            0,
            0,
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let body = physics
        .insert_with_local_grid(
            BodyDef::dynamic(ColliderShape::Circle { radius: 0.25 }),
            grid,
        )
        .unwrap();
    let legacy = legacy_body_snapshot(&physics.snapshot().unwrap(), 5);

    let restored = BodyPhysics::restore(&legacy).unwrap();
    let grid = restored.local_grid(body).unwrap().unwrap();
    assert_eq!(grid.cell_count(), 1);
    assert_eq!(grid.thermal_edge_state(0, 0), None);
}
