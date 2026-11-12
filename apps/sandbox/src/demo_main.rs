//! 参照デモ (仕様 09)。FallingSandEngine の動画に合わせた暗い洞窟の世界を開く。

fn main() -> Result<(), Box<dyn std::error::Error>> {
    charon_app::demo::run_demo(charon_app::demo::DemoConfig::discover())
}
