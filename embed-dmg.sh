#!/bin/zsh
# Post-build script: Embed DMG into the app bundle

APP_PATH="src-tauri/target/release/bundle/macos/macOS Backup Suite.app"
DMG_SOURCE="src-tauri/target/release/bundle/dmg/macOS Backup Suite_1.0.0_aarch64.dmg"
DMG_DEST="$APP_PATH/Contents/Resources/macOS Backup Suite.dmg"

if [[ -f "$DMG_SOURCE" ]] && [[ -d "$APP_PATH" ]]; then
    cp "$DMG_SOURCE" "$DMG_DEST"
    echo "✅ DMG eingebettet in App-Bundle"
else
    echo "❌ DMG oder App nicht gefunden"
fi
