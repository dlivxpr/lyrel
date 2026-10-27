#!/bin/bash
# 正式 macOS 资产的只读签名门控；不导入证书、不修改钥匙串或 TCC。
set -euo pipefail

APP="${1:?usage: verify-macos-release-signing.sh <lyrel.app> <team-id>}"
TEAM_ID="${2:?expected Apple Developer Team ID is required}"
if [[ ! "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]]; then
    echo "verify-macos-release-signing: expected a 10-character Apple Developer Team ID" >&2
    exit 1
fi
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
if [[ "$BUNDLE_ID" != io.github.dlivxpr.lyrel ]]; then
    echo "verify-macos-release-signing: unexpected bundle identifier: $BUNDLE_ID" >&2
    exit 1
fi

# 验证签发链与 Team，而不是只相信 codesign 展示的 Identifier/Authority 文本。
# Apple 的 Developer ID Application 策略允许同 Team 合法换证，不钉死 cdhash。
PUBLISHER_REQUIREMENT="anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"$TEAM_ID\""
/usr/bin/codesign --verify --strict "-R=$PUBLISHER_REQUIREMENT and identifier \"io.github.dlivxpr.lyrel\"" "$APP"
/usr/bin/codesign --verify --deep --strict "$APP"
for binary in lyrel-app lyrel lyrel-updater; do
    if [[ ! -x "$APP/Contents/MacOS/$binary" ]]; then
        echo "verify-macos-release-signing: missing executable: $binary" >&2
        exit 1
    fi
    /usr/bin/codesign --verify --strict "-R=$PUBLISHER_REQUIREMENT" "$APP/Contents/MacOS/$binary"
done
/usr/bin/xcrun stapler validate "$APP"
echo "Verified Developer ID Application, Team $TEAM_ID, bundle identity and stapled notarization: $APP"
