# Charon Engine

Charon Engineは、Rustでゲームルールやコンテンツを定義できる、オープンソースの2Dサンドボックスゲームエンジンです。シーン・入力・資産を管理するゲーム実行基盤と、マテリアルや世界を扱うサンドボックスシミュレーションを組み合わせています。

## エンジン概要

ゲームごとのルールをエンジン本体へ固定せず、ゲーム作者が素材、キャラクター、操作、進行、独自の振る舞いを組み立てられることを目指しています。Terraria風・Noita風のゲームを含む幅広い2Dゲーム制作を対象としています。

Rustの構造体とトレイトでSceneや独自システムを実装し、型付きAPIからエンジンの機能を利用します。Cargoはビルドに含める実装と依存関係を管理し、YAMLは実行環境の設定に使います。

## エンジンの詳細

現在のコードベースには、次の機能群があります。

- **ゲーム実行基盤**：Scene、plugin、入力、固定更新、Systemの実行順序、headless実行
- **資産と表示**：資産pack、画像・音声・fontの読込、sprite、camera、描画adapter
- **世界と保存**：多層の疎なworld、chunk活動域、block個体、原子的編集、世代保存と復元
- **サンドボックスsimulation**：粉体・液体・気体の輸送、圧力、混合、物質粒子、物性と力、保存可能な処理Job、熱輸送、相変化、反応
- **剛体と破壊**：回転body、衝突、流体・熱との連成、物質を持つ破壊可能bodyを提供します（E07/Q06実装済み。検証範囲は[検証記録](new_docs/evidence/E07/README.md)を参照）。

製品の設計目標、公開APIの方針、機能ごとの仕様は [`new_docs/README.md`](new_docs/README.md) から参照できます。設計上の目標と、現在コードから利用できる機能は各設計・検証資料で区別しています。

## 使い方

Rust 1.98.1をインストールし、リポジトリを取得してheadless作例を実行します。

```sh
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy
git clone https://github.com/sofia-gros/charon-engine.git
cd charon-engine
cargo +1.98.1 run --locked --example assets_scene
```

`assets_scene`は資産packを読み込み、spriteとcameraを使った画像を生成します。対応する表示環境でwindow表示を試す場合は次を実行します。

```sh
cargo +1.98.1 run --locked --example assets_scene -- --window
```

その他の作例は [`examples/`](examples/) にあります。Sceneと入力、世界、保存、マテリアルsimulationなどの例から、目的に近いものを選べます。

- [作者向け制作手順](new_docs/02_CREATOR_WORKFLOW.md)
- [Rust APIの設計方針](new_docs/03_CREATOR_API_MODEL.md)
- [エンジン設定の使い分け](new_docs/05_ENGINE_CONFIGURATION.md)

## 開発方法

リポジトリを取得してRust 1.98.1を使います。製品仕様と開発上のルールは [`new_docs/`](new_docs/README.md) と [`AGENTS.md`](AGENTS.md) を参照してください。機能の実装では、関係する設計書、コード、対象テストを一緒に確認します。

```sh
cargo +1.98.1 fmt --all -- --check
cargo +1.98.1 check --locked
cargo +1.98.1 test --locked
cargo +1.98.1 clippy --all-targets --locked -- -D warnings
cargo +1.98.1 doc --lib --no-deps --locked
```

## ライセンス

エンジン本体は[MIT License](LICENSE)です。依存crateと同梱資産には、それぞれのライセンスが適用されます。
