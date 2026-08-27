---
name: tauri-native-chrome
description: >
  Tauri 2 native window chrome integration — embed app content into the OS title bar strip while keeping every system affordance (caption buttons, Snap Layouts, system menu, double-click maximize, focus dimming, touch, a11y) owned by the OS. Use this skill whenever the user mentions Tauri title bar, native chrome, DWM custom frame, window controls, caption buttons, WindowControls, MenuRow in titlebar, titleBarStyle, decorations, overlay titlebar, or wants app content inside the native title bar on Windows, macOS, or Linux.
---

# Tauri Native Chrome Integration

Embed app content into the OS title bar strip without taking ownership of any system control.

## Core Invariant

Never custom-draw system chrome. Caption buttons (min / max / close), window frame, system menu, snap/docking, double-click maximize, right-click system menu, inactive dimming, touch, IME, and accessibility are owned and rendered by the OS. The only narrow exception is a controlled "glyph-drawn + action-forwarded" pattern for window-control glyphs (font glyphs drawn in the page, actions forwarded through Tauri window APIs) — and even then the hit region for the maximize button and its Snap Layouts behavior stay on the OS.

## When to Use

- You want menus, search fields, status indicators, or other app UI to live inside the title bar strip instead of on a separate row below it.
- You need Windows Snap Layouts, the system menu, double-click to maximize/restore, and inactive dimming to keep working.
- You are customizing window chrome in a Tauri 2 app on Windows, macOS, or Linux.

## Platform Overview

| Platform | Approach | Outcome |
|----------|----------|---------|
| **Windows** | DWM Custom Frame (`DwmExtendFrameIntoClientArea` + `WM_NCCALCSIZE` + `WM_NCHITTEST` + composition hosting) | Only correct path — see below and `references/windows.md` |
| **macOS** | `titleBarStyle: Overlay` — traffic lights remain system-owned, content flows into the bar; app menus go to the system menu bar | First-class Tauri capability |
| **Linux** | SSD by default (WM draws chrome); whether to use CSD is a per-environment decision | Content cannot enter the bar under X11 SSD — that is a real platform boundary |

> Details: `references/windows.md` / `references/macos-linux.md` / `references/frontend.md`.

## Windows DWM Custom Frame — Three Moves + Two Prerequisites

### Move 1 — Extend the client area to the top (`WM_NCCALCSIZE`)

When `wParam == TRUE`, let the default handler compute the standard client rect, then reclaim `rgrc[0].top` back to the window top. On maximized windows the window rect extends beyond the screen by one frame thickness — compensate by adding `fy` (`SM_CYSIZEFRAME + SM_CXPADDEDBORDER`) back.

### Move 2 — Hit-test in three stages (`WM_NCHITTEST`, order is load-bearing)

```
1. DwmDefWindowProc first — caption-button hits belong to DWM (enables Snap Layouts)
2. DefSubclassProc       — resize borders and corners belong to the system
3. Manual               — top border HTTOP / drag strip HTCAPTION / everything else HTCLIENT to WebView
```

### Move 3 — Re-extend after activation and DPI changes

On `WM_ACTIVATE` and `WM_DPICHANGED`, re-call `DwmExtendFrameIntoClientArea` (margins are physical pixels, so they must be recomputed after a DPI change) and force an immediate re-layout with `SWP_FRAMECHANGED`.

### Two prerequisites for visible caption buttons (both required)

| Prerequisite | What it does | If omitted |
|--------------|--------------|------------|
| `WS_EX_NOREDIRECTIONBITMAP` added to `GWL_EXSTYLE` after window creation | Removes the redirection bitmap so the DWM frame can paint on top of DirectComposition content | Buttons are logically present (`DwmDefWindowProc` returns 8/9/20) but invisible |
| `DefaultBackgroundColor` alpha = 0 + fully transparent button reserve in the page | Composition content sits above the DWM frame in z-order; every pixel in the button reserve must be transparent to let the frame show through | Same — opaque page pixels cover the DWM buttons |

> wry's default hosts WebView2 as a child window (`Chrome_WidgetWin_0`) that physically covers the DWM buttons. You must switch to **composition-controller hosting** (`ICoreWebView2CompositionController` + a DirectComposition visual tree, no child window). See `references/windows.md`.

### Button-zone partitioning (must stay in sync with the frontend)

- Three standard buttons, each **46 logical pixels** wide, **138 logical pixels** total reserve on the right. The frontend must not paint any opaque pixel in this reserve.
- **Maximize** (`HTMAXBUTTON` or the hit returned by `DwmDefWindowProc`) — leave hit-testing and the action to the OS so Snap Layouts and maximize/restore work natively. The frontend only draws the glyph and hover state; it attaches no click handler.
- **Minimize / Close** — return `HTCLIENT` so the page receives the click, then forward via Tauri window APIs (`getCurrentWindow().minimize()` / `hide()` or `close()`).
- Every other interactive element in the title bar strip returns `HTCLIENT`; the single drag strip returns `HTCAPTION`.

### System menu on right-click

