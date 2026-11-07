#!/usr/bin/env python3
"""Pack the native bundle into target/stormfront-<version>.hnmod (docs/api/mod-files.md in
Horizon): horizon-native.json plus mixins/stormfront/..., byte-for-byte reproducible."""
import json
import pathlib
import re
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "bundle" / "stormfront"
COMPAT = json.loads((ROOT / "compatibility.json").read_text())


def allowed(rel: pathlib.PurePosixPath) -> bool:
    """The file kinds a native bundle may carry."""
    parts = rel.parts
    if rel.suffix in (".rs", ".toml"):
        return True
    if parts[0] == "data" or (len(parts) > 2 and parts[1] == "data"):
        return True
    return bool(re.match(r"(README|LICENSE|LICENCE|COPYING|NOTICE).*|\.gitignore$", rel.name)) and len(parts) <= 2


def main() -> int:
    manifest = (BUNDLE / "horizon-mixin.toml").read_text()
    mod_id = re.search(r'^id = "([a-z0-9_]+)"', manifest, re.M).group(1)
    version = re.search(r'^version = "([^"]+)"', manifest, re.M).group(1)
    pin = re.search(r'^horizon_revision = "([0-9a-f]{40})"', manifest, re.M).group(1)
    if pin != COMPAT["horizon_revision"]:
        print(f"horizon-mixin.toml pins {pin}, compatibility.json {COMPAT['horizon_revision']}", file=sys.stderr)
        return 1
    native = {
        "schema": 1,
        "id": mod_id,
        "version": version,
        "game": COMPAT["game"],
        "horizon": pin,
        "mixins": COMPAT["horizon_mixins_revision"],
        "hooks": COMPAT["hooks"],
    }
    files = []
    for path in sorted(BUNDLE.rglob("*")):
        if path.is_dir() or "target" in path.relative_to(BUNDLE).parts:
            continue
        rel = pathlib.PurePosixPath(path.relative_to(BUNDLE).as_posix())
        if not allowed(rel):
            print(f"not allowed in a native bundle: {rel}", file=sys.stderr)
            return 1
        files.append((f"mixins/{mod_id}/{rel}", path.read_bytes()))
    out = ROOT / "target" / f"{mod_id}-{version}.hnmod"
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        def add(name: str, data: bytes) -> None:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
        add("horizon-native.json", (json.dumps(native, indent=2) + "\n").encode())
        for name, data in files:
            add(name, data)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
