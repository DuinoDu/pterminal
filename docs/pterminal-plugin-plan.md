# pterminal 插件系统实施方案（参考 VS Code）

## 问题与目标
- 目标：把 pterminal 收敛为「终端内核 + 插件平台」。
- 内核保留：PTY/ANSI 解析、渲染管线、分屏布局与焦点基础能力。
- 非内核能力（左边栏、Tab 类型/内容、命令、通知与工具视图）全部通过插件贡献。
- 性能要求：插件化后 `pterminal-cli -- bench` 无退化，并增加插件激活与启动耗时监控。

## 当前代码现状（落地点）
- UI 行为当前硬编码在 `crates/pterminal-ui/src/slint_app.rs`：
  - `on_tab_clicked` / `on_tab_close_clicked` / `on_new_tab_clicked`
  - `on_sidebar_item_clicked`
  - `handle_ipc_request` 直接 match 固定方法集合
- UI 结构当前固定在 `crates/pterminal-ui/ui/app.slint`：
  - `TabBar`/`Sidebar` 数据模型是内建结构体（`TabInfo`、`SidebarItem`）
- 内核模型可复用：
  - `crates/pterminal-core/src/workspace/mod.rs`
  - `crates/pterminal-core/src/split/mod.rs`

## 参考 VS Code 的机制映射到 pterminal
1. **Manifest 贡献模型**
   - 插件包包含 `plugin.json`，声明 `activationEvents`、`contributes.*`、`permissions`。
2. **按事件延迟激活**
   - 只有在命令/视图/Tab 类型被触发时激活对应插件。
3. **主进程与扩展宿主隔离**
   - 插件代码运行在独立 extension host 进程，主 UI 进程仅做渲染与调度。
4. **主线程贡献注册表**
   - `ContributionRegistry` 作为唯一 UI 数据源，Slint 不再感知具体业务实现。
5. **可观测与容错**
   - 插件激活耗时、RPC 错误、崩溃重启次数可观测并可通过 CLI 查询。

## 目标架构
### 1) 新增模块
- `crates/pterminal-plugin-api`
  - 共享协议与类型：Manifest、ActivationEvent、Command、SidebarView、TabType、RPC 消息。
- `crates/pterminal-sdk`
  - 面向插件作者的 Rust SDK（类型化 API、生命周期封装、权限检查、View Sandbox 绑定）。
- `crates/pterminal-sdk-macros`（可选）
  - `#[plugin_main]`、贡献点声明宏与 manifest 校验辅助。
- `crates/pterminal-plugin-host`
  - 插件宿主（独立进程）：加载插件入口、维护生命周期、与主进程双向 RPC。
- `crates/pterminal-ui` 内新增 `plugin/` 子模块
  - `PluginManager`：扫描、索引、激活、重启治理。
  - `ContributionRegistry`：聚合侧边栏项、Tab 类型、命令。
  - `PluginBridge`：主进程 <-> host 的 typed RPC。
  - `PluginViewRuntime`：受限加载 Slint 子视图（sandbox）。

### 2) 插件目录与包格式（MVP）
- 路径：`~/.config/pterminal/plugins/<publisher>.<name>/`
- 运行时策略（修正）：
  - **MVP 默认 Rust 插件二进制**（独立进程），不做 in-process `cdylib`。
  - 插件协议是语言无关的 JSON-RPC；JS/TS 可作为后续 runtime adapter（Node/Deno）能力，不作为第一阶段必选。
- 最小文件（Rust 插件）：
  - `plugin.json`
  - `bin/<target-triple>/plugin`（插件可执行文件）
- `plugin.json` 关键字段（MVP）：
  - `id`, `name`, `version`
  - `sdk.version`（声明兼容的 `pterminal-sdk` 主版本）
  - `runtime`（`native` | `node`，MVP 先落 `native`）
  - `entry`（二进制或脚本入口）
  - `ui.mode`（`data` | `slint-sandbox`，MVP 默认 `data`）
  - `activationEvents`
  - `contributes.commands[]`
  - `contributes.sidebarViews[]`
  - `contributes.tabTypes[]`
  - `permissions[]`（如 `terminal.topology.read` / `terminal.pane.state.read` / `terminal.pane.content.read` / `terminal.control.freeze` / `ui.overlay.show` / `webview.embed`）

