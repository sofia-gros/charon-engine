//! MOD のスクリプトの実行系 (仕様 16)。Rhai の小さな部分だけを、ゲームのロジック (`Game`) として動かす。
//!
//! スクリプトは世界を直接触らない。関数は **命令** を溜めるだけで、`on_tick` が終わったあとにエンジンがまとめて行う。
//! 読み取りは、tick の初めに渡す写し (`tick()`、`player_x()` など) だけ。実行回数・命令の数・文字列の長さに上限がある。

use std::sync::{Arc, Mutex};

use charon_core::cell::MaterialId;
use charon_core::coords::CellPos;
use charon_game::game::GameEvent;
use rhai::{AST, Engine, Scope};

use crate::api::{Context, Game};
use crate::audio::SoundEvent;

/// 1 回の呼び出しで、スクリプトが実行できる命令の数。
pub const MAX_OPERATIONS: u64 = 50_000;
/// 1 tick に溜められる命令の数。超えた分は捨てる。
pub const MAX_COMMANDS: usize = 256;
/// 実行時エラーがこの回数に達したら、そのスクリプトは止める。
pub const MAX_ERRORS: u32 = 10;

/// スクリプトが頼む操作。
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Command {
    Shake(f32),
    Play(SoundEvent),
    /// 持ち物にその材質を足す (プレイヤーの)。
    Give(String, u32),
    /// セルを材質に置き換える。
    SetCell(i32, i32, String),
    /// その位置 (セル) に、材質を個数だけ落とす (拾える、仕様 10 §19)。
    SpawnDrop(i32, i32, String, u32),
}

/// スクリプトから読める、tick の初めの写し。
#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    tick: i64,
    player: Option<(i64, i64)>,
    hp: i64,
}

#[derive(Debug, Default)]
struct Shared {
    snapshot: Snapshot,
    commands: Vec<Command>,
    log: Vec<String>,
}

/// スクリプトで動くゲームロジック。
pub struct ScriptGame {
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    shared: Arc<Mutex<Shared>>,
    has_tick: bool,
    has_start: bool,
    has_event: bool,
    errors: u32,
}

impl std::fmt::Debug for ScriptGame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptGame").field("errors", &self.errors).finish_non_exhaustive()
    }
}

