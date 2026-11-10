# Charon Engine

Noita / Terraria のような落下砂 (falling sand) ゲームを作るための Rust 製 2D エンジン。

[FallingSandEngine](https://github.com/PieKing1215/FallingSandEngine) の調査で見つかった問題
(砂しか動かないシミュレーション、未定義動作に依存した並列化、重いピクセル表現、非決定性) を、
設計の最初から解決することを目的にしています。

現在は **M0 (環境整備)** の段階です。計画は [docs/ROADMAP.md](docs/ROADMAP.md) を参照してください。

## 開発

Rust の最新 stable を使います (`rust-toolchain.toml` で指定)。

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

## 構成

| パス | 内容 |
|---|---|
| `crates/charon-core` | シミュレーション核。描画に依存せずヘッドレスで動く |
| `docs/ROADMAP.md` | 設計原則とマイルストーン |
