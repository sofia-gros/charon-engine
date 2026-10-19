# E05：物質の輸送と圧力の公開API

## 状態

E05は実装中。このページは追加した輸送・局所移動・圧力緩和・押出し探索の契約を説明する。E05全体の完了やQ04の通過を意味しない。検証と残作業は[evidence/E05](evidence/E05/README.md)へ記録する。

## 位置と物質の所有権

`ContentRegistry::channel`は位置へ属する場を登録する。圧力や配線の値は、物質が動いてもその位置に残る。

`ContentRegistry::material_channel(layer_key, definition)`は、指定layerの物質に所属する値を登録する。energy、timer、混合状態への型付きIDなどを格納できる。同じ座標の別layerの輸送には追従しない。対象layerは先に登録する。Contentの確定後に所有権を変更しない。

この追従経路は一つのWorld cellに一つのchannel要素が対応する格子を扱う。原点は任意で、World座標から原点を引いてchannel座標へ変換する。変換のi32範囲を超える操作は失敗する。別解像度の分配・合流は通常channelと作者の明示的な変換取引を使う。別解像度を登録できる既存APIを変更・制限するものではない。

`World::channel_layout::<T>`と`channel_material_layer::<T>`で定義を確認できる。値型が違えばエラーになる。

## 原子的な輸送

- `WorldEdit::exchange_cells(layer, first, second)`は隣接二点を交換する。
- `WorldEdit::transport_path(layer, path)`は経路の各旧値を一つ先へ送り、末尾の旧値を先頭へ戻す。押出しには末尾の空cellを含める。

CellCoreのflags・variationと、そのlayerの全物質channelを同じ取引で輸送する。呼出側が移すchannelを列挙する必要はなく、選択漏れを防ぐ。経路の全点について読込、境界、重複、隣接、block個体、通常編集予算を検査する。編集予約数は`経路cell数 × (所属する物質channel数 + 1)`で、地形だけを数えてchannelの費用を隠さない。block個体は暗黙に分解しない。斜め隣接を許すが、形状による通過可否はsolverが判定する。

channelの旧値を退避してから対象点を既定値へ戻し、再配置する。これにより、一つしかchannel chunkを確保できない場合も、空になったchunkを解放して移動先を確保できる。全channelについて輸送後に残る非既定値数と必要chunk数・byte数を配列確保前に計算し、World全体とchannel個別の上限を検査する。容量を解放するchannelから処理し、一時的な確保量もWorld上限を超えない。他の値が残っていて容量が足りなければ失敗する。失敗をcallbackが無視しても、外側のWorld取引は地形・channel・先行編集をまとめて破棄する。

保存形式の構造は変更していない。所有権はContent manifestに含まれ、位置固定と物質追従を互換と誤認しない。通常channelだけの既存manifestは維持する。channel値は既存のcheckpoint・区域退避・復帰に従う。

## 局所移動の参照phase

`WorldEdit::transport_materials(layer, candidate_limit)`はSimulate leaseで活動しているlayerの非block cellを走査する。Runtimeへ隠れた重力を自動追加する処理ではない。作者Systemがphaseを明示して呼び、World時計の更新はRuntimeへ任せる。

| 分類 | 候補の順 |
| --- | --- |
| Solid | 移動しない |
| Powder | 下、左右の斜め下 |
| Liquid | 下、左右 |
| Gas | 上、左右の斜め上、左右 |

下から上へ、行内はWorld tickで左右を交互に走査する。異物質の交換は、下降する重い物質または上昇する軽い物質と、液体・気体の移動先との間で行う。固体と粉体を密度交換で崩さない。両側の直交点が共に塞がる角を斜めに通過しない。

両端の予約で、押し退けられた物質も同じphase中に再移動しない。予約scratchは活動chunk当たり4096bit。活動外・未読込・World境界を空きとして使わない。候補数や編集容量が足りない場合は部分更新を成功扱いにしない。このphaseは1cellの局所移動であり、速度残差・複数cell sweepの実装済み表示には使わない。

## 圧力緩和

`WorldEdit::relax_pressure(layer, PressureRelaxation)`は、位置固定の1対1格子のi64 channelへ非負Q16の圧力を保存する。反復数・重力係数・流体cell予算を呼出側が渡す。標準4反復を固定の製品制約にしない。

