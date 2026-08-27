# Frontend Contract — Framework-Agnostic

> This reference covers the page side of native chrome integration. All examples are vanilla HTML/CSS/JS. Adapt to React, Vue, Svelte, Solid, or any framework by translating the same DOM and measurement calls.

## 1. Layout Contract

### Title bar row

```html
<header class="title-bar">
  <div class="title-bar__left">
    <!-- app-specific: logo, menus, tabs, search, etc. -->
  </div>

  <!-- The ONLY HTCAPTION zone. Every other element in this row is HTCLIENT. -->
  <div id="title-bar-spacer" class="title-bar__spacer" aria-hidden="true"></div>

  <div class="title-bar__right">
    <!-- app-specific: status, actions -->
  </div>

  <div class="window-controls" aria-label="Window controls">
    <button id="btn-min" class="wc-btn" aria-label="Minimize">—</button>
    <span  id="btn-max" class="wc-btn wc-btn--passive" aria-hidden="true">□</span>
    <button id="btn-close" class="wc-btn wc-btn--close" aria-label="Close">×</button>
  </div>
</header>
```

```css
.title-bar {
  position: relative;              /* anchor for .window-controls; do NOT create a stacking context */
  display: flex;
  align-items: center;
  gap: 8px;
  height: 40px;                    /* must match backend BAND_HEIGHT */
  padding: 0 calc(138px + 8px) 0 12px; /* right reserve = 3×46px buttons + gap */
  border-bottom: 1px solid var(--line);
  /* On Windows, do not paint an opaque background over the button reserve.
     Keep that strip transparent so the DWM frame shows through. */
}

.title-bar__spacer {
  flex: 1;                         /* stretches to fill leftover space → the drag strip */
  align-self: stretch;
}

.window-controls {
  position: absolute;
  top: 0; right: 0; bottom: 0;
  display: flex;
  align-items: stretch;
  padding-left: 8px;               /* matches the 8px gap in .title-bar padding */
}

.wc-btn {
  width: 46px;                     /* must match backend BUTTON_WIDTH */
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  cursor: pointer;
}

.wc-btn--passive { cursor: default; }  /* maximize: owned by OS at hit-test level */
```

Key rules:

- `BAND_HEIGHT` (40), `BUTTON_WIDTH` (46), `BUTTONS_RESERVE` (138) — backend and CSS must agree. Change one, change the other.
- On Windows, the 138px button reserve must stay fully transparent (page background + container + buttons). Any opaque pixel there covers the DWM buttons.
- The maximize element has no click handler — the OS owns its hit region (`HTMAXBUTTON`) and its action (including Snap Layouts). It only paints a glyph and hover state.

### macOS traffic-light gutter

Leave ~70px at the left edge for the traffic lights (system-drawn). No Rust hit-test changes needed on macOS.

### Layering discipline

No ancestor of `.title-bar` may create a stacking context (`isolation: isolate`, `transform`, `filter`, `will-change`, `opacity < 1`, `backdrop-filter`, etc.). Keep `.title-bar` at `z-index: auto`. Floating layers (dropdowns, menus, popovers) anchor to the title bar and need a global `z-index` token that escapes to the root stacking context.

## 2. Drag-Strip Negotiation

Exactly one flexible gap in the title bar is the drag strip (`HTCAPTION`). All other elements are `HTCLIENT` and go to the WebView. The page measures the gap in logical/CSS pixels and reports it to Rust, where `WM_NCHITTEST` converts to physical pixels using `dpi / 96`.

```ts
import { invoke } from "@tauri-apps/api/core";

const spacer = document.getElementById("title-bar-spacer") as HTMLElement;

function reportDragRegion() {
  const rect = spacer.getBoundingClientRect();
  // logical pixels — backend scales by dpi/96 in WM_NCHITTEST
  invoke("set_drag_region", { x: rect.left, width: rect.width }).catch(() => {
    // not inside Tauri (browser preview / tests) — ignore
  });
}

reportDragRegion();
const observer = new ResizeObserver(reportDragRegion);
observer.observe(spacer);
// on unmount: observer.disconnect()
```

