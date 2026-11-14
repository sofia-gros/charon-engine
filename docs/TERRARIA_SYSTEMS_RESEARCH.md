# Terraria 型のゲームシステム調査と実装範囲 (M28 調査)

2026-11-14。依頼: 操作・GUI に加え、武器、防具、道具、エフェクト、戦闘、地形その他の**ゲーム体験全体**を Terraria 型にする。戦闘・地形には Noita 型のセル物理を組み合わせる。これは**新たな実装の完了報告ではない**。公開 MOD のコードを読み、既存仕様との差と実装順を記録する。

## 1. 参照の位置づけと権利

- Terraria の挙動・遊びのループは比較対象。tModLoader の `ExampleMod` は **MOD の作り方を示す例**であり、Terraria 本体の正確な内部実装やモバイル UI の仕様を証明しない。参照したブランチは `stable` (2026-11-14 に閲覧)。数値や更新順をそのまま移植しない。
- [tModLoader の MIT ライセンス](https://github.com/tModLoader/tModLoader/blob/stable/LICENSE) と [Magic Storage の MIT ライセンス](https://github.com/blushiemagic/MagicStorage/blob/1.4.4/LICENSE) を確認した。**参照と、著作権表示・ライセンスの維持が必要なコードの複製は区別する**。本調査ではコード・素材をリポジトリに取り込んでいない。
- [Calamity Mod 公開ソースの LICENSE.md](https://github.com/CalamityTeam/CalamityModPublic/blob/1.4.4/LICENSE.md) は参照利用を許す一方、権利を留保し、コードを抜き出す場合はクレジットを求め、MOD の再配布を禁止する。**Calamity のコード・絵・音・名前・キャラクターは取り込まない**。
- 操作や、装備のセット効果・状態異常などの一般的な仕組みを、Rust と現行 API で独立に設計・実装する。Terraria/Noita 固有のアイテム名、敵、地形の絵、台詞、音、ロゴ、世界地図、データの丸写しや特徴的な組み合わせの複製は避ける。「完全」は各ゲームの全コンテンツ・全数値の 1:1 複製を意味しない。製品化前に類似性と素材の権利を別途確認する。

## 2. 実際に読んだ MOD コードから分かる設計要素

| 領域 | 読んだソースと観察 | Charon の現在との差 (詳細な数値は**未確定**) |
|---|---|---|
| 武器 | [`ExampleSword.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Weapons/ExampleSword.cs): 使用間隔とアニメーションを別に持ち、自動連続使用、ダメージ種別・会心・ノックバック・命中時のデバフ・レシピを組み合わせる。[`ExampleMagicWeapon.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Weapons/ExampleMagicWeapon.cs): 杖で弾・マナ消費・状況による消費量を定義 | `ItemKind::Weapon` は 3 クラスと基本ダメージが中心。自動使用、攻撃モーション、弾薬、会心、状況効果を分けた定義が必要。Noita 型の杖/呪文は独自の別レイヤーとして保つ |
| 防具・装飾品 | [`ExampleHelmet.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Armor/ExampleHelmet.cs) と [`ExampleHood.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Armor/ExampleHood.cs): 3 部位セット判定とボーナス、攻撃強化・マナ消費軽減・視覚効果。`Accessories` の[例一覧](https://github.com/tModLoader/tModLoader/tree/stable/ExampleMod/Content/Items/Accessories)には移動・ジャンプ・耐性などもある | 5 装備枠はあるが、セット効果と装飾品の個別挙動、見た目のレイヤーがない。効果を着脱時だけ加算せず、装備から毎 tick 合成して二重適用を防ぐ |
| 道具 | [`ExamplePickaxe.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Tools/ExamplePickaxe.cs): 掘る力と使用間隔・攻撃アニメーションを区別。`Tools` の[例一覧](https://github.com/tModLoader/tModLoader/tree/stable/ExampleMod/Content/Items/Tools)に斧相当、ドリル、釣り竿、フックなど | `Tool { pick }` はつるはしのみ。土台のレーザー掘削は残し、斧/ハンマー/機能道具・採掘速度・対象ごとの適性を**新仕様として**定義する。セル粉の掘削と 8×8 ブロック破壊を混同しない |
| 戦闘の更新順 | [`HitModifiersShowcase.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Items/Weapons/HitModifiersShowcase.cs): 命中前の補正と命中後の効果を分離。例自身がマルチ非対応と明記するモードもある | 現在は近接の前方箱・単一の防御式・固定ノックバック。独自の整数演算順を仕様に固定し、命中前→軽減→体力→命中後/地形反応の一回性を検証。例の浮動小数点や乱数をコピーしない |
| 弾・視覚 | [`ExampleHomingProjectile.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Projectiles/ExampleHomingProjectile.cs): 遅延後に標的を取得し、射線・生存を再確認。必ずしも Noita 型ではない。[`ExampleGravityDebuffBullet.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Projectiles/ExampleGravityDebuffBullet.cs): 貫通、古い位置を使う軌跡、命中時デバフ。 [`ExampleDefenseDebuff.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Buffs/ExampleDefenseDebuff.cs): 継続時間のある能力変化 | 弾は直撃/爆発/着火があるが、貫通・追尾・反射・弾幕、攻撃に同期する粒/光/音/ヒット表示は未整理。**ゲーム結果を変える効果**と**描画専用の演出**を区別する |
| 敵 | [`ExampleCustomAISlimeNPC.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/NPCs/ExampleCustomAISlimeNPC.cs): 睡眠→発見→跳躍→滞空→落下の明示的な状態遷移、動きとアニメーションの分離、出現条件 | 現在の追跡/射撃と spawner にボス専用フェーズ、地形・流体への反応、予兆、ドロップの進行条件を追加する余地。元の敵・行動列をそのまま再現しない |
| 地形・世界 | [`ExampleSand.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/Tiles/ExampleSand.cs) は Terraria の砂タイルを一時的に弾へ変えて落とす例。Charon は**セルを swap して質量保存**するため、その方式に置換しない。[`ChestItemWorldGen.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Common/Systems/ChestItemWorldGen.cs) は地形生成後の宝箱への配置例。 [`ExampleWorldGenHookingSystem.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Common/Systems/ExampleWorldGenHookingSystem.cs) は生成パスに割り込む例 | 地形の層/バイオーム/構造物はあるが、遊びの進行と連動する木・資源・機能ブロック・箱・鉱石のティア・探索の導線が未完成。**チャンク境界に依存しない純粋な座標関数**のまま拡張する |
| クラフト・整理 | [`ExampleRecipes.cs`](https://github.com/tModLoader/tModLoader/blob/stable/ExampleMod/Content/ExampleRecipes.cs): 代替素材のグループ、作業台、環境条件、作成後の処理を分離。[`MagicStorage/Sorting/ItemFilter.cs`](https://github.com/blushiemagic/MagicStorage/blob/1.4.4/Sorting/ItemFilter.cs) は武器/道具/防具/消耗品/設置品の検索・フィルタの実例 | 現在の RON レシピは素材と作業台のみ、持ち物は種類ごとの個数が中心。スロットの入れ替え・検索・条件・箱との往復は独立した操作とテストが必要。Magic Storage 固有のネットワーク倉庫を前提にしない |

上表は**設計の調査**であり、MOD のコードから Terraria 本体の衝突式・正確な確率・実機の UI 位置を逆算したものではない。`ExampleMod` は C# / float / `Main.rand` を使うが、Charon の `charon-core` とゲームの決定的な入力に、その方法を持ち込まない。

## 3. 対象範囲と依存関係

| 領域 | 最初の成立条件 (優先) | 機能の拡張 (順序は先行仕様と整合) | 実測/照合 |
|---|---|---|---|
| 操作・GUI (M28) | 画面の状態、ホットバーと持ち物、複数指、クラフト/装備の完結した操作 | 機能画面、スロット移動、詳細表示、アクセシビリティ | Android 版の**実機記録**との動作/配置比較。スクリーンショット・動画なしに 100% と判定しない |
| 武器・道具 | 近接/遠隔/魔法に別の使用規則、ブロックの採掘時間と素材ドロップ | 斧/ハンマー、弾薬、複数攻撃、追尾、Noita 型の組合せ杖 | 同じ武器で入力保持・射程・弾数・クールダウンを比較し、再現不能な箇所は独自仕様と明記 |
| 防具・状態異常 | 装備/セットの能力値、着脱と保存、被弾後の無敵時間 | アクセサリー、効果の相互作用、耐性・弱点 | 着脱・複数効果・期限切れのヘッドレステスト。防御式は Charon 仕様を優先 |
| 戦闘 × Noita | 武器→弾/近接→地形・敵→熱/燃焼→ダメージ→ドロップの一連 | 爆発・流体/気体との反応、環境の連鎖、予兆とボスの段階 | 質量保存・反応の順序・同一 seed のハッシュ・休眠へ戻ること。GPU の軌跡・光は別の画像テスト |
| 地形 × Noita | 地表→洞窟→深部→ボスへ到達でき、資源で装備を作れる | 木/地下構造物/機能ブロック/箱/水域/独自の環境反応 | seed 群、チャンク境界、洞窟の到達性、生成時間。粉・液体・熱のセル物理を損なわない |
| 他の必須要素 | NPC/店/進行のうち成立する最小限、複数の保存枠、チュートリアル、音、日本語 | 釣り/乗り物/イベント/追加バイオームなどの候補を個別に仕様化 | 通しプレイで製品として検証。**協力プレイは現時点で対象外** ([PRODUCT_REVIEW.md §8b](PRODUCT_REVIEW.md)) |

「ありとあらゆる」を、すべての武器名・全ボス・全アイテムの再現と解釈して際限なく追加しない。まず**仕組みの網羅** (カテゴリー、用途、組合せ、生成とドロップと保存) を定義し、その後で**独自のコンテンツ**を増やす。Noita の要素も、個々の固有名やアセットではなくセルの連鎖反応・杖の組合せ・物理的な環境影響を狙う。

## 4. 実装に移す前の契約 (次のタスク群の仕様作成用)

1. **M28 は未完了で保留**。2026-11-14 に持ち主が M28 を保留して第1段階 (M29) を先行すると決めた ([ROADMAP.md](ROADMAP.md))。本調査と追加の公開資料は実機 Terraria の現行版の操作動画や端末比較の代わりにはならない。`TERRARIA_PARITY.md` に実機依存の差分が残る。
2. M28 以降は上表を**原子的なタスク**に分け、各タスクで読む仕様・公開する API/RON・正確な規則・テスト名・パフォーマンス/視覚確認を先に固定する。未確定の係数・ドロップ率・世界生成の確率を MOD から無断で持ち込まない。仕様同士の矛盾は `CLAUDE.md` の手順に従う。
3. 実装の順は「保存と UI の土台 → 資源と道具/製作 → 戦闘とエフェクト → 地形・進行 → 実機調整」。既存の [PRODUCT_REVIEW.md §9.2](PRODUCT_REVIEW.md) は**提案**で、M28 の名前が現行 [ROADMAP.md](ROADMAP.md) と異なる。実行順は ROADMAP が優先。Noita 型のセル/熱/反応、`unsafe` 禁止、`rng::hash`、`convert`、`set`/`swap`、`charon-core` の依存制限を維持する。
4. 「似ている/完成」を判定するには、カテゴリ別の**操作ログ** (入力→状態→表示) とスクリーンショット、同一 seed の決定性・休眠テスト、保存して再開するテストが必要。Android 実機の既存の免除は**未検証**を意味し、100% の達成証明にはならない。

## 5. 調査の限界

調べたものは、tModLoader `ExampleMod` の代表的な武器・防具・道具・弾・状態異常・敵・砂・世界生成・レシピと、Magic Storage のフィルタ、Calamity の**ライセンス条件**まで。全 MOD・全バージョン・Terraria 本体の内部コードを調べたわけではない。上表の個別の仕組みの多くは**実装候補**であり、利用者の実機で確認した仕様ではない。100% の断言や、Terraria/Noita の全コンテンツのコピーを、この調査だけで正当化しない。
