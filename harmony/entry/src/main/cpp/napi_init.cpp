/*
 * GoPtop 鸿蒙原生宿主的 NAPI 薄层。
 *
 * 分工（见 crates/goptop-ohos/src/lib.rs 的说明）：**NAPI 用 C++ 写**，头文件与
 * ABI 由 DevEco SDK 保证；**Rust 侧只出 C ABI**（`goptop_call` / `goptop_free`）。
 * 手写 NAPI 的 Rust FFI 声明等于凭记忆复刻一份 ABI，错了在设备上只会表现为
 * 「加载即崩」或「静默不注册」，没有本地复现手段。
 *
 * 本层只做三件事：字符串搬运、AI 的线程调度、票号管理。
 *
 * ## 三个导出方法
 *
 * - `call(cmd, argsJson) -> string`  —— **同步**，微秒级的规则命令（落子/悔棋/状态）。
 * - `aiPost(reqJson) -> number`      —— 起一次后台分析，立即返回票号。
 * - `aiPoll(ticket) -> string`       —— 取结果；未完成返回空串。
 *
 * ## 为什么 AI 要拆成 post/poll 而不是直接同步调
 *
 * `goptop_call("ai_analyze", …)` 是同步阻塞的纯计算，按预算要跑 0.3~3 秒。ArkWeb 的
 * javaScriptProxy 方法在 UI 线程上执行，直接同步调会把整个界面冻住（与桌面端必须
 * `spawn_blocking`、Web 端必须开 Worker 是同一件事）。
 *
 * 而 javaScriptProxy **不支持返回 Promise**（ArkTS 的 Promise 不会被 marshalling 成
 * JS 的 Promise），所以不能用「NAPI promise + await」这条常规路。改成 post/poll：
 * 计算跑在 NAPI 的 async work 线程池上，JS 侧用定时器轮询票号。全部用到的都是
 * 板上钉钉的同步 NAPI 能力，没有一处依赖「某版本才支持的 marshalling 行为」。
 */
#include <napi/native_api.h>
#include <hilog/log.h>

#include <chrono>
#include <cstdlib>
#include <map>
#include <mutex>
#include <string>

#undef LOG_DOMAIN
#undef LOG_TAG
#define LOG_DOMAIN 0x3200
#define LOG_TAG "GoPtopNapi"

// Rust 侧（crates/goptop-ohos）导出的 C ABI。返回的字符串必须交 goptop_free 释放。
extern "C" char *goptop_call(const char *cmd, const char *args_json);
extern "C" void goptop_free(char *p);

namespace {

/**
 * 票号表条目：结果（空 = 还在算）+ 出生时刻。
 *
 * born 用于回收「无人认领」的条目——JS 侧有放弃轮询的路径（应用冻结进后台超过
 * 前端 15s 轮询上限、分析进行中页面 reload 销毁 JS 上下文），之后写完的结果再也
 * 没人来 poll，而本模块存活整个进程、表没有任何其他回收点，不清理就是只增不减。
 */
struct Entry {
  std::string out;
  std::chrono::steady_clock::time_point born;
};

std::mutex g_mu;
std::map<int, Entry> g_results;
int g_next_ticket = 1;

/** 取一个字符串参数；非字符串或缺失返回空串（不抛，调用方自己兜底）。 */
std::string Arg(napi_env env, napi_value v) {
  size_t len = 0;
  if (napi_get_value_string_utf8(env, v, nullptr, 0, &len) != napi_ok) {
    return {};
  }
  std::string s(len, '\0');
  // 用 &s[0] 而不是 s.data()：`std::string::data()` 的非 const 重载是 C++17 才有的，
  // 这套工具链按 C++14 编（NDK 的 CMake 工具链没设 CMAKE_CXX_STANDARD），
  // 在那个标准下 data() 只返回 const char*，NAPI 收的是 char*，直接编译不过
  if (napi_get_value_string_utf8(env, v, &s[0], len + 1, &len) != napi_ok) {
    return {};
  }
  return s;
}

/** 造一个 JS 字符串返回值。 */
napi_value Str(napi_env env, const std::string &s) {
  napi_value out = nullptr;
  napi_create_string_utf8(env, s.c_str(), s.size(), &out);
  return out;
}

/**
 * 调一次 Rust 宿主并把结果拷成 std::string。
 *
 * `goptop_call` 的返回值是堆上分配、**必须**由 `goptop_free` 释放的——漏掉就是
 * 每次调用泄漏一块（落子每次几十字节，长对局下会累积）。
 */
std::string CallRust(const std::string &cmd, const std::string &args) {
  char *raw = goptop_call(cmd.c_str(), args.c_str());
  if (raw == nullptr) {
    return "null";
  }
  std::string out(raw);
  goptop_free(raw);
  return out;
}

/* ---------------- 同步命令 ---------------- */

napi_value Call(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2] = {nullptr, nullptr};
  if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc < 1) {
    // 自定义定界符 `json`：默认的 `)"` 会撞上正文里的 `argsJson)"}`，把原始字符串
    // 提前收尾，报的是「expected ')'」——看着像括号不配，其实在字符串里
    return Str(env, R"json({"error":"goptop.call 需要 (cmd, argsJson)"})json");
  }
  // 少传 argsJson 时按空对象处理：Rust 侧对缺失参数一律走各自的默认值/no_game
  std::string args = argc >= 2 ? Arg(env, argv[1]) : "{}";
  if (args.empty()) {
    args = "{}";
  }
  return Str(env, CallRust(Arg(env, argv[0]), args));
}

