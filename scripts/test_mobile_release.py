import importlib.util
from pathlib import Path
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), Path(__file__).with_name(f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


check = load("check-mobile-release")
altstore = load("altstore-source")


class MobileReleaseVersionTests(unittest.TestCase):
    def manifests(self, version="1.0.0"):
        return {"version": version}, {"expo": {"version": version}}

    def test_tag_matches_manifests(self):
        self.assertEqual(check.validate_versions(*self.manifests(), "mobile-v1.0.0"), "1.0.0")
        self.assertEqual(check.validate_versions(*self.manifests("1.12.3"), "mobile-v1.12.3"), "1.12.3")
        self.assertEqual(check.validate_versions(*self.manifests()), "1.0.0")

    def test_desktop_tags_and_prereleases_are_rejected(self):
        for tag in ["v1.0.0", "mobile-v1.0.0-beta", "mobile-1.0.0", "mobile-v01.0.0"]:
            with self.assertRaises(ValueError):
                check.validate_versions(*self.manifests(), tag)
        with self.assertRaises(ValueError):
            check.validate_versions(*self.manifests("1.0.1"), "mobile-v1.0.0")
        with self.assertRaises(ValueError):
            check.validate_versions({"version": "1.0.0"}, {"expo": {"version": "1.0.1"}})

    def test_build_numbers_increase_with_versions(self):
        self.assertEqual(check.build_number("1.0.0"), 10000)
        self.assertEqual(check.build_number("1.2.3"), 10203)
        self.assertLess(check.build_number("1.9.99"), check.build_number("1.10.0"))
        self.assertEqual(altstore.build_number("1.2.3"), "10203")


class AltStoreSourceTests(unittest.TestCase):
    def test_new_source_lists_the_release(self):
        source = altstore.update(None, "1.0.0", 1234, "2026-10-26T00:00:00Z")
        app = source["apps"][0]
        self.assertEqual(app["bundleIdentifier"], "com.chanyounghur.pinchy")
        self.assertEqual(app["versions"][0]["downloadURL"], "https://github.com/chanyounghur/Pinchy/releases/download/mobile-v1.0.0/Pinchy-1.0.0.ipa")
        self.assertEqual(app["versions"][0]["size"], 1234)
        self.assertEqual(app["versions"][0]["buildVersion"], "10000")

    def test_versions_prepend_replace_and_cap(self):
        source = None
        for i in range(12):
            source = altstore.update(source, f"1.0.{i}", 10 + i, "2026-10-26T00:00:00Z")
        versions = [v["version"] for v in source["apps"][0]["versions"]]
        self.assertEqual(versions[:2], ["1.0.11", "1.0.10"])
        self.assertEqual(len(versions), 10)
        again = altstore.update(source, "1.0.11", 999, "2026-10-27T00:00:00Z")
        self.assertEqual(again["apps"][0]["versions"][0]["size"], 999)
        self.assertEqual(len(again["apps"][0]["versions"]), 10)


if __name__ == "__main__":
    unittest.main()
