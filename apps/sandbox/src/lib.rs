//! Android NativeActivity 用のサンドボックス入口。

#[cfg(any(target_os = "android", target_arch = "wasm32"))]
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

/// Android の復帰時も `charon-app` 共通のイベントループを使う。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    let Some(save_root) = app.internal_data_path().map(|path| path.join("saves/default")) else {
        eprintln!("Android の保存先を取得できません");
        return;
    };
    let event_loop = match winit::event_loop::EventLoop::builder().with_android_app(app).build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("Android イベントループの作成に失敗: {err}");
            return;
        }
    };
    let (world, streamer, metadata, physics) = match world::open(&save_root) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("Android ワールドを読み込めません: {err}");
            return;
        }
    };
    let selected = world::initial_material(&world);
    if let Err(err) = charon_app::run_with_streaming(
        event_loop,
        world,
        selected,
        world::make_world,
        streamer,
        save_root,
        metadata,
        physics,
    ) {
        eprintln!("Android サンドボックスの実行に失敗: {err}");
    }
}