After client-area extension, `DefWindowProc` no longer shows the system menu automatically for `HTCAPTION` hits. Re-invoke it manually — the menu itself remains a system menu, drawn and managed by the OS:

```rust
let menu = GetSystemMenu(hwnd, false);
SetForegroundWindow(hwnd); // required before TrackPopupMenuEx
let cmd = TrackPopupMenuEx(
    menu,
    TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
    x, y, hwnd, None,
).0;
if cmd != 0 {
    PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(cmd as usize), lparam);
}
```

## Minimal Tauri Project Wiring

### tauri.conf.json

```json
{
  "app": {
    "windows": [{
      "decorations": true,
      "visible": false
    }]
  }
}
```

- `decorations: true` — keep the standard styles (`WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`). Do not patch styles yourself.
- `visible: false` combined with a `show_main_window` command invoked after the first frame commits — "ready-to-show" pattern that prevents a white flash while WebView2 initializes.

### Cargo dependencies (Windows only)

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.61", features = [
  "Win32_Foundation",
  "Win32_Graphics_Dwm",
  "Win32_Graphics_Gdi",
  "Win32_Graphics_DirectComposition",
  "Win32_Graphics_Dxgi",
  "Win32_System_Com",
  "Win32_UI_HiDpi",
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_UI_Shell",
  "Win32_UI_WindowsAndMessaging",
] }
```

### Rust setup (`lib.rs` / `main.rs`)

```rust
#[cfg(target_os = "windows")]
{
    let window = app.get_webview_window("main").expect("main window");
    let dark = matches!(window.theme().ok(), Some(tauri::Theme::Dark));
    window_frame::install(&window, dark);
    let win = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::ThemeChanged(theme) = event {
            window_frame::set_nc_dark(&win, matches!(theme, tauri::Theme::Dark));
        }
    });
}

