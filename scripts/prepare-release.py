"""Validate a complete build matrix and produce a static Tauri updater feed."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import sys
from urllib.parse import quote


def prepare(source, destination, version):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Expected a stable version")
    source, destination = Path(source), Path(destination)
    files = [p for p in source.rglob("*") if p.is_file()]
    if len({p.name for p in files}) != len(files):
        raise ValueError("Duplicate release asset names")
    installers = [p for p in files if p.suffix in {".dmg", ".exe", ".msi"}]
    if sorted(p.suffix for p in installers) != [".dmg", ".dmg", ".exe", ".msi"]:
        raise ValueError("Expected two macOS installers and two Windows installers")
    patterns = {
        "darwin-aarch64": "*_aarch64.app.tar.gz",
        "darwin-x86_64": "*_x64.app.tar.gz",
        "windows-x86_64": "*_x64-setup.exe",
    }
    platforms = {}
    for platform, pattern in patterns.items():
        matches = list(source.rglob(pattern))
        if len(matches) != 1:
            raise ValueError(f"Expected one updater for {platform}, got {matches}")
        artifact = matches[0]
        signature = Path(str(artifact) + ".sig").read_text(encoding="utf-8").strip()
        if not signature:
            raise ValueError(f"Empty signature for {platform}")
        platforms[platform] = {
            "url": f"https://github.com/chanyounghur/Pinchy/releases/download/v{version}/{quote(artifact.name)}",
            "signature": signature,
        }
    destination.mkdir(parents=True, exist_ok=False)
    for artifact in files:
        shutil.copy2(artifact, destination / artifact.name)
    feed = {"version": version, "notes": f"Pinchy {version}", "platforms": platforms}
    (destination / "latest.json").write_text(json.dumps(feed, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    sums = [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}" for p in sorted(destination.iterdir())]
    (destination / "SHA256SUMS.txt").write_text("\n".join(sums) + "\n", encoding="utf-8")
    return feed


if __name__ == "__main__":
    prepare(*sys.argv[1:])
