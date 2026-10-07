"""Check that release tags and all application manifests describe one version."""
import json
import os
from pathlib import Path
import re
import tomllib


def validate_versions(package, tauri, cargo, lock, tag=None):
    versions = [package["version"], tauri["version"], cargo["package"]["version"]]
    versions += [p["version"] for p in lock["package"] if p["name"] == "pinchy" and "source" not in p]
    if len(versions) != 4 or len(set(versions)) != 1:
        raise ValueError(f"Version mismatch across package.json, Tauri, Cargo and Cargo.lock: {versions}")
    version = versions[0]
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError(f"Expected a stable x.y.z version, got {version}")
    if tag is not None:
        if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
            raise ValueError("Automatic releases require a stable vX.Y.Z tag")
        if tag != f"v{version}":
            raise ValueError(f"Tag {tag} does not match application version {version}")
    return version


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    tag = os.environ.get("GITHUB_REF_NAME") if os.environ.get("GITHUB_EVENT_NAME") == "push" else None
    version = validate_versions(
        json.loads((root / "package.json").read_text(encoding="utf-8")),
        json.loads((root / "src-tauri/tauri.conf.json").read_text(encoding="utf-8")),
        tomllib.loads((root / "src-tauri/Cargo.toml").read_text(encoding="utf-8")),
        tomllib.loads((root / "src-tauri/Cargo.lock").read_text(encoding="utf-8")),
        tag,
    )
    print(f"Validated Pinchy {version}" + (f" for {tag}" if tag else " (build only)"))
