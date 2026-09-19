#!/usr/bin/env bash
# 鸿蒙原生宿主构建：goptop-ohos（Rust，C ABI）→ aarch64-unknown-linux-ohos → libgoptop_ohos.so。
#
# 产物由 harmony/entry 的 CMake 链接进 NAPI 模块（见 harmony/entry/src/main/cpp）。
# **鸿蒙能跑原生代码，所以规则与 AI 都直接链接原生 crate**——wasm 只是 Web 端的
# 编译目标（见 .agents/memory/2026-09-19-wasm-is-web-only-native-platforms-link-rust.md）。
#
# 前置：rustup target add aarch64-unknown-linux-ohos
#       DevEco Studio（提供 OHOS NDK：clang + ld.lld + sysroot）
#
# 用法：bash scripts/build-ohos.sh [--debug]
#
# 关于那个 junction：OHOS NDK 装在 "D:\Huawei\DevEco Studio\..."，路径里有空格。
# RUSTFLAGS 是**按空白切分**的，`-C link-arg=--sysroot=.../DevEco Studio/...` 会被
# 切成两段，clang 收不到 sysroot，于是退回环境里的 mingw `ld.exe`（GNU BFD，PE 目标），
# 报 `lld: error: unknown argument: -z`——错的是链接器被换掉了，不是它不认 -z。
# 所以这里先在 target/ 下建一个无空格的 junction，所有路径都从它走。
set -euo pipefail
cd "$(dirname "$0")/.."

# DevEco SDK 根：默认按 DevEco Studio 的安装位置推导，可用 DEVECO_SDK_HOME 覆盖
SDK_HOME="${DEVECO_SDK_HOME:-D:\\Huawei\\DevEco Studio\\sdk}"
NDK_REAL="$SDK_HOME/default/openharmony/native"
NDK_REAL_WIN="$(cygpath -w "$NDK_REAL")"
NDK_LINK="$(pwd)/target/ohos-ndk"
# 传给 clang / rustc 的必须是 Windows 形态（`G:/…`）：它们是原生 Windows 程序，
# `/g/ClaudeProjects/…` 会被当成「当前盘根下的 g\ClaudeProjects\…」，报的却是
# 「找不到 crti.o / -lc」——看着像 sysroot 少文件，其实是路径根本没进去。
NDK_LINK_WIN="$(cygpath -m "$NDK_LINK")"

if [ ! -d "$NDK_REAL" ]; then
  echo "找不到 OHOS NDK：$NDK_REAL" >&2
  echo "请用 DEVECO_SDK_HOME 指向 DevEco 的 sdk 目录（形如 .../DevEco Studio/sdk）" >&2
  exit 1
fi

# 建/刷新无空格 junction（已存在且可用时跳过）。
# 用 PowerShell 而不是 cmd 的 mklink：Git Bash 会把 mklink 的 `/J` 和绝对路径当 Unix
# 路径改写（实测生成出 `\D:\Huawei\...` 这种带前导反斜杠的坏链接）。
if [ ! -d "$NDK_LINK/llvm/bin" ]; then
  rm -rf "$NDK_LINK" 2>/dev/null || true
  mkdir -p target
  powershell -NoProfile -Command "New-Item -ItemType Junction -Path '$(cygpath -w "$NDK_LINK")' -Target '$NDK_REAL_WIN' | Out-Null"
fi

PROFILE_FLAG="--release"
if [ "${1:-}" = "--debug" ]; then PROFILE_FLAG=""; fi

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$NDK_LINK_WIN/llvm/bin/clang.exe"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUSTFLAGS="-C link-arg=--target=aarch64-linux-ohos -C link-arg=--sysroot=$NDK_LINK_WIN/sysroot -C link-arg=-D__MUSL__ -C link-arg=-B$NDK_LINK_WIN/llvm/bin -C link-arg=-fuse-ld=lld"

cargo build -p goptop-ohos $PROFILE_FLAG --target aarch64-unknown-linux-ohos

OUT="target/aarch64-unknown-linux-ohos/$([ -n "$PROFILE_FLAG" ] && echo release || echo debug)/libgoptop_ohos.so"
echo "鸿蒙原生宿主已构建：$OUT"
echo "把它放到 harmony/entry/src/main/cpp/libs/arm64-v8a/ 后由 CMake 链接。"
