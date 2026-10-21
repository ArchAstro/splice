#!/bin/sh
# Builds Splice.app from a compiled binary.
#
#   scripts/bundle-macos.sh <splice binary> <output folder> [version]
#
# The bundle is ad-hoc signed, which is enough to run it on the machine that
# built it and on any Mac once quarantine is cleared. Signing with a Developer
# ID and notarizing happen in the release workflow.
set -eu

binary="$1"
out="$2"
repo="$(cd "$(dirname "$0")/.." && pwd)"
version="${3:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo/Cargo.toml" | head -1)}"
app="$out/Splice.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/splice"
chmod 755 "$app/Contents/MacOS/splice"
cp -R "$repo/skill" "$app/Contents/Resources/skill"
cp "$repo/LICENSE" "$app/Contents/Resources/LICENSE"

# The icon in every size macOS asks for.
iconset="$(mktemp -d)/splice.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    double=$((size * 2))
    sips -z "$size" "$size" "$repo/assets/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z "$double" "$double" "$repo/assets/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/splice.icns"
rm -rf "$(dirname "$iconset")"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Splice</string>
    <key>CFBundleDisplayName</key>
    <string>Splice</string>
    <key>CFBundleIdentifier</key>
    <string>ai.archastro.splice</string>
    <key>CFBundleExecutable</key>
    <string>splice</string>
    <key>CFBundleIconFile</key>
    <string>splice</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.video</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>© ArchAstro. MIT license.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Splice records your microphone with a screen recording when you turn Microphone on.</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Splice project</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>LSHandlerRank</key>
            <string>Owner</string>
            <key>CFBundleTypeIconFile</key>
            <string>splice</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>ai.archastro.splice.project</string>
            </array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Media</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.movie</string>
                <string>public.image</string>
                <string>public.audio</string>
            </array>
        </dict>
    </array>
    <key>UTExportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>ai.archastro.splice.project</string>
            <key>UTTypeDescription</key>
            <string>Splice project</string>
            <key>UTTypeIconFile</key>
            <string>splice</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.data</string>
                <string>public.database</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>splice</string>
                </array>
            </dict>
        </dict>
    </array>
</dict>
</plist>
PLIST

plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --sign - "$app" 2>/dev/null
echo "$app"
