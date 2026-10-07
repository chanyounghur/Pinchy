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

### 릴리스

`vX.Y.0` 형식의 태그를 푸시하면 GitHub Actions가 macOS·Windows 설치 파일을 빌드해서 릴리스를 올립니다. 패치 태그(`vX.Y.1` 등)는 빌드하지 않습니다.

1. `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`의 버전을 맞추고 `Cargo.lock`을 갱신합니다.
2. 커밋해서 `main`에 푸시합니다.
3. `git tag v0.2.0 && git push origin v0.2.0`

Actions 탭에서 워크플로를 수동 실행하면 릴리스 없이 설치 파일만 아티팩트로 받을 수 있습니다.

## 로드맵

- [ ] 기기 간 동기화 (iCloud Drive나 Syncthing 폴더를 이용한 파일 기반 동기화)
