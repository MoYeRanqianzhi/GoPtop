/**
 * PosterStrip —— 顶部黑条（poster-strip）+ Tauri 窗口控制三键（最小化/最大化/关闭）。
 *
 * 技能 .claude/skills/tauri-native-chrome 的落地（2026-09-13，用户拍板「三键融入黑条」
 * 且「必须是真实 Windows 控制按钮功能」）。
 *
 * 路径与边界：wry 0.55 的 WebView2 子窗口宿主覆盖 DWM caption 命中区，理想 DWM
 * 合成宿主未上游化（需 fork）。已实现真实 Snap Layouts：src-tauri/src/titlebar.rs
 * 在最大化按钮正上方放透明子窗口（WM_NCHITTEST 恒 HTMAXBUTTON），OS 提供真实
 * 悬停弹层与最大化/还原点击；最小化/关闭经 Tauri window API 转发（技能允许的窄例外）。
 * - Windows：Segoe Fluent Icons / Segoe MDL2 Assets 字形（与系统 caption 同源），
 *   最大化键无点击处理器（命中归 OS）。
 * - Linux：GNOME 风格 SVG 符号（WebKitGTK 无 MDL2 字体）；三键经 API 转发。
 * - macOS：红绿灯样式（未经实机验证，标记 untested）。
 * Web 端（非 Tauri）不渲染三键、不设拖拽区，黑条保持纯装饰。
 */
import { useEffect, useRef, useState } from "react";
import { isTauri } from "../net/links";

type Platform = "win" | "mac" | "lin";

function detectPlatform(): Platform {
  const ua = navigator.userAgent;
  if (ua.includes("Mac")) return "mac";
  if (ua.includes("Linux")) return "lin";
  return "win";
}

/** Linux（GNOME 风格）10×10 符号：细描边，与 Adwaita 窗口控制同构。 */
function LinGlyph({ kind }: { kind: "min" | "max" | "restore" | "close" }) {
  const stroke = { stroke: "currentColor", strokeWidth: 1, fill: "none" } as const;
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      {kind === "min" && <path d="M1 5.5 H9" {...stroke} />}
      {kind === "max" && <rect x="1.5" y="1.5" width="7" height="7" {...stroke} />}
      {kind === "restore" && (
        <>
          <rect x="1.5" y="3.5" width="5" height="5" {...stroke} />
          <path d="M3.5 1.5 H8.5 V6.5" {...stroke} />
        </>
      )}
      {kind === "close" && (
        <>
          <path d="M1.5 1.5 L8.5 8.5" {...stroke} />
          <path d="M8.5 1.5 L1.5 8.5" {...stroke} />
        </>
      )}
    </svg>
  );
}

export function PosterStrip() {
  const [maximized, setMaximized] = useState(false);
  const [maxHover, setMaxHover] = useState(false);
  const [platform] = useState<Platform>(detectPlatform);
  const maxBtnRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    document.documentElement.classList.add("app-tauri");
    return () => document.documentElement.classList.remove("app-tauri");
  }, []);

  // 最大化状态跟踪（谁触发的无所谓：OS overlay、tao 双击、Linux 转发键都会走 resize）
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        setMaximized(await win.isMaximized());
        const un = await win.onResized(async () => setMaximized(await win.isMaximized()));
        // StrictMode 双挂载：卸载先到则立即注销订阅，不留给二次挂载后泄漏
        if (cancelled) un();
        else off = () => un();
      } catch { /* 非 Tauri 环境（浏览器预览） */ }
    })();
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  // Windows：覆盖层把最大化点击回传为事件（titlebar.rs），这里执行真正的切换；
  // 悬停进入/离开同样回传（覆盖层吞掉了按钮的 mousemove，CSS :hover 失效），
  // 在按钮上手动挂悬停底色。悬停 Snap Layouts 弹层由 OS 基于 HTMAXBUTTON 命中提供。
  const overlayActive = isTauri() && platform === "win";
  useEffect(() => {
    if (!overlayActive) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unClick = await listen("titlebar://max-click", () => control("toggleMaximize"));
        const unHover = await listen("titlebar://max-hover", (e) => setMaxHover(e.payload === true));
        const un = () => { unClick(); unHover(); };
        if (cancelled) un();
        else off = un;
      } catch { /* 非 Tauri 环境 */ }
    })();
    return () => {
      cancelled = true;
      off?.();
    };
  }, [overlayActive]);

  // Windows Snap Layouts 覆盖层几何同步：按钮矩形（逻辑 px）上报 Rust。
  // 触发源：按钮尺寸变化（ResizeObserver）与窗口尺寸变化（resize 事件，含最大化）。
  useEffect(() => {
    if (!overlayActive) return;
    const btn = maxBtnRef.current;
    if (!btn) return;
    const report = () => {
      const r = btn.getBoundingClientRect();
      void (async () => {
        try {
          const { invoke } = await import("@tauri-apps/api/core");
          await invoke("snap_overlay_set_rect", { x: r.left, y: r.top, w: r.width, h: r.height });
        } catch { /* 非 Tauri 环境 */ }
      })();
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(btn);
    window.addEventListener("resize", report);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", report);
    };
  }, [overlayActive]);

  async function control(action: "minimize" | "toggleMaximize" | "close") {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow()[action]();
    } catch { /* 权限缺失或非 Tauri 环境：按钮无效即静默 */ }
  }

  if (!isTauri()) {
    return (
      <div className="poster-strip">
        <span className="poster-strip__text">
          GoPtop · P2P Gomoku & Go · Neubrutalism · 优先直连 · 服务器可选中转
        </span>
      </div>
    );
  }

  // 三键字形按平台规范：Windows=MDL2 字形（10px 居中），Linux=GNOME 风格 SVG，macOS=红绿灯。
  // Windows 最大化键不挂点击处理器：命中与动作归 OS（src-tauri/src/titlebar.rs 覆盖层）。
  const isWin = platform === "win";
  const glyph = (kind: "min" | "max" | "restore" | "close") => {
    if (platform === "win") {
      const code = kind === "min" ? "\uE921" : kind === "close" ? "\uE8BB" : maximized && kind === "max" ? "\uE923" : "\uE922";
      return <span className="wc-glyph--win" aria-hidden="true">{code}</span>;
    }
    if (platform === "lin") return <LinGlyph kind={maximized && kind === "max" ? "restore" : kind} />;
    // macOS 红绿灯（untested：本仓库暂无 mac 实机测试面）
    const dot = kind === "close" ? "#FF5F57" : kind === "min" ? "#FEBC2E" : "#28C840";
    return <span aria-hidden="true" style={{ width: 10, height: 10, borderRadius: 999, background: dot, display: "inline-block" }} />;
  };

  return (
    <div className="poster-strip" data-tauri-drag-region>
      <span className="poster-strip__text" data-tauri-drag-region>
        GoPtop · P2P Gomoku & Go · Neubrutalism · 优先直连 · 服务器可选中转
      </span>
      <div className="window-controls">
        <button className="wc-btn" aria-label="最小化" title="最小化" onClick={() => control("minimize")}>
          {glyph("min")}
        </button>
        <button
          ref={maxBtnRef}
          className={maxHover ? "wc-btn wc-btn--hover" : "wc-btn"}
          aria-label={maximized ? "还原" : "最大化"}
          title={maximized ? "还原" : "最大化"}
          onClick={isWin ? undefined : () => control("toggleMaximize")}
        >
          {glyph("max")}
        </button>
        <button className="wc-btn wc-btn--close" aria-label="关闭" title="关闭" onClick={() => control("close")}>
          {glyph("close")}
        </button>
      </div>
    </div>
  );
}
