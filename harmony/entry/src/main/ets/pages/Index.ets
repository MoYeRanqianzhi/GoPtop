/**
 * GoPtop HarmonyOS 壳页面：ArkWeb Web 组件加载打包进 rawfile 的前端构建产物。
 *
 * 方案（2026-09-13）：Tauri 没有 HarmonyOS target，Web 组件壳是唯一路径。
 * - 前端 dist 整体拷贝到 resources/rawfile/app/（构建前由脚本同步）。
 * - 用虚拟域名 https://appassets.goptop 承载，onInterceptRequest 把路径映射到
 *   rawfile：带扩展名 → 静态资源；无扩展名 → SPA 路由回退 index.html。
 *   history.pushState 路由（nav()）在刷新/直达时也能命中回退。
 * - domStorageAccess 必须开：identity/头像等存 localStorage，关了每启重生成。
 */
import { webview } from '@kit.ArkWeb';

const SCHEME_HOST = 'https://appassets.goptop/';

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

  build() {
    Column() {
      // 初始 src 用根路径：SPA 路由把 /index.html 解析成用户主页段，/ 才是菜单页
      Web({ src: SCHEME_HOST, controller: this.controller })
        .domStorageAccess(true)
        .javaScriptAccess(true)
        .fileAccess(true)
        .mediaPlayGestureAccess(true)
        .zoomAccess(false)
        .width('100%')
        .height('100%')
        .onInterceptRequest((event) => {
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
