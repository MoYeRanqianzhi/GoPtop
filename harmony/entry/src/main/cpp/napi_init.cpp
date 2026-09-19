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

#include <cstdlib>
#include <map>
#include <mutex>
#include <string>

// Rust 侧（crates/goptop-ohos）导出的 C ABI。返回的字符串必须交 goptop_free 释放。
extern "C" char *goptop_call(const char *cmd, const char *args_json);
extern "C" void goptop_free(char *p);

namespace {

/** 票号表：票 → 结果（空 = 还在算）。**只有完成回调与 aiPoll 会碰它**。 */
std::mutex g_mu;
std::map<int, std::string> g_results;
int g_next_ticket = 1;

/** 取一个字符串参数；非字符串或缺失返回空串（不抛，调用方自己兜底）。 */
std::string Arg(napi_env env, napi_value v) {
  size_t len = 0;
  if (napi_get_value_string_utf8(env, v, nullptr, 0, &len) != napi_ok) {
    return {};
  }
  std::string s(len, '\0');
  if (napi_get_value_string_utf8(env, v, s.data(), len + 1, &len) != napi_ok) {
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
    return Str(env, R"({"error":"goptop.call 需要 (cmd, argsJson)"})");
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
  g_results[job->ticket] = std::move(out);
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
  {
    std::lock_guard<std::mutex> lock(g_mu);
    job->ticket = g_next_ticket++;
    g_results[job->ticket] = "";  // 占位：空串即「还没算完」
  }
  napi_async_work work = nullptr;
  if (napi_create_async_work(env, nullptr, nullptr, AiExecute, AiComplete, job, &work) != napi_ok ||
      napi_queue_async_work(env, work) != napi_ok) {
    std::lock_guard<std::mutex> lock(g_mu);
    g_results.erase(job->ticket);
    delete job;
    napi_throw_error(env, nullptr, "无法排队 AI 分析任务");
    return nullptr;
  }
  // work 句柄本身由运行时在完成后回收；票号是给 JS 侧的唯一凭据
  napi_value out = nullptr;
  napi_create_int32(env, job->ticket, &out);
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
  if (it == g_results.end() || it->second.empty()) {
    return Str(env, "");  // 未完成（或票号早已取走）
  }
  // 取走即删：票号是一次性的，重复取会返回空串而不是重复交付同一份结果
  std::string out = std::move(it->second);
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

extern "C" __attribute__((constructor)) void RegisterGoptopModule(void) {
  napi_module_register(&g_module);
}