前反復の配列から次配列へJacobi更新し、自由表面を0、固体・block・活動外・未読込をno-flowにする。閉じた連結領域は最小座標のanchorに前回の平均を保持する。計算中間値はi128で、i64へ戻せなければ失敗する。全反復が完成してからchannelを一括公開する。全出力の非既定値数から最終的な必要chunk数・World全体のbyte数を事前検査し、channelの既定値へ戻る出力を先に反映して空chunkを解放する。既定値が0以外でも、座標の走査順で不要な同時確保を要求しない。圧力更新で物質やenergyを発行しない。

気体の温度・体積によるsource、ポンプsourceの標準化、広域緩和の保存可能な継続処理は残作業。このAPIがそれらも実行済みという意味ではない。

## 満杯経路の押出し

`PressurePushSearch::new`で流体始点と圧力条件、node容量を指定する。`step(world, budget)`で最大budget nodeの四近傍を上・左・右・下の固定順に調べる。同じ素材の活動cellを通過し、空き先との圧力差と重力項が閾値を超えた経路をReadyとする。活動中の空cellは相対圧0として扱い、位置固定channelに残った旧圧力値を使わない。保存から復元したReady経路の再検査・公開にも同じ境界条件を適用する。

Searching、Ready、NoOutletを区別する。容量不足はエラーであり、NoOutletへ丸めない。step失敗時はcursorとfrontierも変更しない。`set_node_capacity`で上限を増やして同じfrontierから再開できるが、StateStoreのrecord byte上限は独立して残る。World ID、Content、material epoch、入力した全活動chunkのrevisionと活動集合を照合する。失効時は古い経路を使わず再探索する。復元recordは親node・隣接・重複・所属領域だけでなく、再開・公開時に実際の物質・空き先・driveも再確認する。

訪問済みbitsetは探索ごとに活動chunk当たり4096bitを保持し、各stepでfrontier全体を複製・再構築しない。追記したnodeとその訪問bitだけを失敗時に巻き戻す。復元時にnode列から索引を一度再構築し、実地形とのnode照合はstepのbudgetを一件ずつ消費する。復元済みnodeの照合中はSearchingを返し、探索cursorを進めず、全照合後に探索を再開する。保存前にReadyまたはNoOutletだった場合も照合が終わるまでSearchingとなる。公開前の全node照合は維持する。

一stepの仕事は最大budget件のnode照合・展開と、活動chunk集合・revisionの検査である。後者は読込済みchunkの走査と、記録した活動chunkのrevision照合に依存する。node Vecの拡張は償却費用で、常に一定時間を保証するものではない。全recordの保存・復元・明示的な候補複製と公開取引は別途node数に比例する費用を持ち、毎stepで全状態を保存する呼出側の費用までbudgetへ含むとは説明しない。保存形式は`CHPUSH01`を維持し、bitsetと照合cursorは保存nodeから再構築する索引として扱う。

探索はStateCodecを実装し、作者がschemaを登録してWorld StateStoreへ保存できる。これは物質payloadをglobal stateへ移す処理ではなく、検索frontierと入力revisionの保存である。

```rust
// search_schemaは起動時にPressurePushSearchのschemaとして登録する。
// 探索中は物質を消費せず、Worldへ進捗だけを保存する。
search.step(&world, 64)?;
world.edit(|edit| edit.set_state(search_key, search_schema, &search))?;
// Readyのときだけ、完成した全経路を一括輸送する。
world.edit(|edit| edit.publish_pressure_push(&search))?;
```

公開でchunk revisionが変わるため同じ探索の二重公開は拒否する。入力集合を保守的に追跡するので、同じ活動集合の別地点の編集でも失効し得る。再探索費用を明示し、保存可能な探索だから並行編集と常に両立すると説明しない。

## E05の残作業

速度・残差とsweep、混合の登録・分離と取引、物質粒子の正規所有権とpool、source/sink/変換receiptを含む収支、気体source、広域圧力更新、dirty/sleepの参照比較、providerの統合・作例とQ04全体を完了してから工程を通過させる。