### 2.1) Sample 插件文件结构（MVP）
- 示例 A：数据驱动插件（`ui.mode = "data"`）
  - `~/.config/pterminal/plugins/acme.workspace-sidebar/`
    - `plugin.json`
    - `bin/darwin-aarch64/plugin`
    - `assets/icon.png`
    - `README.md`
- 示例 B：Slint Sandbox 子视图插件（`ui.mode = "slint-sandbox"`）
  - `~/.config/pterminal/plugins/acme.timer-reminder/`
    - `plugin.json`
    - `bin/darwin-aarch64/plugin`
    - `views/main.slint`
    - `assets/`
- 示例 C：浏览器 Tab 插件（依赖 host WebView 容器）
  - `~/.config/pterminal/plugins/acme.browser-tab/`
    - `plugin.json`
    - `bin/darwin-aarch64/plugin`
    - `assets/`
    - `README.md`

### 2.2) `plugin.json` 最小示例（数据驱动）
```json
{
  "id": "acme.workspace-sidebar",
  "name": "Workspace Sidebar",
  "version": "0.1.0",
  "sdk": { "version": "1.x" },
  "runtime": "native",
  "entry": "bin/darwin-aarch64/plugin",
  "ui": { "mode": "data" },
  "activationEvents": ["onStartupFinished"],
  "contributes": {
    "commands": [{ "id": "acme.workspace.focus", "title": "Focus Workspace Panel" }],
    "sidebarViews": [{ "id": "acme.workspace.tree", "title": "Workspaces", "order": 100 }],
    "tabTypes": []
  },
  "permissions": ["terminal.topology.read", "terminal.pane.state.read", "notification.send"]
}
```

### 3) 运行时流程
1. 启动扫描插件目录并解析 manifest。
2. 构建激活索引：`activation event -> plugin ids`。
3. 创建 extension host（按需，MVP 先单 host）。
4. UI 触发事件（命令执行、点击侧栏、创建某类 tab）时激活插件。
5. 插件在 `activate(context)` 中注册贡献点，主进程更新 registry，Slint 自动重绘。
6. host 异常时隔离失败插件并退避重启，主 UI 不崩溃。

## 贡献点设计（覆盖你的诉求）
### A. 左边栏（Sidebar）插件化
- 主进程只保留容器与渲染，不内置业务项。
- 插件贡献：
  - `sidebarViews`: id/title/icon/order
  - 数据提供协议：`getChildren(viewId, nodeId?)`
  - 行为协议：`onClick(viewId, nodeId, action)`
- `app.slint` 从 `ContributionRegistry.sidebar_items` 渲染，点击统一走 command dispatch。

### B. Tab 类型与内容插件化
- 新抽象：
  - `TabKind::Terminal | TabKind::Plugin { plugin_id, tab_type }`
  - `TabInstance { id, title, kind, state }`
- 插件贡献 `tabTypes`：
  - 创建入口：`createTab(initialArgs) -> TabInstanceState`
  - 内容渲染协议（MVP）：主进程托管容器，插件通过 RPC 提供视图模型与事件处理。
- 内建 terminal tab 作为 `builtin.terminal` 插件贡献，确保“除 terminal 内核外皆插件化”。

### C. Plugin View Sandbox（受限 Slint 子视图）
- 目标：允许插件提供「子视图」，但不允许修改主窗口组件树与核心渲染路径。
- 注入边界：
  - 仅可挂载到预留容器（如 sidebar panel / plugin tab content 区域）。
  - 仅可加载插件包内声明的 `.slint` 入口（通过 `slint-interpreter`）。
- 组件协议（View ABI）：
  - 必需输入属性：`model`（JSON/模型数据）、`theme`、`size`。
  - 必需回调：`dispatch(action, payload)`，由主程序转为白名单 RPC。
  - 禁止直接访问 PTY、workspace manager、文件系统等核心对象。
- 白名单 API（通过 RPC）：
  - `command.execute`
  - `workspace.list/select`
  - `tab.open/close/update`
  - `notification.send`
- 稳定性与回退：
  - 视图加载失败时降级到错误占位视图，不影响主 UI。
  - 单插件视图异常可隔离/卸载，不传播到主线程。

## pterminal-sdk 设计（给插件开发者）
### SDK 分层
- `pterminal-plugin-api`：协议层（serde 类型 + RPC 枚举），主程序与插件共享。
- `pterminal-sdk`：开发层（高阶封装），插件作者直接依赖。
- `pterminal-sdk-macros`：可选语法糖，减少样板代码。

