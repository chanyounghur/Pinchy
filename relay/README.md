# Pinchy Relay

여러 기기의 Pinchy가 같은 클립보드 히스토리를 쓰게 해 주는 서버입니다. 히스토리의 원본이 여기에 있고, 연결된 모든 기기는 변경 내역을 받아 똑같이 따라갑니다. 한 기기에서 복사하면 다른 모든 기기에 쌓이고, 한 기기에서 지우면 모든 기기에서 지워집니다. 보관 기간과 최대 용량도 서버가 정해 모든 기기에 적용합니다.

내용과 기기 이름은 각 기기의 Pinchy가 기기끼리만 아는 공유 키로 암호화합니다(XChaCha20-Poly1305). 서버는 암호문, 크기, 변경 시각만 알고, 공유 키는 서버를 거쳐 가더라도 서버가 풀 수 없는 형태로만 전달됩니다.

relay 1.0은 0.x의 우편함 방식과 호환되지 않습니다. 0.x 데이터 폴더로 실행하면 기기 목록은 유지하고 전달 대기 중이던 항목만 지웁니다. Pinchy 데스크톱도 함께 1.0으로 올려야 합니다.

```bash
curl https://<도메인>/health
# {"service":"pinchy-relay","status":"ok","version":"0.1.0"}
```

## 실행

