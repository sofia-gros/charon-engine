#![allow(clippy::unwrap_used)] // 不正なテスト用ファイル操作は直ちに失敗させる。

use std::path::PathBuf;

use charon_core::cell::MaterialId;
use charon_core::material::Phase;
use charon_game::mods::{ModDirectory, ModPack};

fn demo_mod() -> ModDirectory {
    ModDirectory {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo"),
        script: false,
    }
}

#[test]
fn demo_mod_loads() {
    let pack = ModPack::load(&[demo_mod()]).unwrap();
    assert_eq!(pack.manifests.len(), 1);
    assert_eq!(pack.manifests[0].id, "fse_demo");
    let materials = &pack.materials;
    for family in ["rainbow_sand", "rainbow_rock"] {
        let first = materials.id(&format!("fse_demo:{family}_0")).unwrap();
        for i in 0..12u16 {
            let id = materials.id(&format!("fse_demo:{family}_{i}")).unwrap();
            assert_eq!(id, MaterialId(first.0 + i), "{family} の ID は連続する");
        }
        assert!(materials.id(&format!("fse_demo:{family}_12")).is_none());
    }
    let sand = materials.get(materials.id("fse_demo:rainbow_sand_3").unwrap());
    assert_eq!(sand.phase, Phase::Powder);
    assert_eq!(sand.color, [150, 230, 60, 255]);
    let rock = materials.get(materials.id("fse_demo:rainbow_rock_3").unwrap());
    assert_eq!(rock.phase, Phase::Solid);
    for name in [
        "glow_moss_yellow",
        "glow_moss_green",
        "glow_moss_red",
        "glow_orange",
        "sign_blue",
        "green_ore",
        "orange_ore",
        "bg_text_green",
    ] {
        let id = materials.id(&format!("fse_demo:{name}")).unwrap();
        assert_eq!(materials.get(id).phase, Phase::Solid, "{name}");
    }
    for name in ["glow_moss_yellow", "glow_moss_green", "glow_moss_red", "glow_orange"] {
        let id = materials.id(&format!("fse_demo:{name}")).unwrap();
        assert!(materials.get(id).emission.iter().any(|&c| c > 0), "{name} は発光する");
    }
    // 組み込み 21 材質の後ろに 32 個。何度読んでも同じ並び。
    assert_eq!(materials.len(), 21 + 32);
    let again = ModPack::load(&[demo_mod()]).unwrap();
    let names = |pack: &ModPack| {
        (0..pack.materials.len())
            .map(|id| pack.materials.get(MaterialId(id as u16)).name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&pack), names(&again));
}
