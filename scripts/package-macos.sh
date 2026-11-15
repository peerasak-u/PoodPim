#!/bin/sh
# Build dist/PoodPim-<version>-mac.zip to give to other people: PoodPim.app preset to the
# serverless backend and a Thai install guide. Neither the server URL nor the API key is in
# the zip (it may be published): give them to each person separately; the app asks for both
# the first time the talk key is pressed. Rotate the key (deploy/lambda/README.md) to cut
# everyone off.
set -eu
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
STAGE=dist/share/PoodPim
ZIP=dist/PoodPim-$VERSION-mac.zip
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# The preset: serverless backend with no server URL or key; the app asks for both.
cat > "$TMP/preset.json" <<'JSON'
{
  "backend": "lambda",
  "server_url": "",
  "api_key": "",
  "mixed_english": true,
  "restore_clipboard": true,
  "start_at_login": true
}
JSON

rm -rf dist/share "$ZIP"
APP="$STAGE/PoodPim.app" POODPIM_PRESET="$TMP/preset.json" ./scripts/bundle-macos.sh
cp scripts/share/install-th.txt "$STAGE/README-TH.txt"
ditto -c -k --keepParent "$STAGE" "$ZIP"
rm -rf dist/share
echo "Built $ZIP ($(du -h "$ZIP" | cut -f1))"
