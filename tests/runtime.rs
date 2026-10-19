//! E02の作者APIだけを通す統合試験。SceneにSend/Syncを要求しないことも検査します。

use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey,
        schema::{Schema, SchemaRegistry},
        state::{StateStore, StoreLimits},
    },
    runtime::{
        AppBuilder, Engine, FrameContext, Plugin, Scene, SceneContext, SceneState, StepResult,
        StopContext,
        config::{EngineSettings, EngineSettingsPatch, SimulationPatch, WorkerCount},
        input::{ActionId, InputBinding, InputEvent, InputValue, TickInput},
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
    },
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}
fn state() -> StateStore {
    let mut schemas = SchemaRegistry::default();
    schemas
        .register(Schema::of::<u64>(key("test:number"), 1, 8).unwrap())
        .unwrap();
    let mut state = StateStore::new(
        key("test:world"),
        schemas,
        StoreLimits {
            records: 16,
            payload_bytes: 128,
            checkpoint_bytes: 4096,
        },
    )
    .unwrap();
    let mut transaction = state.transaction().unwrap();
    transaction
        .put(key("test:count"), key("test:number"), &0_u64)
        .unwrap();
    transaction.commit().unwrap();
    state
}
fn event(tick: u64, sequence: u64, pressed: bool) -> TickInput {
    TickInput {
        tick,
        reset: false,
        events: vec![InputEvent {
            source: 0,
            sequence,
            binding: InputBinding::Keyboard(key("keyboard:enter")),
            value: InputValue::Button(pressed),
        }],
    }
}
fn count(engine: &Engine) -> u64 {
    let worlds = engine.worlds();
    worlds
        .get(worlds.id("main").unwrap(), key("test:count"))
        .unwrap()
}

struct Title {
    action: Option<ActionId>,
    trace: Rc<RefCell<Vec<&'static str>>>,
}
impl Scene for Title {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.trace.borrow_mut().push("title-start");
        self.action = Some(ctx.input().action("start").key("enter").register()?);
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if ctx.input().just_pressed(self.action.unwrap())? {
            ctx.scenes().switch("game")?;
        }
        Ok(())
    }
    fn stop(&mut self, _: &mut StopContext<'_>) -> EngineResult<()> {
        self.trace.borrow_mut().push("title-stop");
        Ok(())
    }
}
struct Game {
    starts: Rc<Cell<u32>>,
    updates: Rc<Cell<u32>>,
    renders: Rc<Cell<u32>>,
    cleanup: Rc<Cell<u32>>,
}
impl Scene for Game {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.starts.set(self.starts.get() + 1);
        let cleanup = Rc::clone(&self.cleanup);
        ctx.on_stop(move || {
            cleanup.set(cleanup.get() + 1);
            Ok(())
        })?;
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.updates.set(self.updates.get() + 1);
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("main")?;
        let value: u64 = worlds.view().get(id, key("test:count"))?;
        worlds.set(id, key("test:count"), key("test:number"), &(value + 1))
    }
    fn render(&mut self, _: &mut FrameContext<'_>) -> EngineResult<()> {
        self.renders.set(self.renders.get() + 1);
        Ok(())
    }
}
fn game() -> Game {
    Game {
        starts: Rc::new(Cell::new(0)),
        updates: Rc::new(Cell::new(0)),
        renders: Rc::new(Cell::new(0)),
        cleanup: Rc::new(Cell::new(0)),
    }
}

#[test]
fn title_input_transitions_at_boundary_and_preserves_world() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let game = game();
    let starts = Rc::clone(&game.starts);
    let mut engine = Engine::builder()
        .world("main", state())
        .scene(
            "title",
            Title {
                action: None,
                trace: Rc::clone(&trace),
            },
        )
        .scene("game", game)
        .start_scene("title")
        .build()
        .unwrap();
    assert_eq!(
        engine.step(event(1, 1, true)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(engine.scene_state("title").unwrap(), SceneState::Stopped);
    assert_eq!(engine.scene_state("game").unwrap(), SceneState::Running);
    assert_eq!(&*trace.borrow(), &["title-start", "title-stop"]);
    assert_eq!(starts.get(), 1);
    assert_eq!(count(&engine), 0);
    engine.step(TickInput::empty(2)).unwrap();
    assert_eq!(count(&engine), 1);
}

struct Control;
impl Scene for Control {
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        match ctx.tick() {
            1 => ctx.scenes().pause("game")?,
            2 => ctx.scenes().resume("game")?,
            _ => {}
        }
        Ok(())
    }
}
#[test]
fn pause_keeps_render_scope_and_world_clock_without_restarting_scene() {
    let game = game();
    let starts = Rc::clone(&game.starts);
    let updates = Rc::clone(&game.updates);
    let renders = Rc::clone(&game.renders);
    let cleanup = Rc::clone(&game.cleanup);
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("game", game)
        .scene("control", Control)
        .start_scene("game")
        .start_scene("control")
        .build()
        .unwrap();
    for tick in 1..=3 {
        engine.step(TickInput::empty(tick)).unwrap();
        engine.frame(0.5).unwrap();
    }
    assert_eq!(starts.get(), 1);
    assert_eq!(updates.get(), 2);
    assert_eq!(renders.get(), 3);
    assert_eq!(cleanup.get(), 0);
    let world = engine.worlds().id("main").unwrap();
    assert_eq!(engine.worlds().tick(world).unwrap(), 3);
    engine.shutdown().unwrap();
    engine.shutdown().unwrap();
    assert_eq!(cleanup.get(), 1);
}

