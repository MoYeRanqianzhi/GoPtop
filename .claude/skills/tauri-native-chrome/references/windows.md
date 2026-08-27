# Windows — DWM Custom Frame + Composition Hosting

> Applies to: Tauri 2 / wry / tao on Windows 10+ when embedding app content into the native title bar strip while keeping every system affordance.

## 1. Why This Path

- **Invariant**: window controls and system interactions must remain owned and rendered by the OS. Custom-drawing them is a maintenance trap and drops Snap Layouts, touch, and accessibility.
- **Canonical model**: Microsoft's "Custom Window Frame Using DWM" — `DwmExtendFrameIntoClientArea` + `WM_NCCALCSIZE` + `WM_NCHITTEST` + `DwmDefWindowProc`. Same pattern used by Chromium and Windows Terminal.
- **Tauri-specific reason**: wry hosts WebView2 as a child window (`Chrome_WidgetWin_0`) parented to the host HWND. That child physically covers the DWM frame, so buttons never appear. You must switch to composition-controller hosting.

## 2. Child Window vs. Composition Hosting

| Dimension | Child-window hosting (wry default) | Composition-controller hosting (target) |
|-----------|------------------------------------|-----------------------------------------|
| WebView carrier | `CreateCoreWebView2Controller` creates a child window | `CreateCoreWebView2CompositionController` + DirectComposition visual tree |
| Child window | Present (`EnumChildWindows` finds it) | None (no WebView child window under the host) |
| DWM buttons | Covered by the child window, invisible | Visible through transparent pixels |
| Input | Child window receives messages directly | Host forwards mouse via `SendMouseInput`; keyboard/IME handled by WebView2 itself |
| Dependency | Works out of the box | Requires a wry fork or building the composition path yourself with `webview2-com` + `DCompositionCreateDevice` |

**Minimal composition tree:**

```
DCompositionCreateDevice / DCompositionCreateDevice2  (pick one; DesktopDevice variant depends on your DirectComposition setup)
  → CreateTargetForHwnd(hwnd, true)
  → CreateVisual (root) → SetRoot(root)
  → CreateVisual (webview) → root.AddVisual(webview)
  → WebView2 Environment → CreateCoreWebView2CompositionController(hwnd)
  → composition.SetRootVisualTarget(webviewVisual as IUnknown)
  → device.Commit()
  → controller.SetBounds(clientRect)   // physical pixels; GetClientRect is already physical
```

`Bounds` defaults to `USE_RAW_PIXELS` (physical pixels), matching `GetClientRect` — no DPI conversion needed.

## 3. Two Prerequisites for Visible Buttons (Both Required)

1. **`WS_EX_NOREDIRECTIONBITMAP`** — add to `GWL_EXSTYLE` after window creation:

   ```rust
   let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
   SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_NOREDIRECTIONBITMAP.0) as isize);
   ```

   Removes the redirection bitmap so the DWM frame can paint on top of DirectComposition content. Without it, `DwmDefWindowProc` hit-testing and Snap Layouts still work logically — they just do not render.

2. **`DefaultBackgroundColor` alpha = 0 + fully transparent button reserve in the page** — in z-order, composition content sits above the DWM frame:

   ```rust
   let c2: ICoreWebView2Controller2 = controller.cast()?;
   c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 })?;
   ```

   Page side:

   ```css
   html, body { background: transparent; }
   .button-reserve { background: transparent; } /* every pixel in the reserve must be transparent */
   ```

   Every layer in the button reserve (WebView background + page background + reserve element) must be transparent for the DWM frame to show through. Other areas of the page can be opaque.

## 4. The Three Moves

### 4.1 Move 1 — `WM_NCCALCSIZE` (extend client area to the top)

```rust
WM_NCCALCSIZE if wparam.0 != 0 => {
    let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
    let proposed_top = params.rgrc[0].top;
    let ret = DefSubclassProc(hwnd, msg, wparam, lparam);
    let dpi = GetDpiForWindow(hwnd);
    let (_, fy) = frame_thickness(dpi); // SM_CYSIZEFRAME + SM_CXPADDEDBORDER
    params.rgrc[0].top = if IsZoomed(hwnd).as_bool() {
        proposed_top + fy  // maximized: window extends fy beyond screen, compensate
    } else {
        proposed_top       // normal: client reaches the window top
    };
    ret
}
```

Leave the left/right/bottom edges at the system-computed values to preserve resize borders and shadows.

### 4.2 Move 2 — `WM_NCHITTEST` (three stages, order is load-bearing)

```rust
WM_NCHITTEST => {
    // 1) DWM first — caption-button hits
    let mut hit = LRESULT(0);
    if DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut hit).as_bool() {
        return hit;
    }
    // 2) Default chain — resize borders
    let hit = DefSubclassProc(hwnd, msg, wparam, lparam);
    if hit != HTCLIENT { return hit; }

    // 3) Manual — top border HTTOP / button reserve / drag strip vs. interactive
    //    (see partitioning below)
}
```

**Button reserve partitioning** (three buttons, **46 logical px each**, **138 total**):

