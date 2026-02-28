# pterminal — Rust 跨平台终端实现计划

> 基于 cmux 功能分析，使用 Rust 技术栈重新实现，目标：跨平台 (macOS/Linux/Windows)、小二进制、AI Agent 友好。

---

## 一、项目定位

| 维度 | cmux (参考) | pterminal (目标) |
|------|-------------|-------------|
| 语言 | Swift | Rust |
| 平台 | macOS only | macOS / Linux / Windows |
| GUI | AppKit + SwiftUI | wgpu + 自绘 (类 Alacritty/Zed) |
| 终端引擎 | libghostty (C) | alacritty_terminal + 自研渲染层 |
| 浏览器 | WKWebView | wry (系统 WebView 绑定) |
| 分屏 | Bonsplit (Swift) | 自研 Rust 分屏引擎 |
| IPC | Unix Socket | Unix Socket + Named Pipe (Windows) |
| CLI | 单文件 Swift | clap 子命令 |
| 二进制大小 | ~50MB+ (含 Ghostty) | 目标 < 15MB (strip + LTO) |
| 配置 | Ghostty config | TOML (兼容读取 Ghostty config) |

---

## 二、技术选型

### 2.1 核心依赖

| 模块 | crate | 理由 |
|------|-------|------|
| **窗口/输入** | `winit` | 跨平台窗口管理事实标准，成熟稳定 |
| **GPU 渲染** | `wgpu` | 跨平台 GPU 抽象 (Vulkan/Metal/DX12/WebGPU) |
| **文本渲染** | `glyphon` + `cosmic-text` | GPU 加速文本渲染，支持 font-fallback 和 shaping |
| **终端模拟** | `alacritty_terminal` | Alacritty 提取的终端模拟库，久经考验 |
| **PTY 管理** | `portable-pty` | 跨平台 PTY 抽象 (wezterm 出品) |
| ~~**WebView**~~ | ~~`wry`~~ | ~~暂不实现，后续按需引入~~ |
| **IPC** | `tokio` + 自研 | Unix Socket / Named Pipe，JSON-RPC 协议 |
| **CLI** | `clap` | 派生宏声明式 CLI，补全生成 |
| **配置** | `serde` + `toml` | TOML 配置解析 |
| **异步运行时** | `tokio` (rt-current-thread) | 轻量单线程异步，用于 IPC/端口扫描 |
| **序列化** | `serde` + `serde_json` | JSON-RPC 通信序列化 |
| **日志** | `tracing` | 结构化日志 |
| **通知** | `notify-rust` | 跨平台桌面通知 |
| **Git 信息** | `gix` (gitoxide) | 纯 Rust Git 实现，读分支名/状态 |
| **热键** | 自研 (基于 winit 事件) | 可自定义快捷键系统 |

### 2.2 二进制瘦身策略

```toml
# Cargo.toml [profile.release]
[profile.release]
opt-level = "z"          # 体积优化
lto = true               # 链接时优化
codegen-units = 1        # 单编译单元
panic = "abort"          # 不保留 unwind 表
strip = true             # 剥离符号

# 可选：用 cargo-bloat 分析、upx 压缩
```

策略：
- 特性门控 (feature gates)：浏览器、分析、自动更新均可选编译
- 不静态链接系统 WebView（wry 调用系统 WebKit/WebView2）
- 避免重量级依赖（如 reqwest，用 ureq 替代）
- 精选 tokio features，仅启用 `rt`, `net`, `io-util`, `sync`

### 2.3 放弃/替代的功能

| cmux 功能 | pterminal 决策 | 理由 |
|-----------|-----------|------|
| PostHog 匿名分析 | ❌ 不实现 | 减小体积，尊重隐私 |
| Sentry 错误追踪 | ❌ 不实现 | 用 panic hook + 本地 crash log 替代 |
| Sparkle 自动更新 | ⚡ 轻量替代 | 自研 GitHub Release 检查 + 提示用户下载 |
| Ghostty 配置兼容 | ✅ 只读兼容 | 可读取 ghostty config 作为 fallback |
| SwiftTerm 备用 | ❌ 不需要 | alacritty_terminal 已跨平台 |

---

## 三、项目结构

