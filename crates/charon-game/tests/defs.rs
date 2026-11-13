#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions, DefsError};
use charon_game::mods::{ModDirectory, ModPack};
use charon_game::projectile::{Payload, ProjectileWorld};
use charon_game::wand::{CastOutcome, SpellError, SpellId};

fn materials() -> MaterialTable {
    MaterialTable::builtin().expect("組み込み材質を読み込む")
}

fn spell(name: &str, replace: bool, damage: i32, payload: &str) -> String {
    format!(
        "(name: \"{name}\", replace: {replace}, mana_cost: 5, cast_delay: 6, speed: 128, \
         gravity: 0, life: 60, damage: {damage}, spread: 0, payload: {payload})"
    )
}

fn spells_only(text: &str) -> DefinitionFiles<'_> {
    DefinitionFiles { spells: Some(text), ..Default::default() }
}

#[test]
fn engine_defaults_load() {
    let materials = materials();
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let names: Vec<&str> = defs.spells.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["spark", "bomb", "sand_shot", "water_shot"], "記載順");
    assert_eq!(defs.spells.id("bomb"), Some(SpellId(1)));
    assert_eq!(defs.wand_names().collect::<Vec<_>>(), ["basic", "bomber"]);
    assert_eq!(defs.enemy_names().collect::<Vec<_>>(), ["walker", "shooter"]);

    // 杖は名前から作れて、定義どおりの呪文・マナを持つ。
    let basic = defs.wand("basic").unwrap();
    assert_eq!(basic.spells, vec![SpellId(0)]);
    assert_eq!((basic.mana, basic.mana_max, basic.mana_regen, basic.recharge), (100, 100, 1, 0));
    assert_eq!(defs.wand("bomber").unwrap().recharge, 20);
    assert_eq!(defs.wand("nothing").unwrap_err(), DefsError::UnknownWand("nothing".to_owned()));

    // 敵は呪文を名前の通りに参照する。
    let shooter = defs.enemy("shooter").unwrap();
    assert_eq!((shooter.width, shooter.height, shooter.max_hp), (16, 24, 30));
    assert_eq!(shooter.attack.as_deref(), Some("spark"));
    assert_eq!(defs.enemy("walker").unwrap().attack, None);
    assert_eq!(defs.enemy("walker").unwrap().contact_damage, 5);
    assert_eq!(shooter.contact_damage, 0, "省略したら 0");
    assert!(defs.enemy("nothing").is_none());
}

#[test]
fn layer_adds_and_replaces() {
    let materials = materials();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    let before = defs.wand("basic").unwrap();
    let mut world = World::new(Arc::new(materials.clone()), 1);
    world.insert_chunk(ChunkPos::new(0, 0));

    let text = format!(
        "[{}, {}]",
        spell("zap", false, 3, "None"),
        // 既定の spark を、名前空間なしで上書きする。
        spell("spark", true, 77, "None"),
    );
    let wands = r#"[(name: "mine", spells: ["zap", "spark", "bomb"], mana_max: 50,
                     mana_regen: 2, recharge: 5)]"#;
    let enemies = r#"[(name: "grunt", width: 6, height: 12, max_hp: 10, walk_speed: 8,
                       jump_speed: 40, sight: 50, attack: Some("zap"), attack_interval: 30)]"#;
    defs.layer(
        DefinitionFiles {
            spells: Some(&text),
            wands: Some(wands),
            enemies: Some(enemies),
            ..Default::default()
        },
        &materials,
        Some("game"),
    )
    .unwrap();

    // 追加された呪文・杖・敵は名前空間が付き、末尾に並ぶ。
    assert_eq!(defs.spells.id("game:zap"), Some(SpellId(4)));
    assert!(defs.spells.id("zap").is_none());
    let mine = defs.wand_def("game:mine").unwrap();
    // 名前は、まず game:、次にそのままで解決される (zap は game:zap、spark と bomb はエンジンの)。
    assert_eq!(mine.spells, ["game:zap", "spark", "bomb"]);
    assert_eq!(defs.enemy("game:grunt").unwrap().attack.as_deref(), Some("game:zap"));

    // 上書きは中身だけを置き換え、位置 (SpellId) と呼び出し側の杖を保つ。
    assert_eq!(defs.spells.id("spark"), Some(SpellId(0)));
    assert_eq!(defs.spells.get(SpellId(0)).unwrap().damage, 77);
    assert_eq!(defs.spells.len(), 5);
    let mut projectiles = ProjectileWorld::new();
    let mut basic = defs.wand("basic").unwrap();
    assert_eq!(basic.spells, before.spells, "同じ位置の呪文を指す");
    let outcome = basic.cast(&defs.spells, &mut projectiles, &world, (0, 0), (1, 0), 0, 1).unwrap();
    let CastOutcome::Fired(id) = outcome else { panic!("撃てない") };
    assert_eq!(projectiles.get(id).unwrap().damage, 77, "上書き後の中身で撃つ");
    assert_eq!(projectiles.get(id).unwrap().payload, Payload::None);

    // 別の層でもう一度上書きでき、後の層が勝つ。
    defs.layer(
        spells_only(&format!("[{}]", spell("spark", true, 5, "None"))),
        &materials,
        Some("x"),
    )
    .unwrap();
    assert_eq!(defs.spells.get(SpellId(0)).unwrap().damage, 5);
}

