# Pinchy

![Pinchy](public/brand/pinchy-wordmark.png)

복사한 텍스트, 링크, 이미지, 파일을 기억해 두는 클립보드 히스토리 앱입니다. 단축키로 패널을 열고 예전에 복사한 걸 골라 바로 붙여넣을 수 있습니다. macOS와 Windows에서 동작합니다.

## 설치

[Releases](https://github.com/chanyounghur/Pinchy/releases)에서 운영체제에 맞는 파일을 받으세요.

- Windows: `*-setup.exe` (또는 `.msi`)
- macOS: Apple Silicon은 `aarch64.dmg`, Intel은 `x64.dmg`

설치 파일에 코드 서명이 없어서 처음 실행할 때 경고가 뜰 수 있습니다. Windows에서는 "추가 정보 → 실행"을 누르면 됩니다. macOS에서는 시스템 설정 → 개인정보 보호 및 보안에서 "그래도 열기"를 누르거나, 터미널에서 아래 명령을 한 번 실행하세요.

```bash
xattr -dr com.apple.quarantine /Applications/Pinchy.app
```

macOS에서는 다른 앱에 붙여넣기 위해 접근성 권한이 필요합니다. 첫 실행 때 요청 창이 뜹니다.

## 사용법

| 동작 | macOS | Windows |
| --- | --- | --- |
| 패널 열기 | `⇧⌘V` | `Ctrl+Shift+V` |
| 항목 이동 | `←` `→` | `←` `→` |
| 붙여넣기 | `↩` | `Enter` |
| N번째 항목 붙여넣기 | `⌘1`–`⌘9` | `Ctrl+1`–`Ctrl+9` |
| 복사만 하기 | `⌘↩` | `Ctrl+Enter` |
| 삭제 | `⌘⌫` | `Ctrl+Backspace` |
| 닫기 | `esc` | `Esc` |

패널이 열리면 바로 입력해서 검색할 수 있습니다. 같은 내용을 다시 복사하면 새 항목을 만들지 않고 기존 항목을 맨 앞으로 올립니다.

트레이(메뉴 막대) 아이콘에서 단축키 변경, 히스토리 비우기, 종료를 할 수 있습니다.

### 데이터 위치

기록은 로컬에만 저장되고 외부로 전송되지 않습니다.

- macOS: `~/Library/Application Support/com.chanyounghur.pastel/`
- Windows: `%APPDATA%\com.chanyounghur.pastel\`

## 개발

[Rust](https://rustup.rs)와 [Bun](https://bun.sh)이 필요합니다. Windows에서는 Visual Studio Build Tools의 C++ 데스크톱 워크로드도 설치해야 합니다.

```bash
bun install
bun run tauri dev
```

설치 파일은 `bun run tauri build`로 만들고, 결과물은 `src-tauri/target/release/bundle/`에 생깁니다. 운영체제별로 다른 코드는 `src-tauri/src/platform.rs`에 모아 두었습니다.

### 앱 업데이트

0.3.0부터 실행 30초 후 및 6시간마다 GitHub Releases에서 새 버전을 확인하고 백그라운드로 다운로드합니다. 트레이의 **업데이트…** 또는 설정 창에서 상태를 볼 수 있고, **업데이트 확인**으로 직접 확인할 수도 있습니다. 다운로드 및 서명 검증이 끝나면 **설치하고 재시작**을 눌러 적용하세요. 업데이트 확인 실패 시 앱과 클립보드 기능은 계속 동작합니다.

0.2.x 사용자는 0.3.0 이상을 한 번 수동 설치해야 합니다. 클립보드 기록과 설정의 저장 위치는 그대로 유지됩니다.

배포 시 `createUpdaterArtifacts`로 생성한 업데이트 파일과 `.sig`, 운영체제별 주소를 담은 `latest.json`을 설치 파일과 함께 올립니다. 모든 플랫폼의 빌드가 성공하고 파일 검증을 통과한 후에만 정식 릴리스를 공개합니다.

업데이트 서명 개인키는 GitHub Actions Secret `TAURI_SIGNING_PRIVATE_KEY`에 저장합니다. 암호를 사용하는 키라면 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`도 설정하세요. 공개키는 `src-tauri/tauri.conf.json`에 있습니다. 개인키는 저장소에 커밋하지 말고 별도로 백업하세요. 이후 릴리스도 같은 키로 서명해야 기존 설치본에서 업데이트할 수 있습니다. 이 서명은 운영체제의 코드 서명·공증과 별개입니다.

### 릴리스 배포

`vX.Y.Z` 형식의 정식 버전 태그를 푸시하면 GitHub Actions가 macOS·Windows 설치 파일을 빌드해서 릴리스를 올립니다. 패치 버전도 지원하며, 프리릴리스 태그는 지원하지 않습니다.

1. `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`의 버전을 맞추고 `Cargo.lock`을 갱신합니다.
2. 커밋해서 `main`에 푸시합니다.
3. `git tag v0.3.0 && git push origin v0.3.0`

Actions 탭에서 워크플로를 수동 실행하면 릴리스 없이 설치 파일만 아티팩트로 받을 수 있습니다.

## 로드맵

- [ ] 기기 간 동기화 (iCloud Drive나 Syncthing 폴더를 이용한 파일 기반 동기화)