struct Restart {
    action: Option<ActionId>,
    ids: Rc<RefCell<Vec<ActionId>>>,
    stale_rejected: Rc<Cell<bool>>,
    cleaned: Rc<Cell<u32>>,
}
impl Scene for Restart {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if let Some(old) = self.action {
            self.stale_rejected.set(ctx.input().held(old).is_err());
        }
        let id = ctx.input().action("start").key("enter").register()?;
        self.action = Some(id);
        self.ids.borrow_mut().push(id);
        let cleaned = Rc::clone(&self.cleaned);
        ctx.on_stop(move || {
            cleaned.set(cleaned.get() + 1);
            Ok(())
        })?;
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if ctx.tick() == 1 {
            ctx.scenes().stop("game")?;
            ctx.scenes().launch("game")?;
        }
        Ok(())
    }
}
#[test]
fn restart_reclaims_actions_and_allocates_a_new_generation() {
    let ids = Rc::new(RefCell::new(Vec::new()));
    let stale = Rc::new(Cell::new(false));
    let cleaned = Rc::new(Cell::new(0));
    let mut engine = Engine::builder()
        .action_capacity(1)
        .scope_capacity(1)
        .scene(
            "game",
            Restart {
                action: None,
                ids: Rc::clone(&ids),
                stale_rejected: Rc::clone(&stale),
                cleaned: Rc::clone(&cleaned),
            },
        )
        .start_scene("game")
        .build()
        .unwrap();
    engine.step(TickInput::empty(1)).unwrap();
    assert!(stale.get());
    assert_eq!(cleaned.get(), 1);
    let ids = ids.borrow();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].index(), ids[1].index());
    assert_ne!(ids[0].generation(), ids[1].generation());
}

struct Counter {
    until: u64,
    fail: bool,
}
impl System for Counter {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("main")?;
        let old: u64 = worlds.view().get(id, key("test:count"))?;
        worlds.set(id, key("test:count"), key("test:number"), &(old + 1))?;
        if self.fail {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "作者Systemの失敗",
            ));
        }
        Ok(if old + 1 < self.until {
            WorkStatus::ContinueSameTick
        } else {
            WorkStatus::Complete
        })
    }
}
fn counter_descriptor() -> SystemDescriptor {
    SystemDescriptor::new(key("test:counter"), Phase::Physics)
        .write(key("test:count"))
        .barrier()
}
#[test]
fn same_tick_resumes_without_publishing_candidates_or_repeating_scene_update() {
    let updates = Rc::new(Cell::new(0));
    struct Observer(Rc<Cell<u32>>);
    impl Scene for Observer {
        fn update(&mut self, _: &mut SceneContext<'_>) -> EngineResult<()> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("game", Observer(Rc::clone(&updates)))
        .start_scene("game")
        .work_budget(1)
        .system(
            counter_descriptor(),
            Counter {
                until: 3,
                fail: false,
            },
        )
        .build()
        .unwrap();
    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Pending(1)
    );
    assert_eq!(engine.tick(), 0);
    assert_eq!(count(&engine), 0);
    engine.frame(0.5).unwrap();
    assert_eq!(engine.resume().unwrap(), StepResult::Pending(1));
    assert_eq!(count(&engine), 0);
    assert_eq!(engine.resume().unwrap(), StepResult::Complete(1));
    assert_eq!(count(&engine), 3);
    assert_eq!(updates.get(), 1);
}
#[test]
fn system_failure_discards_world_changes_and_runs_scope_cleanup() {
    let game = game();
    let cleaned = Rc::clone(&game.cleanup);
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("game", game)
        .start_scene("game")
        .system(
            counter_descriptor(),
            Counter {
                until: 1,
                fail: true,
            },
        )
        .build()
        .unwrap();
    let world = engine.worlds().id("main").unwrap();
    let before = engine.worlds().checkpoint(world).unwrap();
    assert!(engine.step(TickInput::empty(1)).is_err());
    assert_eq!(engine.tick(), 0);
    assert_eq!(engine.worlds().checkpoint(world).unwrap(), before);
    assert_eq!(cleaned.get(), 1);
    assert!(engine.step(TickInput::empty(1)).is_err());
}

#[test]
fn system_access_and_dependency_cycles_are_rejected() {
    let mut engine = Engine::builder()
        .world("main", state())
        .system(
            SystemDescriptor::new(key("test:counter"), Phase::Physics).read(key("test:count")),
            Counter {
                until: 1,
                fail: false,
            },
        )
        .build()
        .unwrap();
    assert!(engine.step(TickInput::empty(1)).is_err());
    assert_eq!(count(&engine), 0);
    let result = Engine::builder()
        .world("main", state())
        .system(
            counter_descriptor().after(key("test:counter")),
            Counter {
                until: 1,
                fail: false,
            },
        )
        .build();
    assert!(matches!(
        result,
        Err(EngineError {
            code: ErrorCode::DependencyCycle,
            ..
        })
    ));
}

#[test]
fn fixed_updates_do_not_depend_on_frame_rate_and_backlog_is_retained() {
    fn run(rate: u64) -> (u64, u64) {
        let mut engine = Engine::builder()
            .world("main", state())
            .system(
                counter_descriptor(),
                Counter {
                    until: 0,
                    fail: false,
                },
            )
            .build()
            .unwrap();
        let mut previous = 0;
        for frame in 1..=rate {
            let next = frame * 1_000_000_000 / rate;
            engine
                .advance(Duration::from_nanos(next - previous), 8)
                .unwrap();
            engine.frame(engine.interpolation()).unwrap();
            previous = next;
        }
        (engine.tick(), count(&engine))
    }
    assert_eq!(run(30), (60, 60));
    assert_eq!(run(60), run(144));
    let mut engine = Engine::builder().build().unwrap();
    assert_eq!(engine.advance(Duration::from_secs(1), 4).unwrap(), 4);
    assert_eq!(engine.until_next_tick(), Duration::ZERO);
    assert_eq!(engine.advance(Duration::ZERO, 100).unwrap(), 56);
}

struct DependencyPlugin {
    key: StableKey,
    depends: Vec<StableKey>,
    trace: Rc<RefCell<Vec<StableKey>>>,
    failing: bool,
}
impl Plugin for DependencyPlugin {
    fn key(&self) -> StableKey {
        self.key
    }
    fn dependencies(&self) -> &[StableKey] {
        &self.depends
    }
    fn build(&self, _: &mut AppBuilder) -> EngineResult<()> {
        self.trace.borrow_mut().push(self.key);
        if self.failing {
            Err(EngineError::new(ErrorCode::InvalidValue, "plugin失敗"))
        } else {
            Ok(())
        }
    }
    fn shutdown(&self) -> EngineResult<()> {
        self.trace.borrow_mut().push(self.key);
        Ok(())
    }
}
#[test]
fn plugins_start_in_dependency_order_and_cleanup_in_reverse_after_failure() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let a = DependencyPlugin {
        key: key("test:a"),
        depends: vec![key("test:z")],
        trace: Rc::clone(&trace),
        failing: true,
    };
    let z = DependencyPlugin {
        key: key("test:z"),
        depends: vec![],
        trace: Rc::clone(&trace),
        failing: false,
    };
    assert!(Engine::builder().plugin(a).plugin(z).build().is_err());
    assert_eq!(
        &*trace.borrow(),
        &[key("test:z"), key("test:a"), key("test:a"), key("test:z")]
    );
}