```
pterminal/
├── Cargo.toml                  # Workspace root
├── Cargo.lock
├── README.md
├── LICENSE
├── pterminal.toml.example          # 示例配置
│
├── crates/
│   ├── pterminal-core/             # 核心业务逻辑（无 GUI 依赖）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config/         # 配置系统
│   │       │   ├── mod.rs
│   │       │   ├── theme.rs    # 主题/配色方案
│   │       │   ├── keymap.rs   # 快捷键映射
│   │       │   └── ghostty_compat.rs  # Ghostty 配置兼容
│   │       ├── workspace/      # 工作区模型
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs  # TabManager 等价
│   │       │   ├── workspace.rs
│   │       │   └── panel.rs    # Panel trait + 类型
│   │       ├── terminal/       # 终端逻辑
│   │       │   ├── mod.rs
│   │       │   ├── pty.rs      # PTY 管理 (portable-pty)
│   │       │   ├── emulator.rs # alacritty_terminal 封装
│   │       │   └── search.rs   # 终端内搜索
│   │       ├── split/          # 分屏布局引擎
│   │       │   ├── mod.rs
│   │       │   ├── tree.rs     # 二叉分割树
│   │       │   └── layout.rs   # 布局计算
│   │       ├── notification/   # 通知系统
│   │       │   ├── mod.rs
│   │       │   └── store.rs    # 通知存储
│   │       ├── port_scanner.rs # 端口扫描
│   │       ├── git_info.rs     # Git 分支/状态
│   │       └── event.rs        # 内部事件总线
│   │
│   ├── pterminal-render/           # GPU 渲染层
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── renderer.rs     # wgpu 渲染器主体
│   │       ├── text.rs         # 文本/字体渲染 (glyphon)
│   │       ├── grid.rs         # 终端网格渲染
│   │       ├── cursor.rs       # 光标渲染 + 动画
│   │       ├── selection.rs    # 选区高亮
│   │       ├── scrollbar.rs    # 滚动条（自动隐藏）
│   │       └── shader/         # WGSL 着色器
│   │           ├── terminal.wgsl
│   │           └── ui.wgsl
│   │
│   ├── pterminal-ui/               # UI 层（自绘 GUI）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── app.rs          # 应用入口、事件循环
│   │       ├── window.rs       # 窗口管理
│   │       ├── sidebar/        # 垂直标签栏
│   │       │   ├── mod.rs
│   │       │   ├── tab_list.rs # 标签列表渲染
│   │       │   ├── drag.rs     # 拖拽排序
│   │       │   └── badge.rs    # 通知徽标
│   │       ├── workspace_view.rs  # 工作区内容视图
│   │       ├── panel_view.rs   # 面板渲染路由
│   │       ├── terminal_view.rs   # 终端面板视图
│   │       ├── search_bar.rs   # 搜索浮层
│   │       ├── command_palette.rs # 命令面板
│   │       ├── notification_page.rs # 通知面板
│   │       ├── input.rs        # 文本输入组件
│   │       └── theme.rs        # UI 主题/颜色
│   │
│   ├── pterminal-ipc/              # IPC 通信层
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs       # Socket 服务端 (App 侧)
│   │       ├── client.rs       # Socket 客户端 (CLI 侧)
│   │       ├── protocol.rs     # JSON-RPC 协议定义
│   │       ├── auth.rs         # 鉴权/安全模式
│   │       └── commands/       # 命令处理器
│   │           ├── mod.rs
│   │           ├── window.rs
│   │           ├── workspace.rs
│   │           ├── pane.rs
│   │           ├── terminal.rs
│   │           ├── notification.rs
│   │           └── status.rs
│   │
│   # pterminal-browser/ — 暂不实现，后续按需引入
│
├── src/                        # GUI 主二进制入口
│   └── main.rs
│
├── cli/                        # CLI 工具二进制
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       └── commands/           # clap 子命令
│           ├── mod.rs
│           ├── window.rs
│           ├── workspace.rs
│           ├── pane.rs
│           ├── terminal.rs
│           ├── notification.rs
│           └── status.rs
│
├── resources/
│   ├── themes/                 # 内置主题 (TOML)
│   │   ├── default-dark.toml
│   │   ├── default-light.toml
│   │   └── monokai.toml
│   ├── shell-integration/      # Shell 集成脚本
│   │   ├── bash.sh
│   │   ├── zsh.sh
│   │   ├── fish.fish
│   │   └── pwsh.ps1
│   ├── icons/                  # 应用图标
│   └── terminfo/               # terminfo 定义
│
├── tests/                      # 集成测试
│   ├── ipc_test.rs
│   ├── split_test.rs
│   └── e2e/                    # 端到端测试 (Python)
│       ├── pterminal.py            # 测试助手
│       └── test_*.py
│
└── scripts/
    ├── build-release.sh        # 发布构建
    ├── package-macos.sh        # macOS .app 打包
    ├── package-linux.sh        # Linux AppImage/deb
    └── package-windows.ps1     # Windows MSI/portable
```

