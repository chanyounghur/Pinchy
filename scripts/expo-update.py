"""Turn `expo export` output into expo-updates manifests (protocol 1).

Usage:
  expo-update.py <dist> <out> --version 1.0.1 --runtime 1 --tag mobile-v1.0.1 \
      --repo chanyounghur/Pinchy --config expoConfig.json [--date ISO] [--key private.pem]

Writes `<out>/assets/<name>` for every bundle and asset (to attach to the
release) and `<out>/updates/<runtime>/<platform>/manifest.json` plus, with a
key, `manifest.sig` (to commit to the repository, where the relay serves
them from). Asset URLs point at the release's download links."""
import base64
import hashlib
import json
import mimetypes
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid

PLATFORMS = ("ios", "android")
NAMESPACE = uuid.UUID("6d1a3e52-5b17-4b64-9c0f-5a8b3c2f1e07")


def build_manifest(dist, out, platform, version, runtime, download_base, created_at, expo_config):
    """Copies the platform's files under out/assets and returns the manifest."""
    dist, out = Path(dist), Path(out)
    metadata = json.loads((dist / "metadata.json").read_text(encoding="utf-8"))
    files = metadata["fileMetadata"].get(platform)
    if not files:
        raise ValueError(f"metadata.json has no {platform} bundle; export with --platform {platform}")
    assets_dir = out / "assets"
    assets_dir.mkdir(parents=True, exist_ok=True)

    def place(path, ext, content_type):
        source = dist / path
        data = source.read_bytes()
        digest = hashlib.sha256(data).digest()
        name = f"{digest.hex()}.{ext}"
        shutil.copyfile(source, assets_dir / name)
        return {
            "hash": base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii"),
            "key": hashlib.md5(data).hexdigest(),
            "contentType": content_type,
            "fileExtension": f".{ext}",
            "url": f"{download_base}/{name}",
        }

    launch = place(files["bundle"], "hbc", "application/javascript")
    assets = [place(a["path"], a["ext"], mimetypes.types_map.get(f".{a['ext']}", "application/octet-stream")) for a in files["assets"]]
    return {
        "id": str(uuid.uuid5(NAMESPACE, f"pinchy/{runtime}/{platform}/{version}")),
        "createdAt": created_at,
        "runtimeVersion": runtime,
        "launchAsset": launch,
        "assets": assets,
        "metadata": {},
        "extra": {"expoClient": expo_config},
    }


def sign(body, key_pem):
    """RSA PKCS#1 v1.5 over SHA-256, base64, as expo-updates verifies it."""
    with tempfile.TemporaryDirectory() as tmp:
        key = Path(tmp) / "key.pem"
        key.write_bytes(key_pem)
        signature = subprocess.run(
            ["openssl", "dgst", "-sha256", "-sign", str(key), "-binary"],
            input=body, capture_output=True, check=True,
        ).stdout
    return base64.b64encode(signature).decode("ascii")


def write_update(dist, out, platform, version, runtime, repo, tag, created_at, expo_config, key_pem=None):
    download_base = f"https://github.com/{repo}/releases/download/{tag}"
    manifest = build_manifest(dist, out, platform, version, runtime, download_base, created_at, expo_config)
    folder = Path(out) / "updates" / runtime / platform
    folder.mkdir(parents=True, exist_ok=True)
    body = json.dumps(manifest, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    (folder / "manifest.json").write_bytes(body)
    if key_pem:
        (folder / "manifest.sig").write_text(sign(body, key_pem) + "\n", encoding="ascii")
    return manifest


if __name__ == "__main__":
    import argparse
    import datetime
    import os

    parser = argparse.ArgumentParser()
    parser.add_argument("dist")
    parser.add_argument("out")
    parser.add_argument("--version", required=True)
    parser.add_argument("--runtime", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--config", required=True, help="output of `expo config --type public --json`")
    parser.add_argument("--date", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))
    parser.add_argument("--key", help="PEM private key; or set EXPO_UPDATES_PRIVATE_KEY to its contents")
    args = parser.parse_args()
    key_pem = Path(args.key).read_bytes() if args.key else os.environ.get("EXPO_UPDATES_PRIVATE_KEY", "").encode()
    expo_config = json.loads(Path(args.config).read_text(encoding="utf-8"))
    for platform in PLATFORMS:
        manifest = write_update(args.dist, args.out, platform, args.version, args.runtime, args.repo, args.tag, args.date, expo_config, key_pem or None)
        print(f"{platform}: update {manifest['id']} with {len(manifest['assets'])} asset(s)" + (" (signed)" if key_pem else " (unsigned)"))
    if not key_pem:
        print("warning: EXPO_UPDATES_PRIVATE_KEY is not set; phones built with a certificate will refuse this update", file=sys.stderr)
