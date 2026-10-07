# Pinchy

![Pinchy](public/brand/pinchy-wordmark.png)

복사한 텍스트·링크·이미지·파일을 모아두고 다시 꺼내 쓰는 클립보드 히스토리 매니저. Tauri 2 + React.

## 기능 (1단계)

- 텍스트 / 링크 / 이미지 / 파일 복사 자동 기록 (중복은 최신으로 끌어올림)
- `⇧⌘V` 로 화면 하단 패널 열기, 검색, `←/→` 이동, `↩` 로 직전 앱에 즉시 붙여넣기
- `⌘↩` 복사만, `⌘⌫` 삭제, `esc` 닫기
- 메뉴바 트레이: 열기 / 히스토리 비우기 / 종료 (Dock 아이콘 없음)

기존 기록과 설정을 유지하기 위해 앱 식별자 `com.chanyounghur.pastel`과 DB 파일명 `pastel.db`는 호환성 목적으로 유지한다. Windows 데이터는 `%APPDATA%/com.chanyounghur.pastel/`에 저장한다.

macOS 데이터: `~/Library/Application Support/com.chanyounghur.pastel/` (`pastel.db` + `images/`)

## 브랜딩

- 앱 이름: **Pinchy**
- B / Soft 로고: 코랄 클립, 둥근 앞 카드, 살구색 뒷 카드
- 원본 시안: `design/pinchy/b-soft-v1/`
- 웹 화면 로고: `public/brand/`
- 앱/설치 아이콘: `src-tauri/icons/` (Windows ICO, macOS ICNS 및 PNG)
- macOS 트레이: `design/pinchy/b-soft-v1/pinchy-tray.svg`에서 생성한 단색 템플릿

아이콘 재생성:

```bash
bun run tauri icon design/pinchy/b-soft-v1/pinchy-app-icon.png --output src-tauri/icons
bun run tauri icon design/pinchy/b-soft-v1/pinchy-tray.svg --output src-tauri/icons/tray --png 32
```

## 개발 실행

```bash
bun install
bun run tauri dev      # 개발 실행
bun run tauri build    # src-tauri/target/release/bundle/ 에 .app / .dmg 생성
```

Rust 툴체인(rustup)이 필요하다. 붙여넣기(⌘V 시뮬레이션)는 macOS 접근성 권한이 필요하며 첫 실행 때 프롬프트가 뜬다.

### Windows

필요한 것: Visual Studio Build Tools(C++ 데스크톱 워크로드), rustup, bun. 그다음 동일하게 `bun install && bun run tauri build`.
단축키는 `Ctrl+Shift+V`, 배경은 아크릴이다. Windows에서는 창 내부 높이가 음수가 되는 문제를 피하기 위해 축소 애니메이션 없이 표시·숨김 처리한다. 플랫폼별 코드는 `src-tauri/src/platform.rs` 한 파일에 모여 있다.

Mac에서 Windows 코드를 타입체크만 하려면(링크는 안 함):

```bash
rustup target add x86_64-pc-windows-gnu && brew install mingw-w64
cd src-tauri && CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar cargo check --target x86_64-pc-windows-gnu
```

## 자동 빌드·릴리스

GitHub Actions의 **Build installers and release** 워크플로가 `vX.Y.0` 태그 푸시에 실행된다 (`Y > 0`). 예: `v0.2.0`, `v0.3.0`, `v1.1.0`. 패치 태그(`v0.2.1`)에는 실행되지 않으며, 메이저 전용 태그(`v1.0.0`)는 버전 검사에서 제외한다. 버전 파일 수정이나 일반 커밋 푸시만으로는 릴리스하지 않는다.

1. `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`의 버전을 같은 값(예: `0.2.0`)으로 변경한다.
2. `cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1`로 `Cargo.lock`을 갱신한다.
3. 변경사항을 커밋하고 `main`에 푸시한 뒤, 해당 커밋에 태그를 붙여 푸시한다.

```bash
git tag v0.2.0
git push origin v0.2.0
```

태그와 네 버전 파일이 일치해야 한다. Apple Silicon DMG, Intel DMG, Windows x64 MSI·Setup EXE가 모두 성공하면 SHA256 체크섬과 함께 릴리스에 자동 게시된다. 기존 릴리스 자산은 덮어쓰지 않으며, 게시 실패로 초안이 남으면 해당 초안을 확인한 뒤 재시도한다.

버전 변경 없이 테스트하려면 GitHub **Actions → Build installers and release → Run workflow**를 실행한다. 수동 실행은 설치 파일을 Actions 아티팩트로 14일 보관하며 공개 릴리스를 만들지 않는다.

현재 macOS는 인증서가 필요 없는 임시 서명(ad-hoc)을 사용하며 Apple 공증은 하지 않는다. Windows 설치 파일도 코드 서명되지 않았다. 빌드 성공은 macOS 실기기 동작 검증을 대신하지 않는다.

## 로드맵

1. [x] macOS에서 Paste 구독 대체
2. [x] Windows 빌드 및 실기기 패널 열기·닫기 확인
3. [ ] 기기 간 동기화 (DB 서버 없이 파일 기반: iCloud Drive / Syncthing 폴더에 append-only 로그)
