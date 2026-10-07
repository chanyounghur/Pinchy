import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("check-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseVersionTests(unittest.TestCase):
    def manifests(self, version="0.2.0"):
        return ({"version": version}, {"version": version}, {"package": {"version": version}},
                {"package": [{"name": "pinchy", "version": version}]})

    def test_minor_tag(self):
        self.assertEqual(release.validate_versions(*self.manifests(), "v0.2.0"), "0.2.0")
        self.assertEqual(release.validate_versions(*self.manifests("1.12.0"), "v1.12.0"), "1.12.0")

    def test_patch_major_prerelease_and_malformed_tags_are_rejected(self):
        for tag in ["v0.2.1", "v1.0.0", "v0.2.0-beta.1", "0.2.0", "v00.2.0", "v0.02.0"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.validate_versions(*self.manifests(), tag)

    def test_tag_must_match_manifests(self):
        with self.assertRaises(ValueError):
            release.validate_versions(*self.manifests(), "v0.3.0")

    def test_all_manifests_must_agree(self):
        for index in range(4):
            manifests = list(self.manifests())
            manifests[index] = self.manifests("0.1.0")[index]
            with self.subTest(index=index), self.assertRaises(ValueError):
                release.validate_versions(*manifests, "v0.2.0")

    def test_missing_lock_entry_is_rejected(self):
        manifests = list(self.manifests())
        manifests[-1] = {"package": []}
        with self.assertRaises(ValueError):
            release.validate_versions(*manifests)

    def test_manual_build_allows_patch_version_without_publishing(self):
        self.assertEqual(release.validate_versions(*self.manifests("0.2.1")), "0.2.1")


if __name__ == "__main__":
    unittest.main()