---

## 四、核心架构设计

### 4.1 分层架构

```
┌─────────────────────────────────────────────────────────────────┐
│                     pterminal (main.rs 入口)                         │
├─────────────────────────────────────────────────────────────────┤
│  pterminal-ui          (自绘 GUI 层)                                 │
│  ├── Sidebar       ←→  WorkspaceView  ←→  CommandPalette       │
│  └── winit 事件循环                                              │
├─────────────────────────────────────────────────────────────────┤
│  pterminal-render      (GPU 渲染层)                                  │
│  ├── wgpu          (Vulkan/Metal/DX12)                          │
│  ├── glyphon       (文本渲染)                                    │
│  └── WGSL shaders  (终端网格/UI 元素)                            │
├─────────────────────────────────────────────────────────────────┤
│  pterminal-core        (核心业务逻辑，平台无关)                       │
│  ├── WorkspaceManager  →  Workspace[]  →  Panel (trait)         │
│  ├── SplitTree         (二叉分割树布局)                           │
│  ├── Terminal          (alacritty_terminal + portable-pty)       │
│  ├── NotificationStore                                          │
│  └── Config / Theme / Keymap                                    │
├─────────────────────────────────────────────────────────────────┤
│  pterminal-ipc         (IPC 通信层)                                  │
│  ├── JSON-RPC Server   (tokio 异步)                              │
│  └── Auth 安全模式                                               │
├─────────────────────────────────────────────────────────────────┤
├─────────────────────────────────────────────────────────────────┤
│  cli/              (独立二进制 pterminal-cli)                         │
│  └── clap 子命令   →  pterminal-ipc::Client                          │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 数据流

```
用户输入 (键盘/鼠标)
    │
    ▼
winit Event → App::handle_event()
    │
    ├─→ 快捷键路由 → KeymapResolver → Action
    │       │
    │       ├─→ WorkspaceManager (新建/切换/关闭标签)
    │       ├─→ SplitTree (分屏操作)
    │       └─→ CommandPalette (打开/搜索)
    │
    ├─→ 文本输入 → 当前 Panel
    │       │
    │       └─→ TerminalPanel → PTY write
    │
    └─→ 鼠标事件 → HitTest → Sidebar / SplitDivider / Panel

PTY 输出 (异步)
    │
    ▼
alacritty_terminal::Term::process_input()
    │
    ├─→ 屏幕缓冲区更新
    ├─→ 标题变更 → Sidebar 更新
    ├─→ 通知检测 → NotificationStore
    └─→ 请求重绘 → wgpu Renderer

IPC 命令 (异步)
    │
    ▼
pterminal-ipc::Server::accept()
    │
    ├─→ 鉴权检查
    ├─→ JSON-RPC dispatch → CommandHandler
    │       │
    │       └─→ 操作 WorkspaceManager / Terminal
    └─→ 返回 JSON 结果
```

### 4.3 核心 Trait 定义

```rust
/// 面板统一接口 — 对应 cmux 的 Panel 协议
pub trait Panel: Send + Sync {
    fn id(&self) -> PanelId;
    fn panel_type(&self) -> PanelType;
    fn title(&self) -> &str;
    fn is_dirty(&self) -> bool;

    fn focus(&mut self);
    fn unfocus(&mut self);
    fn close(&mut self);

    fn handle_input(&mut self, event: &InputEvent);
    fn render(&self, renderer: &mut Renderer, rect: Rect);

    fn has_unread_notification(&self) -> bool;
    fn trigger_flash(&mut self);
}

pub enum PanelType {
    Terminal,
    // Browser — 暂不实现
}

