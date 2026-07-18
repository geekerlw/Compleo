#!/bin/bash
# Build, sign, and install Compleo to /Applications
set -e

cd "$(dirname "$0")"

echo "Building..."
npx tauri build --debug 2>&1 | grep -E "(Finished|Built|Bundling.*\.app|error)" | head -5

echo "Killing old instance..."
pkill -9 -f "[Cc]ompleo" 2>/dev/null || true
sleep 1

echo "Installing to /Applications..."
rm -rf /Applications/Compleo.app
cp -R src-tauri/target/debug/bundle/macos/Compleo.app /Applications/

echo "Signing with fixed identifier..."
codesign --force --sign - --identifier "com.compleo.app" /Applications/Compleo.app

echo "Launching..."
open /Applications/Compleo.app

echo "✅ Done! Compleo installed and running."
