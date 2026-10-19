# E05：Q16物性と速度輸送への標準・独自provider統合

## 保存契約と不変条件

`MaterialBuilder::motion_properties` は `MaterialMotionProperties` の重力倍率、抗力、浮力、摩擦を素材Contentへ登録する。各係数は無次元Q16。抗力・摩擦は `0..=65536`、重力倍率・浮力倍率は非負の`u32`とし、最終Q16速度へ変換できない計算を成功として扱わない。物性を明示した素材はContentのmanifestへ拡張領域 `MOTNPX01` を付記し、値だけ変更した別Contentとの保存互換性を偽らない。物性を指定しない旧Contentのmanifestは変更しない。

`WorldEdit::integrate_material_forces` は、activeな非空・非blockのcellを既存の確定走査順に列挙し、対象layerの `MaterialMotion` 追従channelへ速度を記録する。重力方向側の隣接cellが未読込なら「空気」と推測せず、その候補の力更新を延期する。容量やProviderの失敗では外側のWorld候補を全てrollbackする。速度の固定小数点Q16と残差は正規channel内に保持し、時間依存の浮動小数点や壁時計へ依存しない。

## 標準式

- 抗力：各軸の速度を`velocity × (65536 - drag_q16)/65536`で減衰させる。0方向への整数除算で負側も処理する。
- 重力：作者が入力した各軸Q16加速度へ素材の重力倍率を適用する。
- 浮力：重力方向の隣接セルが自身より重い液体・気体なら、質量差÷隣接質量を使い、その方向の重力と逆向きへ加速度を加える。
- 摩擦：重力方向に固体・blockが接触するとき、重力に直交する軸の速度を素材の摩擦係数で減衰させる。

この基準式はゲーム用のcell接触物性であり、SI物理や任意形状CCDを保証しない。標準以外の接触、複合流体や連成を作者が実装する場合は `MaterialForceProvider` を渡して標準式を**完全置換**する。providerは不変のWorldから保存済み作者状態を参照できるが、callbackに非保存の時間状態を持たせない。利用するProviderのkey/versionと重力などのProfileは作者の実行設定・保存契約へ含める。標準providerの定義を独自providerへ強制しない。

`WorldEdit::step_material_force_motion` は力積分と既存 `transport_material_motion` のDDA・予約・全物質channel追従を単一World候補で行う。失敗時は力だけ先に適用済みになることがなく、1tickに二重積分を発生させない。正規粒子の力積分とcell/particle共通の標準phase予約は実装済み。Runtimeの自動schedule、保存可能な粒子Job、複数Provider・別表現を含むdirty/sleep連携は**別途E05残作業**であり、この実装単位でQ04を合格にしない。

### 作者Providerのsleep参加

`MaterialForceDependencyStencil`は候補cellの周囲にある空間依存を、各軸最大64 cellの矩形半径で宣言する。`integrate`が直接読むWorldのcell/channelだけでなく、`MaterialForceInput::neighbor`など結果へ影響する全ての空間入力を含める。Providerが読むWorld作者状態、tick、その他の非空間入力は`MaterialForceProvider::sleep_invariant`で毎sleep tick検査する。契約を宣言しない既定Providerは通常の参照経路を使う。

`World::step_material_force_motion_sleeping_with_provider`は一度参照実行して全active候補が静止していることを検査し、provider key/version、profile、Content/epoch、World枝、active chunk revision、依存chunk集合を派生証明へ記録する。sleep中は不変条件と直近のchunk変更を確認し、依存範囲内の変更、active集合変更、枝・設定の不一致で通常経路へ戻る。trackerのcodec復元後は証明を信用せず、必ず参照実行で再構築する。証明作成の走査・scratchは候補数とactive chunk数に比例し、sleep時はProviderの不変条件callbackと変更chunk数を確認する。4,096静止候補の合成比較では参照実行とsleepの時間・割当を測定したが、動的材料、複数active chunk、Materials Runtime統合のprofile性能はQ04/E05の出口検証に残る。

## 正規粒子の力積分

`WorldEdit::integrate_material_particle_forces` はactiveなresident正規粒子だけを世代付きhandleで列挙し、同じ `MaterialForceProvider` を一候補につき一度だけ実行する。退避中またはinactiveな粒子は速度を書き換えない。粒子の位置はQ16連続座標なので、セル速度channelの`remainder_q16`は粒子の実状態ではない。providerが非0の端数を返した場合は隠れた変位を失わないよう全候補をエラーにする。作者は`MaterialForceInput::particle`から対象handleと完全な粒子payloadを読み取れる。

`WorldEdit::step_material_particle_forces` は、全粒子の力積分後に既存 `step_material_particles` の整数DDA、衝突予約・着地を同じWorld編集候補で確定する。速度・位置・物質所有は既存のcheckpoint/restoreに従い、力またはDDAの容量不足では全体を巻き戻す。セル版と同じくproviderは未変更Worldを共通の参照状態として観測し、先行粒子の新速度に依存しない。

このAPIも毎tick自動で実行されるRuntime標準Systemや広域Jobを代替しない。単一tickの一括候補上限を残していること、作者のschedule profile・provider key/versionを保存・一致検査することはE05全体の未達として記録する。

## 検証

- `tests/material_forces.rs` は標準重力/抗力/摩擦/浮力、独自providerによる置換、容量rollback、DDAと保存再開の一致、Contentの物性値域とfingerprint差異を検査する。
- 同ファイルの粒子回帰では途中checkpointを挟む連続Q16位置と正規粒子の力積分の一致、端数不正・速度容量による全体rollbackを確認する。
- `examples/material_forces.rs` は重力0.5 cell/tick²の2tickを途中でWorld保存し、連続実行と再開のcheckpoint一致と移動先cellを確認する。
- `cargo test --locked --test material_forces` と `cargo run --locked --example material_forces` を対象検証する。全体・全OS CIはE05 evidenceへ記録する。
