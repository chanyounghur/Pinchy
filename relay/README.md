# Pinchy Relay

여러 컴퓨터의 Pinchy가 텍스트, 링크, 이미지를 주고받을 때 거쳐 가는 중계 서버입니다. 한 기기가 보낸 항목은 등록된 다른 모든 기기의 우편함에 들어가고, 모든 기기가 받아 가면 삭제됩니다. 받지 않은 항목은 7일 뒤 삭제됩니다.

내용은 각 기기의 Pinchy가 공유 키로 암호화해서 보냅니다(XChaCha20-Poly1305). 서버는 암호문, 크기, 기기 이름만 알고 내용은 읽을 수 없습니다. 공유 키는 서버로 보내지 않습니다.

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
| `--data-dir` | `PINCHY_RELAY_DATA_DIR` | 기기 목록, 우편함, 인증서 저장 위치. 기본값 macOS `~/Library/Application Support/pinchy-relay`, 그 외 `~/.local/share/pinchy-relay` |
| `--listen` | `PINCHY_RELAY_LISTEN` | 받는 주소. 기본값 `127.0.0.1:8787` |
| `--domain` | `PINCHY_RELAY_DOMAIN` | 공개 도메인. 지정하면 HTTPS로 실행 |
| `--cache-dir` | `PINCHY_RELAY_CACHE_DIR` | 인증서와 ACME 계정 저장 위치. 기본값 데이터 폴더의 `acme` |
| `--contact` | `PINCHY_RELAY_CONTACT` | Let's Encrypt 만료 알림을 받을 이메일 |
| `--staging` | `PINCHY_RELAY_STAGING` | Let's Encrypt 테스트 환경 사용 (브라우저가 신뢰하지 않는 인증서) |
| `--retention-days` | `PINCHY_RELAY_RETENTION_DAYS` | 받아 가지 않은 항목 보관 기간. 기본값 7일 |
| `--max-blob-mb` | `PINCHY_RELAY_MAX_BLOB_MB` | 이미지 최대 크기. 기본값 64MB |

처음에는 `--staging`과 임시 `--cache-dir`로 발급이 되는지 먼저 확인하세요. 운영 환경은 같은 도메인에서 검증에 여러 번 실패하면 한동안 발급을 막습니다. 인증서 저장 폴더를 유지하면 재시작할 때 다시 발급받지 않습니다.

## 기기 등록

Pinchy를 연결할 기기마다 토큰을 하나씩 만듭니다. 서버가 쓰는 데이터 폴더와 같은 폴더를 보도록, 서버를 실행하는 계정에서 실행하세요.

```bash
pinchy-relay add-device "회사 맥"
pinchy-relay list-devices
pinchy-relay remove-device "회사 맥"
```

토큰은 만들 때 한 번만 보여 주고, 서버에는 해시만 저장합니다. 토큰을 잃어버렸거나 기기를 더 쓰지 않으면 `remove-device`로 지우고 다시 만드세요. 지운 기기 앞으로 남은 항목도 함께 지워집니다.

각 기기의 Pinchy 설정 → **기기 간 공유**에 서버 주소, 그 기기의 토큰, 공유 키를 입력합니다. 공유 키는 첫 기기에서 **새 키 만들기**로 만들고, 나머지 기기에는 같은 키를 입력합니다. 키가 다른 기기가 보낸 항목은 열 수 없어 서버에 남아 있다가, 키를 맞추면 받습니다.

## API

`/health`를 제외한 모든 요청에 `Authorization: Bearer <기기 토큰>`이 필요합니다. 같은 주소에서 토큰 인증이 10분 안에 20번 실패하면 10분 동안 그 주소의 요청을 거부합니다.

| 요청 | 설명 |
| --- | --- |
| `GET /v1/me` | 이 기기 이름과 다른 기기 목록 |
| `POST /v1/items` | `{"id", "envelope", "has_blob"}`. 다른 모든 기기에 전달. `id`는 보내는 기기가 정한 32자 소문자 16진수 |
| `PUT /v1/items/{id}/blob` | 이미지 본문 업로드. 업로드가 끝나야 받는 기기에 보입니다 |
| `GET /v1/inbox?wait=25` | 받을 항목 목록. 비어 있으면 최대 `wait`초(최대 30) 기다렸다가 응답 |
| `GET /v1/items/{id}/blob` | 이미지 본문 다운로드 |
| `POST /v1/items/{id}/ack` | 받았음을 알림. 모든 기기가 받으면 서버에서 삭제 |

## macOS 상시 실행

```bash
./install-macos.sh <도메인> [이메일]
```

빌드한 실행 파일을 `~/.local/bin/pinchy-relay`에 설치하고, 로그인 시 시작되고 종료되면 다시 실행되는 LaunchAgent(`com.chanyounghur.pinchy-relay`)로 등록합니다. 기본으로 `0.0.0.0:8443`에서 받으며 `PINCHY_RELAY_LISTEN`으로 바꿀 수 있습니다. 해당 포트를 다른 프로그램이 쓰고 있으면 설치를 중단합니다.

- 데이터: `~/Library/Application Support/pinchy-relay/` (`relay.db`, `blobs/`)
- 인증서: `~/Library/Application Support/pinchy-relay/acme/`
- 로그: `~/Library/Application Support/pinchy-relay/relay.log`

다시 실행하면 새로 빌드한 파일로 교체합니다. 제거하려면 아래 명령을 실행하세요.

```bash
launchctl bootout gui/$(id -u)/com.chanyounghur.pinchy-relay
rm ~/Library/LaunchAgents/com.chanyounghur.pinchy-relay.plist ~/.local/bin/pinchy-relay
```

macOS 방화벽을 켜 두었다면 `pinchy-relay`의 수신 연결을 허용해야 합니다. 허용하지 않으면 외부 연결이 TLS 핸드셰이크 단계에서 멈추고, 인증서 발급도 실패합니다. 다시 설치하면 실행 파일 서명이 바뀌므로 다시 허용하세요.

```bash
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add ~/.local/bin/pinchy-relay
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp ~/.local/bin/pinchy-relay
```

빌드 중 `tapi error: malformed file ... unknown architecture` 링커 오류가 나면 Command Line Tools보다 새 SDK가 기본으로 잡힌 것입니다. 설치된 SDK 중 Command Line Tools 버전에 맞는 것을 지정하세요.

```bash
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk ./install-macos.sh <도메인>
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
