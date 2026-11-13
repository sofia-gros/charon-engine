#!/usr/bin/env bash
# コミット前に必ず通す確認をまとめて実行する。CI も同じ内容を実行する。
# 使い方: scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

run() { echo; echo "==> $*"; "$@"; }

run cargo fmt --all --check
run cargo clippy --workspace --all-targets -- -D warnings
run cargo clippy --workspace --all-targets --no-default-features -- -D warnings
run cargo test --workspace
run cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" run cargo doc --workspace --no-deps

# Android は核とアプリ、Web は 1 スレッドのサンドボックス全体を確認する。
check_target() {
    local target="$1"; shift
    if rustup target list --installed | grep -qx "$target"; then
        run cargo check -p charon-core --target "$target" "$@"
    else
        echo; echo "==> $target は未インストールのため省略 (rustup target add $target)"
    fi
}
check_target aarch64-linux-android
if rustup target list --installed | grep -qx aarch64-linux-android; then
    run cargo check -p charon-render -p charon-app -p charon-sandbox --lib --target aarch64-linux-android
fi
web_core_compiles() {
    if rustup target list --installed | grep -qx wasm32-unknown-unknown; then
        run cargo check -p charon-core --target wasm32-unknown-unknown --no-default-features
        run cargo check -p charon-sandbox --lib --target wasm32-unknown-unknown --no-default-features
        run cargo clippy -p charon-sandbox --lib --target wasm32-unknown-unknown --no-default-features -- -D warnings
    else
        echo; echo "==> wasm32-unknown-unknown は未インストールのため省略 (rustup target add wasm32-unknown-unknown)"
    fi
}
web_core_compiles

echo; echo "すべての確認が通りました。"
