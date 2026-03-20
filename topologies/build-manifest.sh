#!/bin/sh
# Auto-generate manifest.json from all topology JSON files in this directory,
# then sync everything to web/topologies/ for WASM serving.
#
# Run after adding/removing topology files:
#   sh topologies/build-manifest.sh

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

# Find all .json files except manifest.json itself, output as JSON array
FILES=$(ls -1 *.json 2>/dev/null | grep -v '^manifest\.json$' | sort)

if [ -z "$FILES" ]; then
    echo '[]' > manifest.json
    echo "manifest.json: empty (no topology files found)"
    exit 0
fi

# Build JSON array
echo '[' > manifest.json
FIRST=true
for f in $FILES; do
    if [ "$FIRST" = true ]; then
        FIRST=false
    else
        printf ',\n' >> manifest.json
    fi
    printf '  "%s"' "$f" >> manifest.json
done
printf '\n]\n' >> manifest.json

COUNT=$(echo "$FILES" | wc -l | tr -d ' ')
echo "manifest.json: $COUNT files"

# Sync to web/topologies/ for WASM builds
WEB_DIR="$DIR/../web/topologies"
mkdir -p "$WEB_DIR"
cp manifest.json "$WEB_DIR/"
for f in $FILES; do
    cp "$f" "$WEB_DIR/"
done
echo "synced to web/topologies/"