#[test]
fn layer_rejects_bad_data() {
    let materials = materials();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    let hash = defs.content_hash();
    let mut reject = |files: DefinitionFiles<'_>, ns: Option<&str>| {
        let error = defs.layer(files, &materials, ns).unwrap_err();
        assert_eq!(defs.content_hash(), hash, "失敗した層は何も変えない: {error}");
        error
    };

    // 置き換えでない定義が既存の名前と重なる。
    let dup = format!("[{}]", spell("spark", false, 1, "None"));
    assert!(matches!(
        reject(spells_only(&dup), None),
        DefsError::Spell(SpellError::Duplicate(name)) if name == "spark"
    ));
    // 名前空間があれば、同じ名前でも別の名前になるので通る (別のテストで確認)。
    // 置き換える対象がない。
    let missing = format!("[{}]", spell("nothing", true, 1, "None"));
    assert!(matches!(
        reject(spells_only(&missing), None),
        DefsError::Spell(SpellError::ReplaceMissing(name)) if name == "nothing"
    ));
    // 同じ層の中で同じ名前を 2 回。
    let twice = format!("[{}, {}]", spell("a", false, 1, "None"), spell("a", false, 2, "None"));
    assert!(matches!(
        reject(spells_only(&twice), None),
        DefsError::Spell(SpellError::Duplicate(_))
    ));
    // 未定義の材質。
    let bad_material =
        format!("[{}]", spell("deposit", false, 0, "Deposit(material: \"no_such_material\")"));
    assert!(matches!(
        reject(spells_only(&bad_material), None),
        DefsError::Spell(SpellError::UnknownMaterial { .. })
    ));
    // 杖・敵が未定義の呪文を参照する。
    let wand = r#"[(name: "w", spells: ["ghost"], mana_max: 10, mana_regen: 1)]"#;
    assert!(matches!(
        reject(DefinitionFiles { wands: Some(wand), ..Default::default() }, None),
        DefsError::UnknownSpell { .. }
    ));
    let enemy = r#"[(name: "e", width: 8, height: 16, max_hp: 10, walk_speed: 1, jump_speed: 1,
                     sight: 1, attack: Some("ghost"), attack_interval: 5)]"#;
    assert!(matches!(
        reject(DefinitionFiles { enemies: Some(enemy), ..Default::default() }, None),
        DefsError::UnknownSpell { .. }
    ));
    // 杖・敵・呪文の置き換えの対象がない / 重複。
    let wand_dup = r#"[(name: "basic", spells: ["spark"], mana_max: 10, mana_regen: 1)]"#;
    assert!(matches!(
        reject(DefinitionFiles { wands: Some(wand_dup), ..Default::default() }, None),
        DefsError::Duplicate { kind: "杖", .. }
    ));
    let enemy_missing = r#"[(name: "ghost", replace: true, width: 8, height: 16, max_hp: 10,
                            walk_speed: 1, jump_speed: 1, sight: 1, attack: None)]"#;
    assert!(matches!(
        reject(DefinitionFiles { enemies: Some(enemy_missing), ..Default::default() }, None),
        DefsError::ReplaceMissing { kind: "敵", .. }
    ));
    // 範囲の外と構文エラー。
    for (field, bad) in
        [("mana_max", "0"), ("mana_max", "10001"), ("mana_regen", "65536"), ("recharge", "-1")]
    {
        let mut text = String::from(
            r#"[(name: "w", spells: ["spark"], mana_max: 10, mana_regen: 1, recharge: 0)]"#,
        );
        let from = text.find(&format!("{field}:")).unwrap();
        let end = text[from..].find([',', ')']).unwrap() + from;
        text.replace_range(from..end, &format!("{field}: {bad}"));
        assert!(
            matches!(
                reject(DefinitionFiles { wands: Some(&text), ..Default::default() }, None),
                DefsError::Invalid { .. }
            ),
            "{field}={bad}"
        );
    }
    let too_many = format!(
        "[(name: \"w\", spells: [{}], mana_max: 10, mana_regen: 1)]",
        vec!["\"spark\""; 17].join(",")
    );
    assert!(matches!(
        reject(DefinitionFiles { wands: Some(&too_many), ..Default::default() }, None),
        DefsError::Invalid { .. }
    ));
    for field in [
        "width: 0",
        "width: 33",
        "height: 0",
        "max_hp: 0",
        "walk_speed: 97",
        "sight: 1025",
        "contact_damage: 1001",
        "contact_damage: -1",
    ] {
        let (key, _) = field.split_once(':').unwrap();
        let base = "width: 8, height: 16, max_hp: 10, walk_speed: 1, jump_speed: 1, sight: 1, \
                    contact_damage: 0";
        let from = base.find(&format!("{key}:")).unwrap();
        let end = base[from..].find(',').map_or(base.len(), |i| i + from);
        let mut fields = base.to_owned();
        fields.replace_range(from..end, field);
        let text = format!("[(name: \"e\", {fields}, attack: None)]");
        assert!(
            matches!(
                reject(DefinitionFiles { enemies: Some(&text), ..Default::default() }, None),
                DefsError::Invalid { .. }
            ),
            "{field}"
        );
    }
    // attack があるのに間隔が 0。
    let no_interval = r#"[(name: "e", width: 8, height: 16, max_hp: 10, walk_speed: 1,
                          jump_speed: 1, sight: 1, attack: Some("spark"), attack_interval: 0)]"#;
    assert!(matches!(
        reject(DefinitionFiles { enemies: Some(no_interval), ..Default::default() }, None),
        DefsError::Invalid { .. }
    ));
    assert!(matches!(
        reject(DefinitionFiles { wands: Some("[(name: "), ..Default::default() }, None),
        DefsError::Parse { file: "wands.ron", .. }
    ));
    // 1 つの層のうち、あとのほうが壊れていると、前のほうも反映されない (原子的)。
    let half = format!("[{}]", spell("fine", false, 1, "None"));
    assert!(matches!(
        reject(
            DefinitionFiles { spells: Some(&half), wands: Some(wand), ..Default::default() },
            Some("g"),
        ),
        DefsError::UnknownSpell { .. }
    ));
    assert!(defs.spells.id("g:fine").is_none());
}

