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

## 로드맵

1. [x] macOS에서 Paste 구독 대체
2. [x] Windows 빌드 및 실기기 패널 열기·닫기 확인
3. [ ] 기기 간 동기화 (DB 서버 없이 파일 기반: iCloud Drive / Syncthing 폴더에 append-only 로그)
