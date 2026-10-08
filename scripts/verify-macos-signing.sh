#!/bin/bash
set -euo pipefail
app=${1:?Pass the built .app path}
certificate="$(cd "$(dirname "$0")" && pwd)/certs/pinchy-code-signing.pem"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
codesign --verify --deep --strict --verbose=2 "$app"
codesign --display --extract-certificates "$tmp/cert" "$app"
openssl x509 -in "$certificate" -outform DER -out "$tmp/expected.der"
cmp "$tmp/expected.der" "$tmp/cert0"
codesign --display --requirements - "$app"
echo 'Verified app signature uses the pinned Pinchy certificate.'