Backend (Rust) stores and consumes it:

```rust
static DRAG_REGION: Mutex<(f32, f32)> = Mutex::new((0.0, 0.0));

#[tauri::command]
pub fn set_drag_region(x: f64, width: f64) {
    *DRAG_REGION.lock().unwrap() = (x as f32, (x + width) as f32);
}

// inside WM_NCHITTEST, after confirming y is inside the title band:
let logical_x = (x - rect.left - fx) as f32 / scale;
let (lo, hi) = *DRAG_REGION.lock().unwrap();
if logical_x >= lo && logical_x < hi {
    return HTCAPTION;
}
return HTCLIENT;
```

If the gap is not reported, the band collapses to all-`HTCLIENT` (cannot drag) or all-`HTCAPTION` (cannot interact) depending on the fallback.

## 3. Window Controls — Glyph-Drawn, Action-Forwarded

```ts
import { getCurrentWindow } from "@tauri-apps/api/window";

// Minimize and close forward through Tauri APIs (system semantics, not custom logic).
document.getElementById("btn-min")!.addEventListener("click", () => {
  getCurrentWindow().minimize().catch(() => {});
});

document.getElementById("btn-close")!.addEventListener("click", () => {
  // Choose one: hide() to tray, close() to quit
  getCurrentWindow().hide().catch(() => {});
});

// Maximize: no click handler. Track state only for the glyph.
let maximized = false;
try {
  const win = getCurrentWindow();
  maximized = await win.isMaximized();
  await win.onResized(async () => { maximized = await win.isMaximized(); });
  // swap glyph: maximized ? "❐" : "□"
} catch {
  // not inside Tauri
}
```

Font/glyph notes (Windows):

- Use `Segoe Fluent Icons` with `Segoe MDL2 Assets` as fallback, `10px` centered in each `46px` cell, or plain text glyphs if no icon font is available.
- The close button hover is conventionally a red background with white glyph.
- Do not use `cursor: pointer` on the passive maximize element.

## 4. Theme Synchronization

```ts
import { invoke } from "@tauri-apps/api/core";

type ThemeMode = "system" | "light" | "dark";

function syncWindowTheme(theme: ThemeMode) {
  invoke("set_window_theme", { theme: theme === "system" ? null : theme }).catch(() => {});
}

function applyTheme(theme: ThemeMode) {
  if (theme === "system") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", theme);
  }
  syncWindowTheme(theme);
}
```

Rust side maps `window.set_theme` to `DWMWA_USE_IMMERSIVE_DARK_MODE` on Windows and `NSWindow.appearance` on macOS. On `ThemeChanged` (system light/dark toggled while in `"system"` mode), forward the new value through `set_nc_dark` so the non-client area tracks live. On Linux, `set_theme` is typically a no-op.

## 5. Ready-to-Show

Prevent the white flash while WebView2 initializes:

```json
// tauri.conf.json
{ "app": { "windows": [{ "visible": false }] } }
```

```rust
#[tauri::command]
fn show_main_window(window: tauri::WebviewWindow) {
    let _ = window.show();
}
```

```ts
// call once after the first frame is committed (e.g. on mount of the root component)
import { invoke } from "@tauri-apps/api/core";
invoke("show_main_window").catch(() => {});
```

Do not call `set_focus` here — the user may have switched away during startup.

## 6. Adapting to Frameworks

| Framework | Spacer ref | Report trigger | Cleanup |
|-----------|------------|----------------|---------|
| React | `useRef` + `useLayoutEffect` | `ResizeObserver` in effect | `observer.disconnect()` in cleanup |
| Vue | `ref` + `onMounted` / `watchEffect` | `ResizeObserver` | `onUnmounted` |
| Svelte | `bind:this` + `onMount` | `ResizeObserver` | return cleanup from `onMount` |
| Solid | `ref` + `onMount` | `ResizeObserver` | `onCleanup` |
| Vanilla | `getElementById` | `ResizeObserver` | `observer.disconnect()` |

The underlying calls (`getBoundingClientRect`, `invoke("set_drag_region", ...)`, `ResizeObserver`) are identical.