### 插件入口（Rust）
- 建议模式：`fn activate(ctx: PluginContext) -> impl Plugin` / `fn deactivate()`
- SDK 提供 host 握手、心跳、错误上报与版本协商，插件无需手写底层 transport。

### SDK API（MVP）
- `ctx.commands.register(id, handler)`
- `ctx.sidebar.register_view(definition, provider)`
- `ctx.tabs.register_type(definition, factory)`
- `ctx.workspace.list/select()`
- `ctx.notifications.send(...)`
- `ctx.state.get/set`（插件私有持久化）
- `ctx.terminal.freeze(scope)` / `ctx.terminal.unfreeze(scope)`（需 `terminal.control.freeze`，用于提醒类插件）

### Terminal Introspection API（按权限开放）
- 目标：支持插件读取 terminal 内部信息（tab/pane 拓扑、pane 状态、pane 内容），但默认最小权限。
- API 与权限映射：
  - `ctx.terminal.topology()` -> `terminal.topology.read`
    - 返回 workspace/tab/pane 层级、active pane、split 结构与几何信息。
  - `ctx.terminal.pane_state(pane_id)` / `ctx.terminal.list_pane_states()` -> `terminal.pane.state.read`
    - 返回 pane 运行状态（alive、title、cwd、rows/cols、focus、cursor/selection 概要等）。
  - `ctx.terminal.pane_content(pane_id, opts)` -> `terminal.pane.content.read`
    - 返回可见区文本与可选 scrollback（可限制行数、是否含样式）。
- 事件订阅（可选）：
  - `ctx.terminal.on_topology_changed(...)`
  - `ctx.terminal.on_pane_state_changed(...)`
  - 订阅事件同样受对应 read 权限约束并带节流。
- 数据保护：
  - 敏感字段默认脱敏（如路径/命令行），需要额外权限或用户显式授权。
  - 内容读取做速率限制与大小上限，避免拖慢渲染与 IPC。

### Sandbox 视图 API
- `ctx.views.mount_sandbox(view_id, model)`：挂载受限 Slint 子视图。
- `ctx.views.update_model(view_id, patch)`：增量更新模型。
- `ctx.views.dispatch(action, payload)`：统一事件回传主程序白名单 API。

### 版本与兼容
- 采用 capability + semver 双重协商：
  - manifest 声明 `sdk.version`
  - 握手返回 `host.capabilities`
- 若版本不兼容，插件被跳过并输出可诊断错误（CLI 可查）。

## IPC / CLI 设计
- 扩展 IPC 命名空间：
  - `plugin.list`, `plugin.inspect`, `plugin.enable`, `plugin.disable`, `plugin.reload`
  - `plugin.logs`, `plugin.stats`
- `pterminal-cli` 增加对应子命令，便于诊断插件激活、崩溃与性能。

## 安全与稳定性
- 权限最小化：manifest 声明 `permissions`，首启需要用户确认。
- 资源治理：单插件超时/高频错误熔断；host 进程崩溃不影响主 UI。
- 数据边界：插件不能直接触达内核对象，只能通过受控 API/RPC。
- 对 `slint-sandbox` 视图做入口白名单与能力白名单校验，禁止越权调用。
- `terminal.*.read` 默认关闭；未授权插件只能访问自身注册的轻量元数据。
- `webview.embed` 与 `terminal.control.freeze` 归类为高风险权限，默认二次确认并可被管理员策略禁用。

## MVP 后 Use Cases（按你的三个插件）
> 规则：以下插件在 **Plugin MVP 完成后**（以 `plugin-benchmark-guardrail` 完成为闸门）再开始实现。

### UC-1 Workspace Sidebar + Notification（参考 cmux）
- 目标：
  - 左侧展示 workspace/pane 树、状态、切换动作；
  - 关键事件（pane 退出、命令完成、异常）发通知。
- 依赖：
  - `plugin-sidebar-extension-point`
  - `plugin-terminal-introspection-api`
  - `plugin-benchmark-guardrail`
- 需要权限：
  - `terminal.topology.read`
  - `terminal.pane.state.read`
  - `notification.send`

