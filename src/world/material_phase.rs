//! E05の複数標準material処理を単一World候補へ統合する公開入口。
//!
//! 各処理を個々のWorld::editで公開すると、後段の圧力・移動・粒子の容量失敗
//! に先行する移動だけが残る。このAPIは一つのWorldEditで全結果を確定する。
//! 呼出側のRuntime SystemはPhase::Materialsで一度だけ提出できる。

use super::{
    LayerId, MaterialForceConfig, MaterialForceMotionReport, MaterialForceProvider,
    MaterialMotionConfig, MaterialParticleForceConfig, MaterialParticleForceStepReport,
    MaterialTransportReport, PressureRelaxation, PressureRelaxationReport, WorldEdit, invalid,
};
use crate::foundation::{EngineError, EngineResult, grid::CellPoint};

/// E05の標準処理を一つのWorld取引内で実行する設定。
///
/// 実行順はpressure→cell force+DDA→scalar→canonical particle force+DDA。
/// 独自Providerの状態は別途保存済みWorld状態から評価し、任意の外部副作用を
/// 持つcallbackは許可しない。各段階を無効にしたとき隠れた既定処理はしない。
#[derive(Clone, Copy, Debug)]
pub struct MaterialPhaseConfig {
    /// 対象となるWorld内の登録済みシミュレーションlayer。
    pub layer: LayerId,
    /// 位置固定圧力channelの標準4回等のJacobi緩和。Noneなら実施しない。
    pub pressure: Option<PressureRelaxation>,
    /// cell用Q16力積分とDDA輸送。Noneなら実施しない。
    pub motion: Option<(MaterialForceConfig, MaterialMotionConfig)>,
    /// 粉・液・気の標準scalar輸送。Noneなら実施しない。
    pub scalar_candidate_limit: Option<usize>,
    /// 正規物質粒子の力積分と整数DDAの上限。Noneなら実施しない。
    pub particle: Option<(MaterialParticleForceConfig, usize)>,
}

/// 省略・適用された処理を区別する結果。既存ソルバーの診断値をそのまま公開。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialPhaseReport {
    /// 圧力緩和の実行結果。
    pub pressure: Option<PressureRelaxationReport>,
    /// cell Q16力学の実行結果。
    pub motion: Option<MaterialForceMotionReport>,
    /// スカラー輸送の実行結果。
    pub scalar: Option<MaterialTransportReport>,
    /// 休止証明付きscalar入口を選んだ場合のsleep/wake診断。
    pub scalar_sleep: Option<super::ScalarMaterialSleepReport>,
    /// 正規粒子力学の実行結果。
    pub particle: Option<MaterialParticleForceStepReport>,
}

/// PressureJob後に作った押出し探索と標準phaseの進捗です。
pub struct MaterialPhasePressureSearchReport {
    /// 同じ候補へ出力したPressureJobの報告。resume時は空です。
    pub pressure: Vec<PressureRelaxationReport>,
    /// 新圧力snapshotから進めた押出し探索の状態。
    pub push_status: super::PressurePushStatus,
    /// 探索継続時に作者が同じWorld StateStoreへ保存するJobです。
    pub pending_search: Option<super::PressurePushSearch>,
    /// MaterialMotionJob/押出し経路の公開報告。探索継続中は空です。
    pub jobs: super::MaterialMotionBatchReport,
    /// 探索がReadyまたはNoOutletとなり標準phaseまで完了した場合の報告。
    pub phase: Option<MaterialPhaseReport>,
}

/// PressureJob後の押出し探索を始める入力です。
#[derive(Clone, Copy, Debug)]
pub struct MaterialPhasePressureSearchInput {
    /// 押出し探索を行うsimulated layer。
    pub layer: LayerId,
    /// 圧力が閾値を超え、空cell経路を探すsource座標。
    pub source: CellPoint,
    /// 探索対象の圧力channel・重力・閾値・node上限。
    pub config: super::PressurePushConfig,
    /// この呼出で許可する探索node数。
    pub budget: usize,
}

