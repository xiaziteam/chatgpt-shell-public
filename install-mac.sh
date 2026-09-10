#!/bin/bash
# ChatGPT虾壳 macOS 打包安装脚本
# 作用：杜绝“/Applications 里躺着旧包”的事故——所有发布必经此脚本，
# 它会校验 md5 并列出包内文件。
# 用法：./install-mac.sh            # 增量：cargo build + 换包内二进制
#       FULL_BUNDLE=1 ./install-mac.sh  # 全量：npx tauri build（含前端）
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
TAURI_DIR="$ROOT/client/src-tauri"
APP_NAME="ChatGPT虾壳.app"
BUNDLE="$TAURI_DIR/target/release/bundle/macos/$APP_NAME"

cd "$TAURI_DIR"
if [ "${FULL_BUNDLE:-0}" = "1" ]; then
  npx tauri build
else
  cargo build --release
  cp -f target/release/chatgpt-shell "$BUNDLE/Contents/MacOS/chatgpt-shell"
fi

RES_DIR="$ROOT/client/src-tauri/resources"
for f in sing-box geoip.db geosite.db; do
  if [ -f "$RES_DIR/$f" ]; then
    cp -f "$RES_DIR/$f" "$BUNDLE/Contents/MacOS/$f"
  fi
  if [ ! -f "$BUNDLE/Contents/MacOS/$f" ]; then
    echo "ERROR: $f 既不在 resources/ 也不在包内，请先放入 $RES_DIR/" >&2
    exit 1
  fi
done

xattr -dr com.apple.quarantine "$BUNDLE" 2>/dev/null || true
codesign --force --deep -s - "$BUNDLE" 2>/dev/null || echo "WARN: ad-hoc 签名失败（不致命）"
rm -rf "/Applications/$APP_NAME"
cp -R "$BUNDLE" "/Applications/$APP_NAME"
xattr -dr com.apple.quarantine "/Applications/$APP_NAME" 2>/dev/null || true

echo "--- 校验（两行 md5 必须一致）---"
md5 "$BUNDLE/Contents/MacOS/chatgpt-shell" "/Applications/$APP_NAME/Contents/MacOS/chatgpt-shell"
ls -la "/Applications/$APP_NAME/Contents/MacOS/"
echo "OK：已安装 $(grep '^version' "$TAURI_DIR/Cargo.toml" | head -1)"