[Rust](https://rustup.rs)가 필요합니다.

```bash
cargo run --release
```

도메인을 지정하지 않으면 `127.0.0.1:8787`에서 HTTP로 실행합니다. 로컬 테스트용이거나 다른 리버스 프록시 뒤에 둘 때 씁니다.

도메인을 지정하면 HTTPS로 실행하고 Let's Encrypt 인증서를 자동으로 받아 갱신합니다. TLS-ALPN-01 방식이라 **외부 443 포트가 `--listen` 포트로 연결돼 있어야** 하며, 80 포트는 필요 없습니다.

```bash
pinchy-relay --listen 0.0.0.0:8443 --domain <도메인>
```

| 옵션 | 환경 변수 | 설명 |
| --- | --- | --- |
| `--data-dir` | `PINCHY_RELAY_DATA_DIR` | 기기 목록, 히스토리, 인증서 저장 위치. 기본값 macOS `~/Library/Application Support/pinchy-relay`, 그 외 `~/.local/share/pinchy-relay` |
| `--listen` | `PINCHY_RELAY_LISTEN` | 받는 주소. 기본값 `127.0.0.1:8787` |
| `--domain` | `PINCHY_RELAY_DOMAIN` | 공개 도메인. 지정하면 HTTPS로 실행 |
| `--cache-dir` | `PINCHY_RELAY_CACHE_DIR` | 인증서와 ACME 계정 저장 위치. 기본값 데이터 폴더의 `acme` |
| `--contact` | `PINCHY_RELAY_CONTACT` | Let's Encrypt 만료 알림을 받을 이메일 |
| `--staging` | `PINCHY_RELAY_STAGING` | Let's Encrypt 테스트 환경 사용 (브라우저가 신뢰하지 않는 인증서) |
| `--retention-days` | `PINCHY_RELAY_RETENTION_DAYS` | 마지막으로 복사한 뒤 이 기간이 지난 항목을 모든 기기에서 삭제. 기본값 30일 |
| `--max-storage-mb` | `PINCHY_RELAY_MAX_STORAGE_MB` | 히스토리 최대 용량. 넘으면 오래된 항목부터 삭제. 기본값 1,024MB |
| `--max-blob-mb` | `PINCHY_RELAY_MAX_BLOB_MB` | 이미지·파일 최대 크기. 기본값 64MB. Pinchy는 파일을 한 번에 50MB까지 올리므로 그보다 크게 두세요 |
| `--sync-concealed` | `PINCHY_RELAY_SYNC_CONCEALED` | 비밀번호 관리자처럼 원본 앱이 "숨김"으로 표시한 복사도 동기화. 기본값 꺼짐. 서버는 내용을 볼 수 없으므로 각 기기가 복사 시점에 이 설정을 따릅니다 |

### 보관 정리

서버가 1분마다 보관 기간과 최대 용량을 확인해 넘치는 항목을 지우고, 지운 항목은 모든 기기에 삭제로 전파됩니다. 어떤 기기에서든 같은 내용을 다시 복사하면 그 항목의 보관 기간이 갱신됩니다. 기기는 서버 정책을 따르기만 하므로 기기별 보관 설정은 없습니다. 삭제 기록은 오랫동안 꺼져 있던 기기를 위해 90일 동안 남겨 두고, 그보다 오래 꺼져 있던 기기는 전체를 다시 받습니다.

처음에는 `--staging`과 임시 `--cache-dir`로 발급이 되는지 먼저 확인하세요. 운영 환경은 같은 도메인에서 검증에 여러 번 실패하면 한동안 발급을 막습니다. 인증서 저장 폴더를 유지하면 재시작할 때 다시 발급받지 않습니다.

## 기기 연결

서버에는 기기를 등록하는 명령이 없습니다. 기기는 모두 Pinchy 설정 → **기기 간 공유**에서 연결합니다.

1. **첫 기기**: 연결된 기기가 하나도 없으면 서버가 로그에 첫 연결 코드를 출력합니다.

   ```
   [pinchy-relay] first device code: 4821-1937-6604 (valid 30 minutes; ...)
   ```

   첫 기기에서 서버 주소와 이 코드를 입력하면 연결되고, 공유 키는 그 기기에서 만들어집니다. 코드는 한 번만 쓸 수 있고 30분 뒤 만료되며, 5번 틀리면 폐기됩니다. 연결된 기기가 없는 동안에는 만료되거나 폐기될 때마다 새 코드를 출력합니다.
2. **다른 기기**: 연결된 기기에서 **기기 추가**를 누르면 `37-4821-1937` 같은 코드가 10분 동안 표시됩니다. 새 기기에서 서버 주소와 이 코드를 입력하면 연결됩니다.

   두 기기는 이 코드로 SPAKE2 키 교환을 해서 서버가 볼 수 없는 통로로 공유 키를 넘깁니다. 앞의 두 자리만 서버가 코드를 찾는 데 쓰고, 나머지 여덟 자리는 서버로 보내지 않습니다. 새 기기가 코드를 맞게 입력했는지 연결된 기기가 확인한 다음에야 서버가 새 기기를 등록하며, 틀린 코드가 한 번이라도 들어오면 그 코드는 취소됩니다.
3. **해제**: 연결된 기기 목록에서 다른 기기를 해제하거나, **이 기기 연결 해제**로 자신을 해제합니다. 해제된 기기의 토큰은 바로 무효가 되고, 그 기기 앞으로 남은 항목도 지워집니다. 마지막 기기까지 해제하면 서버가 다시 첫 연결 코드를 출력합니다.

### 초기화

모든 기기를 잃어버렸다면 서버의 기기 목록과 히스토리를 지우고 다시 시작합니다. 남은 항목은 어차피 잃어버린 공유 키 없이는 열 수 없습니다. **`acme/` 폴더는 지우지 마세요.** 인증서를 짧은 시간에 여러 번 다시 받으면 Let's Encrypt 발급 제한에 걸립니다.

```bash
launchctl bootout gui/$(id -u)/com.chanyounghur.pinchy-relay
cd ~/Library/Application\ Support/pinchy-relay && rm -rf relay.db relay.db-wal relay.db-shm blobs
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.chanyounghur.pinchy-relay.plist
grep 'first device code' ~/Library/Application\ Support/pinchy-relay/relay.log | tail -1
```

## API

`Authorization: Bearer <기기 토큰>`이 필요한 요청과 연결 전에 쓰는 요청이 있습니다. 같은 주소에서 토큰 인증, 첫 연결 코드, 없는 페어링 코드가 합쳐서 10분 안에 20번 실패하면 10분 동안 그 주소의 요청을 거부합니다.

| 요청 | 인증 | 설명 |
| --- | --- | --- |
| `GET /health` | 없음 | 서버 상태 |
| `POST /v1/first` | 없음 | `{"code", "label"}`. 첫 연결 코드로 첫 기기를 등록하고 토큰을 받음 |
| `POST /v1/pairings` | 토큰 | `{"msg"}`. SPAKE2 첫 메시지로 페어링을 열고 두 자리 번호를 받음 |
| `GET /v1/pairings/{번호}` | 토큰 | 새 기기의 응답(`msg`, `confirm`, `name`) 확인 |
| `POST /v1/pairings/{번호}/approve` | 토큰 | `{"payload", "label"}`. 새 기기를 등록하고 봉인한 공유 키를 전달 |
| `DELETE /v1/pairings/{번호}` | 토큰 | 페어링 취소 |
| `POST /v1/pairings/{번호}/join` | 없음 | 새 기기가 페어링에 참여. 한 번만 가능 |
| `POST /v1/pairings/{번호}/answer` | 없음 | 새 기기의 SPAKE2 응답과 확인값 |
| `GET /v1/pairings/{번호}/result?ticket=` | 없음 | 승인되면 토큰과 봉인한 공유 키를 받음 |
| `GET /v1/me`, `PUT /v1/me` | 토큰 | 연결된 기기 수 확인, 이 기기의 봉인한 이름(`label`) 변경 |
| `GET /v1/devices`, `DELETE /v1/devices/{id}` | 토큰 | 기기 목록, 기기 해제 |
| `POST /v1/join-codes`, `DELETE /v1/join-codes` | 토큰 | QR용 일회용 연결 코드 발급(10분), 취소. 공유 키는 QR에만 들어가고 서버에는 오지 않음 |
| `POST /v1/join` | 없음 | `{"code", "label"}`. QR 코드로 새 기기를 등록하고 토큰을 받음 |
| `GET /v1/policy` | 토큰 | 보관 기간, 최대 용량, 파일 크기 제한, 숨김 복사 동기화 여부, 현재 사용량 |
| `PUT /v1/items/{id}` | 토큰 | `{"envelope", "has_blob"}`. 항목을 만들거나 봉인한 내용을 바꿈. `id`는 기기가 정한 32자 소문자 16진수. 지워진 항목은 410 |
| `PUT /v1/items/{id}/blob` | 토큰 | 이미지·파일 본문 업로드. 만든 기기만, 한 번만. 업로드가 끝나야 다른 기기에 보입니다 |
| `GET /v1/items/{id}/blob` | 토큰 | 이미지·파일 본문 다운로드 |
| `DELETE /v1/items/{id}` | 토큰 | 항목을 모든 기기에서 삭제 |
| `DELETE /v1/items` | 토큰 | 히스토리 전체 삭제 |
| `GET /v1/changes?since=0&wait=25&limit=200` | 토큰 | `since` 이후 바뀐 항목(삭제 포함)을 순서대로. 응답의 `next`를 다음 `since`로 쓰고, `more`가 참이면 바로 이어서 요청. 비어 있으면 최대 `wait`초(최대 30) 기다림. `since`가 너무 오래됐으면 409이며 0부터 다시 받아야 함 |

## macOS 상시 실행

```bash
./install-macos.sh <도메인> [이메일]
```

빌드한 실행 파일을 `~/.local/bin/pinchy-relay`에 설치하고, 로그인 시 시작되고 종료되면 다시 실행되는 LaunchAgent(`com.chanyounghur.pinchy-relay`)로 등록합니다. 기본으로 `0.0.0.0:8443`에서 받으며 `PINCHY_RELAY_LISTEN`으로 바꿀 수 있습니다. 보관 정책은 `PINCHY_RELAY_RETENTION_DAYS`, `PINCHY_RELAY_MAX_STORAGE_MB`, `PINCHY_RELAY_SYNC_CONCEALED=true`로 넘기면 LaunchAgent에 저장되어 다음 업데이트에도 유지됩니다.

```bash
PINCHY_RELAY_SYNC_CONCEALED=true ./install-macos.sh <도메인> [이메일]
``` 해당 포트를 다른 프로그램이 쓰고 있으면 설치를 중단합니다. 연결된 기기가 없으면 설치가 끝날 때 첫 연결 코드를 보여 줍니다.

서버 코드가 바뀌었을 때는 인자 없이 다시 실행하면 됩니다. 도메인과 이메일은 이전 설치에서 가져오고, 새로 빌드한 파일이 이전과 같으면 아무것도 바꾸지 않습니다.

```bash
~/workspace/Pinchy/relay/install-macos.sh
```

- 데이터: `~/Library/Application Support/pinchy-relay/` (`relay.db`, `blobs/`)
- 인증서: `~/Library/Application Support/pinchy-relay/acme/`
- 로그: `~/Library/Application Support/pinchy-relay/relay.log`

Rust는 rustup 기본 위치(`~/.cargo/bin`)에서 찾고, 기본 SDK가 Command Line Tools보다 새로워서 링크가 깨지는 경우(`tapi error: malformed file ... unknown architecture`) Command Line Tools 버전에 맞는 SDK를 골라 씁니다.

### 방화벽

macOS 방화벽이 켜져 있으면 `pinchy-relay`의 수신 연결을 허용해야 합니다. 허용하지 않으면 외부 연결이 TLS 핸드셰이크 단계에서 멈추고, 인증서 발급도 실패합니다. 설치 스크립트가 처음 설치할 때 한 번 허용하며, 이때 관리자 비밀번호를 묻습니다. 방화벽은 허용한 경로(`~/.local/bin/pinchy-relay`)를 다시 빌드한 뒤에도 계속 허용하므로 이후 업데이트에서는 묻지 않습니다.

### 제거

```bash
launchctl bootout gui/$(id -u)/com.chanyounghur.pinchy-relay
rm ~/Library/LaunchAgents/com.chanyounghur.pinchy-relay.plist ~/.local/bin/pinchy-relay
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --remove ~/.local/bin/pinchy-relay
```

## ipTIME 공유기 설정

관리 페이지(보통 `http://192.168.0.1`)의 **고급 설정**에서 진행합니다. 펌웨어 버전에 따라 메뉴 이름이 조금 다를 수 있습니다.

1. **서버 Mac의 내부 IP 고정**: 네트워크 관리 → 내부 네트워크 설정 → DHCP 서버 설정 → 사용 중인 IP 주소 정보에서 Mac을 선택하고 **수동 주소 등록**. IP가 바뀌면 포트포워드가 엉뚱한 기기를 가리킵니다.
2. **DDNS**: 특수기능 → DDNS 설정에서 `<이름>.iptime.org`를 등록하고 상태가 정상인지 확인합니다. 이 이름이 `--domain` 값입니다.
3. **포트포워드**: NAT/라우터 관리 → 포트포워드 설정에서 아래 규칙 하나만 추가합니다.

   | 항목 | 값 |
   | --- | --- |
   | 내부 IP | 1번에서 고정한 Mac IP |
   | 프로토콜 | TCP |
   | 외부 포트 | 443 |
   | 내부 포트 | 8443 |

   외부 포트는 443을 쓰세요. 회사 네트워크는 보통 80·443 외의 포트로 나가는 연결을 막습니다.
4. **정리**: 보안 기능 → 공유기 접속/보안 관리에서 외부 원격 관리를 끕니다. 쓰지 않는 포트포워드 규칙(예: 예전 80 포트 규칙)도 지웁니다.

공유기 안쪽 기기에서 `https://<이름>.iptime.org/health`로 확인해도 대부분 동작하지만, 실제 확인은 휴대폰 LTE·5G처럼 바깥 네트워크에서 하는 것이 정확합니다.

## 회사 네트워크에서 확인할 것

회사 Mac 브라우저에서 `https://<이름>.iptime.org/health`를 엽니다.

- JSON이 보이면 경로가 열려 있습니다.
- 접속 차단 안내 페이지가 뜨면 회사 웹 필터가 동적 DNS 도메인을 막는 것입니다. 개인 도메인을 사서 iptime.org 주소를 CNAME으로 가리키고 그 도메인으로 실행하면 대부분 통과합니다.
- 인증서 경고가 뜨면 회사 프록시가 HTTPS를 가로채 검사하는 것입니다. Pinchy는 운영체제의 프록시 설정과 신뢰하는 인증서를 따르므로 브라우저에서 열리면 대부분 Pinchy도 연결됩니다.
