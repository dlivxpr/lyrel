#!/bin/bash
# source 本脚本可用于本地构建；CI 同时写入 GITHUB_ENV，供后续步骤继承。
# 只选择进程级 DEVELOPER_DIR，不修改全局 xcode-select 或安装工具链。
set -euo pipefail

has_required_sdk() {
  local version
  version=$(DEVELOPER_DIR="$1" xcrun --sdk macosx --show-sdk-version 2>/dev/null) || return 1
  [[ "$version" =~ ^([0-9]+)(\.[0-9]+)*$ ]] && (( BASH_REMATCH[1] >= 26 ))
}

select_macos_sdk() {
  local developer_dir="${DEVELOPER_DIR:-}" candidate sdk_version sdk_path
  if [[ -n "$developer_dir" ]]; then
    if ! has_required_sdk "$developer_dir"; then
      echo "::error::DEVELOPER_DIR=$developer_dir does not provide macOS SDK 26 or newer; install/select a compatible full Xcode." >&2
      return 1
    fi
  else
    candidate=$(xcode-select -p 2>/dev/null || true)
    if [[ -n "$candidate" ]] && has_required_sdk "$candidate"; then
      developer_dir="$candidate"
    else
      for candidate in /Applications/Xcode*.app/Contents/Developer; do
        if [[ -d "$candidate" ]] && has_required_sdk "$candidate"; then
          developer_dir="$candidate"
          break
        fi
      done
    fi
    if [[ -z "$developer_dir" ]]; then
      echo "::error::GPUI Fast requires macOS SDK 26 or newer. No compatible Xcode is installed; a macos-15 runner label is not SDK evidence." >&2
      return 1
    fi
  fi

  export DEVELOPER_DIR="$developer_dir"
  sdk_version=$(xcrun --sdk macosx --show-sdk-version)
  sdk_path=$(xcrun --sdk macosx --show-sdk-path)
  export SDKROOT="$sdk_path"
  export MACOSX_DEPLOYMENT_TARGET=15.0
  echo "--- selected macOS desktop toolchain ---"
  echo "DEVELOPER_DIR=$DEVELOPER_DIR"
  xcodebuild -version
  echo "macOS SDK=$sdk_version"
  echo "SDKROOT=$SDKROOT"
  echo "MACOSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET"
  xcrun --sdk macosx --find clang
  # Kit enables runtime_shaders; an offline Metal compiler is not a prerequisite.
  xcrun --sdk macosx --find metal || echo "metal compiler unavailable (Kit uses runtime_shaders)"
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf '%s\n' "DEVELOPER_DIR=$DEVELOPER_DIR" "SDKROOT=$SDKROOT" \
      "MACOSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET" >> "$GITHUB_ENV"
  fi
}

select_macos_sdk
