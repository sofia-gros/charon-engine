//! Android NativeActivity 用の入口 (参照デモ) と、Web の入口 (サンドボックス)。

#[cfg(target_arch = "wasm32")]
mod world;

#[cfg(target_os = "android")]
use winit::platform::android::{EventLoopBuilderExtAndroid, activity::AndroidApp};

/// Web の入口もサンドボックスの材質・剛体を共通のアプリへ渡す。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web_main() -> Result<(), wasm_bindgen::JsValue> {
    use charon_core::testkit::material_of;
    use charon_world::generator::NoiseGenerator;
    use std::sync::Arc;

    let world = world::make_world(1);
    let brush = world::initial_material(&world);
    let generator = Arc::new(NoiseGenerator {
        dirt: material_of(&world, 'd'),
        stone: material_of(&world, '#'),
    });
    let physics = world::demo_body(&world)
        .map_err(|err| wasm_bindgen::JsValue::from_str(&err.to_string()))?;
    let loop_ = winit::event_loop::EventLoop::new()
        .map_err(|err| wasm_bindgen::JsValue::from_str(&err.to_string()))?;
    charon_app::run_web(loop_, world, brush, world::make_world, generator, physics)
        .map_err(|err| wasm_bindgen::JsValue::from_str(&err))
}

/// APK に入れるデモの MOD (`assets/mods/fse_demo`)。起動時に内部ストレージへ書き出して読む。
#[cfg(target_os = "android")]
const DEMO_MOD_FILES: [(&str, &str); 6] = [
    ("script.rhai", include_str!("../../../assets/mods/fse_demo/script.rhai")),
    ("manifest.ron", include_str!("../../../assets/mods/fse_demo/manifest.ron")),
    ("materials.ron", include_str!("../../../assets/mods/fse_demo/materials.ron")),
    ("reactions.ron", include_str!("../../../assets/mods/fse_demo/reactions.ron")),
    ("spells.ron", include_str!("../../../assets/mods/fse_demo/spells.ron")),
    ("wands.ron", include_str!("../../../assets/mods/fse_demo/wands.ron")),
];

/// デモの MOD を `dir` に書き出す (毎回上書きするので、アプリを更新すれば内容も更新される)。
#[cfg(target_os = "android")]
fn extract_demo_mod(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in DEMO_MOD_FILES {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(())
}

/// Android の入口。参照デモ (`charon-demo`) を、タッチ・ゲームパッド操作で動かす (仕様 10 §10)。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    let Some(data) = app.internal_data_path() else {
        eprintln!("Android の保存先を取得できません");
        return;
    };
    let mod_dir = data.join("mods/fse_demo");
    if let Err(err) = extract_demo_mod(&mod_dir) {
        eprintln!("デモのデータを書き出せません: {err}");
        return;
    }
    let event_loop = match winit::event_loop::EventLoop::builder().with_android_app(app).build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("Android イベントループの作成に失敗: {err}");
            return;
        }
    };
    let config = charon_app::demo::DemoConfig {
        mod_dir,
        save_base: data.join("saves/demo"),
        settings: data.join("settings.ron"),
    };
    if let Err(err) = charon_app::demo::run_demo_with(event_loop, config) {
        eprintln!("Android デモの実行に失敗: {err}");
    }
}