#[test]
fn content_hash_tracks_overrides() {
    let materials = materials();
    let base = Definitions::engine_defaults(&materials).unwrap();
    assert_eq!(
        base.content_hash(),
        Definitions::engine_defaults(&materials).unwrap().content_hash()
    );

    let apply = |text: &str, ns: Option<&str>| {
        let mut defs = base.clone();
        defs.layer(spells_only(text), &materials, ns).unwrap();
        defs.content_hash()
    };
    let one = format!("[{}]", spell("spark", true, 41, "None"));
    let two = format!("[{}]", spell("spark", true, 42, "None"));
    let (a, b) = (apply(&one, None), apply(&two, None));
    assert_ne!(a, base.content_hash(), "名前が同じでも中身が変われば変わる");
    assert_ne!(a, b);
    assert_eq!(a, apply(&one, None), "同じ層を同じ順で重ねれば一致する");
    // 層の順序が結果を変える: A→B と B→A は別の定義。
    let order = |first: &str, second: &str| {
        let mut defs = base.clone();
        defs.layer(spells_only(first), &materials, None).unwrap();
        defs.layer(spells_only(second), &materials, None).unwrap();
        defs.content_hash()
    };
    assert_ne!(order(&one, &two), order(&two, &one));
    // 追加した呪文の名前空間も内容に含まれる。
    let added = format!("[{}]", spell("extra", false, 1, "None"));
    assert_ne!(apply(&added, Some("a")), apply(&added, Some("b")));
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct TempMods(PathBuf);

impl TempMods {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "charon-defs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// 材質は空の MOD。`files` は (ファイル名, 中身)。
    fn add(&self, dir: &str, id: &str, files: &[(&str, &str)]) -> ModDirectory {
        let path = self.0.join(dir);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("manifest.ron"), format!("(id: \"{id}\", version: 1)")).unwrap();
        fs::write(path.join("materials.ron"), "[]").unwrap();
        fs::write(path.join("reactions.ron"), "[]").unwrap();
        for (name, text) in files {
            fs::write(path.join(name), text).unwrap();
        }
        ModDirectory { path, script: false }
    }
}

