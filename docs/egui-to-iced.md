# egui → iced 0.13 迁移说明

## 迁移原因

| 方面 | egui / eframe | iced 0.13 |
|------|--------------|-----------|
| 架构模式 | 即时模式（Immediate Mode） | Elm 架构（Model-View-Update） |
| 状态管理 | 结构体字段直接可变 | 不可变 view + Message 驱动 update |
| 渲染 | 每帧重绘整个 UI | 基于差异的局部更新 |
| 类型安全 | 较弱（很多 `&mut` 传递） | 较强（消息类型明确） |
| 社区活跃度 | 稳定但以游戏/工具 UI 为主 | Rust GUI 生态中增长最快 |

迁移动机：iced 的 Elm 架构更适合长期维护，类型安全更好，且 0.13 版本已足够稳定。

---

## 架构变化：即时模式 → Elm 架构

### egui（即时模式）

```rust
// 每帧都调用，直接读写状态
impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if ui.button("Click me").clicked() {
                self.count += 1;
            }
            ui.label(format!("Count: {}", self.count));
        });
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}
```

### iced（Elm 架构）

```rust
// 状态不可变，通过消息驱动
#[derive(Debug, Clone)]
enum Message {
    Increment,
}

fn update(state: &mut MyApp, message: Message) -> Task<Message> {
    match message {
        Message::Increment => { state.count += 1; Task::none() }
    }
}

fn view(state: &MyApp) -> Element<'_, Message> {
    column![
        button("Click me").on_press(Message::Increment),
        text(format!("Count: {}", state.count)),
    ].into()
}
```

---

## Cargo.toml 变化

```toml
# 迁移前
eframe = "0.29"

# 迁移后
iced = { version = "0.13", features = ["advanced", "tokio"] }
```

- `advanced`：启用 `text_editor` 等高级 widget
- `tokio`：启用 `iced::time::every` 订阅（线程池后端不提供此 API）

---

## API 对照表

### 入口

| egui | iced 0.13 |
|------|-----------|
| `eframe::run_native(title, options, factory)` | `iced::application(title, update, view).run_with(\|\| new())` |
| `eframe::NativeOptions { viewport: ... }` | `iced::window::Settings { size, min_size, ... }` |
| `Box::new(\|cc\| { setup_fonts(&cc.egui_ctx); Ok(Box::new(App::new())) })` | `.run_with(\|\| AutoInputApp::new())` 返回 `(State, Task<Message>)` |

### 布局

| egui | iced 0.13 |
|------|-----------|
| `ui.horizontal(\|ui\| { ... })` | `row![widget1, widget2].spacing(8)` |
| `ui.vertical(\|ui\| { ... })` | `column![widget1, widget2].spacing(8)` |
| `ui.group(\|ui\| { ... })` | `container(content).style(container::rounded_box)` |
| `ui.with_layout(right_to_left, ...)` | `row![widget, horizontal_space(), right_widget]` |
| `ui.add_space(8.0)` | `.spacing(8)` 或 `vertical_space()` |
| `ui.separator()` | `horizontal_rule(1)` |

### 基础 Widget

| egui | iced 0.13 |
|------|-----------|
| `ui.label("text")` | `text("text")` |
| `ui.heading("text")` | `text("text").size(20)` |
| `RichText::new("s").strong()` | `text("s").font(Font { weight: Bold, .. })` |
| `RichText::new("s").small().color(c)` | `text("s").size(11).color(c)` |
| `ui.button("B").clicked()` | `button("B").on_press(Message::Clicked)` |
| `ui.add_enabled(cond, button)` | `button.on_press_maybe(if cond { Some(msg) } else { None })` |
| `ui.checkbox(&mut v, "label")` | `checkbox("label", v).on_toggle(Message::Changed)` |
| `ui.radio_value(&mut v, val, "label")` | `radio("label", val, Some(v), Message::Selected)` |
| `egui::TextEdit::singleline(&mut s)` | `text_input("placeholder", &s).on_input(Message::Changed)` |
| `egui::TextEdit::singleline(...).password(true)` | `text_input(...).secure(true)` |
| `egui::TextEdit::multiline(&mut s)` | `text_editor(&content).on_action(Message::Action)` |
| `egui::DragValue::new(&mut v).range(0..=100)` | `text_input("0", &v_str).on_input(Message::Changed)` + 手动解析 |
| `egui::ComboBox::new(id).show_ui(\|ui\| {...})` | `pick_list(options, selected, Message::Selected)` |
| `egui::ScrollArea::vertical().show(ui, \|ui\| {...})` | `scrollable(content)` |

