/**
 * GoPtop HarmonyOS 壳页面：ArkWeb Web 组件加载打包进 rawfile 的前端构建产物。
 *
 * 方案（2026-09-13）：Tauri 没有 HarmonyOS target，Web 组件壳是唯一路径。
 * - 前端 dist 整体拷贝到 resources/rawfile/app/（**无自动脚本**：改完前端须手工重拷——
 *   删旧目录再整体复制 frontend/dist 并去掉 _redirects，步骤见
 *   .agents/docs/real-device-testing.md；漏拷则壳里跑的是旧产物）。
 * - 用虚拟域名 https://appassets.goptop 承载，onInterceptRequest 把路径映射到
 *   rawfile：带扩展名 → 静态资源；无扩展名 → SPA 路由回退 index.html。
 *   history.pushState 路由（nav()）在刷新/直达时也能命中回退。
 * - domStorageAccess 必须开：每标签页的用户 id 走 sessionStorage；平台存储不可用时
 *   （见下）的前端门面也回退到这里，关了会每启重生成身份。
 *
 * 平台存储（用户拍板 2026-09-19：数据按平台规定位置落盘）：由本壳注入 `goptopHost`
 * 代理，把前端 net/store 门面的读写落到**应用私有目录 filesDir/store.json**（无需任何权限）。注意：本模块 backup_config.json
 * 开了 allowToBackupRestore 且未配 includes，store.json 落在系统默认备份集内——「卸载即清除」
 * 只描述本地文件，系统备份/恢复可把它带回重装后的设备。
 * 前端探测到该代理即用平台存储，探测不到才回落 localStorage。
 */
import { webview } from '@kit.ArkWeb';
import { fileIo as fs } from '@kit.CoreFileKit';
import { common } from '@kit.AbilityKit';
// 原生宿主：NAPI 薄层（cpp/napi_init.cpp）转发到 Rust 的 goptop_call。
// **鸿蒙能跑原生代码，规则与 AI 都直接链接原生 crate**——wasm 只是 Web 端的编译目标
// （见 .agents/memory/2026-09-19-wasm-is-web-only-native-platforms-link-rust.md）。
import goptop from 'libgoptop.so';


// 虚拟域名（承载打包资源，公网不可达）。host 必须与前端 frontend/src/net/links.ts 的
// ARKWEB_SHELL_HOST 逐字一致：isShellRuntime() 靠它判定「打包壳」并决定分享链接基地址，
// 只改一处会让分享链接指向对方打不开的 appassets.goptop。
const SCHEME_HOST = 'https://appassets.goptop/';

/** 存储文件名（与桌面壳 Rust 侧同名，便于排查时对照）。 */
const STORE_FILE = 'store.json';

/**
 * 宿主能力代理：**一个对象、一次注册**，注入给 Web 页的前端门面。
 *
 * 两件事都在这里：
 * - **存储**（`storeLoad/storeSet/storeRemove`）：契约与 src-tauri/src/store.rs 一致，
 *   `storeLoad()` 返回整表 JSON 字符串，`set/remove` 就地改一行并整表写回。同步方法
 *   ——前端门面是同步读语义（设置要在首帧可读），换异步会牵动 React 渲染时序。
 * - **原生宿主**（`call/aiPost/aiPoll`）：转发到 NAPI（`libgoptop.so`），最终落到
 *   Rust 的 `goptop_call`。
 *
 * **为什么合成一个类**：`Web` 组件上的 `.javaScriptProxy()` 是单值属性，
 * 调用两次只有最后一次生效——实测先注册的 `goptopStore` 直接变成 undefined，
 * 而设备日志里只留一句「native proxy object not found」，看不出是被覆盖了。
 * ArkWeb 没有多对象重载（`web.d.ts` 里只有 `javaScriptProxy(value: JavaScriptProxy)`）。
 */
class GoptopHostProxy {
  private path: string = '';

  constructor(dir: string) {
    this.path = `${dir}/${STORE_FILE}`;
  }