#[test]
fn settings_validate_unknown_keys_and_merge_without_resetting_siblings() {
    use std::io::Write;
    let file = std::env::temp_dir().join(format!("charon-e02-settings-{}.yml", std::process::id()));
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .unwrap();
    output.write_all(b"schema_version: 1\nwindow:\n  width: 900\n  fullscreen: true\nassets:\n  root: ./example-assets\n").unwrap();
    drop(output);
    let settings = EngineSettings::resolve(
        Some(&file),
        &EngineSettingsPatch::default().window_size(1024, 768),
    )
    .unwrap();
    assert_eq!(settings.window.width, 1024);
    assert!(settings.window.fullscreen);
    assert_eq!(
        settings.assets.root,
        file.parent().unwrap().join("./example-assets")
    );
    std::fs::write(&file, "player:\n  speed: 5\n").unwrap();
    assert!(EngineSettings::resolve(Some(&file), &EngineSettingsPatch::default()).is_err());
    std::fs::remove_file(file).unwrap();
    assert!(
        Engine::builder()
            .settings(EngineSettingsPatch::default().step_hz(0))
            .build()
            .is_err()
    );
    for workers in [0_u16, 65] {
        assert!(
            Engine::builder()
                .settings(EngineSettingsPatch {
                    simulation: Some(SimulationPatch {
                        system_workers: Some(WorkerCount::Count(workers)),
                        ..SimulationPatch::default()
                    }),
                    ..EngineSettingsPatch::default()
                })
                .build()
                .is_err()
        );
    }
    assert!(Engine::builder().work_budget(0).build().is_err());
}

#[test]
fn invalid_tick_and_input_duplicates_do_not_advance_world() {
    let mut engine = Engine::builder().world("main", state()).build().unwrap();
    assert!(engine.step(TickInput::empty(2)).is_err());
    assert_eq!(engine.tick(), 0);
    let mut input = event(1, 1, true);
    input.events.push(input.events[0]);
    assert!(engine.step(input).is_err());
    assert_eq!(count(&engine), 0);
}

