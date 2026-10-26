import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("expo_update", Path(__file__).with_name("expo-update.py"))
expo_update = importlib.util.module_from_spec(spec)
spec.loader.exec_module(expo_update)


def fake_export(root):
    dist = Path(root) / "dist"
    (dist / "_expo/static/js/ios").mkdir(parents=True)
    (dist / "assets").mkdir()
    (dist / "_expo/static/js/ios/entry-abc.hbc").write_bytes(b"bundle")
    (dist / "assets/0a32").write_bytes(b"png-bytes")
    (dist / "metadata.json").write_text(json.dumps({
        "version": 0,
        "bundler": "metro",
        "fileMetadata": {"ios": {"bundle": "_expo/static/js/ios/entry-abc.hbc", "assets": [{"path": "assets/0a32", "ext": "png"}]}},
    }))
    return dist


class ManifestTests(unittest.TestCase):
    def test_manifest_points_at_release_assets_by_hash(self):
        with tempfile.TemporaryDirectory() as root:
            dist = fake_export(root)
            out = Path(root) / "out"
            manifest = expo_update.write_update(dist, out, "ios", "1.0.1", "1", "o/r", "mobile-v1.0.1", "2026-10-26T00:00:00.000Z", {"name": "Pinchy"})
            bundle_hash = hashlib.sha256(b"bundle")
            self.assertEqual(manifest["runtimeVersion"], "1")
            self.assertEqual(manifest["launchAsset"]["url"], f"https://github.com/o/r/releases/download/mobile-v1.0.1/{bundle_hash.hexdigest()}.hbc")
            self.assertEqual(manifest["launchAsset"]["hash"], base64.urlsafe_b64encode(bundle_hash.digest()).rstrip(b"=").decode())
            self.assertEqual(manifest["launchAsset"]["contentType"], "application/javascript")
            self.assertEqual(manifest["assets"][0]["contentType"], "image/png")
            self.assertEqual(manifest["assets"][0]["fileExtension"], ".png")
            self.assertEqual(manifest["extra"]["expoClient"], {"name": "Pinchy"})
            self.assertTrue((out / "assets" / f"{bundle_hash.hexdigest()}.hbc").exists())
            self.assertTrue((out / "assets" / f"{hashlib.sha256(b'png-bytes').hexdigest()}.png").exists())
            written = json.loads((out / "updates/1/ios/manifest.json").read_text())
            self.assertEqual(written, manifest)
            self.assertFalse((out / "updates/1/ios/manifest.sig").exists(), "unsigned without a key")
            again = expo_update.write_update(dist, out, "ios", "1.0.1", "1", "o/r", "mobile-v1.0.1", "2026-10-27T00:00:00.000Z", {})
            self.assertEqual(again["id"], manifest["id"], "the id depends on runtime, platform and version only")
            self.assertNotEqual(expo_update.write_update(dist, out, "ios", "1.0.2", "1", "o/r", "t", "2026-10-26T00:00:00.000Z", {})["id"], manifest["id"])

    def test_missing_platform_is_an_error(self):
        with tempfile.TemporaryDirectory() as root:
            dist = fake_export(root)
            with self.assertRaises(ValueError):
                expo_update.write_update(dist, Path(root) / "out", "android", "1.0.1", "1", "o/r", "t", "2026-10-26T00:00:00.000Z", {})
            with self.assertRaises(ValueError, msg="iOS needs milliseconds in createdAt"):
                expo_update.write_update(dist, Path(root) / "out", "ios", "1.0.1", "1", "o/r", "t", "2026-10-26T00:00:00Z", {})
            self.assertRegex(expo_update.now_created_at(), expo_update.CREATED_AT)

    @unittest.skipUnless(shutil.which("openssl"), "openssl not installed")
    def test_signature_verifies_with_the_public_key(self):
        with tempfile.TemporaryDirectory() as root:
            key = Path(root) / "key.pem"
            subprocess.run(["openssl", "genrsa", "-out", str(key), "2048"], check=True, capture_output=True)
            public = Path(root) / "pub.pem"
            subprocess.run(["openssl", "rsa", "-in", str(key), "-pubout", "-out", str(public)], check=True, capture_output=True)
            dist = fake_export(root)
            out = Path(root) / "out"
            expo_update.write_update(dist, out, "ios", "1.0.1", "1", "o/r", "t", "2026-10-26T00:00:00.000Z", {}, key.read_bytes())
            body = (out / "updates/1/ios/manifest.json").read_bytes()
            signature = base64.b64decode((out / "updates/1/ios/manifest.sig").read_text())
            sig_file = Path(root) / "sig.bin"
            sig_file.write_bytes(signature)
            verified = subprocess.run(
                ["openssl", "dgst", "-sha256", "-verify", str(public), "-signature", str(sig_file)],
                input=body, capture_output=True,
            )
            self.assertEqual(verified.returncode, 0, verified.stderr)


if __name__ == "__main__":
    unittest.main()
