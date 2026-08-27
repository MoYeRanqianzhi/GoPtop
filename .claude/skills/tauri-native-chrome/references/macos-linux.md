# macOS 与 Linux

## macOS — Overlay 标题栏

### 原理

macOS 窗口的交通灯（红黄绿）与标题栏由系统绘制，Tauri 官方支持 `titleBarStyle: Overlay` 将内容延伸进标题栏带，同时交通灯保持系统管理。

### tauri.conf.json

```json
{
  "app": {
    "windows": [{
      "titleBarStyle": "Overlay",
      "hiddenTitle": true,
      "decorations": true
    }]
  }
}
```

- `titleBarStyle: "Overlay"` — 内容进栏，交通灯悬浮于内容之上（系统画）。
- `hiddenTitle: true` — 不在标题栏重复显示窗口标题（标题由页面自定或省略）。
- 交通灯位置由系统决定，无需手动定位；内容需在左端留出交通灯避让区（约 70pt）。

### 应用菜单

macOS 应用菜单在屏幕顶部系统菜单栏，非窗口内。通过 Tauri 的 `App::set_menus` 或 `MenuBuilder` 声明，系统绘制与管理。

### 主题

`window.set_theme` 在 macOS 上映射为 `NSWindow.appearance`，与 Windows 的 `DWMWA_USE_IMMERSIVE_DARK_MODE` 对称。

---

## Linux — SSD / CSD 分支

### 现状

Linux 窗口装饰由窗口管理器（WM）决定，存在两种模式：

| 模式 | 装饰归属 | 内容进栏 |
|------|----------|----------|
| **SSD**（Server-Side Decoration） | WM 进程绘制标题栏与边框 | 应用内容进不去标题栏（真实平台边界） |
| **CSD**（Client-Side Decoration） | 应用进程绘制 | 可自定，但需应用承担全部装饰绘制与交互 |

### 策略

- **默认走 SSD**，将装饰交还 WM，不做 CSD 自绘。
- 是否为特定发行版/桌面环境（GNOME/KDE/Wayland/X11）启用 CSD，需单独呈报裁定后才动。X11 SSD 下"应用组件进标题栏"物理不可行，须如实告知而非强行自绘。
- Tauri 在 Linux 上的 `decorations` 与 `titleBarStyle` 行为随 WM 而异，需在目标发行版实测验证。

### 与 Windows/macOS 的差异

- 无 DWM 合成帧与 `WS_EX_NOREDIRECTIONBITMAP` 概念。
- 无交通灯 Overlay 语义。
- 主题跟随由 GTK/Qt 主题与 WM 共同决定，`window.set_theme` 在 Linux 上通常无效果。