/* ---------------- AI：后台算 + 轮询取 ---------------- */

struct AiJob {
  std::string req;
  int ticket;
};

/** async work 的执行体：跑在 NAPI 的线程池上，**不是** JS 线程。 */
void AiExecute(napi_env /*env*/, void *data) {
  auto *job = static_cast<AiJob *>(data);
  std::string out = CallRust("ai_analyze", job->req);
  std::lock_guard<std::mutex> lock(g_mu);
  // 只改 out 不动 born：保留占位时的出生时刻，寿命从 post 起算。
  // 占位可能已被上限驱逐（operator[] 重建出零值 born 的条目），补成当前时刻，
  // 否则重建的条目下一轮清扫就会被立刻回收。
  Entry &e = g_results[job->ticket];
  e.out = std::move(out);
  if (e.born == std::chrono::steady_clock::time_point{}) {
    e.born = std::chrono::steady_clock::now();
  }
}

/** async work 的完成体：跑回 JS 线程，这里只做清理（结果已由 AiExecute 写好）。 */
void AiComplete(napi_env /*env*/, napi_status /*status*/, void *data) {
  delete static_cast<AiJob *>(data);
}

napi_value AiPost(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1] = {nullptr};
  if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc < 1) {
    napi_throw_error(env, nullptr, "goptop.aiPost 需要 (reqJson)");
    return nullptr;
  }
  auto *job = new AiJob{Arg(env, argv[0]), 0};
  int ticket = 0;
  {
    std::lock_guard<std::mutex> lock(g_mu);
    // 顺带清扫陈旧条目（60s 远超分析最长预算 3s，正常在算的占位不会被误扫；
    // 即使被扫——进程冻结超过 57s 的病态情形——AiExecute 的 operator[] 会重建，
    // 行为无害）：
    const auto now = std::chrono::steady_clock::now();
    for (auto it = g_results.begin(); it != g_results.end();) {
      if (now - it->second.born > std::chrono::seconds(60)) {
        OH_LOG_DEBUG(LOG_APP, "AI 票号 %{public}d 超龄无人认领，回收", it->first);
        it = g_results.erase(it);
      } else {
        ++it;
      }
    }
    ticket = g_next_ticket++;
    job->ticket = ticket;
    g_results.emplace(ticket, Entry{std::string(), now});  // 占位：空串即「还没算完」
    // 硬上限兜底：按时间的清扫依赖「之后还有下一次 aiPost」才触发，短时间反复
    // 弃票时条目仍会先堆积，这里把表压回 16 条。丢的必然是最老的低票号——正常
    // 并发不过两三张票，能堆到 17 张只可能是早已弃取的票；在算占位被驱逐也无害
    // （同上，AiExecute 会重建）。
    if (g_results.size() > 16) {
      g_results.erase(g_results.begin());
    }
  }
  // `async_resource_name` **不能传 nullptr**：Node 的 NAPI 文档写明它是必填，
  // 传空在 OHOS 上会让 napi_create_async_work 直接返回 napi_invalid_arg，
  // 表现为「无法排队 AI 分析任务」——而真正的错因（参数为空）得翻 NAPI 源码才看得出来。
  napi_value res_name = nullptr;
  napi_create_string_utf8(env, "goptop.ai", NAPI_AUTO_LENGTH, &res_name);
  napi_async_work work = nullptr;
  napi_status cst = napi_create_async_work(env, nullptr, res_name, AiExecute, AiComplete, job, &work);
  napi_status qst = cst == napi_ok ? napi_queue_async_work(env, work) : cst;
  if (qst != napi_ok) {
    OH_LOG_ERROR(LOG_APP, "AI 任务排队失败 create=%{public}d queue=%{public}d", (int)cst, (int)qst);
    if (cst == napi_ok) {
      // create 成功而 queue 失败：work 从未入队，完成回调不会执行，运行时不会
      // 接手回收——句柄所有权仍在调用方，必须显式删除（create 就失败时 work 本
      // 为 nullptr，守卫避免对空句柄调用）
      napi_delete_async_work(env, work);
    }
    std::lock_guard<std::mutex> lock(g_mu);
    g_results.erase(ticket);
    delete job;
    napi_throw_error(env, nullptr, "无法排队 AI 分析任务");
    return nullptr;
  }
  // 票号取自局部变量而不是 `job->ticket`：任务已经排队，若它跑得够快，
  // `AiComplete` 会在这一行之前把 `job` 删掉——读它就是一个 use-after-free。
  // work 句柄由运行时在完成后回收（上面 queue 失败分支里则由本侧显式删除）。
  napi_value out = nullptr;
  napi_create_int32(env, ticket, &out);
  return out;
}