impl WorldEdit<'_> {
    /// E05の標準処理を原子的に実行し、成功なら全reportを返します。
    ///
    /// 外側のWorld::edit、World::edit_material_accounted、またはRuntimeの
    /// WorldAccess::edit_material_accountedから呼びます。後段が失敗した場合は
    /// `WorldEdit::run` が失敗状態を覚え、最終World候補と台帳を破棄します。
    /// このAPI自体はRegionのIOを実行せず、ロード完了済みの入力だけを読む。
    pub fn step_material_phase(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialPhaseReport> {
        self.run(|editor| {
            editor.step_material_phase_inner(config, provider, None, false, None, None)
        })
    }

    /// 標準scalarのsleep/wake trackerを同じMaterials取引へ接続します。
    ///
    /// trackerは値で受け取り、phaseと全後段処理が成功したときだけ更新値を返します。
    /// callerは返されたtrackerをWorld StateStoreへ保存するか、次のphaseへ渡せます。
    /// checkpointからdecodeしたtrackerは未認証として参照輸送を一度実行します。
    pub fn step_material_phase_with_scalar_sleep(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        mut tracker: super::ScalarMaterialSleepTracker,
    ) -> EngineResult<(MaterialPhaseReport, super::ScalarMaterialSleepTracker)> {
        self.run(|editor| {
            let report = editor.step_material_phase_inner(
                config,
                provider,
                None,
                false,
                Some(&mut tracker),
                None,
            )?;
            Ok((report, tracker))
        })
    }

    /// 正規粒子の静止sleep証明を標準Materials phaseへ接続します。
    ///
    /// trackerは値で受け取り、圧力・cell移動・scalar・粒子処理の取引全体が成功したときだけ
    /// 更新値を返します。先行phaseが粒子の活動chunkを変えた場合やsourceを予約した場合は
    /// sleep証明を使わず参照DDAへ戻します。Runtimeでは返却trackerを同じ監査付き編集で
    /// World StateStoreへ保存できます。decodeした証明は参照実行を一度通します。
    pub fn step_material_phase_with_particle_sleep(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        mut tracker: super::MaterialParticleSleepTracker,
    ) -> EngineResult<(
        MaterialPhaseReport,
        super::MaterialParticleSleepTracker,
        super::MaterialParticleSleepReport,
    )> {
        self.run(|editor| {
            if config.particle.is_none() {
                return Err(invalid("粒子sleepにはphaseのparticle設定が必要"));
            }
            let mut particle_sleep_report = None;
            let report = editor.step_material_phase_inner(
                config,
                provider,
                None,
                false,
                None,
                Some((&mut tracker, &mut particle_sleep_report)),
            )?;
            let particle_sleep_report =
                particle_sleep_report.ok_or_else(|| invalid("粒子sleep report欠落"))?;
            Ok((report, tracker, particle_sleep_report))
        })
    }

    /// 広域移動・押出しJobと標準MaterialPhaseを一つの同tick候補へ統合します。
    ///
    /// 広域Jobの全入力と経路を先に検証し、Job同士の原子的な公開後も同じ予約bitsetを
    /// cell DDA・scalar・粒子へ渡します。経路・source・destination・斜め角確認cellは
    /// 後段の標準solverが使用しません。後段が失敗した場合はJob公開も含めてrollbackします。
    /// Job検証は既存batch内でも行うため、二重検証分をjob_cellsへ加算し、Job用scratchと
    /// 共有予約scratchの合計をjob_bytesへ収めてから確保します。
    ///
    /// 実行順は広域Job公開→通常MaterialPhase（圧力、cell force+DDA、scalar、粒子）です。
    /// 押出し探索は渡されたsnapshotで検証され、同tickの圧力更新から再探索しません。
    pub fn step_material_phase_with_jobs(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        motion_jobs: &[super::MaterialMotionJob],
        pressure_pushes: &[super::PressurePushSearch],
    ) -> EngineResult<(super::MaterialMotionBatchReport, MaterialPhaseReport)> {
        let (jobs, _, phase) = self.step_material_phase_with_pressure_jobs(
            config,
            provider,
            motion_jobs,
            pressure_pushes,
            &[],
        )?;
        Ok((jobs, phase))
    }

    /// PressureJob出力・広域移動・押出し・標準MaterialPhaseを一候補へ統合します。
    ///
    /// PressureJobと移動/押出しJobは元の同じWorldから事前検証し、公開後に通常MaterialPhaseを
    /// 実行します。PressurePushSearchは入力時点の圧力snapshotに従い、PressureJob出力から
    /// 再探索しません。pressure出力報告を含む全結果を返し、後段solverが失敗すればJob出力もrollbackします。
    /// batch内の再検証と標準phaseの共有予約を含む仕事量・scratch量を先に合算し、WorldLimitsを
    /// 超える場合はWorld候補を変更する前に失敗します。
    pub fn step_material_phase_with_pressure_jobs(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        motion_jobs: &[super::MaterialMotionJob],
        pressure_pushes: &[super::PressurePushSearch],
        pressure_jobs: &[super::PressureJob],
    ) -> EngineResult<(
        super::MaterialMotionBatchReport,
        Vec<super::PressureRelaxationReport>,
        MaterialPhaseReport,
    )> {
        self.run(|editor| {
            if !editor.world.content.layer(config.layer)?.simulated {
                return Err(invalid("Materials phase対象はsimulated layerのみ"));
            }
            let shared_reservations = config.motion.is_some()
                || config.scalar_candidate_limit.is_some()
                || config.particle.is_some();
            let movement_items = motion_jobs
                .len()
                .checked_add(pressure_pushes.len())
                .ok_or_else(|| invalid("MaterialPhase移動Job数overflow"))?;
            let share_wide_reservations = shared_reservations && movement_items != 0;
            let item_count = motion_jobs
                .len()
                .checked_add(pressure_pushes.len())
                .and_then(|value| value.checked_add(pressure_jobs.len()))
                .ok_or_else(|| invalid("MaterialPhase広域Job数overflow"))?;
            if item_count > editor.world.limits.jobs as usize {
                return Err(crate::foundation::EngineError::capacity(
                    "MaterialPhase広域Job数",
                    item_count as u64,
                    editor.world.limits.jobs.into(),
                ));
            }
            let movement_work = super::material_motion_job::material_job_validation_work(
                editor.world,
                motion_jobs,
                pressure_pushes,
            )?;
            let (pressure_work, pressure_scratch, _) =
                super::pressure_job::pressure_jobs_cost(editor.world, pressure_jobs)?;
            let movement_work_multiplier = if share_wide_reservations { 2 } else { 1 };
            let total_work = movement_work
                .checked_mul(movement_work_multiplier)
                .and_then(|value| value.checked_add(pressure_work))
                .ok_or_else(|| invalid("MaterialPhaseとPressureJob検証量overflow"))?;
            if total_work > editor.world.limits.job_cells {
                return Err(crate::foundation::EngineError::capacity(
                    "MaterialPhaseと広域Jobの合計検証予算",
                    total_work as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            let movement_scratch =
                super::material_motion_job::integrated_material_job_scratch_bytes(
                    editor.world,
                    motion_jobs,
                    pressure_pushes,
                    share_wide_reservations,
                )?;
            let scratch_bytes = movement_scratch
                .checked_add(pressure_scratch)
                .ok_or_else(|| invalid("MaterialPhaseとPressureJob scratch byte数overflow"))?;
            if scratch_bytes > editor.world.limits.job_bytes {
                return Err(crate::foundation::EngineError::capacity(
                    "MaterialPhaseと広域Jobの合計scratch byte数",
                    scratch_bytes as u64,
                    editor.world.limits.job_bytes as u64,
                ));
            }

            let layer_key = editor.world.content.layer(config.layer)?.key;
            let mut reservations = if share_wide_reservations {
                Some(super::transport::CellReservations::new(editor.world)?)
            } else {
                None
            };
            let mut force_reservations = false;
            if let Some(reservations) = reservations.as_mut() {
                let (actual_work, has_job_reservations) =
                    super::material_motion_job::reserve_material_job_paths(
                        editor.world,
                        motion_jobs,
                        pressure_pushes,
                        layer_key,
                        reservations,
                    )?;
                if actual_work != movement_work {
                    return Err(invalid("MaterialPhase広域Job検証量の見積り不一致"));
                }
                force_reservations = has_job_reservations;
            }
            let (jobs, pressure_reports) = editor
                .publish_material_motion_pressure_jobs_and_pushes(
                    motion_jobs,
                    pressure_pushes,
                    pressure_jobs,
                )?;
            let phase = editor.step_material_phase_inner(
                config,
                provider,
                reservations,
                force_reservations,
                None,
                None,
            )?;
            Ok((jobs, pressure_reports, phase))
        })
    }

    /// PressureJob出力の新snapshotから押出しを開始し、Readyなら標準phaseまで原子的に進めます。
    ///
    /// 押出し探索はpressure出力を候補へ適用した後に開始するため、旧圧力snapshotを使いません。
    /// 探索がbudget内に終わらない場合はpressure出力とpending searchだけを候補へ残し、
    /// cell/scalar/particleの後段は実行しません。呼出側はpending searchをWorld StateStoreへ
    /// 保存し、`resume_material_phase_with_pressure_search`で続けます。Ready時には押出し経路を
    /// phase共通reservationへ登録してから通常処理を実行し、同じ流体tokenの二重輸送を防ぎます。
    /// 初回呼出では広域MaterialMotionJobを受け付けません。pressure出力でchunk revisionが進むため、
    /// 継続時に追加する広域Jobはpressure公開後のWorldを基準に生成する必要があります。後段が
    /// 失敗した場合はpressure出力を含む候補全体をrollbackします。
    pub fn step_material_phase_with_pressure_job_search(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        pressure_jobs: &[super::PressureJob],
        push: MaterialPhasePressureSearchInput,
    ) -> EngineResult<MaterialPhasePressureSearchReport> {
        self.run(|editor| {
            preflight_pressure_search_material_phase(
                editor.world,
                config,
                &[],
                pressure_jobs,
                push.config.nodes as usize,
                push.budget,
            )?;
            let prepared = super::pressure_job::prepare_pressure_jobs(editor.world, pressure_jobs)?;
            let pressure = super::pressure_job::apply_pressure_outputs(editor, prepared)?;
            let mut search =
                super::PressurePushSearch::new(editor.world, push.layer, push.source, push.config)?;
            let push_status = search.step(editor.world, push.budget)?;
            finish_pressure_search_material_phase(
                editor,
                config,
                provider,
                &[],
                pressure,
                search,
                push_status,
            )
        })
    }

    /// 新圧力snapshotに紐づく押出し探索を再開し、完了したら同じphaseで押出し・移動・solverを行います。
    ///
    /// 進行中の探索は以前のWorld StateStoreから復元して渡します。探索中は他の物質更新を公開せず、
    /// 入力revisionの失効を避けます。Ready時には押出し経路とpressure公開後のWorldから生成した
    /// MaterialMotionJobを共有予約へ登録し、標準phaseと一取引で確定します。NoOutletなら押出しを
    /// 省略して標準phaseを実行します。
    pub fn resume_material_phase_with_pressure_search(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        motion_jobs: &[super::MaterialMotionJob],
        mut search: super::PressurePushSearch,
        budget: usize,
    ) -> EngineResult<MaterialPhasePressureSearchReport> {
        self.run(|editor| {
            preflight_pressure_search_material_phase(
                editor.world,
                config,
                motion_jobs,
                &[],
                search.node_capacity(),
                budget,
            )?;
            let push_status = search.step(editor.world, budget)?;
            finish_pressure_search_material_phase(
                editor,
                config,
                provider,
                motion_jobs,
                Vec::new(),
                search,
                push_status,
            )
        })
    }

    fn step_material_phase_inner(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
        mut reservations: Option<super::transport::CellReservations>,
        force_reservations: bool,
        scalar_sleep: Option<&mut super::ScalarMaterialSleepTracker>,
        particle_sleep: Option<(
            &mut super::MaterialParticleSleepTracker,
            &mut Option<super::MaterialParticleSleepReport>,
        )>,
    ) -> EngineResult<MaterialPhaseReport> {
        if !self.world.content.layer(config.layer)?.simulated {
            return Err(invalid("Materials phase対象はsimulated layerのみ"));
        }
        let pressure = config
            .pressure
            .map(|setting| self.relax_pressure(config.layer, setting))
            .transpose()?;
        if reservations.is_none()
            && (config.motion.is_some() || config.scalar_candidate_limit.is_some())
        {
            reservations = Some(super::transport::CellReservations::new(self.world)?);
        }
        let motion = config
            .motion
            .map(|(force, motion)| {
                if force.channel != motion.channel {
                    return Err(invalid("力積分とDDAの速度channel不一致"));
                }
                let force_report = if force_reservations {
                    self.integrate_material_forces_reserved(
                        config.layer,
                        force,
                        provider,
                        reservations.as_ref().expect("motionには予約scratchが必要"),
                    )?
                } else {
                    self.integrate_material_forces(config.layer, force, provider)?
                };
                let movement = self.transport_material_motion_reserved(
                    config.layer,
                    motion,
                    reservations.as_mut().expect("motionには予約scratchが必要"),
                )?;
                Ok(MaterialForceMotionReport {
                    force: force_report,
                    motion: movement,
                })
            })
            .transpose()?;
        let mut scalar_sleep_report = None;
        let scalar = if let Some(limit) = config.scalar_candidate_limit {
            let reservations = reservations.as_mut().expect("scalarには予約scratchが必要");
            Some(if let Some(tracker) = scalar_sleep {
                let candidate_tracker = std::mem::take(tracker);
                let (sleep_report, next_tracker) = self.step_scalar_materials_sleeping(
                    candidate_tracker,
                    config.layer,
                    limit,
                    reservations,
                )?;
                *tracker = next_tracker;
                scalar_sleep_report = Some(sleep_report);
                sleep_report.reference
            } else {
                self.transport_materials_reserved(config.layer, limit, reservations)?
            })
        } else {
            None
        };
        let particle = if let Some((forces, dda_steps)) = config.particle {
            if let Some((tracker, sleep_report)) = particle_sleep {
                let sleep = self.step_material_particle_force_motion_sleeping_reserved(
                    tracker,
                    config.layer,
                    forces,
                    dda_steps,
                    provider,
                    reservations.as_mut(),
                )?;
                *sleep_report = Some(sleep);
                Some(sleep.reference)
            } else if let Some(reservations) = reservations.as_ref() {
                Some(self.step_material_particle_forces_in_layer_reserved(
                    config.layer,
                    forces,
                    provider,
                    dda_steps,
                    reservations,
                )?)
            } else {
                Some(self.step_material_particle_forces_in_layer(
                    config.layer,
                    forces,
                    provider,
                    dda_steps,
                )?)
            }
        } else {
            None
        };
        Ok(MaterialPhaseReport {
            pressure,
            motion,
            scalar,
            scalar_sleep: scalar_sleep_report,
            particle,
        })
    }
}

fn preflight_pressure_search_material_phase(
    world: &super::World,
    config: MaterialPhaseConfig,
    motion_jobs: &[super::MaterialMotionJob],
    pressure_jobs: &[super::PressureJob],
    search_node_capacity: usize,
    budget: usize,
) -> EngineResult<()> {
    if budget == 0 || search_node_capacity == 0 {
        return Err(invalid("PressureJob押出し統合の探索budget・node上限"));
    }
    if !world.content.layer(config.layer)?.simulated {
        return Err(invalid("Materials phase対象はsimulated layerのみ"));
    }
    let item_count = motion_jobs
        .len()
        .checked_add(pressure_jobs.len())
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| invalid("PressureJob押出し統合のjob数overflow"))?;
    if item_count > world.limits.jobs as usize {
        return Err(EngineError::capacity(
            "PressureJob・移動・押出し統合Job数",
            item_count as u64,
            world.limits.jobs.into(),
        ));
    }

    let (pressure_work, pressure_scratch, _) =
        super::pressure_job::pressure_jobs_cost(world, pressure_jobs)?;
    let active_chunks = world.chunks.values().filter(|chunk| chunk.active).count();
    // 検索生成とrevision照合はchunk map全体を走査し、訪問bitsetだけがactive chunkに比例する。
    let loaded_chunks = world.chunks.len();
    let search_step_work = loaded_chunks
        .checked_mul(2)
        .and_then(|value| value.checked_add(budget.min(search_node_capacity)))
        .ok_or_else(|| invalid("PressureJob押出し探索work量overflow"))?;
    let motion_work =
        super::material_motion_job::material_job_validation_work(world, motion_jobs, &[])?
            .checked_add(
                search_node_capacity
                    .checked_mul(8)
                    .ok_or_else(|| invalid("PressureJob押出し経路work量overflow"))?,
            )
            .ok_or_else(|| invalid("PressureJob押出し統合work量overflow"))?;
    let share_phase_reservations = config.motion.is_some()
        || config.scalar_candidate_limit.is_some()
        || config.particle.is_some();
    let movement_work = if share_phase_reservations {
        motion_work
            .checked_mul(2)
            .ok_or_else(|| invalid("PressureJob押出し二重検証量overflow"))?
    } else {
        motion_work
    };
    let total_work = pressure_work
        .checked_add(search_step_work)
        .and_then(|value| value.checked_add(movement_work))
        .ok_or_else(|| invalid("PressureJob押出し統合work量overflow"))?;
    if total_work > world.limits.job_cells {
        return Err(EngineError::capacity(
            "PressureJob・探索・MaterialPhase検証予算",
            total_work as u64,
            world.limits.job_cells as u64,
        ));
    }

    let search_bytes = search_node_capacity
        .checked_mul(std::mem::size_of::<(CellPoint, u32)>())
        .and_then(|bytes| {
            active_chunks
                .checked_mul(
                    std::mem::size_of::<(super::ChunkCoord, u64)>()
                        + std::mem::size_of::<(super::ChunkCoord, [u64; 64])>(),
                )
                .and_then(|chunk_bytes| bytes.checked_add(chunk_bytes))
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<super::PressurePushSearch>()))
        .ok_or_else(|| invalid("PressureJob押出しsearch scratch byte数overflow"))?;
    let movement_scratch =
        super::material_motion_job::integrated_material_job_scratch_bytes_bounded(
            world,
            motion_jobs,
            1,
            search_node_capacity,
            share_phase_reservations,
        )?;
    let scratch_bytes = pressure_scratch
        .checked_add(search_bytes)
        .and_then(|bytes| bytes.checked_add(movement_scratch))
        .ok_or_else(|| invalid("PressureJob押出し統合scratch byte数overflow"))?;
    if scratch_bytes > world.limits.job_bytes {
        return Err(EngineError::capacity(
            "PressureJob・探索・MaterialPhase scratch byte数",
            scratch_bytes as u64,
            world.limits.job_bytes as u64,
        ));
    }
    Ok(())
}

fn finish_pressure_search_material_phase(
    editor: &mut WorldEdit<'_>,
    config: MaterialPhaseConfig,
    provider: &dyn MaterialForceProvider,
    motion_jobs: &[super::MaterialMotionJob],
    pressure: Vec<PressureRelaxationReport>,
    search: super::PressurePushSearch,
    push_status: super::PressurePushStatus,
) -> EngineResult<MaterialPhasePressureSearchReport> {
    match push_status {
        super::PressurePushStatus::Searching => Ok(MaterialPhasePressureSearchReport {
            pressure,
            push_status,
            pending_search: Some(search),
            jobs: super::MaterialMotionBatchReport::default(),
            phase: None,
        }),
        super::PressurePushStatus::Ready => {
            let (jobs, phase) = editor.step_material_phase_with_jobs(
                config,
                provider,
                motion_jobs,
                std::slice::from_ref(&search),
            )?;
            Ok(MaterialPhasePressureSearchReport {
                pressure,
                push_status,
                pending_search: None,
                jobs,
                phase: Some(phase),
            })
        }
        super::PressurePushStatus::NoOutlet => {
            let (jobs, phase) =
                editor.step_material_phase_with_jobs(config, provider, motion_jobs, &[])?;
            Ok(MaterialPhasePressureSearchReport {
                pressure,
                push_status,
                pending_search: None,
                jobs,
                phase: Some(phase),
            })
        }
    }
}
