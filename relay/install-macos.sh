#!/bin/sh
# Builds pinchy-relay and runs it as a LaunchAgent that serves HTTPS for a
# public domain, starting at login and restarting if it exits.
#
#   ./install-macos.sh <domain> [contact-email]
#
# PINCHY_RELAY_LISTEN overrides the listen address (default 0.0.0.0:8443).
set -eu

if [ $# -lt 1 ]; then
  echo "usage: $0 <domain> [contact-email]" >&2
  exit 64
fi
DOMAIN=$1
CONTACT=${2:-}
LISTEN=${PINCHY_RELAY_LISTEN:-0.0.0.0:8443}
PORT=${LISTEN##*:}
LABEL=com.chanyounghur.pinchy-relay
DATA="$HOME/Library/Application Support/pinchy-relay"
BIN="$HOME/.local/bin/pinchy-relay"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN_TARGET="gui/$(id -u)"

cd "$(dirname "$0")"
cargo build --release

# Stop a previous install first so its own port does not count as taken.
launchctl bootout "$DOMAIN_TARGET/$LABEL" 2>/dev/null || true
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "port $PORT is already in use:" >&2
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >&2
  exit 1
fi

mkdir -p "$(dirname "$BIN")" "$DATA/acme" "$HOME/Library/LaunchAgents"
install -m 0755 target/release/pinchy-relay "$BIN"

CONTACT_ENV=""
if [ -n "$CONTACT" ]; then
  CONTACT_ENV="		<key>PINCHY_RELAY_CONTACT</key>
		<string>$CONTACT</string>"
fi

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>$BIN</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PINCHY_RELAY_DATA_DIR</key>
		<string>$DATA</string>
		<key>PINCHY_RELAY_LISTEN</key>
		<string>$LISTEN</string>
		<key>PINCHY_RELAY_DOMAIN</key>
		<string>$DOMAIN</string>
		<key>PINCHY_RELAY_CACHE_DIR</key>
		<string>$DATA/acme</string>
$CONTACT_ENV
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>StandardOutPath</key>
	<string>$DATA/relay.log</string>
	<key>StandardErrorPath</key>
	<string>$DATA/relay.log</string>
</dict>
</plist>
EOF

LOG="$DATA/relay.log"
LOG_START=$(wc -c < "$LOG" 2>/dev/null || echo 0)
launchctl bootstrap "$DOMAIN_TARGET" "$PLIST"
echo "pinchy-relay is serving https://$DOMAIN on $LISTEN"
echo "log: $LOG"

# While no device is connected, the relay prints a first-device code.
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  CODE=$(tail -c +$((LOG_START + 1)) "$LOG" 2>/dev/null | grep -o 'first device code: [0-9-]*' | tail -1 | cut -d' ' -f4)
  if [ -n "$CODE" ]; then
    echo
    echo "First device code: $CODE"
    echo "Enter it in Pinchy (Settings > 기기 간 공유) on your first device within 30 minutes."
    echo "If it expires, the relay prints a new one to the log."
    exit 0
  fi
done
echo "No first device code was printed, so a device is probably already connected."
echo "Add more devices from a connected device's settings (기기 추가)."