struct Buttons {
    action: Option<ActionId>,
    log: Rc<RefCell<Vec<(bool, bool, bool)>>>,
    exclusive: bool,
}
impl Scene for Buttons {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.action = Some(
            ctx.input()
                .action("move")
                .key("enter")
                .gamepad(0, "dpad-left")
                .register()?,
        );
        if self.exclusive {
            ctx.scenes().exclusive_input(true)?;
        }
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let input = ctx.input();
        let id = self.action.unwrap();
        self.log.borrow_mut().push((
            input.held(id)?,
            input.just_pressed(id)?,
            input.just_released(id)?,
        ));
        Ok(())
    }
}
#[test]
fn multiple_bindings_repeat_and_focus_reset_have_correct_edges() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut engine = Engine::builder()
        .action_capacity(1)
        .scene(
            "game",
            Buttons {
                action: None,
                log: Rc::clone(&log),
                exclusive: false,
            },
        )
        .start_scene("game")
        .build()
        .unwrap();
    engine.step(event(1, 1, true)).unwrap();
    engine.step(event(2, 2, true)).unwrap();
    engine
        .step(TickInput {
            tick: 3,
            reset: false,
            events: vec![
                InputEvent {
                    source: 1,
                    sequence: 1,
                    binding: InputBinding::Gamepad {
                        player: 0,
                        control: key("gamepad:dpad-left"),
                    },
                    value: InputValue::Button(true),
                },
                InputEvent {
                    source: 0,
                    sequence: 3,
                    binding: InputBinding::Keyboard(key("keyboard:enter")),
                    value: InputValue::Button(false),
                },
            ],
        })
        .unwrap();
    engine
        .step(TickInput {
            tick: 4,
            reset: true,
            events: vec![],
        })
        .unwrap();
    assert_eq!(
        &*log.borrow(),
        &[
            (true, true, false),
            (true, false, false),
            (true, true, true),
            (false, false, true)
        ]
    );
}

#[test]
fn modal_input_uses_launch_order_instead_of_registration_order() {
    let lower = Rc::new(RefCell::new(Vec::new()));
    let upper = Rc::new(RefCell::new(Vec::new()));
    let mut engine = Engine::builder()
        .scene(
            "menu",
            Buttons {
                action: None,
                log: Rc::clone(&upper),
                exclusive: true,
            },
        )
        .scene(
            "game",
            Buttons {
                action: None,
                log: Rc::clone(&lower),
                exclusive: false,
            },
        )
        .start_scene("game")
        .start_scene("menu")
        .build()
        .unwrap();
    engine.step(event(1, 1, true)).unwrap();
    assert_eq!(&*lower.borrow(), &[(false, false, false)]);
    assert_eq!(&*upper.borrow(), &[(true, true, false)]);
}

struct BadStart;
impl Scene for BadStart {
    fn start(&mut self, _: &mut SceneContext<'_>) -> EngineResult<()> {
        Err(EngineError::new(ErrorCode::InvalidValue, "開始失敗"))
    }
}
struct SwitchAfterWrite;
impl Scene for SwitchAfterWrite {
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("main")?;
        worlds.set(id, key("test:count"), key("test:number"), &99_u64)?;
        ctx.scenes().switch("bad")
    }
}
#[test]
fn failed_target_scene_start_does_not_publish_completed_candidate_world() {
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("title", SwitchAfterWrite)
        .scene("bad", BadStart)
        .start_scene("title")
        .build()
        .unwrap();
    let id = engine.worlds().id("main").unwrap();
    let before = engine.worlds().checkpoint(id).unwrap();
    assert!(engine.step(TickInput::empty(1)).is_err());
    assert_eq!(engine.tick(), 0);
    assert_eq!(count(&engine), 0);
    assert_eq!(engine.worlds().checkpoint(id).unwrap(), before);
    assert_eq!(engine.scene_state("bad").unwrap(), SceneState::Failed);
}

struct PauseWorld;
impl Scene for PauseWorld {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("main")?;
        worlds.pause(id, true)
    }
}
#[test]
fn world_bound_systems_freeze_independently_of_scene_updates() {
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("menu", PauseWorld)
        .start_scene("menu")
        .system(
            counter_descriptor().in_world(key("game:main")),
            Counter {
                until: 0,
                fail: false,
            },
        )
        .build()
        .unwrap();
    engine.step(TickInput::empty(1)).unwrap();
    assert_eq!(count(&engine), 0);
    let id = engine.worlds().id("main").unwrap();
    assert_eq!(engine.worlds().tick(id).unwrap(), 0);
}

#[test]
fn independent_clock_domains_preserve_fractional_time_and_pause_across_save() {
    use charon_engine::{
        foundation::codec::{Decoder, Encoder, StateCodec},
        runtime::clock::ClockDomain,
    };
    let mut clock = ClockDomain::new(1, 3).unwrap();
    assert_eq!(clock.advance(1).unwrap(), 0);
    let mut out = Encoder::new(32);
    clock.encode(&mut out).unwrap();
    let bytes = out.finish();
    let mut restored = ClockDomain::decode(&mut Decoder::new(&bytes, 32).unwrap()).unwrap();
    restored.pause(true);
    assert_eq!(restored.advance(100).unwrap(), 0);
    restored.pause(false);
    assert_eq!(restored.advance(2).unwrap(), 1);
    assert_eq!(restored.tick(), 1);
    assert!(ClockDomain::new(1, 0).is_err());
}

