# GoPtop 鸿蒙原生模块：NAPI 薄层（本目录的 napi_init.cpp）+ Rust 宿主（预编译 .so）。
#
# Rust 侧的产物由 scripts/build-ohos.sh 生成，放在 libs/${OHOS_ARCH}/libgoptop_ohos.so。
# **不在这里用 CMake 调 cargo**：交叉编译要用 OHOS NDK 的链接器与 sysroot，
# 那套环境变量（含一个绕开 SDK 路径空格的 junction）在 build-ohos.sh 里，
# 塞进 CMake 会让两条构建路径各维护一份，迟早分叉。
cmake_minimum_required(VERSION 3.5.0)
project(goptop)

set(NATIVERENDER_ROOT_PATH ${CMAKE_CURRENT_SOURCE_DIR})

add_library(goptop SHARED napi_init.cpp)

target_include_directories(goptop PRIVATE ${NATIVERENDER_ROOT_PATH})

# Rust 宿主（IMPORTED：只链接，不参与本工程的编译）
add_library(goptop_ohos SHARED IMPORTED)
set_target_properties(goptop_ohos PROPERTIES
    IMPORTED_LOCATION ${NATIVERENDER_ROOT_PATH}/libs/${OHOS_ARCH}/libgoptop_ohos.so)

find_library(hilog-lib hilog_ndk.z)

target_link_libraries(goptop PUBLIC libace_napi.z.so goptop_ohos ${hilog-lib})
