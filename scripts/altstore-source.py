"""Maintain mobile/altstore.json, the AltStore/SideStore source that lists every
phone release. Usage: altstore-source.py <source.json> <version> <ipa path> <date>
Adds (or replaces) the version at the front and keeps the ten newest."""
import json
from pathlib import Path
import sys

REPO = "https://github.com/chanyounghur/Pinchy"
RAW = "https://raw.githubusercontent.com/chanyounghur/Pinchy/main"
MIN_IOS = "16.4"
KEEP = 10

DESCRIPTION = (
    "컴퓨터에서 복사한 건 휴대폰에서, 휴대폰에서 공유한 건 컴퓨터에서 바로 쓸 수 있어요. "
    "Pinchy 데스크톱과 같은 서버에 연결해서 히스토리를 보고, 눌러서 복사하고, 공유 시트로 업로드해요. "
    "내용은 기기에서 암호화해서 서버도 볼 수 없어요."
)


def build_number(version):
    major, minor, patch = (int(part) for part in version.split("."))
    return str(major * 10000 + minor * 100 + patch)


def update(source, version, size, date, notes=None):
    entry = {
        "version": version,
        "buildVersion": build_number(version),
        "date": date,
        "localizedDescription": notes or f"Pinchy {version}. 변경 내용: {REPO}/releases/tag/mobile-v{version}",
        "downloadURL": f"{REPO}/releases/download/mobile-v{version}/Pinchy-{version}.ipa",
        "size": size,
        "minOSVersion": MIN_IOS,
    }
    source = source or {}
    apps = source.get("apps") or [{}]
    app = apps[0]
    versions = [v for v in app.get("versions", []) if v.get("version") != version]
    app.update(
        {
            "name": "Pinchy",
            "bundleIdentifier": "com.chanyounghur.pinchy",
            "developerName": "chanyounghur",
            "subtitle": "모든 기기에서 같은 클립보드를",
            "localizedDescription": DESCRIPTION,
            "iconURL": f"{RAW}/mobile/assets/icon.png",
            "tintColor": "#ff6652",
            "category": "utilities",
            "screenshots": app.get("screenshots", []),
            "versions": [entry] + versions[: KEEP - 1],
            "appPermissions": {
                "entitlements": ["com.apple.security.application-groups"],
                "privacy": {
                    "NSCameraUsageDescription": "데스크톱 Pinchy의 QR을 찍어 연결할 때만 카메라를 써요.",
                    "NSPhotoLibraryUsageDescription": "앨범에서 고른 사진을 히스토리에 올릴 때만 사진을 써요.",
                    "NSPhotoLibraryAddUsageDescription": "히스토리의 사진을 앨범에 저장할 때만 써요.",
                },
            },
        }
    )
    source.update(
        {
            "name": "Pinchy",
            "identifier": "com.chanyounghur.pinchy.source",
            "subtitle": "Pinchy 휴대폰 앱",
            "description": "Pinchy 휴대폰 앱을 설치하고 업데이트할 수 있어요.",
            "iconURL": f"{RAW}/mobile/assets/icon.png",
            "website": REPO,
            "apps": [app],
            "news": source.get("news", []),
        }
    )
    return source


if __name__ == "__main__":
    path, version, ipa, date = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]), sys.argv[4]
    existing = json.loads(path.read_text(encoding="utf-8")) if path.exists() else None
    source = update(existing, version, ipa.stat().st_size, date)
    path.write_text(json.dumps(source, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{path}: {len(source['apps'][0]['versions'])} version(s), newest {version}")
