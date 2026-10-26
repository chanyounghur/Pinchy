# Pinchy 휴대폰 앱

데스크톱 Pinchy가 연결된 릴레이 서버의 히스토리를 폰에서 봅니다. 탭하면 폰 클립보드로 복사되고, 다른 앱에서 공유 시트로 보내거나 앱 안의 **클립보드 붙여넣기**로 올리면 연결된 모든 기기의 히스토리에 나타납니다. 내용은 폰에서 암호화되어 서버는 읽지 못합니다. 데스크톱 1.0 이상과 릴레이 1.0 이상이 필요합니다.

폰에서는 OS가 백그라운드 클립보드 감시를 막기 때문에 데스크톱처럼 자동으로 쌓이지는 않습니다. 앱을 열었을 때 받아오고, 올리는 것은 공유 시트나 붙여넣기 버튼으로 합니다.

## 연결

1. 컴퓨터의 Pinchy 설정 → 공유 → **휴대폰 추가**를 누르면 QR이 나옵니다. QR에는 서버 주소, 10분짜리 일회용 코드, 공유 키가 들어 있으니 다른 사람에게 보이지 않게 하세요.
2. 폰 Pinchy를 열어 QR을 찍습니다. 카메라가 안 되면 QR 아래 링크를 복사해 폰으로 보낸 뒤 **복사한 링크로 연결**을 누르세요.
3. 바로 전체 히스토리를 받아옵니다. 설정에서 기기 이름, 연결된 기기, 서버의 보관 정책과 사용량을 볼 수 있습니다.

연결을 끊으려면 설정 맨 아래 **연결 해제**. 이 폰의 키와 받아 둔 히스토리가 지워지고 서버의 기기 목록에서도 빠집니다.

## 설치

릴리즈는 [GitHub Releases](https://github.com/chanyounghur/Pinchy/releases)의 `mobile-vX.Y.Z` 태그에 올라갑니다. 데스크톱 릴리즈(`vX.Y.Z`)와 별개입니다.

### Android

`Pinchy-X.Y.Z.apk`를 폰에서 받아 설치합니다. 처음 한 번 "알 수 없는 앱 설치"를 허용해야 합니다. 모든 릴리즈가 같은 키로 서명되어 새 버전은 이전 버전 위에 그대로 설치됩니다.

### iPhone

IPA는 서명되어 있지 않으며, 설치하는 사람의 Apple ID로 서명해 넣습니다. 무료 Apple ID로도 되지만 7일마다 다시 서명해야 하고, AltStore·SideStore가 이를 자동으로 해 줍니다.

1. 맥에 [AltServer](https://altstore.io)를 설치하고, 폰을 USB로 연결해 Finder에서 "Wi-Fi에서 이 iPhone 보기"를 켭니다.
2. AltServer 메뉴 → Install AltStore → 폰 선택 → Apple ID 로그인. 폰에서 설정 → 일반 → VPN 및 기기 관리에서 그 Apple ID를 신뢰하고, 설정 → 개인정보 보호 및 보안에서 **개발자 모드**를 켭니다.
3. AltStore의 Sources에 다음 주소를 추가하면 Pinchy가 목록에 나타납니다.

   ```
   https://raw.githubusercontent.com/chanyounghur/Pinchy/main/mobile/altstore.json
   ```

4. 설치. 이후 같은 Wi-Fi에 맥이 켜져 있으면 AltStore가 백그라운드에서 재서명하고, 새 릴리즈는 AltStore에 업데이트로 뜹니다.

집 밖에서도 재서명이 되게 하려면 [SideStore](https://sidestore.io)를 쓰면 됩니다. 소스 주소는 같습니다. 무료 계정은 동시에 앱 3개까지만 서명할 수 있고(AltStore 자체 포함), 사이드로딩에 쓰는 Apple ID가 가끔 보안 잠금을 받는 사례가 있으니 전용 Apple ID를 하나 만들어 쓰는 편이 안전합니다. 다른 앱에서 공유 시트로 보내는 기능은 앱 안의 확장(share extension)이 함께 서명되어야 하므로, AltStore가 확장을 포함해 설치하겠는지 물으면 **포함**을 고르세요.

## 개발

[Bun](https://bun.sh)과 Node 22가 필요합니다. 네이티브 모듈을 쓰므로 Expo Go로는 실행할 수 없고 개발 빌드가 필요합니다.

```bash
cd mobile
bun install
bun run typecheck
bun run test                 # 프로토콜 단위 테스트
PINCHY_RELAY_BIN=../relay/target/debug/pinchy-relay bun run test   # 실제 릴레이와 통합 테스트
bunx expo run:ios            # 맥 + Xcode, 연결된 iPhone 또는 시뮬레이터
bunx expo run:android        # Android Studio 또는 연결된 폰
```

- `src/protocol/`: 데스크톱 `src-tauri/src/share.rs`와 같은 암호화와 릴레이 API. `test/vectors.json`은 Rust 쪽에서 `cargo test --lib dump_mobile_vectors -- --ignored`로 만든 알려진 답이고, TS 구현이 그것을 열 수 있어야 합니다.
- `src/sync.ts`: 변경 피드를 따라가는 동기화 엔진. 첫 연결에 전체를 받고 이후 롱폴, 409면 처음부터 다시.
- `src/store/`: SQLite 캐시(`db.ts`)와 보안 저장소의 세션(`session.ts`).
- `app/`: expo-router 화면. `index`(히스토리), `join`(QR), `item/[id]`, `settings`, `add`(올리기 시트).

## 릴리즈

`mobile-vX.Y.Z` 태그를 푸시하면 GitHub Actions가 Android APK와 미서명 iOS IPA를 만들어 릴리즈에 올리고, `mobile/altstore.json`에 새 버전을 추가해 `main`에 커밋합니다.

1. `mobile/package.json`과 `mobile/app.json`의 버전을 맞춥니다. 빌드 번호는 `app.config.js`가 버전에서 계산합니다(1.2.3 → 10203).
2. 커밋해서 `main`에 푸시합니다.
3. `git tag mobile-v1.0.0 && git push origin mobile-v1.0.0`

Actions 탭에서 수동 실행하면 릴리즈 없이 APK와 IPA만 아티팩트로 받을 수 있습니다.

### Android 서명 키

릴리즈 APK는 저장소 Secrets의 keystore로 서명합니다. 한 번 만들어 두고 바꾸지 마세요. 키가 바뀌면 기존 설치본 위에 업데이트할 수 없습니다.

```bash
keytool -genkeypair -v -keystore pinchy-release.jks -alias pinchy -keyalg RSA -keysize 2048 -validity 10000
base64 -i pinchy-release.jks | pbcopy
```

| Secret | 값 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | keystore 파일의 base64 |
| `ANDROID_KEYSTORE_PASSWORD` | keystore 암호 |
| `ANDROID_KEY_ALIAS` | `pinchy` |
| `ANDROID_KEY_PASSWORD` | 키 암호 (keytool에서 따로 정하지 않았다면 keystore 암호와 같음) |

keystore 파일은 저장소에 넣지 말고 따로 백업하세요. iOS 쪽은 서명하지 않으므로 Secret이 필요 없습니다.
