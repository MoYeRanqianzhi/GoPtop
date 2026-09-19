//! 宿主能力抽象 —— transport 与「谁来执行平台动作」之间的接缝。
//!
//! 同一份传输逻辑要服务三种宿主：Tauri command（桌面/Android）、鸿蒙 NAPI、
//! 以及测试用的无头桩。把平台能力抽成 trait，逻辑就只有一份；各宿主只要实现这五个
//! 动作（存储读写算一个，落到 trait 上是 6 个方法）。
//!
//! **全部要求 `Send + Sync`**：native 侧 IO 跑在 tokio 任务里，宿主要能被跨线程
//! 共享（wasm 侧没这个约束，那边的等价物是 `window.goptop*` 全局钩子）。

/// 宿主能力。
pub trait Host: Send + Sync {
    /// 读设置类键值（桌面 `~/.goptop`、移动端私有目录、鸿蒙 filesDir）。
    fn storage_get(&self, key: &str) -> Option<String>;
    /// 写/删设置类键值（`None` = 删除）。
    fn storage_set(&self, key: &str, value: Option<&str>);
    /// 复制到剪贴板。
    fn copy(&self, text: &str);
    /// 提示条（`None` = 清除；`ms` = 自动清除毫秒）。
    fn notice(&self, text: Option<&str>, ms: Option<u32>);
    /// SPA 导航。
    fn nav(&self, path: &str);
    /// 状态已变：宿主应把最新 snapshot 推给 UI。
    fn emit(&self, snapshot_json: &str);
}

/// 无头桩：全部 no-op，只为在测试里跑通状态机。
///
/// 用户要求的「扔掉前端换一个无界面模拟前端」正是靠它——业务逻辑若真在 Rust 里，
/// 那么这五个方法是**唯一**需要宿主提供的东西，界面可以完全不存在。
#[derive(Default)]
pub struct HeadlessHost {
    /// 存储用进程内 map（模拟落盘）。
    pub storage: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// 记录收到的提示与导航，供测试断言。
    pub notices: std::sync::Mutex<Vec<String>>,
    pub navs: std::sync::Mutex<Vec<String>>,
    /// emit 计数（UI 推送次数）。
    pub emits: std::sync::atomic::AtomicU64,
}

impl Host for HeadlessHost {
    fn storage_get(&self, key: &str) -> Option<String> {
        self.storage.lock().ok()?.get(key).cloned()
    }
    fn storage_set(&self, key: &str, value: Option<&str>) {
        if let Ok(mut m) = self.storage.lock() {
            match value {
                Some(v) => {
                    m.insert(key.to_string(), v.to_string());
                }
                None => {
                    m.remove(key);
                }
            }
        }
    }
    fn copy(&self, _text: &str) {}
    fn notice(&self, text: Option<&str>, _ms: Option<u32>) {
        if let Ok(mut v) = self.notices.lock() {
            v.push(text.unwrap_or("").to_string());
        }
    }
    fn nav(&self, path: &str) {
        if let Ok(mut v) = self.navs.lock() {
            v.push(path.to_string());
        }
    }
    fn emit(&self, _snapshot_json: &str) {
        self.emits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
