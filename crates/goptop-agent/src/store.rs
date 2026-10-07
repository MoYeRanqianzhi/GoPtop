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
    // 首尾空白一并剥掉：模型写路径带尾随空格是高频手误，而「以空格结尾的路径」
    // 从来不是本意——剥掉比原样存库对模型更友好。
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "invalid path {raw:?}: a /memory path cannot be empty. Use paths like /memory/notes/opponent-style.md (see /index)."
        ));
    }
    if trimmed.contains('\\') {
        return Err(format!(
            "invalid path {raw:?}: backslash is not allowed — use \"/\" as the separator."
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!(
            "invalid path {raw:?}: control characters (newline/tab) are not allowed in paths."
        ));
    }
    // 逐段走一遍：`..` 是越狱口（虽然这里只是 SQLite 键不是真文件系统，键里混进
    // `..` 会让 /memory 的路径空间自相矛盾），空段折叠即「剥首尾/折叠连续 /」。
    let mut folded = String::with_capacity(trimmed.len());
    for seg in trimmed.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg == ".." {
            return Err(format!(
                "invalid path {raw:?}: \"..\" segments are not allowed. Memory paths always live under /memory."
            ));
        }
        folded.push_str(seg);
        folded.push('/');
    }
    folded.pop(); // 尾部多出的那个 '/'
    if folded.is_empty() {
        return Err(format!(
            "invalid path {raw:?}: a /memory path cannot be empty. Use paths like /memory/notes/opponent-style.md (see /index)."
        ));
    }
    if folded.len() > MAX_PATH_LEN {
        return Err(format!(
            "invalid path: longer than {MAX_PATH_LEN} characters (got {}). Shorten it.",
            folded.len()
        ));
    }
    Ok(folded)
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
    /// 超单文件配额（[`MAX_FILE_BYTES`]) 或 ns 总量配额（[`MAX_NS_BYTES`]）；
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
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("memory store: cannot create directory {}: {e}", dir.display()))?;
        }
        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| format!("memory store: cannot open {}: {e}", db_path.display()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files(
                 ns         TEXT NOT NULL,
                 path       TEXT NOT NULL,
                 content    TEXT NOT NULL,
                 size       INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY(ns, path)
             );
             CREATE TABLE IF NOT EXISTS meta(
                 ns         TEXT PRIMARY KEY,
                 bytes_used INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("memory store: cannot create tables: {e}"))?;
        Ok(Self { conn: MutexConnection(std::sync::Mutex::new(conn)) })
    }

    /// 锁内跑一段同步 SQL。所有方法共用这一个入口：锁中毒=先前有 SQL panic（bug），
    /// 静默回空会撕裂账目（files 与 meta 对不上），按仓库惯例 `into_inner` 硬闯——
    /// 数据仍在，自愈优先于放大故障。
    fn with_conn<T>(&self, f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self.conn.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

impl VfsStore for NativeStore {
    fn read(&self, ns: &str, path: &str) -> Result<Option<String>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT content FROM files WHERE ns = ?1 AND path = ?2")
                .map_err(sql_err)?;
            let mut rows = stmt.query(rusqlite::params![ns, path]).map_err(sql_err)?;
            match rows.next().map_err(sql_err)? {
                Some(row) => Ok(Some(row.get(0).map_err(sql_err)?)),
                None => Ok(None),
            }
        })
    }

    fn write(&self, ns: &str, path: &str, content: &str) -> Result<(), String> {
        let len = content.len();
        if len > MAX_FILE_BYTES {
            return Err(format!(
                "file too large: {len} bytes. The limit is {MAX_FILE_BYTES} bytes (256KB) per file — split the note into several files."
            ));
        }
        self.with_conn(|conn| {
            // 查额与写入必须同事务：并发两个 write 同时过额检、先后落库会把
            // bytes_used 记成「各自都合法、合计超限」的假账。Mutex 已串行化调用，
            // 事务是第二道保险（防未来调用方绕过 Mutex 复用连接）。
            let tx = conn.transaction().map_err(sql_err)?;
            let old: u64 = tx
                .query_row(
                    "SELECT size FROM files WHERE ns = ?1 AND path = ?2",
                    rusqlite::params![ns, path],
                    |row| row.get::<_, i64>(0),
                )
                .map(|s| s as u64)
                .unwrap_or(0);
            let used: u64 = tx
                .query_row(
                    "SELECT bytes_used FROM meta WHERE ns = ?1",
                    rusqlite::params![ns],
                    |row| row.get::<_, i64>(0),
                )
                .map(|s| s as u64)
                .unwrap_or(0);
            // 覆盖写按净增量计额（used - old + len），而非 used + len：同一文件
            // 反复编辑是记忆库的主路径，字面相加会把「原地改写」误判成超限。
            let Some(after) = used.checked_sub(old).and_then(|u| u.checked_add(len as u64)) else {
                return Err("memory store: usage accounting underflow (corrupted meta)".to_string());
            };
            if after > MAX_NS_BYTES as u64 {
                return Err(format!(
                    "memory quota exceeded: this write would bring {ns} to {after} bytes (limit {MAX_NS_BYTES}). Shorten old files first — read them, then edit them smaller."
                ));
            }
            tx.execute(
                "INSERT INTO files(ns, path, content, size, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(ns, path) DO UPDATE SET content = excluded.content,
                     size = excluded.size, updated_at = excluded.updated_at",
                rusqlite::params![ns, path, content, len as i64, now_ms()],
            )
            .map_err(sql_err)?;
            tx.execute(
                "INSERT INTO meta(ns, bytes_used) VALUES (?1, ?2)
                 ON CONFLICT(ns) DO UPDATE SET bytes_used = excluded.bytes_used",
                rusqlite::params![ns, after as i64],
            )
            .map_err(sql_err)?;
            tx.commit().map_err(sql_err)
        })
    }

    fn delete(&self, ns: &str, path: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let tx = conn.transaction().map_err(sql_err)?;
            // 先取 size 再删（删完行就没了，回收额度只能用删前读到的值）；
            // 额度回收与删除同事务：撤记录不回额度，quota 会永久虚高。
            let old: Option<i64> = tx
                .query_row(
                    "SELECT size FROM files WHERE ns = ?1 AND path = ?2",
                    rusqlite::params![ns, path],
                    |row| row.get(0),
                )
                .map(Some)
                .unwrap_or(None);
            let deleted = tx
                .execute("DELETE FROM files WHERE ns = ?1 AND path = ?2", rusqlite::params![ns, path])
                .map_err(sql_err)?;
            if deleted > 0 {
                tx.execute(
                    "UPDATE meta SET bytes_used = MAX(0, bytes_used - ?2) WHERE ns = ?1",
                    rusqlite::params![ns, old.unwrap_or(0)],
                )
                .map_err(sql_err)?;
            }
            tx.commit().map_err(sql_err)?;
            Ok(deleted > 0)
        })
    }

    fn list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT path FROM files WHERE ns = ?1 ORDER BY path")
                .map_err(sql_err)?;
            let rows = stmt
                .query_map(rusqlite::params![ns], |row| row.get::<_, String>(0))
                .map_err(sql_err)?;
            // 前缀过滤放 Rust 侧而非 SQL LIKE：path 里可能出现 %/_ 字符（模型写的
            // 笔记内容不可控），LIKE 还得转义，全量拉回内存过滤更直白也够快
            // （8MB/256KB 上限决定了行数撑死几百行）。
            Ok(rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_err)?
                .into_iter()
                .filter(|p| p.starts_with(prefix))
                .collect())
        })
    }

    fn usage(&self, ns: &str) -> Result<u64, String> {
        self.with_conn(|conn| {
            let used: i64 = conn
                .query_row(
                    "SELECT bytes_used FROM meta WHERE ns = ?1",
                    rusqlite::params![ns],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(used as u64)
        })
    }
}

