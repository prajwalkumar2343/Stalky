#!/bin/bash
# Build AuraBot for local testing or Developer ID distribution.

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}🔨 AuraBot Build Script${NC}"
echo "========================"

APP_NAME="AuraBot"
VERSION="1.0.0"
BUNDLE_ID="com.yourname.aurabot"
BUILD_MODE="${AURABOT_BUILD_MODE:-dev}"
SIGNING_IDENTITY="${DEVELOPER_ID_APPLICATION:-}"
NOTARIZE="${AURABOT_NOTARIZE:-0}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"
NOTARY_APPLE_ID="${NOTARY_APPLE_ID:-}"
NOTARY_PASSWORD="${NOTARY_PASSWORD:-}"
NOTARY_TEAM_ID="${NOTARY_TEAM_ID:-}"
APP_BUNDLE="${APP_NAME}.app"
DMG_NAME="${APP_NAME}-${VERSION}.dmg"
RW_DMG_NAME="${APP_NAME}-${VERSION}-temp.dmg"
MEMORY_SERVICE_DIR="../../services/memory-pglite"
STAGING_DIR="$(mktemp -d /tmp/aurabot-dmg.XXXXXX)"
DMG_DEVICE=""
DMG_MOUNT_POINT=""

usage() {
    cat <<EOF
Usage: AURABOT_BUILD_MODE=dev|release ./scripts/build-app.sh

Environment:
  AURABOT_BUILD_MODE        dev (default) or release
  DEVELOPER_ID_APPLICATION Developer ID Application signing identity for release
  AURABOT_NOTARIZE         1 to submit the release DMG to Apple notary service
  NOTARY_PROFILE           notarytool keychain profile name, preferred
  NOTARY_APPLE_ID          Apple ID for notarytool fallback credentials
  NOTARY_PASSWORD          App-specific password for notarytool fallback credentials
  NOTARY_TEAM_ID           Apple Developer Team ID for notarytool fallback credentials
EOF
}

for ARG in "$@"; do
    case "${ARG}" in
        --release)
            BUILD_MODE="release"
            ;;
        --dev)
            BUILD_MODE="dev"
            ;;
        --notarize)
            NOTARIZE="1"
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo -e "${RED}Error: unknown argument ${ARG}${NC}"
            usage
            exit 1
            ;;
    esac
done

if [ "${BUILD_MODE}" != "dev" ] && [ "${BUILD_MODE}" != "release" ]; then
    echo -e "${RED}Error: AURABOT_BUILD_MODE must be dev or release${NC}"
    exit 1
fi

if [ "${BUILD_MODE}" = "release" ] && [ -z "${SIGNING_IDENTITY}" ]; then
    echo -e "${RED}Error: release builds require DEVELOPER_ID_APPLICATION${NC}"
    echo "Example: DEVELOPER_ID_APPLICATION='Developer ID Application: Example, Inc. (TEAMID)' AURABOT_BUILD_MODE=release ./scripts/build-app.sh"
    exit 1
fi

if [ "${BUILD_MODE}" = "release" ]; then
    if ! security find-identity -v -p codesigning | grep -F "${SIGNING_IDENTITY}" >/dev/null; then
        echo -e "${RED}Error: Developer ID signing identity was not found or is not valid${NC}"
        security find-identity -v -p codesigning || true
        exit 1
    fi
fi

# Check directory
if [ ! -f "Package.swift" ]; then
    echo -e "${RED}Error: Run this from apps/macos directory${NC}"
    exit 1
fi

if [ ! -f "${MEMORY_SERVICE_DIR}/package.json" ]; then
    echo -e "${RED}Error: Memory PGlite service not found at ${MEMORY_SERVICE_DIR}${NC}"
    exit 1
fi

if ! command -v node >/dev/null 2>&1; then
    echo -e "${RED}Error: node is required on the packaging machine so it can be bundled into the app${NC}"
    exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
    echo -e "${RED}Error: npm is required on the packaging machine to build the memory service${NC}"
    exit 1
fi

if [ "${NOTARIZE}" = "1" ] && ! command -v xcrun >/dev/null 2>&1; then
    echo -e "${RED}Error: xcrun is required for notarization${NC}"
    exit 1
fi

# Clean
echo -e "${YELLOW}🧹 Cleaning...${NC}"
rm -rf "${APP_BUNDLE}" "${APP_NAME}-${VERSION}.zip" "${DMG_NAME}" "${RW_DMG_NAME}"
swift package clean

cleanup() {
    if [ -n "${DMG_DEVICE}" ]; then
        hdiutil detach "${DMG_DEVICE}" >/dev/null 2>&1 || true
    fi
    rm -rf "${STAGING_DIR}"
}
trap cleanup EXIT

# Build memory backend
echo -e "${YELLOW}🧠 Building memory backend...${NC}"
(cd "${MEMORY_SERVICE_DIR}" && npm install && npm run build)

# Build
echo -e "${YELLOW}🔨 Building...${NC}"
swift build -c release

# Create bundle
echo -e "${YELLOW}📦 Creating app bundle...${NC}"
mkdir -p "${APP_BUNDLE}/Contents/MacOS"
mkdir -p "${APP_BUNDLE}/Contents/Resources"

