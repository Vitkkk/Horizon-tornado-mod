#!/usr/bin/env bash
# Build Stormfront Lite: the wasm32 cdylib, the component (wit-component, as Horizon does),
# the host check under Horizon Android's limits, and target/stormfront_lite-<version>.hmod.
set -euo pipefail
cd "$(dirname "$0")"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
cargo test --release -p stormfront-lite
cargo build --release --target wasm32-unknown-unknown -p stormfront-lite
cargo run --release -p stormfront-lite-tools -- pack target/wasm32-unknown-unknown/release/stormfront_lite.wasm target/mod.wasm stormfront-lite/horizon-mod.toml
cargo run --release -p stormfront-lite-hosttest -- target/mod.wasm
python3 - <<'PY'
import re, zipfile, pathlib
root = pathlib.Path('.')
manifest = (root / 'stormfront-lite/horizon-mod.toml').read_text()
mod_id = re.search(r'^id = "([a-z0-9_]+)"', manifest, re.M).group(1)
version = re.search(r'^version = "([^"]+)"', manifest, re.M).group(1)
out = root / 'target' / f'{mod_id}-{version}.hmod'
with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
    for name, data in [('horizon-mod.toml', manifest.encode()), ('mod.wasm', (root / 'target/mod.wasm').read_bytes())]:
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.external_attr = 0o100644 << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, data)
print(out)
PY
