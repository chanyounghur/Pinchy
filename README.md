# Pastel

Paste 앱을 대체하는 클립보드 히스토리 매니저. Tauri 2 + React.

## 기능 (1단계)

- 텍스트 / 링크 / 이미지 / 파일 복사 자동 기록 (중복은 최신으로 끌어올림)
- `⇧⌘V` 로 화면 하단 패널 열기, 검색, `←/→` 이동, `↩` 로 직전 앱에 즉시 붙여넣기
- `⌘↩` 복사만, `⌘⌫` 삭제, `esc` 닫기
- 메뉴바 트레이: 열기 / 히스토리 비우기 / 종료 (Dock 아이콘 없음)

데이터: `~/Library/Application Support/com.chanyounghur.pastel/` (`pastel.db` + `images/`)

## 개발

```bash
bun install
bun run tauri dev      # 개발 실행
bun run tauri build    # src-tauri/target/release/bundle/ 에 .app / .dmg 생성
```

Rust 툴체인(rustup)이 필요하다. 붙여넣기(⌘V 시뮬레이션)는 macOS 접근성 권한이 필요하며 첫 실행 때 프롬프트가 뜬다.

## 로드맵

1. [x] macOS에서 Paste 구독 대체
2. [ ] Windows 빌드 (코드는 cross-platform, 단축키 `Shift+Win+V` 등 조정 필요)
3. [ ] 기기 간 동기화 (DB 서버 없이 파일 기반: iCloud Drive / Syncthing 폴더에 append-only 로그)