impl Drop for TempMods {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn mod_definitions_load_in_id_order() {
    let dir = TempMods::new();
    // MOD の ID は b_mod が a_mod より後。ディレクトリ名と列挙順は逆にする。
    let later = format!("[{}]", spell("spark", true, 22, "None"));
    let earlier = format!("[{}]", spell("spark", true, 11, "None"));
    let b = dir.add("a-first-dir", "b_mod", &[("spells.ron", &later)]);
    let a = dir.add("z-last-dir", "a_mod", &[("spells.ron", &earlier)]);
    let plain = dir.add("m-dir", "c_mod", &[]);
    let forward = ModPack::load(&[b.clone(), a.clone(), plain.clone()]).unwrap();
    let backward = ModPack::load(&[plain, a, b]).unwrap();
    let damage = |pack: &ModPack| pack.definitions.spells.get(SpellId(0)).unwrap().damage;
    // a_mod (11) のあとに b_mod (22) が重なるので、後の b_mod が勝つ。
    assert_eq!((damage(&forward), damage(&backward)), (22, 22));
    assert_eq!(forward.definitions.content_hash(), backward.definitions.content_hash());
    // ファイルがなくても読め、エンジンの既定が残る。
    let none = ModPack::load(&[]).unwrap();
    assert_eq!(none.definitions.spells.len(), 4);
    assert_eq!(none.definitions.spells.get(SpellId(0)).unwrap().damage, 8);
}

#[test]
fn mod_definition_errors_stop_loading() {
    let dir = TempMods::new();
    let broken = dir.add("broken", "broken", &[("wands.ron", "[(name: ")]);
    let error = ModPack::load(&[broken]).unwrap_err();
    assert!(
        matches!(error, charon_game::mods::ModError::Definitions(DefsError::Parse { .. })),
        "{error}"
    );
    // 名前空間が付くので、MOD 同士が同じ名前の呪文を足しても衝突しない。
    let text = format!("[{}]", spell("shared", false, 1, "None"));
    let one = dir.add("one", "one", &[("spells.ron", &text)]);
    let two = dir.add("two", "two", &[("spells.ron", &text)]);
    let pack = ModPack::load(&[one, two]).unwrap();
    assert!(pack.definitions.spells.id("one:shared").is_some());
    assert!(pack.definitions.spells.id("two:shared").is_some());
}

fn hook(name: &str, replace: bool, extra: &[(&str, &str)]) -> String {
    let mut fields = vec![
        ("max_length", "100"),
        ("min_length", "4"),
        ("speed", "320"),
        ("segments", "24"),
        ("gravity", "6"),
        ("iterations", "8"),
        ("reel_speed", "24"),
        ("pull_accel", "8"),
        ("pull_max", "48"),
        ("cooldown", "20"),
    ];
    for (key, value) in extra {
        let slot = fields.iter_mut().find(|(k, _)| k == key).unwrap();
        slot.1 = value;
    }
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    format!("(name: \"{name}\", replace: {replace}, {})", body.join(", "))
}

fn hooks_only(text: &str) -> DefinitionFiles<'_> {
    DefinitionFiles { hooks: Some(text), ..Default::default() }
}

