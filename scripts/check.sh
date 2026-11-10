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

# 核がモバイルと Web でもコンパイルできること (docs/spec/06-rendering-platform.md §7)。
check_target() {
    local target="$1"; shift
    if rustup target list --installed | grep -qx "$target"; then
        run cargo check -p charon-core --target "$target" "$@"
    else
        echo; echo "==> $target は未インストールのため省略 (rustup target add $target)"
    fi
}
check_target aarch64-linux-android
check_target wasm32-unknown-unknown --no-default-features

echo; echo "すべての確認が通りました。"