struct CapabilitiesPlugin;
impl Plugin for CapabilitiesPlugin {
    fn key(&self) -> StableKey {
        key("test:render")
    }
    fn build(&self, app: &mut AppBuilder) -> EngineResult<()> {
        app.service(key("engine:render"), key("test:render"), vec![], ())?;
        app.render_capabilities(charon_engine::runtime::RenderCapabilities {
            backends: vec!["vulkan".into()],
            max_dimension: 4096,
            grid_lighting: true,
        })
    }
}
#[test]
fn capability_resolution_rejects_unavailable_backend_and_resolves_auto() {
    use charon_engine::runtime::config::RenderPatch;
    let result = Engine::builder()
        .plugin(CapabilitiesPlugin)
        .settings(EngineSettingsPatch {
            render: Some(RenderPatch {
                backend: Some("metal".into()),
                ..RenderPatch::default()
            }),
            ..EngineSettingsPatch::default()
        })
        .build();
    assert!(matches!(
        result,
        Err(EngineError {
            code: ErrorCode::UnsupportedCapability,
            ..
        })
    ));
    let engine = Engine::builder()
        .plugin(CapabilitiesPlugin)
        .build()
        .unwrap();
    assert_eq!(engine.settings().render.backend, "vulkan");
}

struct Edges {
    action: Option<ActionId>,
    edges: Rc<Cell<(bool, bool, bool)>>,
}
impl Scene for Edges {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.action = Some(ctx.input().action("start").key("enter").register()?);
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let input = ctx.input();
        let id = self.action.unwrap();
        self.edges.set((
            input.held(id)?,
            input.just_pressed(id)?,
            input.just_released(id)?,
        ));
        Ok(())
    }
}
#[test]
fn press_and_release_in_one_tick_are_both_observable() {
    let edges = Rc::new(Cell::new((false, false, false)));
    let mut engine = Engine::builder()
        .scene(
            "game",
            Edges {
                action: None,
                edges: Rc::clone(&edges),
            },
        )
        .start_scene("game")
        .build()
        .unwrap();
    let mut input = event(1, 1, true);
    input.events.push(event(1, 2, false).events[0]);
    engine.step(input).unwrap();
    assert_eq!(edges.get(), (false, true, true));
}

struct HeldCounter {
    action: Option<ActionId>,
}
impl Scene for HeldCounter {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        self.action = Some(ctx.input().action("start").key("enter").register()?);
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if ctx.input().held(self.action.unwrap())? {
            let mut worlds = ctx.worlds();
            let world = worlds.view().id("main")?;
            let value: u64 = worlds.view().get(world, key("test:count"))?;
            worlds.set(world, key("test:count"), key("test:number"), &(value + 1))?;
        }
        Ok(())
    }
}
#[test]
fn scheduled_input_and_recording_are_independent_of_display_frequency() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    for hz in [30_u64, 60, 144] {
        let mut engine = Engine::builder()
            .world("main", state())
            .scene("game", HeldCounter { action: None })
            .start_scene("game")
            .build()
            .unwrap();
        engine
            .queue_input_at(1, event(1, 1, true).events[0])
            .unwrap();
        engine
            .queue_input_at(6, event(6, 2, false).events[0])
            .unwrap();
        for frame in 1..=hz {
            // 差分の合計を正確に1秒とし、浮動小数の丸めでtickが欠けないようにします。
            let nanos = frame * 1_000_000_000 / hz - (frame - 1) * 1_000_000_000 / hz;
            engine.advance(Duration::from_nanos(nanos), 8).unwrap();
            engine.frame(engine.interpolation()).unwrap();
        }
        assert_eq!(engine.tick(), 60);
        assert_eq!(count(&engine), 5);
        let input = engine.last_input().unwrap();
        let mut out = Encoder::new(4096);
        input.encode(&mut out).unwrap();
        let bytes = out.finish();
        let mut reader = Decoder::new(&bytes, 4096).unwrap();
        assert_eq!(&TickInput::decode(&mut reader).unwrap(), input);
        reader.finish().unwrap();
    }
    let input = event(1, 1, true);
    let mut out = Encoder::new(4096);
    input.encode(&mut out).unwrap();
    let bytes = out.finish();
    for length in 0..bytes.len() {
        let mut reader = Decoder::new(&bytes[..length], 4096).unwrap();
        assert!(TickInput::decode(&mut reader).is_err());
    }
}

