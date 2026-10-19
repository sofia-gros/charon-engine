# 基準アルゴリズムと取引の補足

## この文書の役割

各システムの実装で判断が分かれやすい、順序、数式、予約、変換を固定する。最適化後も同じ正規結果を要求する。係数や予算は設定またはcontentのversion付き入力とし、測定時の値をfixtureへ残す。

本書は標準providerの参照アルゴリズムである。作者providerへ同じ数式・順序・丸めを強制しない。[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)に従って別profileを登録し、そのprofile内の参照経路、保存、再現性を検証する。共通の所有権・予約・原子的commitの契約は置換しても維持する。

## 取引の手順

```text
read_setを取得し、対象revisionを記録
write_setと全出力数、イベント数、allocator需要を列挙
範囲、権限、schema、参照、数値、容量を検証
必要chunk、slot、buffer、出力を予約
read_setのrevisionと競合を再確認
全変更と収支をcommitし、予約を確定
receiptと確定イベントを公開
```

prepare中は入力を変更しない。予約に失敗したらすべて解放する。commit中は追加確保や作者callbackなどの失敗する操作を呼ばず、検証済みの代入と移動で終える。commitに必要なcodecや変換ruleはprepareで評価する。

複数Worldをまたぐ移動はWorldId順に予約して一つの所有者がcommitする。network clientが別Worldへ直接書き込む権限を取得する方式にはしない。

## 熱の整数計算

phase familyは、各相の温度区間、整数熱容量C、潜熱L、連続なenergy境界を持つ。TはmK、Hはengine energy。単相では`H = H0 + C × (T - T0)`、転移区間では温度を固定してphase fractionを`(H - Hstart) / L`から求める。逆変換は区分順と0方向丸めを固定する。

辺(a,b)の平衡移動量`Qeq`は、enthalpy family上で`T(a-q) >= T(b+q)`を保つ最大整数qとして求める。同じ厳密単調な顕熱区間では直接解き、区間横断・潜熱・温度段差は整数二分探索する。伝導率とdtから求めた`Qraw`を符号付きで辺固有の1/8単位creditへ累積し、creditが8に達した整数energyだけを移す。実際の`abs(Q)`は累積Qeq/8およびQrawの両方を超えず、Qはaから減らしbへ同量を加える。phase境界では潜熱区間の温度関数を直接評価し、energyを丸め直して発行しない。

伝導flux residualと平衡上限creditは各正方向辺(+x,+y)が保存し、物質移動後も格子位置に属する。異なるsidecarの再構築時にも所有する辺を安定座標から決める。計算中間値はi128、conductance residualは保存i64、1/8 creditは0..7として検証する。

## 圧力場の基準

各fluid cellにQ16のpressureを持つ。activeな空き先は大気圧を相対0、固体とUnavailableはno-flow境界とする。液体ではgravity項を持つ近傍緩和、気体では連結量・温度から求めたtarget pressureへの緩和を加える。

一反復を次の式とする。Pは前反復の配列を読み、新配列へ書く。

```text
Pnext(i) = max(0, trunc((Σj(P(j) + rho_avg(i,j) × g × (yi - yj))
                         + lambda(i) × Ptarget(i)) / (neighbor_count + lambda(i))))
```

free surfaceにはP=0を与える。閉じた液体領域は前tickの平均圧力を一つの安定anchorで保持し、ポンプ等のsourceがあればその値を適用する。気体のPtargetは連結領域の換算mass/空間量と温度の比から計算する。係数、上限、lambdaはmaterial schemaに含める。

既定4反復を各tickで実行し、未収束のPも正規状態として継続する。source探索が件数予算を超える場合はfrontierと部分集計を保存し、完成前の結果を公開しない。移動のdriveは`Psrc - Pdst + rho × g × (ydst - ysrc)`で、閾値と移動予算を満たした候補だけを作る。通常の重力落下・局所の横均しは別候補として併用する。

pressureは流れを選ぶためのproxyで、tokenの量そのものではない。圧力更新でcell、粒子、energyを発行しない。

## 物質移動と競合

候補にはsource、全通過cell、destination、必要sidecar、期待revisionを含める。destinationが空なら交換、異物質ならmassと浮力のruleを評価する。安定した走査順で最初の予約を採用し、競合した候補はそのphaseでは再適用しない。

押し出し経路は空き先から逆向きに交換し、全経路の状態とstampを同時に移す。経路の途中だけを確定しない。body reservoirへの移行は、cellからreservoir recordへの所有権移動として同じ収支を使う。

## 爆発と破壊

爆発は中心、半径、pressure/heat/damageの減衰curve、遮蔽、対象mask、物質出力ruleを持つ。候補cellを距離と安定座標順に評価し、破壊抵抗と遮蔽を適用する。circleに含まれる全cellを無条件消去するだけの実装にしない。

通常上限内の爆発は一取引で予約する。分割型はEffectJobとして残りregion、予約、実行済み範囲を保存する。演出を一度再生したことを理由に未処理の物質を消費しない。shape変更、navigation、light、block個体、jointの依存を同じ変更recordから更新する。

## 形状と高速衝突

point rayはinteger DDAで通過cellを列挙し、同時に辺を越える場合は両側候補を固定順で検査する。半径がある物体はpoint rayの代わりにshape sweepを使う。broad phaseの候補から、mask/convex shapeとの最初の接触時刻を求める。

controllerの残移動は`remaining - normal × min(0, dot(remaining, normal))`で接線へ投影する。接触のtieはaxis、cell/BodyIdの順で決める。stepは上昇・水平・下降の各経路をsweepし、頭上と着地点が空いているときだけ採用する。

## 状態の変換表

| 変換 | Cell/particle/bodyのtoken | energy/timer | 作者instance state |
| --- | --- | --- | --- |
| 通常交換・移動 | そのまま移す | そのまま移す | 所有先座標・対象を同時に移す |
| terrainからbody | local gridへ移す | localへ移す | body/local instanceへremap |
| body分断 | 成分へ配分 | 対応cellへ保持 | 残存anchorと成分のruleに従い移す |
| body/terrainから粒子 | particle tokenへ移す | payloadへ移す | 登録された粒子用codecへ移す |
| 反応・phase | 定義された入出力を計上 | energy収支とtimer消費を同時確定 | retain/migrate/drop/consumeをruleで明示 |
| 採掘・drop・配置 | 変換ruleの発行/消費 | item化のruleで明示 | item/block間の移行ruleを適用 |

必要な移行ruleがない場合は、その個体状態を失う変換を拒否する。拡張を登録しただけで、将来のすべての変換に対応済みと扱わない。

## 境界と検証

熱、圧力、反応、shape、navigation、生成のすべてでLoaded/Unavailable/WorldBoundaryを区別する。境界を通常の空cellとして扱った最適化は不合格にする。

各アルゴリズムは最小scene、負座標、全境界、満容量、数値上限、save再開、独自stateの変換fixtureを持つ。[試験群](18_VALIDATION_AND_RELEASE.md)へ具体的なテスト名と実行結果を登録してから工程を通過させる。