napi_value AiPoll(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1] = {nullptr};
  if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc < 1) {
    return Str(env, "");
  }
  int32_t ticket = 0;
  if (napi_get_value_int32(env, argv[0], &ticket) != napi_ok) {
    return Str(env, "");
  }
  std::lock_guard<std::mutex> lock(g_mu);
  auto it = g_results.find(ticket);
  if (it == g_results.end() || it->second.out.empty()) {
    return Str(env, "");  // 未完成（或票号早已取走）
  }
  // 取走即删：票号是一次性的，重复取会返回空串而不是重复交付同一份结果
  std::string out = std::move(it->second.out);
  g_results.erase(it);
  return Str(env, out);
}

/* ---------------- 模块注册 ---------------- */

napi_value Init(napi_env env, napi_value exports) {
  napi_property_descriptor desc[] = {
      {"call", nullptr, Call, nullptr, nullptr, nullptr, napi_default, nullptr},
      {"aiPost", nullptr, AiPost, nullptr, nullptr, nullptr, napi_default, nullptr},
      {"aiPoll", nullptr, AiPoll, nullptr, nullptr, nullptr, napi_default, nullptr},
  };
  napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
  return exports;
}

napi_module g_module = {
    .nm_version = 1,
    .nm_flags = 0,
    .nm_filename = nullptr,
    .nm_register_func = Init,
    // 名字决定 ArkTS 侧的 `import … from 'libgoptop.so'`
    .nm_modname = "goptop",
    .nm_priv = nullptr,
    .reserved = {nullptr},
};

}  // namespace

// **必须 `used`**：鸿蒙的构建带 `-ffunction-sections` + `--gc-sections`，而本函数除了
// 被 `.init_array` 引用之外没有任何调用点——没有 `used` 时它会被整段回收，`.init_array`
// 剩一个全零的段（实测如此）。后果是模块**从不注册**：`import … from 'libgoptop.so'`
// 得到一个空对象，方法调用在 ArkTS 里抛异常，ArkWeb 只转述成一句
// 「napi api call fail」，完全看不出是「构造器被 GC 掉了」。
extern "C" __attribute__((constructor, used)) void RegisterGoptopModule(void) {
  napi_module_register(&g_module);
}