#[test]
fn hook_definitions_layer_and_replace() {
    let materials = materials();
    let base = Definitions::engine_defaults(&materials).unwrap();
    // エンジンの既定。
    assert_eq!(base.hook_names().collect::<Vec<_>>(), ["grapple"]);
    let grapple = base.hook("grapple").unwrap();
    assert_eq!((grapple.max_length, grapple.segments, grapple.cooldown), (200, 24, 20));
    assert!(base.hook("nothing").is_none());

    // ゲームの追加 (名前空間付き) と上書き。
    let mut defs = base.clone();
    let text = format!(
        "[{}, {}]",
        hook("long", false, &[("max_length", "300")]),
        hook("grapple", true, &[("pull_max", "100")]),
    );
    defs.layer(hooks_only(&text), &materials, Some("game")).unwrap();
    assert_eq!(defs.hook_names().collect::<Vec<_>>(), ["grapple", "game:long"]);
    assert_eq!(defs.hook("game:long").unwrap().max_length, 300);
    assert_eq!(defs.hook("grapple").unwrap().pull_max, 100, "名前をそのまま使って上書き");
    assert_ne!(defs.content_hash(), base.content_hash(), "上書きで内容ハッシュが変わる");

    // 重複・置き換え先なし・範囲外・構文エラーは何も変えずに拒否する。
    let hash = defs.content_hash();
    let mut reject = |text: &str| {
        let error = defs.layer(hooks_only(text), &materials, None).unwrap_err();
        assert_eq!(defs.content_hash(), hash, "失敗した層は何も変えない: {error}");
        error
    };
    let one = |name: &str, replace: bool, extra: &[(&str, &str)]| {
        format!("[{}]", hook(name, replace, extra))
    };
    assert!(matches!(
        reject(&one("grapple", false, &[])),
        DefsError::Duplicate { kind: "道具", .. }
    ));
    assert!(matches!(
        reject(&one("ghost", true, &[])),
        DefsError::ReplaceMissing { kind: "道具", .. }
    ));
    for (key, bad) in [
        ("max_length", "3"),
        ("max_length", "401"),
        ("min_length", "1"),
        ("min_length", "65"),
        ("speed", "15"),
        ("speed", "513"),
        ("segments", "3"),
        ("segments", "65"),
        ("gravity", "65"),
        ("iterations", "0"),
        ("iterations", "33"),
        ("reel_speed", "97"),
        ("pull_accel", "65"),
        ("pull_max", "0"),
        ("pull_max", "193"),
        ("cooldown", "601"),
        ("cooldown", "-1"),
    ] {
        assert!(
            matches!(reject(&one("bad", false, &[(key, bad)])), DefsError::Invalid { .. }),
            "{key}={bad}"
        );
    }
    // min_length が max_length を超える。
    assert!(matches!(
        reject(&one("bad", false, &[("max_length", "10"), ("min_length", "20")])),
        DefsError::Invalid { .. }
    ));
    assert!(matches!(reject("[(name: "), DefsError::Parse { file: "hooks.ron", .. }));
}

#[test]
fn mod_hooks_load_after_engine_defaults() {
    let dir = TempMods::new();
    let later = format!("[{}]", hook("grapple", true, &[("pull_max", "77")]));
    let earlier = format!("[{}]", hook("rope", false, &[("max_length", "50")]));
    let a = dir.add("one", "a_mod", &[("hooks.ron", &earlier)]);
    let b = dir.add("two", "b_mod", &[("hooks.ron", &later)]);
    let pack = ModPack::load(&[b, a]).unwrap();
    assert_eq!(pack.definitions.hook_names().collect::<Vec<_>>(), ["grapple", "a_mod:rope"]);
    assert_eq!(pack.definitions.hook("grapple").unwrap().pull_max, 77);
    assert_eq!(pack.definitions.hook("a_mod:rope").unwrap().max_length, 50);
    // ファイルがなくても読め、エンジンの既定が残る。
    assert_eq!(ModPack::load(&[]).unwrap().definitions.hook("grapple").unwrap().pull_max, 48);
}