cp ".build/release/${APP_NAME}" "${APP_BUNDLE}/Contents/MacOS/"
chmod +x "${APP_BUNDLE}/Contents/MacOS/${APP_NAME}"

# Copy SwiftPM resource bundles used by Bundle.module.
find ".build/release" -maxdepth 1 -name "*.bundle" -exec cp -R {} "${APP_BUNDLE}/" \;
find ".build/release" -maxdepth 1 -name "*.bundle" -exec cp -R {} "${APP_BUNDLE}/Contents/Resources/" \;

# Copy resources to normal app locations so runtime code does not need to
# touch SwiftPM's generated Bundle.module accessor.
mkdir -p "${APP_BUNDLE}/Contents/Resources/BrowserExtension"
cp -R "BrowserExtension/chromium" "${APP_BUNDLE}/Contents/Resources/BrowserExtension/"

# Bundle the Memory PGlite service and a node executable for normal app launches.
MEMORY_BUNDLE_DIR="${APP_BUNDLE}/Contents/Resources/MemoryPglite"
mkdir -p "${MEMORY_BUNDLE_DIR}/node/bin"
cp -R "${MEMORY_SERVICE_DIR}/dist" "${MEMORY_BUNDLE_DIR}/"
cp -R "${MEMORY_SERVICE_DIR}/node_modules" "${MEMORY_BUNDLE_DIR}/"
cp "${MEMORY_SERVICE_DIR}/package.json" "${MEMORY_BUNDLE_DIR}/"
cp "${MEMORY_SERVICE_DIR}/package-lock.json" "${MEMORY_BUNDLE_DIR}/"
if [ -d "${MEMORY_SERVICE_DIR}/templates" ]; then
    cp -R "${MEMORY_SERVICE_DIR}/templates" "${MEMORY_BUNDLE_DIR}/"
fi
NODE_BIN="$(command -v node)"
cp "${NODE_BIN}" "${MEMORY_BUNDLE_DIR}/node/bin/node"
chmod +x "${MEMORY_BUNDLE_DIR}/node/bin/node"

# Homebrew Node can be a small executable that resolves libnode via @rpath.
# Include the adjacent library when present so the bundled backend can launch.
mkdir -p "${MEMORY_BUNDLE_DIR}/node/lib"
NODE_REALPATH="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "${NODE_BIN}")"
for NODE_LIB_CANDIDATE in \
    "$(dirname "$(dirname "${NODE_REALPATH}")")/lib"/libnode*.dylib \
    "$(dirname "$(dirname "${NODE_BIN}")")/lib"/libnode*.dylib
do
    if [ -f "${NODE_LIB_CANDIDATE}" ]; then
        rm -f "${MEMORY_BUNDLE_DIR}/node/lib/$(basename "${NODE_LIB_CANDIDATE}")"
        cp -f "${NODE_LIB_CANDIDATE}" "${MEMORY_BUNDLE_DIR}/node/lib/"
    fi
done

# Info.plist
cat > "${APP_BUNDLE}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2024</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>Aura uses browser automation only to read the current tab title and URL when browser extension context is unavailable.</string>
    <key>NSAudioCaptureUsageDescription</key>
    <string>Aura uses system audio capture only when you choose screen and audio context features.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Aura uses microphone access only when you choose voice input or meeting features.</string>
</dict>
</plist>
EOF

echo "APPL????" > "${APP_BUNDLE}/Contents/PkgInfo"

sign_code() {
    local path="$1"
    local identifier="${2:-}"
    shift 2 || true
    local extra_args=("$@")
    local identity="-"
    local requirement_args=(--requirements "=designated => identifier \"${BUNDLE_ID}\"")
    local release_args=()

    if [ "${BUILD_MODE}" = "release" ]; then
        identity="${SIGNING_IDENTITY}"
        requirement_args=()
        release_args=(--options runtime --timestamp)
    fi

    local identifier_args=()
    if [ -n "${identifier}" ]; then
        identifier_args=(-i "${identifier}")
    fi

    codesign \
        --force \
        --sign "${identity}" \
        "${release_args[@]}" \
        "${requirement_args[@]}" \
        "${identifier_args[@]}" \
        "${extra_args[@]}" \
        "${path}"
}

is_macho_file() {
    file "$1" | grep -E 'Mach-O|dynamically linked shared library' >/dev/null
}

# Sign nested code before the containing app. Apple recommends signing each
# code item directly instead of relying on --deep as the signing strategy.
echo -e "${YELLOW}🔏 Signing nested code...${NC}"
while IFS= read -r -d '' CANDIDATE; do
    if [ "${CANDIDATE}" = "${APP_BUNDLE}/Contents/MacOS/${APP_NAME}" ]; then
        continue
    fi

    if is_macho_file "${CANDIDATE}"; then
        RELATIVE_PATH="${CANDIDATE#${APP_BUNDLE}/Contents/}"
        CODE_IDENTIFIER="${BUNDLE_ID}.$(echo "${RELATIVE_PATH}" | tr '/ _' '...')"
        sign_code "${CANDIDATE}" "${CODE_IDENTIFIER}"
    fi