/// 分屏树节点
pub enum SplitNode {
    Leaf(PanelId),
    Split {
        direction: SplitDirection,
        ratio: f32,            // 0.0 ~ 1.0
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

pub enum SplitDirection {
    Horizontal,
    Vertical,
}
```

---

## 五、分阶段实施计划

### Phase 1: 骨架搭建 — 单终端窗口

**目标**: 能打开窗口、启动 shell、正确渲染终端输出、处理键盘输入。

**模块**:
- `pterminal-core/config` — TOML 配置加载 (字体、颜色、快捷键)
- `pterminal-core/terminal` — alacritty_terminal + portable-pty 封装
- `pterminal-render` — wgpu 初始化、终端网格渲染、文本渲染
- `src/main.rs` — winit 窗口 + 事件循环

**关键里程碑**:
1. winit 窗口 + wgpu surface 初始化
2. 字体加载 + glyphon 文本渲染
3. PTY 启动 shell 进程
4. alacritty_terminal 处理 VT 序列
5. 终端网格 → GPU 渲染管线
6. 键盘输入 → PTY 写入
7. 光标渲染 + 闪烁动画
8. 选区 + 复制/粘贴
9. 滚动 + 滚动条
10. ANSI 256 色 + TrueColor 渲染

### Phase 2: 多标签 + 分屏

**目标**: 垂直标签栏、工作区管理、水平/垂直分屏。

**模块**:
- `pterminal-core/workspace` — WorkspaceManager + Workspace + Panel trait
- `pterminal-core/split` — 二叉分割树
- `pterminal-ui/sidebar` — 垂直标签栏渲染
- `pterminal-ui/workspace_view.rs` — 分屏内容渲染

**关键里程碑**:
1. Panel trait 实现 (TerminalPanel)
2. SplitTree 布局引擎（插入/删除/调整比例）
3. 分屏拖拽调整分割线
4. WorkspaceManager 创建/删除/切换
5. 侧边栏标签列表渲染
6. 标签拖拽排序
7. 快捷键路由系统 (Keymap)
8. 焦点管理 (Tab / 方向键切换面板)

### Phase 3: 侧边栏增强

**目标**: 侧边栏显示 Git 分支、工作目录、监听端口。

**模块**:
- `pterminal-core/git_info.rs` — gix 读取 Git 分支/状态
- `pterminal-core/port_scanner.rs` — 端口扫描
- `pterminal-ui/sidebar/badge.rs` — 通知/状态徽标

**关键里程碑**:
1. 从 PTY cwd 提取当前工作目录
2. gix 读取 .git/HEAD → 分支名
3. 端口扫描 (合并请求批处理)
4. 侧边栏信息渲染 (分支 + 目录 + 端口)
5. 通知徽标 (未读数)

### Phase 4: IPC + CLI

**目标**: Socket API 服务端 + CLI 客户端，实现编程控制。

**模块**:
- `pterminal-ipc` — JSON-RPC over Unix Socket / Named Pipe
- `cli/` — clap 子命令

**关键里程碑**:
1. JSON-RPC 协议定义 (方法/参数/响应)
2. tokio 异步 Socket 服务端
3. 鉴权系统 (off / password / allow-all)
4. 核心命令实现:
   - `ping`, `capabilities`, `identify`
   - `list-windows`, `new-window`, `focus-window`, `close-window`
   - `list-workspaces`, `new-workspace`, `close-workspace`, `select-workspace`
   - `list-panes`, `new-split`, `focus-pane`
   - `send`, `send-key`, `read-screen`, `capture-pane`
   - `notify`, `list-notifications`, `clear-notifications`
   - `set-status`, `set-progress`, `log`
5. CLI 二进制 (clap) 连接 Socket 发送命令
6. Shell 补全生成 (bash/zsh/fish/pwsh)

### Phase 5: 通知系统 + tmux 兼容

**目标**: 终端通知检测、桌面通知、通知面板；确保 tmux 用户的通知链路畅通。

**模块**:
- `pterminal-core/notification` — 通知存储、已读/未读管理
- `pterminal-ui/notification_page.rs` — 通知面板 UI

**关键里程碑**:
1. OSC 序列 / bell / 自定义模式检测通知
2. NotificationStore 管理 (添加/已读/清除)
3. 桌面通知 (notify-rust)
4. 通知面板 UI (列表/跳转/清除)
5. 侧边栏/Dock 徽标联动
6. **tmux 兼容层** — 详见下方 Phase 5.1

#### Phase 5.1: tmux 兼容设计

**问题**: tmux 运行在 pterminal 的 PTY 内时，会拦截 OSC 序列，导致 PTY 通道的通知无法穿透。

**解决方案 — 双通道通知**:

```
通道 A (PTY 直连，无 tmux 时):
  AI Agent → OSC 777 → PTY → pterminal 检测 → 通知

通道 B (Socket 旁路，tmux 下推荐):
  AI Agent → pterminal-cli notify "消息" → Unix Socket → pterminal → 通知
```

**具体实现**:

1. **Shell 集成脚本自动注入** — pterminal 的 shell-integration 脚本检测到 `$TMUX` 环境变量时，
   自动将通知函数切换为 Socket 模式：
   ```bash
   # ~/.config/pterminal/shell-integration/bash.sh
   if [ -n "$TMUX" ]; then
     pterminal_notify() { pterminal-cli notify "$@"; }
   else
     pterminal_notify() { printf '\e]777;notify;%s;%s\a' "$1" "$2"; }
   fi
   ```

2. **tmux OSC 透传配置提示** — 首次检测到 tmux 运行时，提示用户添加 tmux 配置：
   ```tmux
   # 允许 OSC 序列透传到外层终端
   set -g allow-passthrough on
   ```
   启用后通道 A 也能工作（tmux 3.3a+），双保险。

3. **tmux 感知的 read-screen** — `pterminal-cli read-screen` 检测到 pane 内运行 tmux 时，
   可选通过 `tmux capture-pane` 获取 tmux 内部 pane 的原始内容（而非 tmux 渲染后的画面）：
   ```bash
   # 读 pterminal pane 的屏幕（看到的是 tmux 渲染后的内容）
   pterminal-cli read-screen

   # 读 tmux 内部活动 pane 的原始内容（穿透 tmux）
   pterminal-cli read-screen --tmux-passthrough
   ```

4. **send 命令穿透** — `pterminal-cli send` 的按键会发到 tmux，tmux 路由到活动 pane，
   这本身是正确行为，无需特殊处理。

**配置项**:
```toml
[tmux]
detect = true                    # 自动检测 tmux
passthrough_hint = true          # 首次检测到时提示配置 allow-passthrough
prefer_socket_notify = true      # tmux 下自动切换 Socket 通知
```

### ~~Phase 6: 内置浏览器~~ — 暂不实现

### Phase 7 → Phase 6: 高级功能

**目标**: 命令面板、搜索、自动更新、窗口装饰。

**模块**:
- `pterminal-ui/command_palette.rs`
- `pterminal-ui/search_bar.rs`
- 自动更新检查

**关键里程碑**:
1. 命令面板 (模糊搜索 + 快捷键提示)
2. 终端内搜索 (浮层 + 高亮)
3. GitHub Release 版本检查 + 提示更新
4. 多窗口支持
5. 窗口弹出 (终端/浏览器独立窗口)
6. 文件拖放到终端
7. 会话持久化 + 恢复

### Phase 7: 跨平台打磨

**目标**: 各平台特定适配与打包。

**关键里程碑**:
1. macOS: .app bundle 打包、Dock 图标/徽标、系统菜单栏
2. Linux: AppImage / .deb / Flatpak、XDG 桌面条目、Wayland + X11
3. Windows: MSI / portable zip、Named Pipe IPC、ConPTY
4. 各平台 CI/CD (GitHub Actions)
5. 跨平台 shell 集成脚本

---

## 六、配置文件设计

```toml
# ~/.config/pterminal/config.toml

[general]
shell = ""                      # 留空则使用 $SHELL 或系统默认
working_directory = ""          # 留空则使用 $HOME
confirm_close_process = true    # 关闭运行中进程时确认
new_workspace_placement = "after-current"  # "top" | "after-current" | "end"

[font]
family = "JetBrains Mono"
size = 14.0
bold_is_bright = false
# 可选覆盖
# family_bold = ""
# family_italic = ""

[theme]
name = "default-dark"           # 内置主题名 或 文件路径
# 覆盖单个颜色
# background = "#1e1e2e"
# foreground = "#cdd6f4"

[window]
opacity = 1.0                   # 0.0 ~ 1.0
blur = false                    # 背景模糊 (macOS/部分 Linux)
decorations = "full"            # "full" | "none" | "transparent"
startup_mode = "windowed"       # "windowed" | "maximized" | "fullscreen"

[scrollback]
lines = 10000
multiplier = 3                  # 鼠标滚轮乘数

[cursor]
style = "block"                 # "block" | "underline" | "beam"
blink = true
blink_interval_ms = 530

[sidebar]
width = 220
show_git_branch = true
show_cwd = true
show_ports = true
show_notification_badge = true

[notification]
enabled = true
detect_bell = true
detect_osc = true               # OSC 777 / OSC 9
# custom_patterns = ["error:", "FAIL"]

[tmux]
detect = true                    # 自动检测 tmux 运行
passthrough_hint = true          # 提示用户配置 allow-passthrough
prefer_socket_notify = true      # tmux 下 shell 集成自动用 Socket 通知

[ipc]
enabled = true
mode = "pterminal-only"             # "off" | "pterminal-only" | "password" | "allow-all"
# password = ""                 # 当 mode = "password" 时使用

[keybindings]
# 格式: "modifier+key" = "action"
"ctrl+shift+t" = "new-workspace"
"ctrl+shift+w" = "close-workspace"
"ctrl+shift+d" = "split-right"
"ctrl+shift+e" = "split-down"
"ctrl+shift+h" = "focus-left"
"ctrl+shift+l" = "focus-right"
"ctrl+shift+j" = "focus-down"
"ctrl+shift+k" = "focus-up"
"ctrl+shift+p" = "command-palette"
"ctrl+shift+f" = "search"
"ctrl+shift+n" = "notifications"
"ctrl+tab" = "next-workspace"
"ctrl+shift+tab" = "prev-workspace"
"ctrl+1..9" = "select-workspace-N"
```

---

## 七、IPC 协议设计

统一使用 JSON-RPC 2.0，简化 cmux 的 V1/V2 双协议：

```jsonc
// 请求
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "workspace.list",
  "params": { "window": "current" }
}

// 成功响应
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "workspaces": [
      { "id": "abc123", "title": "dev", "active": true, "pane_count": 3 }
    ]
  }
}

