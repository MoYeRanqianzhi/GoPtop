//! 平台本地存储（用户拍板 2026-09-19：数据按平台规定的区域落盘）。
//!
//! 分工（前端 `frontend/src/net/store.ts` 是唯一读写门面）：
//! - **桌面壳**：`~/.goptop/store.json`（用户指定目录）；
//! - **移动端（Android/iOS，同为本 Tauri 后端）**：平台给应用的私有数据目录
//!   （`app_data_dir()`，Android 落在 `/data/data/<pkg>/…`，随应用卸载清除）；
//! - **Web 端**：浏览器 localStorage（前端门面的回退路径，不经本模块）；
//! - **鸿蒙壳（ArkWeb，无 Rust 后端）**：ArkTS 注入的 `goptopStore` 代理落应用 `filesDir/store.json`，不经本模块。
//!
//! 为什么单文件 JSON：全量键值只有几条（昵称/服务器/STUN/默认规则/头像），
//! 一个文件即可原子落盘、便于用户备份与排查；头像 data URL 也在其中（限长见
//! `MAX_VALUE`，超限拒绝写入而不是悄悄截断）。
//!
//! 写入一律**先写临时文件再 rename**：进程被杀不会留下半截 JSON（原文件保持
//! 完好），Windows 上 `std::fs::rename` 底层是 MOVEFILE_REPLACE_EXISTING，可直接覆盖。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// 存储文件名（目录见 [`store_dir`]）。
const FILE: &str = "store.json";
/// 单值长度上限：头像 data URL 是最大的一个，4MB 足够且能挡住异常写入把文件撑爆。
const MAX_VALUE: usize = 4 * 1024 * 1024;
/// 键名规范：只允许 `goptop:xxx` 这类字符，杜绝任何越权/歧义键名。
fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_'))
}

/// 进程内缓存：前端只在启动时 `store_load` 一次，之后读写都经这里，
/// 避免每次设置变更都重新解析文件；同时保证并发命令下内存与文件一致。
static MEM: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

/// 存储目录：桌面 = `~/.goptop`；移动端 = 平台私有数据目录。
pub fn store_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(desktop)]
    {
        let home = app.path().home_dir().map_err(|e| format!("取用户主目录失败: {e}"))?;
        return Ok(home.join(".goptop"));
    }
    #[cfg(mobile)]
    {
        return app.path().app_data_dir().map_err(|e| format!("取应用数据目录失败: {e}"));
    }
    #[cfg(not(any(desktop, mobile)))]
    {
        let _ = app;
        Err("不支持的平台".into())
    }
}

/// 从目录读存储；文件不存在 = 空表（首次启动的正常路径，不是错误）。
fn load_from(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = dir.join(FILE);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
}

/// 原子写回：目录不存在则创建，先写 `.tmp` 再 rename 覆盖。
fn save_to(dir: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {} 失败: {e}", dir.display()))?;
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.tmp"));
    let text = serde_json::to_string_pretty(map).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("落盘 {} 失败: {e}", path.display()))?;
    Ok(())
}

/// 取出（必要时首次加载）内存缓存；`f` 在其上就地修改。
fn with_mem<T>(app: &AppHandle, f: impl FnOnce(&mut BTreeMap<String, String>) -> T) -> Result<T, String> {
    let mut guard = MEM.lock().map_err(|_| "存储锁中毒".to_string())?;
    if guard.is_none() {
        *guard = Some(load_from(&store_dir(app)?)?);
    }
    let map = guard.as_mut().expect("刚填充过");
    Ok(f(map))
}

/// 全量读取（前端启动时调一次，之后自行用内存副本）。
#[tauri::command]
pub fn store_load(app: AppHandle) -> Result<BTreeMap<String, String>, String> {
    let snapshot = with_mem(&app, |m| m.clone())?;
    Ok(snapshot)
}

/// 写入一个键（落盘失败会返回错误，前端据此回退到浏览器存储并提示）。
#[tauri::command]
pub fn store_set(app: AppHandle, key: String, value: String) -> Result<(), String> {
    if !valid_key(&key) {
        return Err(format!("非法键名: {key}"));
    }
    if value.len() > MAX_VALUE {
        return Err(format!("值过长（{} > {MAX_VALUE}）", value.len()));
    }
    let dir = store_dir(&app)?;
    with_mem(&app, |m| {
        m.insert(key, value);
        save_to(&dir, m)
    })?
}

/// 删除一个键（不存在也算成功——前端语义与 localStorage.removeItem 一致）。
#[tauri::command]
pub fn store_remove(app: AppHandle, key: String) -> Result<(), String> {
    if !valid_key(&key) {
        return Err(format!("非法键名: {key}"));
    }
    let dir = store_dir(&app)?;
    with_mem(&app, |m| {
        m.remove(&key);
        save_to(&dir, m)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独立临时目录（进程内并发跑也不会互相踩）。
    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("goptop-store-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn load_missing_file_is_empty_not_error() {
        let d = tmp_dir("missing");
        assert!(load_from(&d).expect("缺文件应视为空").is_empty());
    }

    #[test]
    fn save_then_load_round_trip() {
        let d = tmp_dir("roundtrip");
        let mut m = BTreeMap::new();
        m.insert("goptop:name".to_string(), "阿甲".to_string());
        m.insert("goptop:avatar".to_string(), "data:image/png;base64,AAA".to_string());
        save_to(&d, &m).expect("写盘");
        assert_eq!(load_from(&d).expect("读盘"), m, "往返必须完全一致（含中文与 data URL）");
    }

    #[test]
    fn save_leaves_no_tmp_behind() {
        let d = tmp_dir("tmp");
        let mut m = BTreeMap::new();
        m.insert("goptop:name".to_string(), "x".to_string());
        save_to(&d, &m).expect("写盘");
        assert!(!d.join("store.json.tmp").exists(), "临时文件必须已被 rename 消费");
    }

    #[test]
    fn corrupt_file_is_reported_not_silently_emptied() {
        let d = tmp_dir("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FILE), "{ 不是 JSON").unwrap();
        assert!(load_from(&d).is_err(), "损坏文件必须报错（由前端回退），不能当空表悄悄覆盖");
    }

    #[test]
    fn key_validation() {
        assert!(valid_key("goptop:name"));
        assert!(valid_key("goptop:server-sel"));
        assert!(!valid_key(""));
        assert!(!valid_key("../escape"));
        assert!(!valid_key("a/b"));
        assert!(!valid_key(&"k".repeat(129)));
    }
}
