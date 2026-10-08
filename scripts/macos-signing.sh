#!/bin/bash
# Only run on an ephemeral GitHub-hosted macOS runner.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "$(uname -s)" == Darwin ]]
: "${RUNNER_TEMP:?}"
keychain="$RUNNER_TEMP/pinchy-signing.keychain-db"
certificate="$(cd "$(dirname "$0")" && pwd)/certs/pinchy-code-signing.pem"

if [[ "${1:-}" == cleanup ]]; then
  if [[ -f "$keychain" ]]; then
    sudo security remove-trusted-cert -d "$certificate"
    security delete-keychain "$keychain"
  fi
  rm -f "$RUNNER_TEMP/pinchy-signing.p12"
  exit 0
fi

: "${MACOS_SIGNING_CERTIFICATE:?Missing signing certificate}"
: "${MACOS_SIGNING_CERTIFICATE_PASSWORD:?Missing certificate password}"
: "${GITHUB_ENV:?}"
umask 077
keychain_password=$(openssl rand -hex 32)
echo "::add-mask::$keychain_password"
printf '%s' "$MACOS_SIGNING_CERTIFICATE" | base64 --decode > "$RUNNER_TEMP/pinchy-signing.p12"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$RUNNER_TEMP/pinchy-signing.p12" -k "$keychain" \
  -P "$MACOS_SIGNING_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "$keychain_password" "$keychain" >/dev/null
# Trust only for code signing, on this disposable runner. No trust is installed
# on end-user Macs. The public certificate pins the expected release identity.
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k "$keychain" "$certificate"
security list-keychains -d user -s "$keychain" "$HOME/Library/Keychains/login.keychain-db"
identity=$(openssl x509 -in "$certificate" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')
security find-identity -v -p codesigning "$keychain" | grep -F "$identity"
printf 'APPLE_SIGNING_IDENTITY=%s\n' "$identity" >> "$GITHUB_ENV"
rm -f "$RUNNER_TEMP/pinchy-signing.p12"
