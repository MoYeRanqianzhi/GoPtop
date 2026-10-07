//! /memory 的持久存储 —— [`VfsStore`] trait（双后端缝）+ 路径规范化与配额。
//!
//! 只服务 `/memory` 子树（/game 是动态合成、不落盘，见 vfs.rs）。表结构与配额
//! 两端一致（计划「记忆库双后端」节）：
//!
//! ```sql
//! files(ns, path, content, size, updated_at, PK(ns, path))
//! meta(ns, bytes_used)
//! ```
//!
//! ns 按对手驱动分：内置=`builtin`、MCP=`mcp`（[`crate::Driver::memory_ns`]）——
//! 记忆是「对手风格」级别的长期积累，两个驱动的笔记互不串味。
//!
//! 后端两端：native=[`NativeStore`]（rusqlite bundled `~/.goptop/agent-memory.db`）；
//! web=IndexedDB，经 `frontend/src/net/agentVfs.ts` 暴露的 `window.goptopVfs*` 钩子
//! （阶段⑤，wasm 编译期 cfg 选后端——trait 就是那道缝）。
//!
//! **为什么不用 store.json/store KV**：单值 4MB 上限、无事务语义，且会把 Agent 的
//! 记忆与 UI 设置搅在同一份文件里互相膨胀。

/// 路径长度上限（计划值域；规范化时超限即拒）。
pub const MAX_PATH_LEN: usize = 256;
/// 单文件上限：256KB（write 前查，超限回人话错误、不写半截）。
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// 每 ns 总量上限：8MB（write 前查 `usage + len`；超限提示删旧文件腾空间）。
pub const MAX_NS_BYTES: usize = 8 * 1024 * 1024;

/// 路径规范化：拒空串、拒 `..`（任意一段）、拒反斜杠与控制字符、拒超 [`MAX_PATH_LEN`]；
/// 剥去首尾 `/`、折叠连续 `/`。返回**存表键形态**的相对路径（如
/// `/notes/opponent-style.md` → `notes/opponent-style.md`）。
///
/// 错误回人话文本（RespondToModel 级）——模型第一次写错路径是常态，错误信息里要
/// 告诉它什么样的路径合法。
///
/// # Errors
/// 路径非法时回 `Err(说明文本)`。
pub fn normalize_path(raw: &str) -> Result<String, String> {
    todo!()
}

/// /memory 的存储抽象。
///
/// **全部方法同步**：rusqlite 连接不跨 await（连接本身不是 Send-friendly 的 async
/// 对象，std Mutex 包住同步调用即可）；web 后端经 wasm-bindgen 同步桥时同样成立。
/// 错误统一回 `String`（人话、可直给模型）——存储层错误没有「分型处理」的消费者，
/// 一个枚举只会让每个调用点多一层 match。
pub trait VfsStore: Send + Sync {
    /// 读一个文件；`None` = 不存在（调用方对 read 工具回「文件不存在」，
    /// 对 edit 工具同样按不存在报错——不隐式创建）。
    ///
    /// # Errors
    /// 存储层故障（IO/SQL）。
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String>;

    /// 整体覆盖写入（upsert）。成功即更新 `meta.bytes_used`（+Δ 与内容长度差一致）。
    ///
    /// # Errors
    /// 超单文件配额（[`MAX_FILE_BYTES`]）或 ns 总量配额（[`MAX_NS_BYTES`]）；
    /// 存储层故障。**不写半截**：先查额后写，两步在连接锁内完成（无事务竞态）。
    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String>;

    /// 删除一个文件并回收额度；返回是否真的删了（不存在=false，不是错误——
    /// delete 幂等，调用方按返回值生成回执即可）。
    ///
    /// # Errors
    /// 存储层故障。
    fn delete(&self, ns: &str, path: &str) -> Result<bool, String>;

    /// 列出前缀下的全部路径（字典序；`prefix` 为空 = 全 ns）。/index 与 grep 的
    /// 遍历都吃这份清单。
    ///
    /// # Errors
    /// 存储层故障。
    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String>;

    /// ns 已用字节（`meta.bytes_used`；写路径负责维护它与 files 逐行求和一致——
    /// 若两者能对不上，说明写路径有 bug，测试用「逐行求和 == usage」钉住）。
    ///
    /// # Errors
    /// 存储层故障。
    fn usage(&self, ns: &str) -> Result<u64, String>;
}

/// native 后端：rusqlite bundled 的单文件库。
///
/// **db 路径由调用方给，本 crate 不自己拼 `~`**：桌面是 `~/.goptop/agent-memory.db`
/// （AgentHub 决定），测试是 `tempdir()` 里的临时文件——平台无关 crate 碰 home 目录
/// 就会与各端的既有落盘规则（Tauri app 目录/移动端私有目录）分叉。
pub struct NativeStore {
    conn: MutexConnection,
}

/// rusqlite 连接的同步互斥包装（字段不 pub；`Connection` 本身 Send 但要独占借用，
/// Mutex 是最小正确形态）。
struct MutexConnection(std::sync::Mutex<rusqlite::Connection>);

impl NativeStore {
    /// 打开（不存在则建库建表；父目录一并创建——首次启动时 `~/.goptop` 可能还没有）。
    ///
    /// # Errors
    /// 打开/建表失败（路径不可写、磁盘满）——AgentHub 据此决定降级成「记忆不可用」
    /// 还是拒绝开局。
    pub fn open(db_path: &std::path::Path) -> Result<Self, String> {
        todo!()
    }
}

impl VfsStore for NativeStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        todo!()
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        todo!()
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        todo!()
    }

    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String> {
        todo!()
    }

    fn usage(&self, ns: &str) -> Result<u64, String> {
        todo!()
    }
}