/// rusqlite 错误 → 人话文本（存储层错误没有分型消费者，统一前缀即可定位）。
fn sql_err(e: rusqlite::Error) -> String {
    format!("memory store error: {e}")
}

/// 当前 Unix 毫秒（updated_at 用；时钟回拨等异常回 0——排序字段错一位无实害）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独享一个 db 文件：并行测试同进程跑，文件名撞车会互相读到对方的表。
    fn temp_db() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("goptop-agent-store-{}-{n}.db", std::process::id()))
    }

    #[test]
    fn 规范化_剥斜杠折叠连续斜杠() {
        assert_eq!(normalize_path("/notes/style.md"), Ok("notes/style.md".into()));
        assert_eq!(normalize_path("notes/style.md"), Ok("notes/style.md".into()));
        assert_eq!(normalize_path("notes//style.md/"), Ok("notes/style.md".into()));
        assert_eq!(normalize_path("///a///b///"), Ok("a/b".into()));
    }

    #[test]
    fn 规范化_拒绝非法形态() {
        assert!(normalize_path("").is_err());
        assert!(normalize_path("   ").is_err());
        assert!(normalize_path("/").is_err());
        assert!(normalize_path("//").is_err());
        assert!(normalize_path("a/../b").is_err());
        assert!(normalize_path("..").is_err());
        assert!(normalize_path("a\\b").is_err());
        assert!(normalize_path("a\nb").is_err());
        assert!(normalize_path(&"x".repeat(MAX_PATH_LEN + 1)).is_err());
        // 边界：恰好 256 合法。
        assert_eq!(normalize_path(&"x".repeat(MAX_PATH_LEN)), Ok("x".repeat(MAX_PATH_LEN)));
        // 错误文案里带原始路径，模型能对上自己写的东西。
        assert!(normalize_path("a/../b").unwrap_err().contains("a/../b"));
    }

    #[test]
    fn round_trip_读写改删列() {
        let store = NativeStore::open(&temp_db()).unwrap();
        // 不存在的文件：None，不隐式创建。
        assert_eq!(store.read("builtin", "notes/a.md").unwrap(), None);

        store.write("builtin", "notes/a.md", "黑喜欢占天元").unwrap();
        assert_eq!(store.read("builtin", "notes/a.md").unwrap().as_deref(), Some("黑喜欢占天元"));

        // 整体覆盖：usage 按 Δ 走。
        store.write("builtin", "notes/a.md", "黑喜欢占天元；白好战").unwrap();
        assert_eq!(store.usage("builtin").unwrap(), "黑喜欢占天元；白好战".len() as u64);

        store.write("builtin", "notes/b.md", "second").unwrap();
        // ns 隔离：另一驱动的记忆互不可见。
        assert_eq!(store.read("mcp", "notes/a.md").unwrap(), None);
        assert_eq!(store.usage("mcp").unwrap(), 0);

        // list 前缀过滤 + 字典序。
        store.write("builtin", "chat/log.md", "x").unwrap();
        assert_eq!(
            store.list("builtin", "").unwrap(),
            vec!["chat/log.md".to_string(), "notes/a.md".to_string(), "notes/b.md".to_string()]
        );
        assert_eq!(store.list("builtin", "notes/").unwrap(), vec!["notes/a.md".to_string(), "notes/b.md".to_string()]);
        assert!(store.list("builtin", "nope/").unwrap().is_empty());

        // delete：删了回 true、再删回 false，额度同步回收。
        assert!(store.delete("builtin", "notes/b.md").unwrap());
        assert!(!store.delete("builtin", "notes/b.md").unwrap());
        assert_eq!(store.read("builtin", "notes/b.md").unwrap(), None);
        let expected = "黑喜欢占天元；白好战".len() as u64 + "x".len() as u64;
        assert_eq!(store.usage("builtin").unwrap(), expected);
    }

    #[test]
    fn usage_与逐行求和一致() {
        let store = NativeStore::open(&temp_db()).unwrap();
        for (p, c) in [("a.md", "12345"), ("b/c.md", " xy "), ("d.md", "汉字三字节乘三")] {
            store.write("builtin", p, c).unwrap();
        }
        let sum: u64 = store
            .list("builtin", "")
            .unwrap()
            .into_iter()
            .map(|p| store.read("builtin", &p).unwrap().unwrap().len() as u64)
            .sum();
        assert_eq!(store.usage("builtin").unwrap(), sum);
    }

    #[test]
    fn 配额_单文件超限拒且不落库() {
        let store = NativeStore::open(&temp_db()).unwrap();
        let big = "x".repeat(MAX_FILE_BYTES + 1);
        let err = store.write("builtin", "big.md", &big).unwrap_err();
        assert!(err.contains("too large"), "错误要说明是单文件超限: {err}");
        // 不写半截：拒了就没有这个键。
        assert_eq!(store.read("builtin", "big.md").unwrap(), None);
        assert_eq!(store.usage("builtin").unwrap(), 0);
        // 恰好 256KB 合法。
        store.write("builtin", "big.md", &"x".repeat(MAX_FILE_BYTES)).unwrap();
        assert_eq!(store.usage("builtin").unwrap(), MAX_FILE_BYTES as u64);
    }

    #[test]
    fn 配额_ns总量按净增量计() {
        // ns 总量（8MB）的用例必须在单文件上限（256KB）之内构造：一个 6MB 的
        // 大文件会先撞单文件限（两个配额的判定序），测不到 ns 这一层——用多个
        // 250KB 文件堆到阈值两侧。
        const F: usize = 250 * 1024;
        let store = NativeStore::open(&temp_db()).unwrap();
        // 32 × 250KB = 8,192,000 ≤ 8MB：全部落库。
        for i in 0..32 {
            store.write("builtin", &format!("f{i:02}.md"), &"a".repeat(F)).unwrap();
        }
        assert_eq!(store.usage("builtin").unwrap(), 32 * F as u64);
        // 第 33 个文件 → 8,448,000 > 8MB：ns 总量拒。
        let err = store.write("builtin", "over.md", &"b".repeat(F)).unwrap_err();
        assert!(err.contains("quota exceeded"), "错误要指向 ns 总量: {err}");
        assert_eq!(store.read("builtin", "over.md").unwrap(), None, "超限不落库");
        // 原地改写 f00 250KB → 50KB：净减，必须放行（字面 used+len 语义会在这里误杀）。
        store.write("builtin", "f00.md", &"c".repeat(50 * 1024)).unwrap();
        assert_eq!(store.usage("builtin").unwrap(), 31 * F as u64 + 50 * 1024);
        // 腾出空间后再写原先被拒的文件即可通过。
        store.write("builtin", "over.md", &"b".repeat(F)).unwrap();
        assert_eq!(store.usage("builtin").unwrap(), 32 * F as u64 + 50 * 1024);
    }

    #[test]
    fn 打开_父目录一并创建() {
        let dir = temp_db().with_extension("d");
        let db = dir.join("nested").join("agent-memory.db");
        NativeStore::open(&db).unwrap();
        assert!(db.exists(), "父目录不存在时应级联创建");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
