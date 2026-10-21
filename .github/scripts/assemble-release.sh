#!/bin/bash
# 在仓库根目录运行：bash .github/scripts/assemble-release.sh <tag> <asset-dir>
# 需要 gh、jq、Bash 与 GNU coreutils；只组装稳定草稿，不发布或覆盖已有资产。
# GITHUB_REPOSITORY 指定目标仓库；DRAFT_RELEASE_ID 可显式恢复已有草稿。
set -euo pipefail

if [[ $# != 2 ]]; then
  echo "usage: assemble-release.sh <tag> <asset-dir>" >&2
  exit 1
fi
TAG="$1"
[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "::error::expected an exact stable version tag"; exit 1;
}
VERSION="${TAG#v}"
REPOSITORY="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
ASSET_DIR="$(cd -- "$2" && pwd)"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf -- "$SCRATCH"' EXIT

EXPECTED=(
  "lyrel-$VERSION-macos-arm64.dmg"
  "lyrel-$VERSION-macos-arm64.zip"
  "lyrel-$VERSION-windows-x86_64-setup.exe"
  "lyrel-$VERSION-windows-x86_64.zip"
  lyrel-server-linux-aarch64-musl
  lyrel-server-linux-x86_64-musl
  lyrel-server-macos-aarch64
  lyrel-server-windows-x86_64.zip
)
printf '%s\n' "${EXPECTED[@]}" | LC_ALL=C sort > "$SCRATCH/expected-assets.txt"
# checksums.txt 由同一入口重新生成，允许安全重跑本次组装。
find "$ASSET_DIR" -mindepth 1 -maxdepth 1 ! -name checksums.txt -printf '%f\n' \
  | LC_ALL=C sort > "$SCRATCH/actual-assets.txt"
diff -u "$SCRATCH/expected-assets.txt" "$SCRATCH/actual-assets.txt"
for NAME in "${EXPECTED[@]}"; do
  [[ -f "$ASSET_DIR/$NAME" && ! -L "$ASSET_DIR/$NAME" && -s "$ASSET_DIR/$NAME" ]] || {
    echo "::error::missing or empty regular release asset: $NAME"; exit 1;
  }
done
bash "$SCRIPT_DIR/generate-checksums.sh" "$ASSET_DIR"
printf '%s\n' checksums.txt >> "$SCRATCH/expected-assets.txt"
LC_ALL=C sort -o "$SCRATCH/expected-assets.txt" "$SCRATCH/expected-assets.txt"

# 验证远端标签并解析实际提交；target_commitish 使用 commit SHA，不传标签名。
TAG_OBJECT="$(gh api "repos/$REPOSITORY/git/ref/tags/$TAG" | jq -e '.object')"
for ((DEPTH = 0; DEPTH < 10; DEPTH++)); do
  [[ "$(jq -r '.type' <<<"$TAG_OBJECT")" == tag ]] || break
  TAG_SHA="$(jq -er '.sha | select(type == "string" and test("^[0-9a-f]{40}$"))' <<<"$TAG_OBJECT")"
  TAG_OBJECT="$(gh api "repos/$REPOSITORY/git/tags/$TAG_SHA" | jq -e '.object')"
done
SOURCE_SHA="$(jq -er 'select(.type == "commit") | .sha | select(type == "string" and test("^[0-9a-f]{40}$"))' <<<"$TAG_OBJECT")" || {
  echo "::error::release tag must resolve to a commit"; exit 1;
}
if [[ -n "${DRAFT_RELEASE_ID:-}" ]]; then
  [[ "$DRAFT_RELEASE_ID" =~ ^[1-9][0-9]*$ ]] || {
    echo "::error::draft release ID must be a positive integer"; exit 1;
  }
  RELEASE="$(gh api "repos/$REPOSITORY/releases/$DRAFT_RELEASE_ID")"
else
  MATCHES="$(gh api --paginate --slurp "repos/$REPOSITORY/releases?per_page=100" \
    | jq -c --arg tag "$TAG" '[.[][] | select(.tag_name == $tag)]')"
  case "$(jq length <<<"$MATCHES")" in
    0)
      # 使用创建响应中的对象和 ID；不依赖列表端点立即看到新草稿。
      RELEASE="$(gh api --method POST "repos/$REPOSITORY/releases" \
        -f tag_name="$TAG" -f target_commitish="$SOURCE_SHA" -f name="$TAG" \
        -f body='' -F draft=true -F prerelease=false)"
      ;;
    1) RELEASE="$(jq -c '.[0]' <<<"$MATCHES")" ;;
    *) echo "::error::multiple releases have the requested tag"; exit 1 ;;
  esac