#[test]
fn invalid_input_can_be_retried_without_losing_scheduled_events() {
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("game", HeldCounter { action: None })
        .start_scene("game")
        .build()
        .unwrap();
    engine
        .queue_input_at(1, event(1, 1, true).events[0])
        .unwrap();
    assert!(engine.step(event(1, 1, true)).is_err());
    assert_eq!(engine.tick(), 0);
    engine.step(TickInput::empty(1)).unwrap();
    assert_eq!(count(&engine), 1);
}
struct IgnoredFailure;
impl Scene for IgnoredFailure {
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("main")?;
        worlds.set(world, key("test:count"), key("test:number"), &99_u64)?;
        // 作者がエラーを捨てても、先行する書込だけを確定させてはなりません。
        let _ = worlds.set(world, key("test:count"), key("test:missing"), &100_u64);
        Ok(())
    }
}
#[test]
fn ignored_command_error_discards_the_entire_tick() {
    let mut engine = Engine::builder()
        .world("main", state())
        .scene("game", IgnoredFailure)
        .start_scene("game")
        .build()
        .unwrap();
    assert!(engine.step(TickInput::empty(1)).is_err());
    assert_eq!(engine.tick(), 0);
    assert_eq!(count(&engine), 0);
}
struct TraceSystem(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);
impl System for TraceSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        assert_eq!(ctx.input().tick, 1);
        self.0.lock().unwrap().push("before");
        Ok(WorkStatus::Complete)
    }
}
struct TraceScene(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);
impl Scene for TraceScene {
    fn update(&mut self, _: &mut SceneContext<'_>) -> EngineResult<()> {
        self.0.lock().unwrap().push("scene");
        Ok(())
    }
}
#[test]
fn before_scene_phase_is_explicit_and_runs_once_with_a_split_budget() {
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut engine = Engine::builder()
        .work_budget(1)
        .system(
            SystemDescriptor::new(key("test:before"), Phase::BeforeScenes),
            TraceSystem(std::sync::Arc::clone(&trace)),
        )
        .scene("game", TraceScene(std::sync::Arc::clone(&trace)))
        .start_scene("game")
        .build()
        .unwrap();
    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(&*trace.lock().unwrap(), &["before", "scene"]);
}

#[test]
fn conflicting_access_requires_explicit_dependency_and_profile_exposes_order() {
    let make = |name, dependency: bool| {
        let descriptor = SystemDescriptor::new(key(name), Phase::Intent).write(key("test:count"));
        if dependency {
            descriptor.after(key("test:a"))
        } else {
            descriptor
        }
    };
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let result = Engine::builder()
        .system(
            make("test:a", false),
            TraceSystem(std::sync::Arc::clone(&trace)),
        )
        .system(
            make("test:b", false),
            TraceSystem(std::sync::Arc::clone(&trace)),
        )
        .build();
    assert!(matches!(
        result,
        Err(EngineError {
            code: ErrorCode::Conflict,
            ..
        })
    ));
    let engine = Engine::builder()
        .system(
            make("test:b", true),
            TraceSystem(std::sync::Arc::clone(&trace)),
        )
        .system(make("test:a", false), TraceSystem(trace))
        .build()
        .unwrap();
    let profile = engine.schedule_profile().unwrap();
    assert_eq!(profile.systems[0].key, key("test:a"));
    assert_eq!(profile.systems[1].key, key("test:b"));
}

#[test]
fn independent_system_workers_one_two_and_four_publish_identical_state() {
    struct IndependentWriter {
        record: StableKey,
        barrier: Option<std::sync::Arc<std::sync::Barrier>>,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        delay: Duration,
    }
    impl System for IndependentWriter {
        fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(barrier) = &self.barrier {
                barrier.wait();
            }
            std::thread::sleep(self.delay);
            let mut worlds = context.worlds();
            let world = worlds.view().id("main")?;
            worlds.set(world, self.record, key("test:number"), &1_u64)?;
            Ok(WorkStatus::Complete)
        }
    }
    let system_keys = [
        "test:writer-a",
        "test:writer-b",
        "test:writer-c",
        "test:writer-d",
    ];
    let record_keys = [
        "test:value-a",
        "test:value-b",
        "test:value-c",
        "test:value-d",
    ];
    let mut reference = None;
    for workers in [1_u16, 2, 4] {
        let barrier = (workers > 1)
            .then(|| std::sync::Arc::new(std::sync::Barrier::new(usize::from(workers.min(4)))));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut builder = Engine::builder()
            .world("main", state())
            .settings(EngineSettingsPatch {
                simulation: Some(SimulationPatch {
                    system_workers: Some(WorkerCount::Count(workers)),
                    ..SimulationPatch::default()
                }),
                ..EngineSettingsPatch::default()
            });
        for index in 0..4 {
            let record = key(record_keys[index]);
            builder = builder.system(
                SystemDescriptor::new(key(system_keys[index]), Phase::Intent)
                    .write(record)
                    .parallel(),
                IndependentWriter {
                    record,
                    barrier: barrier.as_ref().map(std::sync::Arc::clone),
                    calls: std::sync::Arc::clone(&calls),
                    delay: Duration::from_millis(u64::from(4 - index as u8) * 5),
                },
            );
        }
        let mut engine = builder.build().unwrap();
        assert_eq!(
            engine.step(TickInput::empty(1)).unwrap(),
            StepResult::Complete(1)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 4);
        let world = engine.worlds().id("main").unwrap();
        let checkpoint = engine.worlds().checkpoint(world).unwrap();
        if let Some(expected) = &reference {
            assert_eq!(
                &checkpoint, expected,
                "worker数{workers}でも公開状態が一致する"
            );
        } else {
            reference = Some(checkpoint);
        }
    }
}

