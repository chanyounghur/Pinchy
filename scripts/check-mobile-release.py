"""Check that a mobile release tag and the app manifests describe one version."""
import json
import os
from pathlib import Path
import re

VERSION = r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"


def validate_versions(package, app, tag=None):
    versions = [package["version"], app["expo"]["version"]]
    if len(set(versions)) != 1:
        raise ValueError(f"Version mismatch across mobile/package.json and mobile/app.json: {versions}")
    version = versions[0]
    if not re.fullmatch(VERSION, version):
        raise ValueError(f"Expected a stable x.y.z version, got {version}")
    if tag is not None:
        if not re.fullmatch(f"mobile-v{VERSION}", tag):
            raise ValueError("Automatic mobile releases require a stable mobile-vX.Y.Z tag")
        if tag != f"mobile-v{version}":
            raise ValueError(f"Tag {tag} does not match mobile app version {version}")
    return version


def build_number(version):
    """The store build number app.config.js derives: 1.2.3 -> 10203."""
    major, minor, patch = (int(part) for part in version.split("."))
    return major * 10000 + minor * 100 + patch


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    tag = os.environ.get("GITHUB_REF_NAME") if os.environ.get("GITHUB_EVENT_NAME") == "push" else None
    version = validate_versions(
        json.loads((root / "mobile/package.json").read_text(encoding="utf-8")),
        json.loads((root / "mobile/app.json").read_text(encoding="utf-8")),
        tag,
    )
    print(f"Validated Pinchy mobile {version}" + (f" for {tag}" if tag else " (build only)"))
