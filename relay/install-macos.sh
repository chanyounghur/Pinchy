#!/bin/sh
# Builds pinchy-relay and runs it as a LaunchAgent that serves HTTPS for a
# public domain, starting at login and restarting if it exits.
#
#   ./install-macos.sh [domain] [contact-email]
#
# The domain and contact are remembered from the previous install, so later
# updates need no arguments, and a build identical to the installed one
# leaves the running relay alone. PINCHY_RELAY_LISTEN overrides the listen
# address (default 0.0.0.0:8443). PINCHY_RELAY_RETENTION_DAYS,
# PINCHY_RELAY_MAX_STORAGE_MB and PINCHY_RELAY_SYNC_CONCEALED (true/false)
# set the policy and are remembered the same way.
set -eu

LABEL=com.chanyounghur.pinchy-relay
DATA="$HOME/Library/Application Support/pinchy-relay"
BIN="$HOME/.local/bin/pinchy-relay"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN_TARGET="gui/$(id -u)"
LOG="$DATA/relay.log"
FIREWALL=/usr/libexec/ApplicationFirewall/socketfilterfw
FIREWALL_MARK="$DATA/firewall.allowed"
BUILD_MARK="$DATA/installed.sha256"

saved() {
  [ -f "$PLIST" ] && plutil -extract "EnvironmentVariables.$1" raw "$PLIST" 2>/dev/null || true
}

DOMAIN=${1:-$(saved PINCHY_RELAY_DOMAIN)}
CONTACT=${2:-$(saved PINCHY_RELAY_CONTACT)}
LISTEN=${PINCHY_RELAY_LISTEN:-$(saved PINCHY_RELAY_LISTEN)}
LISTEN=${LISTEN:-0.0.0.0:8443}
PORT=${LISTEN##*:}
RETENTION=${PINCHY_RELAY_RETENTION_DAYS:-$(saved PINCHY_RELAY_RETENTION_DAYS)}
MAX_STORAGE=${PINCHY_RELAY_MAX_STORAGE_MB:-$(saved PINCHY_RELAY_MAX_STORAGE_MB)}
SYNC_CONCEALED=${PINCHY_RELAY_SYNC_CONCEALED:-$(saved PINCHY_RELAY_SYNC_CONCEALED)}
case "${SYNC_CONCEALED:-}" in
  ""|true|false) ;;
  *) echo "PINCHY_RELAY_SYNC_CONCEALED must be true or false" >&2; exit 64 ;;
esac
if [ -z "$DOMAIN" ]; then
  echo "usage: $0 <domain> [contact-email]" >&2
  exit 64
fi

# Toolchain: rustup's default location, and the SDK that matches the
# Command Line Tools when a newer SDK is the default and breaks linking.
if [ -d "$HOME/.cargo/bin" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
fi
if [ -z "${SDKROOT:-}" ] && [ "$(xcode-select -p 2>/dev/null)" = /Library/Developer/CommandLineTools ]; then
  CLT_MAJOR=$(pkgutil --pkg-info=com.apple.pkg.CLTools_Executables 2>/dev/null | awk '/^version:/ { split($2, v, "."); print v[1] }')
  if [ -n "$CLT_MAJOR" ] && [ -d "/Library/Developer/CommandLineTools/SDKs/MacOSX$CLT_MAJOR.sdk" ]; then
    export SDKROOT="/Library/Developer/CommandLineTools/SDKs/MacOSX$CLT_MAJOR.sdk"
  fi
fi
command -v cargo >/dev/null || { echo "cargo not found; install Rust from https://rustup.rs" >&2; exit 1; }

cd "$(dirname "$0")"
cargo build --release
BUILD=$(shasum -a 256 target/release/pinchy-relay | cut -d' ' -f1)

mkdir -p "$(dirname "$BIN")" "$DATA/acme" "$HOME/Library/LaunchAgents"
chmod 700 "$DATA"

UNCHANGED=false
if [ -f "$BUILD_MARK" ] && [ "$(cat "$BUILD_MARK")" = "$BUILD" ] && [ -x "$BIN" ] &&
  [ "$(saved PINCHY_RELAY_DOMAIN)" = "$DOMAIN" ] && [ "$(saved PINCHY_RELAY_CONTACT)" = "$CONTACT" ] &&
  [ "$(saved PINCHY_RELAY_LISTEN)" = "$LISTEN" ] && [ "$(saved PINCHY_RELAY_RETENTION_DAYS)" = "$RETENTION" ] &&
  [ "$(saved PINCHY_RELAY_MAX_STORAGE_MB)" = "$MAX_STORAGE" ] &&
  [ "$(saved PINCHY_RELAY_SYNC_CONCEALED)" = "$SYNC_CONCEALED" ] &&
  launchctl print "$DOMAIN_TARGET/$LABEL" >/dev/null 2>&1; then
  UNCHANGED=true
fi

if [ "$UNCHANGED" = false ]; then
  STAGED="$BIN.new"
  install -m 0755 target/release/pinchy-relay "$STAGED"

  # Stop a previous install first so its own port does not count as taken.
  launchctl bootout "$DOMAIN_TARGET/$LABEL" 2>/dev/null || true
  if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    rm -f "$STAGED"
    echo "port $PORT is already in use:" >&2
    lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >&2
    exit 1
  fi
  mv "$STAGED" "$BIN"

  # Optional settings go into the plist only when set.
  optional_env() {
    [ -n "$2" ] && printf '\t\t<key>%s</key>\n\t\t<string>%s</string>\n' "$1" "$2" || true
  }
  CONTACT_ENV=$(optional_env PINCHY_RELAY_CONTACT "$CONTACT")
  POLICY_ENV=$(optional_env PINCHY_RELAY_RETENTION_DAYS "$RETENTION"
    optional_env PINCHY_RELAY_MAX_STORAGE_MB "$MAX_STORAGE"
    optional_env PINCHY_RELAY_SYNC_CONCEALED "$SYNC_CONCEALED")

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
$POLICY_ENV
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>StandardOutPath</key>
	<string>$LOG</string>
	<key>StandardErrorPath</key>
	<string>$LOG</string>
</dict>
</plist>
EOF
fi

LOG_START=0
[ -f "$LOG" ] && LOG_START=$(wc -c < "$LOG")
if [ "$UNCHANGED" = false ]; then
  launchctl bootstrap "$DOMAIN_TARGET" "$PLIST"
  printf '%s\n' "$BUILD" > "$BUILD_MARK"
fi

# The firewall keeps allowing the same path after the binary is rebuilt, so
# ask for an administrator password only the first time. This runs after the
# relay is back up so a pending password prompt causes no downtime.
if "$FIREWALL" --getglobalstate | grep -q 'enabled'; then
  if [ "$(cat "$FIREWALL_MARK" 2>/dev/null)" != "$BIN" ]; then
    echo "Allowing incoming connections to $BIN in the macOS firewall (administrator password required)."
    sudo "$FIREWALL" --add "$BIN" >/dev/null
    sudo "$FIREWALL" --unblockapp "$BIN" >/dev/null
    printf '%s\n' "$BIN" > "$FIREWALL_MARK"
  fi
fi

if [ "$UNCHANGED" = true ]; then
  echo "pinchy-relay is already up to date and running."
  exit 0
fi
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
