#!/usr/bin/env bash
# Web サンドボックスを 1 スレッドの wasm として組み立てる。
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p charon-sandbox --lib --release --target wasm32-unknown-unknown --no-default-features
wasm-bindgen target/wasm32-unknown-unknown/release/charon_sandbox.wasm \
  --target web --out-dir apps/sandbox/web/pkg --out-name charon_sandbox
printf 'Web サンドボックス: apps/sandbox/web/index.html\n'
