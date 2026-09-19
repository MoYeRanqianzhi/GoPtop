/**
 * `libgoptop.so` 的 ArkTS 类型声明（NAPI 原生模块）。
 *
 * 三端契约的鸿蒙侧入口：逻辑全在 Rust（`crates/goptop-ohos`），这里只是签名。
 * 与桌面/Android 的 Tauri command 同源（都只是 `goptop_core::json_api` 的宿主适配）。
 *
 * 构建期 hvigor 会核对本文件与 .so 的导出：缺了它会告警
 * 「module for 'libgoptop.so' is not verified」。
 */
export const call: (cmd: string, argsJson: string) => string;
export const aiPost: (reqJson: string) => number;
export const aiPoll: (ticket: number) => string;
