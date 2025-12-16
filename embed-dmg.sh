#!/bin/zsh
# Post-build script: Embed DMG into the app bundle and set custom icon

APP_PATH="src-tauri/target/release/bundle/macos/macOS Backup Suite.app"
DMG_SOURCE="src-tauri/target/release/bundle/dmg/macOS Backup Suite_1.0.0_aarch64.dmg"
DMG_DEST="$APP_PATH/Contents/Resources/macOS Backup Suite.dmg"
ICON_PATH="src-tauri/icons/icon.icns"

if [[ -f "$DMG_SOURCE" ]] && [[ -d "$APP_PATH" ]]; then
    cp "$DMG_SOURCE" "$DMG_DEST"
    echo "✅ DMG eingebettet in App-Bundle"
    
    # Set custom icon on DMG file
    if [[ -f "$ICON_PATH" ]]; then
        # Create a temporary directory for the icon resource
        TEMP_DIR=$(mktemp -d)
        TEMP_RSRC="$TEMP_DIR/icon.rsrc"
        
        # Convert icns to resource fork format using sips and DeRez/Rez
        # First, we use fileicon if available, otherwise use a manual approach
        if command -v fileicon &> /dev/null; then
            fileicon set "$DMG_SOURCE" "$ICON_PATH"
            fileicon set "$DMG_DEST" "$ICON_PATH"
            echo "✅ Icon zugewiesen (via fileicon)"
        else
            # Manual approach using Python and macOS APIs
            python3 << EOF
import Cocoa
import os

def set_icon(file_path, icon_path):
    workspace = Cocoa.NSWorkspace.sharedWorkspace()
    icon = Cocoa.NSImage.alloc().initWithContentsOfFile_(icon_path)
    if icon:
        success = workspace.setIcon_forFile_options_(icon, file_path, 0)
        return success
    return False

icon_path = "$ICON_PATH"
dmg_source = "$DMG_SOURCE"
dmg_dest = "$DMG_DEST"

if set_icon(dmg_source, icon_path):
    print("✅ Icon zugewiesen an DMG")
else:
    print("⚠️ Icon konnte nicht zugewiesen werden an DMG")

if set_icon(dmg_dest, icon_path):
    print("✅ Icon zugewiesen an eingebettetes DMG")
else:
    print("⚠️ Icon konnte nicht zugewiesen werden an eingebettetes DMG")
EOF
        fi
        
        rm -rf "$TEMP_DIR"
    else
        echo "⚠️ Icon nicht gefunden: $ICON_PATH"
    fi
else
    echo "❌ DMG oder App nicht gefunden"
fi