// 错误响应
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": { "code": -32602, "message": "Workspace not found" }
}
```

### 命令命名空间

| 命名空间 | 方法 | 对应 cmux |
|----------|------|-----------|
| `system` | `ping`, `capabilities`, `identify` | 同名 |
| `window` | `list`, `current`, `new`, `focus`, `close` | `*-window` |
| `workspace` | `list`, `new`, `close`, `select`, `current`, `rename`, `reorder` | `*-workspace` |
| `pane` | `list`, `new`, `focus`, `close`, `split`, `move`, `reorder`, `read_screen`, `capture` | `*-pane/*-surface` |
| `terminal` | `send`, `send_key` | `send`, `send-key` |
| ~~`browser`~~ | ~~暂不实现~~ | — |
| `notification` | `send`, `list`, `clear`, `mark_read` | `notify`, `list/clear-notifications` |
| `status` | `set`, `clear`, `list`, `set_progress`, `clear_progress` | `set/clear/list-status` |

---

## 八、关键技术难点与方案

### 8.1 终端 GPU 渲染

**难点**: 终端字符网格需要高效 GPU 渲染，每帧可能刷新数千个字符。

**方案**: 
- 字符单元实例化渲染 (instanced rendering)：每个单元格 = 1 个实例，包含 (行, 列, 字形索引, 前景色, 背景色, 属性标志)
- 字形缓存纹理图集 (glyph atlas)：使用 glyphon 预光栅化字形到纹理
- 脏区域追踪：只重新上传变化的行到 GPU buffer
- 参考实现：Alacritty 的渲染器设计

### 8.2 跨平台 PTY

**难点**: Unix PTY (forkpty) vs Windows ConPTY API 差异大。

**方案**: 
- 使用 `portable-pty` 统一抽象 (wezterm 团队维护，成熟可靠)
- Unix: /dev/ptmx → forkpty
- Windows: CreatePseudoConsole (ConPTY)

### 8.3 自绘 UI 框架

**难点**: 不使用 egui/iced 等框架，需自行处理 UI 元素渲染和交互。

**方案**:
- 最小化 UI 元素：侧边栏、分割线、搜索栏、命令面板、通知面板
- 使用 wgpu + glyphon 直接绘制（与终端共享渲染管线）
- 事件处理：winit 事件 → hit test → 路由到对应 UI 组件
- 优势：统一渲染管线、零额外依赖、完全可控

> **备选**：如果自绘 UI 工作量过大，可考虑使用 `iced` 作为 UI 框架，
> 它基于 wgpu，二进制增量约 2-3MB，提供现成的组件系统。

### 8.4 Named Pipe IPC (Windows)

**难点**: Windows 不支持 Unix Domain Socket。

**方案**: 
- 编译时条件：`#[cfg(unix)]` → tokio UnixListener，`#[cfg(windows)]` → tokio NamedPipeServer
- 协议层 (JSON-RPC) 保持一致，仅传输层差异

---

## 九、Cargo.toml Workspace 配置草案

```toml
[workspace]
resolver = "2"
members = [
    "crates/pterminal-core",
    "crates/pterminal-render",
    "crates/pterminal-ui",
    "crates/pterminal-ipc",
    # "crates/pterminal-browser",  # 暂不实现
    "cli",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
license = "MIT"

[workspace.dependencies]
# 终端
alacritty_terminal = "0.24"
portable-pty = "0.8"

# GPU 渲染
wgpu = "24"
winit = "0.30"
glyphon = "0.7"
cosmic-text = "0.12"

# 异步 / IPC
tokio = { version = "1", features = ["rt", "net", "io-util", "sync", "macros"] }

# 序列化 / 配置
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# CLI
clap = { version = "4", features = ["derive"] }

# Git
gix = { version = "0.68", default-features = false, features = ["max-performance-safe"] }

# 通知
notify-rust = "4"

# 日志
tracing = "0.1"
tracing-subscriber = "0.3"

# 工具
uuid = { version = "1", features = ["v4"] }
directories = "5"
```

---

## 十、二进制产物

| 二进制 | 功能 | 预估大小 (release strip) |
|--------|------|--------------------------|
| `pterminal` | GUI 终端应用 | ~6-10 MB |
| `pterminal-cli` | CLI 控制工具 | ~2-3 MB |

> 备注：浏览器功能暂不实现，后续以 feature gate 形式引入。

---

## 十一、与 cmux 的功能对照表

| cmux 功能 | pterminal Phase | 优先级 | 备注 |
|-----------|-------------|--------|------|
| 终端渲染 (Ghostty) | P1 | 🔴 必须 | alacritty_terminal 替代 |
| 垂直标签栏 | P2 | 🔴 必须 | |
| 水平/垂直分屏 | P2 | 🔴 必须 | |
| Ghostty 配置兼容 | P1 | 🟡 重要 | 只读解析 |
| Git 分支显示 | P3 | 🟡 重要 | |
| 端口扫描 | P3 | 🟡 重要 | |
| Socket API | P4 | 🔴 必须 | AI Agent 核心能力 |
| CLI 工具 | P4 | 🔴 必须 | |
| 通知系统 | P5 | 🔴 必须 | AI Agent 核心能力 |
| 内置浏览器 | — | ⏸️ 暂缓 | 后续按需引入 |
| 命令面板 | P6 | 🟡 重要 | |
| 终端内搜索 | P6 | 🟡 重要 | |
| 自动更新 | P6 | 🟢 可选 | 轻量实现 |
| 多窗口 | P6 | 🟢 可选 | |
| 文件拖放 | P6 | 🟢 可选 | |
| PostHog 分析 | — | ❌ 不做 | |
| Sentry 追踪 | — | ❌ 不做 | 本地 crash log 替代 |

---

## 十二、开发节奏建议

| 阶段 | 内容 | 产出 |
|------|------|------|
| **P1** | 单终端窗口 | 能用的终端，通过基本 VT 测试 |
| **P2** | 标签 + 分屏 | 多工作区、分屏，可日常使用 |
| **P3** | 侧边栏增强 | Git/端口/目录信息显示 |
| **P4** | IPC + CLI | Socket API + CLI 工具，可被 AI Agent 控制 |
| **P5** | 通知 | 通知检测/推送/面板 |
| **P6** | 高级功能 | 命令面板、搜索、更新、多窗口 |
| **P7** | 跨平台 | Linux/Windows 适配 + 打包 |

> 建议从 P1 开始，每个阶段完成后都应该是可运行的状态 (always shippable)。
> P1-P4 完成后即具备 AI Agent 核心使用场景。
> 浏览器功能暂不实现，后续按需以 feature-gated crate 形式引入。
