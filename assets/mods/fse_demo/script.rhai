// デモ MOD のスクリプト (仕様 16)。エンジンが毎 tick 呼ぶ。
// 使える関数: shake(強さ) play("jump"|"shoot"|"anchor"|"pickup") give("材質", 個数)
//             set_cell(x, y, "材質") tick() has_player() player_x() player_y() hp()

// 敵を倒したら、拾う音を鳴らす。掘った石は、10 個ごとに小さく画面を揺らす。
fn on_event(kind, a, b) {
    if kind == "defeated" {
        play("pickup");
        shake(0.2);
    }
    if kind == "dug" && (a + b) % 10 == 0 {
        shake(0.05);
    }
}