### UC-2 Timer Reminder（1h 冻结 + 2m 倒计时 + 自动恢复）
- 目标：
  - 周期提醒“请喝水并走动走动”；
  - 到点冻结 terminal 输入，展示 2 分钟倒计时 overlay，倒计时结束自动恢复。
- 依赖：
  - `plugin-freeze-overlay-capability`
  - `plugin-benchmark-guardrail`
- 需要权限：
  - `terminal.control.freeze`
  - `ui.overlay.show`
  - `notification.send`

## 未来能力：插件自渲染 Tab（非 MVP）
- 目标：允许插件“自己渲染进 tab”，插件拿到 tab 可渲染区域并输出帧。
- 定位：Phase 3 能力，不阻塞 MVP 和上述 3 个 use case 先落地。
- 关键前置：
  - 跨进程渲染桥（共享纹理/共享 surface + IPC 同步）
  - 帧率与背压控制（避免拖慢 terminal 主渲染）
  - GPU/窗口资源隔离与崩溃回退
  - 输入事件路由与焦点安全策略
- 交付方式：先做技术预研与 PoC，再决定是否产品化。


### UC-3 Browser Tab（调试 Web App）
- 目标：
  - 新建 browser tab，支持 URL 导航、基础调试动作（后续扩展 devtools bridge）。
- 依赖：
  - `plugin-tab-type-extension-point`
  - `plugin-webview-container`（主进程 WebView 容器）
  - `plugin-benchmark-guardrail`
- 需要权限：
  - `webview.embed`
  - `tab.open/close/update`

## 性能策略与验收
### 性能策略
- Manifest 扫描缓存（mtime + hash）避免每次冷启动全量解析。
- 延迟激活，未触发插件不加载执行代码。
- RPC 批量化与去抖，减少 UI 线程抖动。

### 验收基线
1. `cargo run --release -p pterminal-cli -- bench --cols 120 --rows 40 --iterations 200`
   - 与当前基线对比无明显回退（重点看 render_pipeline、text_update_buffers）。
2. 插件空载启动：启动耗时与首帧时延可观测，且不显著退化。
3. 压测场景：启用多个 sidebar/tab 插件后，输入与滚动流畅性保持稳定。

## 分阶段实施（对应 SQL todo）
1. `plugin-core-model`：定义贡献模型与运行时状态结构。
2. `plugin-manifest-loader`：完成扫描/校验/索引与启停状态。
3. `plugin-host-rpc`：落地 extension host 与 typed RPC。
4. `plugin-sdk-crate`：实现 `pterminal-sdk`/`pterminal-sdk-macros` 与示例插件模板。
5. `plugin-terminal-introspection-api`：实现 terminal 拓扑/状态/内容读取 API 与权限校验。
6. `plugin-sidebar-extension-point`：将左边栏切到插件贡献。
7. `plugin-tab-type-extension-point`：支持插件 Tab 类型与内容。
8. `plugin-view-sandbox`：实现受限 Slint 子视图加载、View ABI 与白名单 RPC。
9. `plugin-builtin-migration`：把现有非终端功能迁到 builtin plugin。
10. `plugin-cli-admin-commands`：补齐 CLI 管理与诊断命令。
11. `plugin-benchmark-guardrail`：建立性能回归闸门并持续验证（MVP 完成闸门）。
12. `plugin-freeze-overlay-capability`：提供冻结/恢复与倒计时 overlay 能力（供提醒插件使用）。
14. `plugin-usecase-workspace-sidebar-notify`：实现 UC-1。
15. `plugin-usecase-timer-freeze`：实现 UC-3。
16. `plugin-usecase-browser-tab-debug`：实现 UC-2。
13. `plugin-webview-container`：提供主进程 BrowserTabContainer（供浏览器插件使用）。
17. `plugin-tab-self-render-future`：插件自渲染 tab 的预研与 PoC（Phase 3）。
18. `plugin-sample-packages`：提供三个 sample plugin 包骨架（workspace sidebar、timer reminder、browser tab）。

## 备注
- MVP 先做「单 host + Rust native 插件 + 本地插件目录」以控制复杂度。
- JS/TS 插件能力放到第二阶段，通过 runtime adapter 接入，不影响主协议。
- Slint 插件化走“双轨”：默认数据驱动渲染；高级场景走 Plugin View Sandbox 子视图，不开放主 UI 任意注入。
- 后续可迭代到多 host（UI/Workspace）与远程插件运行位点（参考 VS Code 多宿主）。
