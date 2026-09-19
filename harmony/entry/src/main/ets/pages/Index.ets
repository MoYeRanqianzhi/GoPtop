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
 * 平台存储（用户拍板 2026-09-19：数据按平台规定位置落盘）：鸿蒙不是 Tauri，Web 侧
 * 没有 Rust 后端，因此由本壳注入 `goptopStore` 代理，把前端 net/store 门面的读写
 * 落到**应用私有目录 filesDir/store.json**（无需任何权限）。注意：本模块 backup_config.json
 * 开了 allowToBackupRestore 且未配 includes，store.json 落在系统默认备份集内——「卸载即清除」
 * 只描述本地文件，系统备份/恢复可把它带回重装后的设备。
 * 前端探测到该代理即用平台存储，探测不到才回落 localStorage。
 */
import { webview } from '@kit.ArkWeb';
import { fileIo as fs } from '@kit.CoreFileKit';
import { common } from '@kit.AbilityKit';

// 虚拟域名（承载打包资源，公网不可达）。host 必须与前端 frontend/src/net/links.ts 的
// ARKWEB_SHELL_HOST 逐字一致：isShellRuntime() 靠它判定「打包壳」并决定分享链接基地址，
// 只改一处会让分享链接指向对方打不开的 appassets.goptop。
const SCHEME_HOST = 'https://appassets.goptop/';

/** 存储文件名（与桌面壳 Rust 侧同名，便于排查时对照）。 */
const STORE_FILE = 'store.json';

/**
 * 平台存储代理：注入给 Web 页的前端门面（javaScriptProxy）。
 *
 * 契约与 src-tauri/src/store.rs 一致：`load()` 返回整表 JSON 字符串，
 * `set/remove` 就地改一行并整表写回。**同步**方法——前端门面是同步读语义
 * （设置要在首帧可读），换异步会牵动 React 渲染时序。
 */
class GoptopStoreProxy {
  private path: string = '';

  constructor(dir: string) {
    this.path = `${dir}/${STORE_FILE}`;
  }

  /** 整表读；文件不存在/损坏都按空表处理（前端会据此决定是否迁移旧数据）。 */
  private readAll(): Record<string, string> {
    const out: Record<string, string> = {};
    try {
      const text = fs.readTextSync(this.path);
      if (text.length === 0) {
        return out;
      }
      const parsed = JSON.parse(text) as Record<string, string>;
      for (const k of Object.keys(parsed)) {
        out[k] = parsed[k];
      }
    } catch (e) {
      // 首次启动文件不存在是正常路径；损坏也按空表（前端首帧不至于白屏）
      return out;
    }
    return out;
  }

  /** 整表写：先截断重写（单文件小表，无需原子替换的复杂度）。 */
  private writeAll(map: Record<string, string>): void {
    const file = fs.openSync(this.path, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
    try {
      fs.writeSync(file.fd, JSON.stringify(map));
    } finally {
      fs.closeSync(file);
    }
  }

  /** 给前端门面的全量读口（JSON 字符串）。 */
  load(): string {
    return JSON.stringify(this.readAll());
  }

  set(key: string, value: string): void {
    const map = this.readAll();
    map[key] = value;
    this.writeAll(map);
  }

  remove(key: string): void {
    const map = this.readAll();
    const next: Record<string, string> = {};
    for (const k of Object.keys(map)) {
      if (k !== key) {
        next[k] = map[k];
      }
    }
    this.writeAll(next);
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
  store: GoptopStoreProxy = new GoptopStoreProxy(
    (getContext(this) as common.UIAbilityContext).filesDir
  );

  build() {
    Column() {
      // 初始 src 用根路径：SPA 路由把 /index.html 解析成用户主页段，/ 才是菜单页
      Web({ src: SCHEME_HOST, controller: this.controller })
        // 平台存储注入（前端 net/store 门面探测 window.goptopStore）
        .javaScriptProxy({
          object: this.store,
          name: 'goptopStore',
          methodList: ['load', 'set', 'remove'],
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
