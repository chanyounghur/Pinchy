import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("prepare", Path(__file__).with_name("prepare-release.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class UpdaterReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "builds"
        self.source.mkdir()
        self.names = ["Pinchy_0.3.0_aarch64.dmg", "Pinchy_0.3.0_x64.dmg", "Pinchy_0.3.0_x64_en-US.msi",
                      "Pinchy_0.3.0_x64-setup.exe", "Pinchy_aarch64.app.tar.gz", "Pinchy_x64.app.tar.gz"]
        for name in self.names:
            (self.source / name).write_bytes(b"test artifact")
            if name.endswith((".exe", ".tar.gz")):
                (self.source / (name + ".sig")).write_text("test-signature\n")

    def prepare(self):
        return module.prepare(self.source, self.root / "release", "0.3.0")

    def test_complete_release(self):
        feed = self.prepare()
        self.assertEqual(set(feed["platforms"]), {"darwin-aarch64", "darwin-x86_64", "windows-x86_64"})
        self.assertTrue(feed["platforms"]["windows-x86_64"]["url"].endswith("/v0.3.0/Pinchy_0.3.0_x64-setup.exe"))
        self.assertEqual(feed["platforms"]["darwin-aarch64"]["signature"], "test-signature")
        self.assertIn("latest.json", (self.root / "release/SHA256SUMS.txt").read_text())

    def test_missing_platform_blocks_publish(self):
        (self.source / "Pinchy_x64.app.tar.gz").unlink()
        with self.assertRaises(ValueError): self.prepare()
        self.assertFalse((self.root / "release").exists())

    def test_missing_signature_blocks_publish(self):
        (self.source / "Pinchy_aarch64.app.tar.gz.sig").unlink()
        with self.assertRaises(FileNotFoundError): self.prepare()

    def test_empty_signature_blocks_publish(self):
        (self.source / "Pinchy_x64.app.tar.gz.sig").write_text("\n")
        with self.assertRaises(ValueError): self.prepare()

    def test_duplicate_names_block_publish(self):
        (self.source / "duplicate").mkdir()
        (self.source / "duplicate" / self.names[0]).write_bytes(b"duplicate")
        with self.assertRaises(ValueError): self.prepare()

    def test_missing_installer_blocks_publish(self):
        (self.source / self.names[0]).unlink()
        with self.assertRaises(ValueError): self.prepare()
