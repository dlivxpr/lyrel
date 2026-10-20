#!/bin/bash
# 在仓库根目录运行：bash .github/scripts/generate-checksums.sh [asset-dir]
# 需要 Bash 和 GNU coreutils；asset-dir 默认 dist，只散列正式资产，不包含打包暂存目录。
# 汇总各平台产物后再运行。输出沿用客户端解析的 <hex>  <裸文件名> 格式。
set -euo pipefail

cd "${1:-dist}"
shopt -s nullglob
ASSETS=(lyrel-*.dmg lyrel-*.zip lyrel-*-setup.exe lyrel-server-*)
if [[ ${#ASSETS[@]} -eq 0 ]]; then
  echo "generate-checksums: no release assets found" >&2
  exit 1
fi
CHECKSUMS="$(mktemp)"
trap 'rm -f "$CHECKSUMS"' EXIT
printf '%s\n' "${ASSETS[@]}" | LC_ALL=C sort -u | while IFS= read -r ASSET; do
  sha256sum "$ASSET"
done > "$CHECKSUMS"
sha256sum -c "$CHECKSUMS"
mv "$CHECKSUMS" checksums.txt
cat checksums.txt