fn locked(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn sound_named(name: &str) -> Option<SoundEvent> {
    Some(match name {
        "jump" => SoundEvent::Jump,
        "shoot" => SoundEvent::Shoot,
        "anchor" => SoundEvent::HookAnchor,
        "pickup" => SoundEvent::Pickup,
        _ => return None,
    })
}

impl ScriptGame {
    /// ソースを読み込む。文法のエラーは `Err` (ゲームを始めずに知らせる)。
    pub fn new(source: &str) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(16);
        engine.set_max_expr_depths(64, 32);
        engine.set_max_string_size(256);
        engine.set_max_array_size(256);
        engine.set_max_map_size(64);
        // `print` / `debug` は、エンジンのログに溜める。
        let log = Arc::clone(&shared);
        engine.on_print(move |text| {
            let mut s = locked(&log);
            if s.log.len() < 64 {
                s.log.push(text.to_string());
            }
        });
        engine.on_debug(|_, _, _| {});

        let push = |shared: &Arc<Mutex<Shared>>, command: Command| {
            let mut s = locked(shared);
            if s.commands.len() < MAX_COMMANDS {
                s.commands.push(command);
            }
        };
        let s = Arc::clone(&shared);
        engine.register_fn("shake", move |amount: f64| {
            push(&s, Command::Shake(amount as f32));
        });
        let s = Arc::clone(&shared);
        engine.register_fn("play", move |name: &str| {
            if let Some(sound) = sound_named(name) {
                push(&s, Command::Play(sound));
            }
        });
        let s = Arc::clone(&shared);
        engine.register_fn("give", move |material: &str, count: i64| {
            let count = u32::try_from(count.clamp(0, 99_999)).unwrap_or(0);
            push(&s, Command::Give(material.to_string(), count));
        });
        let s = Arc::clone(&shared);
        engine.register_fn("set_cell", move |x: i64, y: i64, material: &str| {
            let clamp = |v: i64| i32::try_from(v).unwrap_or(0);
            push(&s, Command::SetCell(clamp(x), clamp(y), material.to_string()));
        });
        let s = Arc::clone(&shared);
        engine.register_fn("spawn_drop", move |x: i64, y: i64, material: &str, count: i64| {
            let clamp = |v: i64| i32::try_from(v).unwrap_or(0);
            let count = u32::try_from(count.clamp(0, 99_999)).unwrap_or(0);
            push(&s, Command::SpawnDrop(clamp(x), clamp(y), material.to_string(), count));
        });
        let s = Arc::clone(&shared);
        engine.register_fn("tick", move || locked(&s).snapshot.tick);
        let s = Arc::clone(&shared);
        engine.register_fn("has_player", move || locked(&s).snapshot.player.is_some());
        let s = Arc::clone(&shared);
        engine.register_fn("player_x", move || locked(&s).snapshot.player.map_or(0, |p| p.0));
        let s = Arc::clone(&shared);
        engine.register_fn("player_y", move || locked(&s).snapshot.player.map_or(0, |p| p.1));
        let s = Arc::clone(&shared);
        engine.register_fn("hp", move || locked(&s).snapshot.hp);

        let ast = engine.compile(source).map_err(|e| e.to_string())?;
        let names: Vec<String> = ast.iter_functions().map(|f| f.name.to_string()).collect();
        let has = |name: &str| names.iter().any(|n| n == name);
        Ok(Self {
            has_tick: has("on_tick"),
            has_start: has("on_start"),
            has_event: has("on_event"),
            engine,
            ast,
            scope: Scope::new(),
            shared,
            errors: 0,
        })
    }

    /// 実行時エラーで止まったか。
    pub fn halted(&self) -> bool {
        self.errors >= MAX_ERRORS
    }

    /// 溜まった `print` の出力を取り出す。
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut locked(&self.shared).log)
    }

    fn call(&mut self, name: &str, args: impl rhai::FuncArgs) {
        if self.halted() {
            return;
        }
        if let Err(err) = self.engine.call_fn::<()>(&mut self.scope, &self.ast, name, args) {
            self.errors += 1;
            eprintln!("スクリプトの {name} でエラー ({}/{MAX_ERRORS}): {err}", self.errors);
        }
    }

    fn apply(commands: Vec<Command>, ctx: &mut Context<'_>) {
        for command in commands {
            match command {
                Command::Shake(amount) => ctx.shake(amount),
                Command::Play(sound) => ctx.play(sound),
                Command::Give(material, count) => {
                    let (Some(id), Some(material)) =
                        (ctx.player, ctx.world.materials().id(&material))
                    else {
                        continue;
                    };
                    ctx.game.inventories.entry(id).or_default().add(material, count);
                }
                Command::SpawnDrop(x, y, material, count) => {
                    let Some(material) = ctx.world.materials().id(&material) else { continue };
                    ctx.game.drops.spawn(
                        material,
                        count,
                        i64::from(x) * 16 + 8,
                        i64::from(y) * 16 + 8,
                    );
                }
                Command::SetCell(x, y, material) => {
                    let Some(material) = ctx.world.materials().id(&material) else { continue };
                    let pos = CellPos::new(x, y);
                    if material == MaterialId::BOUNDARY
                        || ctx.world.get(pos).material == MaterialId::BOUNDARY
                    {
                        continue;
                    }
                    let cell = ctx.world.make_cell(material, pos);
                    ctx.world.set(pos, cell);
                }
            }
        }
    }

    fn snapshot(ctx: &Context<'_>) -> Snapshot {
        let me = ctx.player.and_then(|id| ctx.entities.get(id).map(|e| (id, e)));
        Snapshot {
            tick: i64::try_from(ctx.tick).unwrap_or(i64::MAX),
            player: me.map(|(_, e)| (e.x16.div_euclid(16), e.y16.div_euclid(16))),
            hp: me.and_then(|(id, _)| ctx.game.vitals.get(id)).map_or(0, |v| i64::from(v.hp)),
        }
    }

    fn run_with(
        &mut self,
        ctx: &mut Context<'_>,
        name: &str,
        events: &[GameEvent],
        on_event: bool,
    ) {
        {
            let mut s = locked(&self.shared);
            s.snapshot = Self::snapshot(ctx);
            s.commands.clear();
        }
        if on_event {
            for event in events {
                let (kind, a, b) = match event {
                    GameEvent::Hit(hit) => ("hit", i64::from(hit.cell.x), i64::from(hit.cell.y)),
                    GameEvent::Defeated(_, pos) => ("defeated", i64::from(pos.x), i64::from(pos.y)),
                    GameEvent::Pickup(pickup) => ("pickup", i64::from(pickup.count), 0),
                    GameEvent::Dug(dug) => ("dug", i64::from(dug.pos.x), i64::from(dug.pos.y)),
                    // 新しい種類の出来事は、スクリプトにはまだ渡さない。
                    _ => continue,
                };
                self.call(name, (kind.to_string(), a, b));
            }
        } else {
            self.call(name, ());
        }
        let commands = std::mem::take(&mut locked(&self.shared).commands);
        Self::apply(commands, ctx);
    }
}

impl Game for ScriptGame {
    fn on_start(&mut self, ctx: &mut Context<'_>) {
        if self.has_start {
            self.run_with(ctx, "on_start", &[], false);
        }
    }

    fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]) {
        if self.has_event && !events.is_empty() {
            self.run_with(ctx, "on_event", events, true);
        }
        if self.has_tick {
            self.run_with(ctx, "on_tick", &[], false);
        }
    }
}

#[cfg(test)]
mod tests;
