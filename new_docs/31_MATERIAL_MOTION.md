# E05：物質の速度・端数と整数格子の経路検査

## 状態と責務

`WorldEdit::transport_material_motion`は、作者が指定した速度を積分し、複数cellの途中を検査して物質を輸送する一つのphaseである。E05全体・Q04の完了を示さない。[輸送・圧力](30_MATERIAL_TRANSPORT.md)と同じWorld取引・所有権を使う。

このphaseは重力・抗力・浮力・摩擦を自動積分せず、物性から速度を決めるproviderの統合も完了していない。作者が速度channelへ与えた値だけを使う。密度交換は既存の局所輸送phase、一般形状・bodyの連続衝突は後続のgeometry/query統合で扱う。これらを実装済みと表示しない。

## 物質と保存する値

`MaterialMotion`は`GridValue`と`StateCodec`を実装する。schemaのcodec上限は24byte以上とし、`ContentRegistry::material_channel`で対象layerへ登録する。格子は1対1、原点は任意でよい。位置固定channelや型が異なるchannel、別layerのchannelは拒否する。

| field | 単位と範囲 |
| --- | --- |
| `velocity_q16: [i64; 2]` | 符号付きQ16 cell/tick。右・下が正 |
| `remainder_q16: [i32; 2]` | 各軸の未消化変位。-65535〜65535 |

端数はまだ整数cellの移動へ使っていない量であり、表示用の連続座標や一般形状のTransformではない。各軸で`total = velocity + remainder`、整数変位を`trunc(total / 65536)`、次の端数を`total % 65536`とする。中間値はi128を使い、負方向も0方向丸めで蓄積する。例えば±0.25 cell/tickは4回のphaseでそれぞれ±1cellとなる。

CellCoreと全物質channelは既存の一括輸送で追従する。位置固定の場は移動しない。速度・端数は通常のchannelとしてcheckpoint・区域保存へ含まれ、新しいWorld保存形式は追加しない。新規channel・schemaを登録するとContent manifestは変わるため、異なるContentの既存保存をそのまま互換とみなさない。

## 走査と経路

対象はSimulate leaseで活動している指定layerの非block・非空cell。既存の局所輸送と同じく下から上、同一行はWorld tickで左右を交互に走査する。明示速度を持つ静止固体cellも対象となるが、block個体は暗黙に分解しない。

整数変位のcell中心間線分を整数DDAで検査する。XとYの次の境界の時刻を整数の交差積で比較し、浮動小数点やOSの時刻へ依存しない。同時交差ではX側の直交cell、Y側の直交cell、斜め先を順に検査し、直交cellが一つでも塞がれば通過しない。非同時交差も途中のcellを省略しない。

この検査は整数格子のトークン経路であり、任意の半径・mask・polygonのshape sweepを代用するものではない。実際の表示形状から物質の通過可否を推測しない。

検査を終えた経路を逆順で`transport_path`へ渡す。始点トークンを末尾へ直接送り、通過した空cellの旧値は一つずつ始点側へ戻す。全物質channelの最終容量を経路全体で検査し、途中の交換ごとの不要な同時確保を要求しない。

始点・通過点・斜め通過の直交検査点をphase内で予約する。先行トークンの経路へ後続が交差する場合は延期し、物理的な壁とは区別する。動かなかったcellを通過予約として扱わず、走査順だけで静止障害物を予約競合へ置き換えない。

## 停止と未準備域

- 活動中の占有cell・block・World境界：最後に通過できたcellまで動き、速度と端数を全軸0とする。反射・滑り・密度交換は行わない。
- 未読込・非活動域：最後に通過できたcellまで動き、速度と今回積分した端数を保持する。非活動域の占有情報を使って物理応答を決めない。
- 先行経路との予約競合：最後に通過できたcellまで動き、速度と端数を保持する。

未実行の整数変位を次tickへの借金として足し込まない。未準備域を読み込んで活動させれば、保持していた速度で次のphaseから移動を再開できる。各種の停止数を`MaterialMotionReport`へ分けて返す。

## 容量と費用

`MaterialMotionConfig`の`candidate_limit`は非block・非空cellの列挙上限、`max_distance`は各軸の整数移動距離上限、`step_limit`はphase全体のDDA境界候補数上限である。標準の局所使用例は距離8を使う。大きな継続処理のJobやそのfrontier保存はこの一括phaseとは別の残作業である。

距離や仕事量が上限を超えた場合は速度をclampせず、容量エラーとする。書込数・channel chunk・World全体のbyte数は既存の編集上限も使う。失敗をcallbackが無視しても、先行する移動・端数更新を含めてWorld取引全体を破棄する。

列挙は活動chunkのcell配列を走査し、候補を並べ替える。経路と角の検査点に移動距離に比例したscratch、予約に活動chunk当たり4096bitを使う。予約済み経路の輸送は経路cell数と所属channel数に応じた費用を持つ。仕事量上限がWorld候補の複製や保存codecまで一定時間に制限するという意味ではない。

## 利用と検証

[実行できるheadless作例](../examples/material_motion.rs)は半cell/tickで20tick進め、壁の直前で停止した物質・速度・端数を保存・復元する。Runtimeの外で`advance_tick`を明示する。RuntimeのSystem内では時計の所有者をRuntimeへ任せ、このphase自体は時計を進めない。

```sh
cargo run --locked --example material_motion
cargo test --locked --test material_motion
```

テストは符号付き端数、全八分象限、壁と角、予約交差、物質payload、位置固定場、未準備域、World境界、誤ったchannel、極端な速度、容量エラーの全体rollback、保存再開を対象とする。結果と未完了項目は[evidence/E05](evidence/E05/README.md)へ記録する。
