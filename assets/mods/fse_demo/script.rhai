// デモ MOD のスクリプト (仕様 16)。エンジンが毎 tick 呼ぶ。
// 使える関数: shake(強さ) play("jump"|"shoot"|"anchor"|"pickup") give("材質", 個数)
//             set_cell(x, y, "材質") spawn_drop(x, y, "材質", 個数)
//             tick() has_player() player_x() player_y() hp()

// 敵を倒したら、その場に石を 3 つ落とす (近づくと拾える)。拾ったら音を鳴らす。
// 掘った石は、10 個ごとに小さく画面を揺らす。
fn on_event(kind, a, b) {
    if kind == "defeated" {
        spawn_drop(a, b, "stone", 3);
        shake(0.2);
    }
    if kind == "pickup" {
        play("pickup");
    }
    if kind == "dug" && (a + b) % 10 == 0 {
        shake(0.05);
    }
}
