#!/usr/bin/env bash
# Stage the Stormfront bundle into a pinned Horizon source with horizon-mixins and type-check
# the real game (what the launcher's native build does, minus the final link).
#
#   tools/verify-horizon.sh /path/to/horizon /path/to/horizon-mixins [check|build]
#
# Needs: a Horizon clone that contains the pinned commit, a horizon-mixins clone, Rust 1.95+,
# and on Linux the ALSA/udev development files Horizon needs. Nothing in either clone is edited:
# the pinned commit is exported with `git archive` into target/stage/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HORIZON="${1:?path to a horizon clone}"
MIXINS="${2:?path to a horizon-mixins clone}"
ACTION="${3:-check}"
PIN="$(sed -n 's/^horizon_revision = "\([0-9a-f]\{40\}\)"/\1/p' "$ROOT/bundle/stormfront/horizon-mixin.toml")"
STAGE="$ROOT/target/stage"
rm -rf "$STAGE/original" "$STAGE/modded"
mkdir -p "$STAGE/original"
git -C "$HORIZON" cat-file -e "$PIN^{commit}" || git -C "$HORIZON" fetch --depth 1 origin "$PIN"
git -C "$HORIZON" archive "$PIN" | tar -x -C "$STAGE/original"
cargo build --locked --manifest-path "$MIXINS/Cargo.toml" -p horizon-mixins-cli
CLI="$MIXINS/target/debug/horizon-mixins"
"$CLI" plan --workspace "$STAGE/original" --mods "$ROOT/bundle" --out "$STAGE/plan.json"
"$CLI" apply --workspace "$STAGE/original" --mods "$ROOT/bundle" --out "$STAGE/modded" > "$STAGE/apply.json"
cd "$STAGE/modded"
if [ "$ACTION" = build ]; then
  cargo build --locked -p horizon-client --bin horizon-client --profile fast
else
  cargo check --locked -p horizon-client --bin horizon-client --no-default-features
fi
echo "ok: $ACTION of Horizon $PIN with Stormfront"