// in generate_handler!
#[cfg(target_os = "windows")] window_frame::set_drag_region,
window_theme::set_window_theme,
show_main_window,
```

Set `WS_EX_NOREDIRECTIONBITMAP` and call `DwmExtendFrameIntoClientArea` during `install`; issue `SWP_FRAMECHANGED` to apply immediately. See `references/windows.md` for the full subclass procedure.

## Generic Frontend Wiring

The pattern below is framework-agnostic (React, Vue, Svelte, Solid, or vanilla). Adapt the snippet to your framework — the mechanics are the same.

### 1. Keep geometry in sync

The backend constants (`BUTTONS_RESERVE = 138` logical px, `BUTTON_WIDTH = 46`, `BAND_HEIGHT = 40`) must match the frontend CSS. Changing one side without the other causes click offsets or opaque pixels covering the system buttons.

```css
.title-bar {
  position: relative; /* anchor for absolute-positioned window controls; do not create a stacking context */
  display: flex;
  align-items: center;
  height: 40px;
  /* right padding = button reserve (138px) + gap (8px) */
  padding: 0 calc(138px + 8px) 0 12px;
  /* do not set a background that covers the button reserve — keep that strip transparent on Windows */
}
```

```css
.window-controls {
  position: absolute;
  top: 0; right: 0; bottom: 0;
  display: flex;
  align-items: stretch;
  padding-left: 8px; /* matches the gap in .title-bar */
}
.window-controls button,
.window-controls [data-role="max"] {
  width: 46px; /* must match backend BUTTON_WIDTH */
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
```

### 2. Negotiate the drag strip

Exactly one stretchable gap in the title bar acts as the drag strip (`HTCAPTION`); every other element is `HTCLIENT`. The gap's rect is measured in the page (logical/CSS pixels) and reported to Rust, where the hit-test converts it to physical pixels using the current DPI scale.

```ts
// Vanilla / framework-agnostic — run on mount and on resize
import { invoke } from "@tauri-apps/api/core";

const spacer = document.getElementById("title-bar-spacer")!;

function reportDragRegion() {
  const rect = spacer.getBoundingClientRect();
  // x/width are logical (CSS) pixels; Rust side scales by dpi/96
  invoke("set_drag_region", { x: rect.left, width: rect.width }).catch(() => {
    // not running inside Tauri (e.g. browser preview) — silently ignore
  });
}

reportDragRegion();
const observer = new ResizeObserver(reportDragRegion);
observer.observe(spacer);
// call observer.disconnect() on unmount
```

Backend stores the region and maps it per-DPI in `WM_NCHITTEST`:

```rust
static DRAG_REGION: Mutex<(f32, f32)> = Mutex::new((0.0, 0.0));

#[tauri::command]
pub fn set_drag_region(x: f64, width: f64) {
    *DRAG_REGION.lock().unwrap() = (x as f32, (x + width) as f32);
}
// in WM_NCHITTEST, after confirming y is inside the title band:
let logical_x = (x - rect.left - fx) as f32 / scale;
let (lo, hi) = *DRAG_REGION.lock().unwrap();
if logical_x >= lo && logical_x < hi { return HTCAPTION; }
return HTCLIENT;
```

### 3. Window controls (glyph-drawn, action-forwarded)

```html
<div class="window-controls">
  <button id="btn-min" aria-label="Minimize">—</button>
  <!-- maximize: no click handler — hit goes to the OS so Snap Layouts works -->
  <span data-role="max" aria-hidden="true" id="btn-max">□</span>
  <button id="btn-close" aria-label="Close">×</button>
</div>
```

```ts
import { getCurrentWindow } from "@tauri-apps/api/window";

document.getElementById("btn-min")!.addEventListener("click", () => {
  getCurrentWindow().minimize().catch(() => {});
});
document.getElementById("btn-close")!.addEventListener("click", () => {
  // hide() for tray semantics, close() to quit — pick one
  getCurrentWindow().hide().catch(() => {});
});

// maximize glyph tracks window state; no click handler
let maximized = false;
try {
  const win = getCurrentWindow();
  maximized = await win.isMaximized();
  await win.onResized(async () => { maximized = await win.isMaximized(); });
} catch {}
// swap glyph: maximized ? "❐" : "□"
```

Use a window-control font (`Segoe Fluent Icons` / `Segoe MDL2 Assets` on Windows, `10px` centered in each `46px` cell) or plain text glyphs if you have no icon font. The maximize element should not use `cursor: pointer` — it is a visual owned by the OS at the hit-test level.

### 4. Theme synchronization

```ts
import { invoke } from "@tauri-apps/api/core";

function syncWindowTheme(theme: "system" | "light" | "dark") {
  invoke("set_window_theme", { theme: theme === "system" ? null : theme }).catch(() => {});
}
// "system" removes any pinned attribute and lets the OS drive it;
// "light"/"dark" pins DWMWA_USE_IMMERSIVE_DARK_MODE via window.set_theme on Windows
// and NSWindow.appearance on macOS.
```

On Windows, `window.set_theme` maps to `DWMWA_USE_IMMERSIVE_DARK_MODE` — the frame and buttons remain system-drawn, only their palette follows the app theme. Forward `ThemeChanged` events back through `set_nc_dark` so "follow system" tracks live.

### 5. Layering rule

No ancestor of the title bar may create a stacking context (`isolation: isolate`, `transform`, `filter`, `will-change`, `opacity < 1`, etc.). Otherwise dropdowns and popovers anchored to the title bar are clipped or mis-ordered. Keep the title bar at `z-index: auto` and let floating layers use design tokens for `z-index`.

See `references/frontend.md` for the full frontend contract including the layering discipline.

## Common Pitfalls

1. Custom-drawing caption buttons — you will never keep up with OS evolution (Snap Layouts, touch, a11y).
2. Forgetting `WS_EX_NOREDIRECTIONBITMAP` — most common cause of "logic works, buttons invisible."
3. Forgetting `DefaultBackgroundColor A=0` or leaving opaque pixels in the button reserve — same symptom, composition z-order covers the DWM frame.
4. Staying on wry's child-window host — physically covers the buttons; switching to composition hosting is required (may need a wry fork until upstream ships it).
5. Mismatched geometry (138 / 46 / 40 differ between Rust and CSS) — clicks land in the wrong zone or opaque pixels hide buttons.
6. Not reporting the drag strip or not scaling it by DPI — hit-test collapses to all-`HTCLIENT` or all-`HTCAPTION`.
7. Not re-extending the frame after DPI change or not issuing `SWP_FRAMECHANGED` — band height drifts on multi-monitor / scale change, or the patch does not apply on the current frame.
8. Missing `fy` compensation on maximized — top edge is clipped by one frame thickness.
9. Wrong `WM_NCHITTEST` order — `DwmDefWindowProc` must be first or system buttons and Snap Layouts are lost.
10. Not re-invoking the system menu for `HTCAPTION` — right-click does nothing after client-area extension.
11. Creating a stacking context on a title bar ancestor — dropdowns are clipped.
12. Forgetting `visible: false` + `show_main_window` — white flash on startup.

## Verification Checklist (POC / regression)

- [ ] System buttons appear and are DWM-drawn (screenshot).
- [ ] Hovering the maximize button shows Snap Layouts.
- [ ] Double-clicking the title band maximizes / restores.
- [ ] Right-clicking the title band shows the system menu (system-drawn).
- [ ] Inactive window dims the title bar (before/after screenshots).
- [ ] Interactive elements inside the title band (buttons, inputs) are clickable and focusable.
- [ ] The drag strip drags the window.
- [ ] Border resize handles work on all edges including the top (`HTTOP`).
- [ ] Multi-DPI and live theme switching track correctly.
- [ ] `EnumChildWindows` shows no child window covering the button area (composition-hosting proof).

## Reference Index

- `references/windows.md` — Windows DWM frame + composition hosting + wry fork, full detail
- `references/macos-linux.md` — macOS Overlay and Linux SSD/CSD branches
- `references/frontend.md` — Frontend contract: layout, drag negotiation, window controls, layering

## Development Principles

- Prove OS interactions with a minimal single-file POC before wiring into the app (screenshot + log evidence). Only promote to full wiring after the POC is green.
- Treat any claim of the form "the platform cannot do X" as unverified until backed by official docs or a local POC. Write "unverified" otherwise.