#[test]
fn parallel_later_system_result_waits_for_same_tick_continuation_order() {
    struct ContinueOnce {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl System for ContinueOnce {
        fn run(&mut self, _: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
            let call = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(if call == 0 {
                WorkStatus::ContinueSameTick
            } else {
                WorkStatus::Complete
            })
        }
    }
    struct WriteAfterContinue {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl System for WriteAfterContinue {
        fn run(&mut self, context: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut worlds = context.worlds();
            let world = worlds.view().id("main")?;
            worlds.set(world, key("test:after"), key("test:number"), &1_u64)?;
            Ok(WorkStatus::Complete)
        }
    }
    let mut checkpoints = Vec::new();
    for workers in [1_u16, 2] {
        let first_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let second_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let patch = EngineSettingsPatch {
            simulation: Some(SimulationPatch {
                system_workers: Some(WorkerCount::Count(workers)),
                ..SimulationPatch::default()
            }),
            ..EngineSettingsPatch::default()
        };
        let mut engine = Engine::builder()
            .world("main", state())
            .settings(patch)
            .work_budget(2)
            .system(
                SystemDescriptor::new(key("test:continue"), Phase::Intent).parallel(),
                ContinueOnce {
                    calls: std::sync::Arc::clone(&first_calls),
                },
            )
            .system(
                SystemDescriptor::new(key("test:later-write"), Phase::Intent)
                    .write(key("test:after"))
                    .parallel(),
                WriteAfterContinue {
                    calls: std::sync::Arc::clone(&second_calls),
                },
            )
            .build()
            .unwrap();
        assert_eq!(
            engine.step(TickInput::empty(1)).unwrap(),
            StepResult::Pending(1)
        );
        assert_eq!(engine.tick(), 0, "未完了SystemのWorld候補は公開しない");
        assert_eq!(engine.resume().unwrap(), StepResult::Complete(1));
        assert_eq!(first_calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(second_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let world = engine.worlds().id("main").unwrap();
        let value: u64 = engine.worlds().get(world, key("test:after")).unwrap();
        assert_eq!(value, 1);
        checkpoints.push(engine.worlds().checkpoint(world).unwrap());
    }
    assert_eq!(checkpoints[0], checkpoints[1]);
}

struct DeferredSystem;
impl System for DeferredSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let world = worlds.view().id("main")?;
        let value: u64 = worlds.view().get(world, key("test:count"))?;
        worlds.set(world, key("test:count"), key("test:number"), &(value + 1))?;
        Ok(WorkStatus::Deferred)
    }
}
#[test]
fn deferred_system_progress_is_canonical_and_resumes_in_the_next_tick() {
    let mut engine = Engine::builder()
        .world("main", state())
        .system(
            SystemDescriptor::new(key("test:deferred"), Phase::Intent).write(key("test:count")),
            DeferredSystem,
        )
        .build()
        .unwrap();
    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(count(&engine), 1);
    assert_eq!(
        engine.step(TickInput::empty(2)).unwrap(),
        StepResult::Complete(2)
    );
    assert_eq!(count(&engine), 2);
}

struct NeverComplete;
impl System for NeverComplete {
    fn run(&mut self, _: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        Ok(WorkStatus::ContinueSameTick)
    }
}
#[test]
fn nonprogressing_same_tick_stops_at_a_deterministic_limit() {
    let mut engine = Engine::builder()
        .world("main", state())
        .work_budget(1)
        .same_tick_limit(2)
        .system(
            SystemDescriptor::new(key("test:loop"), Phase::Intent),
            NeverComplete,
        )
        .build()
        .unwrap();
    assert_eq!(
        engine.step(TickInput::empty(1)).unwrap(),
        StepResult::Pending(1)
    );
    assert_eq!(
        engine.resume().unwrap_err().code,
        ErrorCode::CapacityExceeded
    );
    assert_eq!(engine.tick(), 0);
    assert_eq!(count(&engine), 0);
}

struct BarrierWriter;
impl System for BarrierWriter {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("main")?;
        let old: u64 = worlds.view().get(id, key("test:count"))?;
        worlds.set(id, key("test:count"), key("test:number"), &(old + 1))?;
        Ok(WorkStatus::Complete)
    }
}
struct MustSkip;
impl System for MustSkip {
    fn run(&mut self, _: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        panic!("非活動Systemは実行されないこと")
    }
}
#[test]
fn skipped_phase_tail_flushes_commands_for_paused_world_and_inactive_scene() {
    for scene_skip in [false, true] {
        for budget in [1, 64] {
            let mut tail = SystemDescriptor::new(key("test:z-tail"), Phase::Intent);
            tail = if scene_skip {
                tail.in_scene(key("game:dormant"))
            } else {
                tail.in_world(key("game:main"))
            };
            let mut builder = Engine::builder().world("main", state()).work_budget(budget);
            builder = if scene_skip {
                builder.scene("dormant", game())
            } else {
                builder.scene("menu", PauseWorld).start_scene("menu")
            };
            let mut engine = builder
                .system(
                    SystemDescriptor::new(key("test:a-writer"), Phase::Intent)
                        .write(key("test:count")),
                    BarrierWriter,
                )
                .system(tail, MustSkip)
                .system(
                    SystemDescriptor::new(key("test:next"), Phase::Transactions)
                        .write(key("test:count")),
                    BarrierWriter,
                )
                .build()
                .unwrap();
            let mut progress = engine.step(TickInput::empty(1)).unwrap();
            while matches!(progress, StepResult::Pending(_)) {
                progress = engine.resume().unwrap();
            }
            // Intentの書込がTransactionsへ届けば、同じ旧値からの上書きにはなりません。
            assert_eq!(count(&engine), 2);
        }
    }
}
#[test]
fn world_activity_membership_cannot_hide_cross_world_record_conflicts() {
    let result = Engine::builder()
        .world("main", state())
        .world("other", state())
        .system(
            SystemDescriptor::new(key("test:a"), Phase::Intent)
                .write(key("test:count"))
                .in_world(key("game:main")),
            BarrierWriter,
        )
        .system(
            SystemDescriptor::new(key("test:b"), Phase::Intent)
                .write(key("test:count"))
                .in_world(key("game:other")),
            BarrierWriter,
        )
        .build();
    assert!(matches!(
        result,
        Err(EngineError {
            code: ErrorCode::Conflict,
            ..
        })
    ));
    // World横断操作は引き続き可能です。依存とbarrierを宣言すれば結果を順に読めます。
    let mut engine = Engine::builder()
        .world("main", state())
        .world("other", state())
        .system(
            SystemDescriptor::new(key("test:a"), Phase::Intent)
                .write(key("test:count"))
                .in_world(key("game:main"))
                .barrier(),
            BarrierWriter,
        )
        .system(
            SystemDescriptor::new(key("test:b"), Phase::Intent)
                .write(key("test:count"))
                .in_world(key("game:other"))
                .after(key("test:a")),
            BarrierWriter,
        )
        .build()
        .unwrap();
    engine.step(TickInput::empty(1)).unwrap();
    assert_eq!(count(&engine), 2);
}
struct RejectConfiguration {
    dependencies: Vec<StableKey>,
}
impl Plugin for RejectConfiguration {
    fn key(&self) -> StableKey {
        key("test:reject")
    }
    fn dependencies(&self) -> &[StableKey] {
        &self.dependencies
    }
    fn configure(&self, _: &serde_yaml::Value) -> EngineResult<()> {
        Err(EngineError::new(ErrorCode::InvalidValue, "設定を拒否"))
    }
    fn build(&self, _: &mut AppBuilder) -> EngineResult<()> {
        panic!("configure失敗後はbuildしない")
    }
    fn shutdown(&self) -> EngineResult<()> {
        panic!("build前のpluginを解放しない")
    }
}
#[test]
fn configure_failure_cleans_only_plugins_whose_build_was_started() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let ready = DependencyPlugin {
        key: key("test:ready"),
        depends: vec![],
        trace: Rc::clone(&trace),
        failing: false,
    };
    let result = Engine::builder()
        .plugin(ready)
        .plugin(RejectConfiguration {
            dependencies: vec![key("test:ready")],
        })
        .build();
    assert!(matches!(
        result,
        Err(EngineError {
            code: ErrorCode::InvalidValue,
            ..
        })
    ));
    assert_eq!(&*trace.borrow(), &[key("test:ready"), key("test:ready")]);
}