  /**
   * 整表读。文件不存在 = 空表（首次启动的正常路径）；**解析失败必须报错**而不是
   * 当空表——与 Tauri 侧 src-tauri/src/store.rs 同位（那边有测试
   * corrupt_file_is_reported_not_silently_emptied 钉死这一契约）。损坏若按空表处理，
   * 下一次 storeSet 会用单键表把整张表覆盖掉，昵称/服务器/头像连同 P2P 身份
   * （goptop:userId）一起静默丢失。报错抛到前端后，net/store 门面的 try/catch 会
   * 走「平台存储不可用 → 回落浏览器存储」分支，数据不再往下丢。
   */
  private readAll(): Record<string, string> {
    const empty: Record<string, string> = {};
    if (!fs.accessSync(this.path)) {
      return empty;
    }
    const text = fs.readTextSync(this.path);
    if (text.trim().length === 0) {
      return empty;
    }
    try {
      const parsed = JSON.parse(text) as Record<string, string>;
      const out: Record<string, string> = {};
      for (const k of Object.keys(parsed)) {
        out[k] = parsed[k];
      }
      return out;
    } catch (e) {
      // 损坏文件先改名 .bad 留痕（保留最新一份样本供排查），再把错误抛出去
      const bad = `${this.path}.bad`;
      try {
        if (fs.accessSync(bad)) {
          fs.unlinkSync(bad);
        }
        fs.renameSync(this.path, bad);
      } catch (e2) {
        // 留痕失败不掩盖原始解析错误
        console.warn(`[goptopHost] store.json 损坏样本留痕失败: ${e2}`);
      }
      throw new Error(`store.json 损坏: ${e}`);
    }
  }

  /**
   * 整表写：先写 `store.json.tmp` 再 rename 覆盖（与 Tauri 侧 store.rs 的 save_to
   * 同构）。此前是 TRUNC 截断后原地重写——写一半被杀（保存头像这种 4MB 级大值时
   * 窗口不小）就剩半截或空 JSON，重启后按空表处理，全部设置与身份丢失。rename
   * 是原子的：任何时刻 store.json 要么是旧完整表，要么是新完整表。
   * 注意 rename 依赖 POSIX 语义「直接替换已存在目标」，设备上若实测拒绝覆盖，
   * 需改为先 unlinkSync 旧文件（残窗退化为一次「文件缺失」，下游已按空表处理）。
   */
  private writeAll(map: Record<string, string>): void {
    const tmp = `${this.path}.tmp`;
    const file = fs.openSync(tmp, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
    try {
      fs.writeSync(file.fd, JSON.stringify(map));
    } finally {
      fs.closeSync(file);
    }
    fs.renameSync(tmp, this.path);
  }

  /* ---- 存储面 ---- */

  /** 给前端门面的全量读口（JSON 字符串）。 */
  storeLoad(): string {
    return JSON.stringify(this.readAll());
  }

  storeSet(key: string, value: string): void {
    const map = this.readAll();
    map[key] = value;
    this.writeAll(map);
  }

  storeRemove(key: string): void {
    const map = this.readAll();
    const next: Record<string, string> = {};
    for (const k of Object.keys(map)) {
      if (k !== key) {
        next[k] = map[k];
      }
    }
    this.writeAll(next);
  }

  /* ---- 原生宿主面（转发到 NAPI → Rust） ----
   *
   * 契约与 src-tauri/src/rules.rs（Tauri command）同源——两边都只是
   * `goptop_core::json_api` 的宿主适配，前端因而能共用一份后端代码。
   *
   * **AI 必须拆成 post/poll**：`call('ai_analyze', …)` 是同步阻塞的纯计算，按预算要跑
   * 0.3~3 秒；javaScriptProxy 的方法在 UI 线程上执行，直接同步调会把界面冻死
   * （桌面端必须 `spawn_blocking`、Web 端必须开 Worker，是同一件事）。而
   * javaScriptProxy **不支持返回 Promise**，所以走「后台算 + JS 轮询票号」。
   */

  /** 同步命令（微秒级）：落子/悔棋/停一手/状态/计分/建局/释放。回执是 JSON 文本。 */
  call(cmd: string, argsJson: string): string {
    return goptop.call(cmd, argsJson);
  }

  /** 起一次后台 AI 分析，立即返回票号（结果用 aiPoll 取）。 */
  aiPost(reqJson: string): number {
    return goptop.aiPost(reqJson);
  }

  /** 取分析结果；未完成返回空串（票号一次性）。 */
  aiPoll(ticket: number): string {
    return goptop.aiPoll(ticket);
  }
}

// 远程调试（ArkWeb inspector），必须在任何 Web 实例创建前调用
webview.WebviewController.setWebDebuggingAccess(true);

function mimeOf(path: string): string {
  if (path.endsWith('.html')) return 'text/html';
  if (path.endsWith('.js')) return 'text/javascript';
  if (path.endsWith('.css')) return 'text/css';
  if (path.endsWith('.json')) return 'application/json';
  if (path.endsWith('.wasm')) return 'application/wasm';
  if (path.endsWith('.svg')) return 'image/svg+xml';
  if (path.endsWith('.png')) return 'image/png';
  if (path.endsWith('.jpg') || path.endsWith('.jpeg')) return 'image/jpeg';
  if (path.endsWith('.webp')) return 'image/webp';
  if (path.endsWith('.ico')) return 'image/x-icon';
  if (path.endsWith('.woff2')) return 'font/woff2';
  if (path.endsWith('.woff')) return 'font/woff';
  return 'application/octet-stream';
}

@Entry
@Component
struct Index {
  controller: webview.WebviewController = new webview.WebviewController();
  // filesDir 由应用上下文给出（应用私有目录，卸载即清）。
  // 用 getContext(this) 而非新的 this.getUIContext().getHostContext()：字段初始化
  // 发生在 UI 上下文绑定之前，新 API 在这个时机取不到（编译期告警是弃用提示，
  // API 20 仍可用；等迁到 aboutToAppear 初始化代理时再一并换）。
  host: GoptopHostProxy = new GoptopHostProxy(
    (getContext(this) as common.UIAbilityContext).filesDir
  );

