#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
unset CLAUDE_CONFIG_DIR CODEX_HOME
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export HOME="$CARGO_TARGET_DIR/ui-fixture-build-home"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_CACHE_HOME="$HOME/cache"
mkdir -p "$HOME"
(cd ui && pnpm build)
cargo build -p orx-host --profile host
mkdir -p "$CARGO_TARGET_DIR/ui-fixture-host"
case "$(uname -s)" in Darwin) host_os=darwin;; Linux) host_os=linux;; *) exit 1;; esac
case "$(uname -m)" in arm64) host_arch=aarch64;; *) host_arch="$(uname -m)";; esac
cp "$CARGO_TARGET_DIR/host/orx-host" "$CARGO_TARGET_DIR/ui-fixture-host/orx-host-$host_os-$host_arch"
export ORX_FIXTURE_HOST_BIN_DIR="$CARGO_TARGET_DIR/ui-fixture-host"
exec cargo test --test ui_fixture -- --ignored --nocapture