done < <(find "${APP_BUNDLE}/Contents" -type f -print0)

echo -e "${YELLOW}🔏 Signing app bundle...${NC}"
sign_code "${APP_BUNDLE}/Contents/MacOS/${APP_NAME}" "${BUNDLE_ID}" --entitlements AuraBot.entitlements
sign_code "${APP_BUNDLE}" "" --entitlements AuraBot.entitlements
codesign --verify --strict --deep "${APP_BUNDLE}"

# Zip it
echo -e "${YELLOW}📦 Creating zip...${NC}"
zip -qr "${APP_NAME}-${VERSION}.zip" "${APP_BUNDLE}"

# DMG staging
echo -e "${YELLOW}💿 Creating DMG...${NC}"
cp -R "${APP_BUNDLE}" "${STAGING_DIR}/"
ln -s /Applications "${STAGING_DIR}/Applications"

hdiutil create \
  -srcfolder "${STAGING_DIR}" \
  -volname "${APP_NAME}" \
  -fs HFS+ \
  -fsargs "-c c=64,a=16,e=16" \
  -format UDRW \
  "${RW_DMG_NAME}"

MOUNT_OUTPUT=$(hdiutil attach -readwrite -noverify -noautoopen "${RW_DMG_NAME}")
DMG_DEVICE=$(echo "${MOUNT_OUTPUT}" | awk '$2 == "Apple_HFS" { print $1; exit }')
DMG_MOUNT_POINT=$(echo "${MOUNT_OUTPUT}" | awk '$2 == "Apple_HFS" { $1 = ""; $2 = ""; sub(/^[ \t]+/, ""); print; exit }')

if [ -z "${DMG_DEVICE}" ] || [ -z "${DMG_MOUNT_POINT}" ]; then
    echo -e "${RED}Error: failed to mount temporary DMG${NC}"
    exit 1
fi

bless --folder "${DMG_MOUNT_POINT}" --openfolder "${DMG_MOUNT_POINT}" >/dev/null 2>&1 || true

osascript <<EOF
tell application "Finder"
    tell disk "$(basename "${DMG_MOUNT_POINT}")"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {120, 120, 920, 600}
        set theViewOptions to the icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 180
        set text size of theViewOptions to 16
        set position of item "${APP_BUNDLE}" of container window to {220, 260}
        set position of item "Applications" of container window to {610, 260}
        update without registering applications
        delay 1
        close
    end tell
end tell
EOF

chmod -Rf go-w "${DMG_MOUNT_POINT}" || true
sync
hdiutil detach "${DMG_DEVICE}"
DMG_DEVICE=""
DMG_MOUNT_POINT=""
hdiutil convert "${RW_DMG_NAME}" -format UDZO -imagekey zlib-level=9 -o "${DMG_NAME}"
rm -f "${RW_DMG_NAME}"

if [ "${BUILD_MODE}" = "release" ]; then
    echo -e "${YELLOW}🔏 Signing DMG...${NC}"
    codesign --force --sign "${SIGNING_IDENTITY}" --timestamp "${DMG_NAME}"
fi

if [ "${NOTARIZE}" = "1" ]; then
    if [ "${BUILD_MODE}" != "release" ]; then
        echo -e "${RED}Error: notarization requires AURABOT_BUILD_MODE=release${NC}"
        exit 1
    fi

    echo -e "${YELLOW}📨 Submitting DMG for notarization...${NC}"
    if [ -n "${NOTARY_PROFILE}" ]; then
        xcrun notarytool submit "${DMG_NAME}" --keychain-profile "${NOTARY_PROFILE}" --wait
    elif [ -n "${NOTARY_APPLE_ID}" ] && [ -n "${NOTARY_PASSWORD}" ] && [ -n "${NOTARY_TEAM_ID}" ]; then
        xcrun notarytool submit "${DMG_NAME}" \
            --apple-id "${NOTARY_APPLE_ID}" \
            --password "${NOTARY_PASSWORD}" \
            --team-id "${NOTARY_TEAM_ID}" \
            --wait
    else
        echo -e "${RED}Error: notarization requires NOTARY_PROFILE or NOTARY_APPLE_ID/NOTARY_PASSWORD/NOTARY_TEAM_ID${NC}"
        exit 1
    fi

    echo -e "${YELLOW}📎 Stapling notarization ticket...${NC}"
    xcrun stapler staple "${DMG_NAME}"
    spctl -a -vv -t open --context context:primary-signature "${DMG_NAME}"
fi

if [ "${BUILD_MODE}" = "release" ]; then
    spctl -a -vv "${APP_BUNDLE}"
fi

echo ""
echo -e "${GREEN}✅ Done!${NC}"
echo ""
echo "Files created:"
echo "  - ${APP_BUNDLE}/ (test locally)"
echo "  - ${APP_NAME}-${VERSION}.zip (distribute this)"
echo "  - ${DMG_NAME} (drag to Applications)"
echo ""
echo "Test: open '${APP_BUNDLE}'"