  build() {
    Column() {
      // 初始 src 用根路径：SPA 路由把 /index.html 解析成用户主页段，/ 才是菜单页
      Web({ src: SCHEME_HOST, controller: this.controller })
        // **只注册这一个**（属性是单值，注册第二个会把第一个顶掉，见类说明）。
        // 前端探测 window.goptopHost：有它就走原生 Rust（规则/AI）+ 平台存储，
        // 没有才回落 wasm + localStorage（老壳仍能跑）。
        .javaScriptProxy({
          object: this.host,
          name: 'goptopHost',
          methodList: ['storeLoad', 'storeSet', 'storeRemove', 'call', 'aiPost', 'aiPoll'],
          controller: this.controller,
        })
        .domStorageAccess(true)
        .javaScriptAccess(true)
        .fileAccess(true)
        .mediaPlayGestureAccess(true)
        .zoomAccess(false)
        .width('100%')
        .height('100%')
        .onInterceptRequest((event: OnInterceptRequestEvent) => {
          const url = event.request.getRequestUrl();
          if (!url.startsWith(SCHEME_HOST)) {
            return null;
          }
          let path = url.substring(SCHEME_HOST.length).split('?')[0].split('#')[0];
          if (path === '' || path === '/') {
            path = 'index.html';
          }
          // 带扩展名 = 静态资源；否则视为 SPA 路由，回退 index.html
          const last = path.substring(path.lastIndexOf('/') + 1);
          const file = last.includes('.') ? `app/${path}` : 'app/index.html';
          const resp = new WebResourceResponse();
          resp.setResponseData($rawfile(file));
          resp.setResponseEncoding('utf-8');
          resp.setResponseMimeType(mimeOf(file));
          resp.setResponseHeader([
            { headerKey: 'Access-Control-Allow-Origin', headerValue: '*' },
            { headerKey: 'Cache-Control', headerValue: 'no-cache' },
          ]);
          resp.setResponseCode(200);
          resp.setReasonMessage('OK');
          return resp;
        })
    }
    .width('100%')
    .height('100%')
  }
}