### 状态与事件

| egui | iced 0.13 |
|------|-----------|
| `ctx.request_repaint_after(150ms)` | `iced::time::every(50ms).map(\|_\| Message::Tick)` 订阅 |
| `ctx.send_viewport_cmd(WindowLevel(AlwaysOnTop))` | Win32 `SetWindowPos(hwnd, HWND_TOPMOST, ...)` |
| `ctx.send_viewport_cmd(Visible(false))` | Win32 `ShowWindow(hwnd, SW_HIDE)` |
| `ctx.input(\|i\| i.viewport().close_requested())` + `CancelClose` | `iced::event::listen_with` 订阅 `CloseRequested` |
| `ctx.request_repaint()` （跨线程唤醒） | 不需要 — Tick 订阅每 50ms 自动轮询 |

### 颜色

| egui | iced 0.13 |
|------|-----------|
| `egui::Color32::from_rgb(r, g, b)` | `iced::Color::from_rgb8(r, g, b)` |
| `egui::Color32::GRAY` | `Color::from_rgb8(150, 150, 150)` |

---

## 字体加载

### egui

```rust
// 启动时调用，支持多字体族
let mut fonts = FontDefinitions::default();
fonts.font_data.insert("chinese", FontData::from_owned(bytes));
fonts.families.get_mut(&Proportional).unwrap().push("chinese".into());
ctx.set_fonts(fonts);
```

### iced 0.13

```rust
// 在 new() 中作为 Task 返回，动态加载
let task = iced::font::load(Cow::Owned(bytes)).map(Message::FontLoaded);
```

iced 0.13 不支持多字体族优先级，加载的字体作为 fallback 全局字体。

---

## 托盘图标与 `ctx_holder` 移除

egui 中，跨线程触发重绘需要持有 `egui::Context` 的 `Arc<Mutex<Option<Context>>>` 引用：

```rust
// egui：需要 ctx_holder 来跨线程 request_repaint
ctx_h.lock().unwrap().as_ref().unwrap().request_repaint();
```

iced 中，通过 `Subscription` 的 Tick 每 50ms 轮询通道，托盘事件处理器只需向 `mpsc::Sender<TrayCmd>` 发送命令：

```rust
// iced：直接发消息，Tick 自动轮询
let _ = tx.send(TrayCmd::ToggleAlwaysOnTop);
// 不需要 ctx_holder，也不需要 request_repaint
```

---

## `text_editor` vs `TextEdit::multiline`

| 方面 | egui `TextEdit::multiline` | iced `text_editor` |
|------|--------------------------|-------------------|
| 状态 | `&mut String` | `text_editor::Content`（不可序列化） |
| 读取文本 | 直接访问 `String` | `content.text()` |
| 更新 | 直接修改字符串 | `content.perform(action)` |
| Action 类型 | N/A | `text_editor::Action`（需 `Clone`） |

---

## 已知行为差异

1. **DragValue 改为 text_input**：iced 没有拖拽调节数值的 widget，改用文本输入框 + 手动 parse。数值范围校验在 `update` 函数中进行。

2. **always_on_top 实现**：iced 0.13 的 `window::change_level` 需要 `window::Id`，而主窗口 ID 没有 `MAIN` 常量（0.13 版本移除了）。改用 Win32 `SetWindowPos(HWND_TOPMOST/HWND_NOTOPMOST)` 直接操作，首次应用在第一个 Tick 触发时。

3. **中文字体**：egui 支持多字体族 fallback（英文用默认字体，中文用系统字体）。iced 0.13 通过 `font::load` 动态加载，效果类似但中英文可能都使用中文字体渲染。

4. **`container::rounded_box` 样式**：分组区块使用 iced 内置的 `rounded_box` 样式，视觉效果与 egui `ui.group()` 接近但不完全相同。

5. **热键 ComboBox → pick_list**：egui 的 `ComboBox` 允许在同一下拉列表中有"(未设置)"选项。iced 的 `pick_list` 通过在选项列表中显式添加 `KeyOpt { vk: 0, name: "(未设置)" }` 实现相同功能。