```rust
let btn = (46.0 * scale).round() as i32;
let zone_right = rect.right - fx;
let close_left = zone_right - btn;
let max_left   = close_left - btn;
let min_left   = max_left - btn;
if x >= close_left { return HTCLIENT; }           // close — page forwards via Tauri API
if x >= max_left {
    // maximize — let DWM own it (Snap Layouts + maximize/restore)
    let mut dwm = LRESULT(0);
    if DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut dwm).as_bool() { return dwm; }
    return LRESULT(9); // HTMAXBUTTON fallback
}
if x >= min_left { return HTCLIENT; }             // minimize — page forwards via Tauri API
```

### 4.3 Move 3 — Re-extend after activation and DPI changes

```rust
WM_ACTIVATE | WM_DPICHANGED => {
    let ret = DefSubclassProc(hwnd, msg, wparam, lparam);
    extend_top_frame(hwnd); // DwmExtendFrameIntoClientArea with physical-pixel margins
    ret
}
```

```rust
unsafe fn extend_top_frame(hwnd: HWND) {
    let dpi = GetDpiForWindow(hwnd);
    let margins = MARGINS {
        cxLeftWidth: 0, cxRightWidth: 0,
        cyTopHeight: band_height_physical(dpi), // BAND_HEIGHT_LOGICAL * dpi/96
        cyBottomHeight: 0,
    };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
}
```

Also call `extend_top_frame` once during `install`, then force an immediate re-layout:

```rust
SetWindowPos(hwnd, None, 0,0,0,0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
```

## 5. System Menu on Right-Click

After client-area extension, `DefWindowProc` no longer shows the system menu automatically for `HTCAPTION` hits. Re-invoke manually — the menu itself remains a system menu:

```rust
WM_NCRBUTTONUP if wparam.0 == HTCAPTION as usize => {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
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
    return LRESULT(0);
}
```

## 6. Input Forwarding (Composition Hosting)

Follow the `ViewComponent` pattern from the WebView2Samples repo:

- **Mouse**: `WM_MOUSEMOVE / LBUTTON* / RBUTTON* / MBUTTON* / MOUSEWHEEL / MOUSELEAVE` → `ICoreWebView2CompositionController::SendMouseInput`
  - `eventKind` = the WM message value (enum values match message numbers)
  - `virtualKeys` = `wparam & 0xFFFF` (`GET_KEYSTATE_WPARAM`)
  - `mouseData` = wheel delta (`wparam >> 16`, signed) for wheel messages, `0` otherwise
  - Wheel coordinates are in screen space — convert with `ScreenToClient`
  - `SetCapture` on button-down, `ReleaseCapture` on button-up; register `TrackMouseEvent(TME_LEAVE)` on first `MOUSEMOVE` to produce `MOUSELEAVE`
  - Forward `WM_MOUSELEAVE` with all-zero coordinates per the API contract
- **Keyboard / IME**: do not forward. In composition hosting, WebView2 manages keyboard and IME internally; the host only needs to forward spatial (mouse/touch/pen) input.
- **Size / position**: `WM_SIZE` → `controller.SetBounds(GetClientRect)`; `WM_MOVE` / `WM_MOVING` → `NotifyParentWindowPositionChanged`
- **Cursor**: handle `CursorChanged` → `SetClassLongPtrW(GCLP_HCURSOR, cursor)`

## 7. Metrics and DPI

```rust
fn frame_thickness(dpi: u32) -> (i32, i32) {
    (
        GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi),
        GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi),
    )
}
fn band_height_physical(dpi: u32) -> i32 {
    (BAND_HEIGHT_LOGICAL * dpi as f32 / 96.0).round() as i32
}
```

`BAND_HEIGHT_LOGICAL` must match the frontend title bar row height (40 logical px is a common choice). Keep the two sides in sync.

## 8. Dark Non-Client Area

```rust
fn apply_dark_nc(hwnd: HWND, dark: bool) {
    let v: i32 = if dark { 1 } else { 0 };
    DwmSetWindowAttribute(
        hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE,
        &raw const v as *const _, size_of::<i32>() as u32,
    );
}
```

Tauri's `window.set_theme(Some(Theme::Dark/Light))` maps to this attribute on Windows. Keep it in sync on `ThemeChanged` events.

## 9. Subclass Chain

Install with `SetWindowSubclass` (e.g. `uIdSubclass = 1`). This coexists with wry's own mouse-forwarding subclass — your procedure only intercepts non-client geometry, hit-testing, and the system-menu right-click; all other messages pass through via `DefSubclassProc`.

## 10. Diagnostic Switches (POC Only, Remove for Production)

- `DSAGENTS_POC_EXTEND=500` — force the top margin to 500 physical px to verify the extension is effective.
- `DSAGENTS_POC_NO_NOREDIR=1` — skip adding `WS_EX_NOREDIRECTIONBITMAP` to reproduce the "logic works, buttons invisible" failure.
- `DSAGENTS_POC_NO_SUBCLASS=1` — skip subclass installation entirely for a bare-window baseline.
