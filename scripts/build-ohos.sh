#!/usr/bin/env bash
# 鸿蒙原生宿主构建：goptop-ohos（Rust，C ABI）→ 鸿蒙 ABI → libgoptop_ohos.so。
#
# 产物由 harmony/entry 的 CMake 链接进 NAPI 模块（见 harmony/entry/src/main/cpp）。
# **鸿蒙能跑原生代码，所以规则与 AI 都直接链接原生 crate**——wasm 只是 Web 端的
# 编译目标（见 .agents/memory/2026-09-19-wasm-is-web-only-native-platforms-link-rust.md）。
#
# 前置：rustup target add aarch64-unknown-linux-ohos x86_64-unknown-linux-ohos
#       DevEco Studio（提供 OHOS NDK：clang + ld.lld + sysroot）
#
# 用法：bash scripts/build-ohos.sh [--debug]
#       --debug 只编到 debug profile（默认 release）
#
# 两个 ABI 都要编，且各自拷到 cpp/libs/<ABI>/：
# - **arm64-v8a**：真机（含「Mate 80」以外的实体设备）。
# - **x86_64**：DevEco 模拟器——它跑的是 x86_64 镜像，只带 arm64 的 HAP 装不上，
#   报 `install parse native so failed: the Abi type supported by the device does not
#   match the Abi type configured in the C++ project`。真机上用不到这份，但缺了就没法
#   在模拟器里联调，而联调正是排查 NAPI 问题唯一的手段。
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
PROFILE_DIR="release"
if [ "${1:-}" = "--debug" ]; then PROFILE_FLAG=""; PROFILE_DIR="debug"; fi

# Rust target → (鸿蒙 ABI 目录名, clang 的 -target)
build_abi() {
  local rust_target="$1" abi_dir="$2" clang_target="$3"
  local var="CARGO_TARGET_$(echo "$rust_target" | tr 'a-z-' 'A-Z_')_LINKER"
  local flags_var="CARGO_TARGET_$(echo "$rust_target" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
  # bash 的间接赋值（`${!var}` 只能读，写要用 printf -v / declare -g）
  # `-soname` 不是可选项：没有它，链接方（entry 的 NAPI 模块）会把**.so 的绝对路径**
  # 写进 DT_NEEDED（实测写进去的是 `G:/ClaudeProjects/.../libs/arm64-v8a/libgoptop_ohos.so`），
  # 设备上那个路径不存在，dlopen libgoptop.so 直接失败——整个原生宿主用不了，而日志里
  # 只有一句「napi api call fail」，看不出是路径问题。
  export "$var=$NDK_LINK_WIN/llvm/bin/clang.exe"
  export "$flags_var=-C link-arg=--target=$clang_target -C link-arg=--sysroot=$NDK_LINK_WIN/sysroot -C link-arg=-D__MUSL__ -C link-arg=-B$NDK_LINK_WIN/llvm/bin -C link-arg=-fuse-ld=lld -C link-arg=-Wl,-soname,libgoptop_ohos.so"

  # C 交叉编译器：传输层拉进了 ring（TLS），它的构建脚本走 cc-rs 编 C。
  # 不设这些变量时报的是 `cc-rs: failed to find tool "cc": program not found`——
  # 看着像本机缺 cc，其实是 cc-rs 不知道要交叉编到鸿蒙。
  #
  # 变量名用**下划线**形式（`CC_aarch64_unknown_linux_ohos`）：cc-rs 查的就是这个，
  # 而带横线的 `CC_aarch64-unknown-linux-ohos` 在 shell 里根本不是合法标识符
  #（`export` 会直接报 not a valid identifier）。
  local tu; tu="$(echo "$rust_target" | tr '-' '_')"
  export "CC_$tu=$NDK_LINK_WIN/llvm/bin/clang.exe"
  export "AR_$tu=$NDK_LINK_WIN/llvm/bin/llvm-ar.exe"
  export "CFLAGS_$tu=--target=$clang_target --sysroot=$NDK_LINK_WIN/sysroot -D__MUSL__"

  cargo build -p goptop-ohos $PROFILE_FLAG --target "$rust_target"

  local src="target/$rust_target/$PROFILE_DIR/libgoptop_ohos.so"
  local dst="harmony/entry/src/main/cpp/libs/$abi_dir/libgoptop_ohos.so"
  mkdir -p "$(dirname "$dst")"
  cp "$src" "$dst"
  echo "  $rust_target → $dst（$(stat -c %s "$dst" 2>/dev/null || echo '?') 字节）"
}

echo "构建鸿蒙原生宿主（$PROFILE_DIR）："
build_abi aarch64-unknown-linux-ohos arm64-v8a aarch64-linux-ohos
build_abi x86_64-unknown-linux-ohos x86_64 x86_64-linux-ohos

echo "完成。改完 Rust 必须重跑本脚本再构建 HAP，否则壳里跑的是上一版逻辑（不报错，只是行为旧）。"