fi

require_draft() {
  jq -e --arg tag "$TAG" \
    '.draft == true and .prerelease == false and .tag_name == $tag and (.id | type == "number")' \
    <<<"$1" >/dev/null || {
    echo "::error::expected the exact stable draft; published releases must never be changed"; exit 1;
  }
}
require_draft "$RELEASE"
RELEASE_ID="$(jq -r .id <<<"$RELEASE")"
RELEASE_API="repos/$REPOSITORY/releases/$RELEASE_ID"
echo "Assembling stable draft release ID: $RELEASE_ID"

# 固定 release ID，每次上传前重查 draft；不使用 --clobber、PATCH 或 DELETE。
# 已有资产必须是相同字节；缺少服务端 digest 时停止，不能猜测内容相同。
ASSETS="$(gh api --paginate --slurp "$RELEASE_API/assets?per_page=100" | jq 'add')"
for NAME in "${EXPECTED[@]}" checksums.txt; do
  FILE="$ASSET_DIR/$NAME"
  RELEASE="$(gh api "$RELEASE_API")"
  require_draft "$RELEASE"
  [[ "$(jq -r .id <<<"$RELEASE")" == "$RELEASE_ID" ]] || {
    echo "::error::release ID changed"; exit 1;
  }
  EXISTING="$(jq -c --arg name "$NAME" '[.[] | select(.name == $name)]' <<<"$ASSETS")"
  if [[ "$(jq length <<<"$EXISTING")" != 0 ]]; then
    DIGEST="sha256:$(sha256sum "$FILE" | cut -d ' ' -f1)"
    jq -e --arg digest "$DIGEST" \
      'length == 1 and .[0].state == "uploaded" and .[0].digest == $digest' \
      <<<"$EXISTING" >/dev/null || {
      echo "::error::existing draft asset differs or has no verifiable digest: $NAME"; exit 1;
    }
    echo "Reusing identical draft asset: $NAME"
    continue
  fi
  gh api --method POST \
    "https://uploads.github.com/repos/$REPOSITORY/releases/$RELEASE_ID/assets?name=$NAME" \
    -H 'Content-Type: application/octet-stream' --input "$FILE" >/dev/null
  echo "Uploaded draft asset: $NAME"
done

# 远端必须恰好九件资产，且逐件 SHA-256 与本次产物一致。
require_draft "$(gh api "$RELEASE_API")"
ASSETS="$(gh api --paginate --slurp "$RELEASE_API/assets?per_page=100" | jq 'add')"
jq -r '.[].name' <<<"$ASSETS" | LC_ALL=C sort > "$SCRATCH/uploaded-assets.txt"
diff -u "$SCRATCH/expected-assets.txt" "$SCRATCH/uploaded-assets.txt"
for NAME in "${EXPECTED[@]}" checksums.txt; do
  DIGEST="sha256:$(sha256sum "$ASSET_DIR/$NAME" | cut -d ' ' -f1)"
  jq -e --arg name "$NAME" --arg digest "$DIGEST" \
    'any(.[]; .name == $name and .state == "uploaded" and .digest == $digest)' \
    <<<"$ASSETS" >/dev/null
done
echo "Draft assembled; CI, asset review and Chinese release notes are required before publication."