#[test]
fn pointer_exit_is_canonical_input_and_does_not_reset_held_buttons() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    type PointerObservation = (Option<(f64, f64)>, bool);
    struct PointerScene {
        pointer: Option<ActionId>,
        button: Option<ActionId>,
        seen: Rc<RefCell<Vec<PointerObservation>>>,
    }
    impl Scene for PointerScene {
        fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
            self.pointer = Some(
                ctx.input()
                    .pointer_action("aim")
                    .binding(InputBinding::Pointer(0))
                    .register()?,
            );
            self.button = Some(ctx.input().action("held").key("enter").register()?);
            Ok(())
        }
        fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
            let input = ctx.input();
            self.seen.borrow_mut().push((
                input.pointer(self.pointer.unwrap())?,
                input.held(self.button.unwrap())?,
            ));
            Ok(())
        }
    }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut engine = Engine::builder()
        .scene(
            "game",
            PointerScene {
                pointer: None,
                button: None,
                seen: Rc::clone(&seen),
            },
        )
        .start_scene("game")
        .build()
        .unwrap();
    let mut first = event(1, 0, true);
    first.events.push(InputEvent {
        source: 0,
        sequence: 1,
        binding: InputBinding::Pointer(0),
        value: InputValue::Pointer { x: 30.5, y: 12.25 },
    });
    engine.step(first).unwrap();
    let exit = TickInput {
        tick: 2,
        reset: false,
        events: vec![InputEvent {
            source: 0,
            sequence: 2,
            binding: InputBinding::Pointer(0),
            value: InputValue::PointerUnavailable,
        }],
    };
    let mut encoder = Encoder::new(1024);
    exit.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut decoder = Decoder::new(&bytes, 1024).unwrap();
    let decoded = TickInput::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    engine.step(decoded).unwrap();
    engine.step(TickInput::empty(3)).unwrap();
    assert_eq!(
        &*seen.borrow(),
        &[(Some((30.5, 12.25)), true), (None, true), (None, true)]
    );
}
