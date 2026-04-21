use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crate::hotkey::{HotkeyDef, HotkeyEvent, HotkeyWorker, HK_CLIPBOARD, HK_CUSTOM,
    KEY_OPTIONS, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use eframe::egui;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

/// 托盘事件类型
#[derive(Debug)]
enum TrayCmd {
    ShowWindow,
    ToggleAlwaysOnTop,
}

// ─────────────────────────────────────────────────────────────────────────────

// ─── 配置持久化 ───────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct AppConfig {
    #[serde(default)]
    vmrun_path: String,
    #[serde(default)]
    vmx_path: String,
    #[serde(default)]
    guest_user: String,
    #[serde(default)]
    guest_pass: String,
    #[serde(default)]
    input_mode: u8,
    #[serde(default = "default_char_delay_ms")]
    char_delay_ms: u64,
    #[serde(default = "default_start_delay_secs")]
    start_delay_secs: u64,
    #[serde(default)]
    always_on_top: bool,
    #[serde(default = "default_hotkey_clipboard")]
    hotkey_clipboard: HotkeyDef,
    #[serde(default = "default_hotkey_custom")]
    hotkey_custom: HotkeyDef,
}

fn default_hotkey_clipboard() -> HotkeyDef {
    HotkeyDef { enabled: false, modifiers: MOD_CTRL | MOD_ALT, vk: 0x70 } // Ctrl+Alt+F1
}
fn default_hotkey_custom() -> HotkeyDef {
    HotkeyDef { enabled: false, modifiers: MOD_CTRL | MOD_ALT, vk: 0x71 } // Ctrl+Alt+F2
}

fn default_char_delay_ms() -> u64 { 50 }
fn default_start_delay_secs() -> u64 { 3 }

fn config_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
    base.join("auto-input").join("config.toml")
}

// ─────────────────────────────────────────────────────────────────────────────

pub struct AutoInputApp {
    // ── 输入来源 ──────────────────────────────────────────────────────────
    use_clipboard: bool,
    custom_text: String,

    // ── 时间设置 ──────────────────────────────────────────────────────────
    /// 每个字符之间的间隔（毫秒）
    char_delay_ms: u64,
    /// 点击"开始"后先等待多少秒再开始输入
    start_delay_secs: u64,

    // ── 输入方式 ──────────────────────────────────────────────────────────
    /// 0=逐字模拟按键  1=粘贴 Ctrl+V  2=VMware vmrun
    input_mode: u8,

    // ── VMware 设置 ──────────────────────────────────────────────────────
    vmrun_path: String,
    vmx_path: String,
    guest_user: String,
    guest_pass: String,

    // ── 运行状态 ──────────────────────────────────────────────────────────
    is_running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    /// vmrun 错误信息
    vmrun_error: Arc<Mutex<String>>,
    /// 上一帧是否处于运行状态，用于检测完成转换
    prev_running: bool,
    /// 点击开始的时刻，用于 UI 倒计时显示
    start_time: Option<Instant>,

    // ── 托盘 ─────────────────────────────────────────────────────────────
    _tray_icon: TrayIcon,
    tray_rx: mpsc::Receiver<TrayCmd>,
    ctx_holder: Arc<Mutex<Option<egui::Context>>>,

    // ── 热键 ─────────────────────────────────────────────────────────────
    hotkey_clipboard: HotkeyDef,
    hotkey_custom: HotkeyDef,
    /// false = 注册失败（与其他程序冲突）
    hotkey_clipboard_ok: bool,
    hotkey_custom_ok: bool,
    hotkey_worker: HotkeyWorker,

    // ── 窗口 & 状态 ──────────────────────────────────────────────────────
    window_visible: bool,
    always_on_top: bool,
    first_frame: bool,
    status_text: String,
}

impl AutoInputApp {
    pub fn new() -> Self {
        let (tray_tx, tray_rx) = mpsc::channel::<TrayCmd>();
        let ctx_holder: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));

        // ─ 提前加载配置，供菜单初始状态使用 ────────────────────────────────────
        let cfg = Self::load_config();

        // ─ 托盘菜单 ──────────────────────────────────────────────────────────────
        let tray_menu = Menu::new();
        let pin_item = CheckMenuItem::new("📍 置顶", true, cfg.always_on_top, None);
        let quit_item = MenuItem::new("退出", true, None);
        tray_menu
            .append_items(&[
                &pin_item,
                &PredefinedMenuItem::separator(),
                &quit_item,
            ])
            .expect("构建托盘菜单失败");

        let pin_id  = pin_item.id().clone();
        let quit_id = quit_item.id().clone();

        // ─ 菜单事件处理 ──────────────────────────────────────────────────────────
        {
            let quit_id = quit_id.clone();
            let tx = tray_tx.clone();
            let ctx_h = ctx_holder.clone();
            tray_icon::menu::MenuEvent::set_event_handler(Some(move |e: tray_icon::menu::MenuEvent| {
                if e.id == quit_id {
                    std::process::exit(0);
                } else if e.id == pin_id {
                    let _ = tx.send(TrayCmd::ToggleAlwaysOnTop);
                    if let Ok(guard) = ctx_h.lock() {
                        if let Some(ctx) = guard.as_ref() {
                            ctx.request_repaint();
                        }
                    }
                }
            }));
        }

        // ─ 托盘图标事件：左键双击 → 通过 Win32 API 直接恢复窗口 ─────────
        {
            let tx = tray_tx.clone();
            let ctx_h = ctx_holder.clone();
            tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
                if let tray_icon::TrayIconEvent::DoubleClick { .. } = e {
                    // 直接通过 Win32 API 显示窗口（绕过 egui 事件循环）
                    unsafe {
                        let title = "Auto Input — 自动输入";
                        let title_wide: Vec<u16> =
                            title.encode_utf16().chain(std::iter::once(0)).collect();
                        let hwnd = winapi::um::winuser::FindWindowW(
                            std::ptr::null(),
                            title_wide.as_ptr(),
                        );
                        if !hwnd.is_null() {
                            winapi::um::winuser::ShowWindow(hwnd, winapi::um::winuser::SW_SHOW);
                            winapi::um::winuser::ShowWindow(hwnd, winapi::um::winuser::SW_RESTORE);
                            winapi::um::winuser::SetForegroundWindow(hwnd);
                        }
                    }
                    // 通知 egui 同步内部可见状态
                    let _ = tx.send(TrayCmd::ShowWindow);
                    if let Ok(guard) = ctx_h.lock() {
                        if let Some(ctx) = guard.as_ref() {
                            ctx.request_repaint();
                        }
                    }
                }
            }));
        }

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Auto Input — 自动输入")
            .with_icon(make_tray_icon())
            .with_menu_on_left_click(false)
            .build()
            .expect("创建托盘图标失败");

        let vmrun_path = if cfg.vmrun_path.is_empty() {
            crate::input::detect_vmrun().unwrap_or_default()
        } else {
            cfg.vmrun_path
        };

        Self {
            use_clipboard: false,
            custom_text: String::new(),
            input_mode: cfg.input_mode,
            vmrun_path,
            vmx_path: cfg.vmx_path,
            guest_user: cfg.guest_user,
            guest_pass: cfg.guest_pass,
            char_delay_ms: cfg.char_delay_ms,
            start_delay_secs: cfg.start_delay_secs,
            is_running: Arc::new(AtomicBool::new(false)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            vmrun_error: Arc::new(Mutex::new(String::new())),
            prev_running: false,
            start_time: None,
            _tray_icon: tray_icon,
            tray_rx,
            ctx_holder,
            window_visible: true,
            always_on_top: cfg.always_on_top,
            first_frame: true,
            status_text: String::from("就绪"),
            hotkey_clipboard: cfg.hotkey_clipboard.clone(),
            hotkey_custom: cfg.hotkey_custom.clone(),
            hotkey_clipboard_ok: true,
            hotkey_custom_ok: true,
            hotkey_worker: {
                let w = HotkeyWorker::spawn();
                w.update(HK_CLIPBOARD, cfg.hotkey_clipboard);
                w.update(HK_CUSTOM,    cfg.hotkey_custom);
                w
            },
        }
    }

    fn load_config() -> AppConfig {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or(AppConfig {
                vmrun_path: String::new(),
                vmx_path: String::new(),
                guest_user: String::new(),
                guest_pass: String::new(),
                input_mode: crate::input::MODE_CHAR,
                char_delay_ms: 50,
                start_delay_secs: 3,
                always_on_top: false,
                hotkey_clipboard: default_hotkey_clipboard(),
                hotkey_custom: default_hotkey_custom(),
            })
    }

    fn save_config(&self) {
        let cfg = AppConfig {
            vmrun_path: self.vmrun_path.clone(),
            vmx_path: self.vmx_path.clone(),
            guest_user: self.guest_user.clone(),
            guest_pass: self.guest_pass.clone(),
            input_mode: self.input_mode,
            char_delay_ms: self.char_delay_ms,
            start_delay_secs: self.start_delay_secs,
            always_on_top: self.always_on_top,
            hotkey_clipboard: self.hotkey_clipboard.clone(),
            hotkey_custom: self.hotkey_custom.clone(),
        };
        if let Ok(text) = toml::to_string_pretty(&cfg) {
            let path = config_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, text);
        }
    }

    fn start_input(&mut self) {
        if self.is_running.load(Ordering::Relaxed) {
            return;
        }
        self.save_config();
        self.stop_flag.store(false, Ordering::Relaxed);
        self.is_running.store(true, Ordering::Relaxed);
        // 清空上次的 vmrun 错误
        if let Ok(mut s) = self.vmrun_error.lock() {
            s.clear();
        }
        self.start_time = Some(Instant::now());
        self.status_text = format!("倒计时：{} 秒…", self.start_delay_secs);

        let is_running = self.is_running.clone();
        let stop_flag = self.stop_flag.clone();
        let use_clipboard = self.use_clipboard;
        let input_mode = self.input_mode;
        let custom_text = self.custom_text.clone();
        let char_delay_ms = self.char_delay_ms;
        let start_delay_secs = self.start_delay_secs;

        if input_mode == crate::input::MODE_VMRUN {
            let vmrun_path = self.vmrun_path.clone();
            let vmx_path = self.vmx_path.clone();
            let guest_user = self.guest_user.clone();
            let guest_pass = self.guest_pass.clone();
            let error_msg = self.vmrun_error.clone();
            let char_delay_ms = self.char_delay_ms;
            std::thread::spawn(move || {
                crate::input::run_vmrun_input(
                    vmrun_path,
                    vmx_path,
                    guest_user,
                    guest_pass,
                    use_clipboard,
                    custom_text,
                    char_delay_ms,
                    start_delay_secs,
                    is_running,
                    stop_flag,
                    error_msg,
                );
            });
        } else {
            std::thread::spawn(move || {
                crate::input::run_input(
                    use_clipboard,
                    input_mode,
                    custom_text,
                    char_delay_ms,
                    start_delay_secs,
                    is_running,
                    stop_flag,
                );
            });
        }
    }

    fn stop_input(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

// ─── eframe::App ─────────────────────────────────────────────────────────────

impl eframe::App for AutoInputApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── 注册 ctx，供托盘事件 handler 唤醒使用 ────────────────────────
        if let Ok(mut guard) = self.ctx_holder.lock() {
            if guard.is_none() {
                *guard = Some(ctx.clone());
            }
        }

        // ── 处理托盘事件 ──────────────────────────────────────────────────
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCmd::ShowWindow => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCmd::ToggleAlwaysOnTop => {
                    self.always_on_top = !self.always_on_top;
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                        if self.always_on_top {
                            egui::viewport::WindowLevel::AlwaysOnTop
                        } else {
                            egui::viewport::WindowLevel::Normal
                        },
                    ));
                    // 恢复窗口（托盘菜单操作时窗口可能是隐藏状态）
                    if !self.window_visible {
                        self.window_visible = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                    self.save_config();
                }
            }
        }

        // ── 处理热键事件 ──────────────────────────────────────────────────
        while let Ok(evt) = self.hotkey_worker.event_rx.try_recv() {
            match evt {
                HotkeyEvent::Triggered(id) => {
                    if !self.is_running.load(Ordering::Relaxed) {
                        let prev = self.use_clipboard;
                        self.use_clipboard = id == HK_CLIPBOARD;
                        self.start_input();
                        self.use_clipboard = prev;
                    }
                }
                HotkeyEvent::RegisterOk(id) => {
                    if id == HK_CLIPBOARD { self.hotkey_clipboard_ok = true; }
                    if id == HK_CUSTOM    { self.hotkey_custom_ok    = true; }
                }
                HotkeyEvent::RegisterFailed(id) => {
                    if id == HK_CLIPBOARD { self.hotkey_clipboard_ok = false; }
                    if id == HK_CUSTOM    { self.hotkey_custom_ok    = false; }
                }
            }
        }

        // ── 拦截关闭事件 → 最小化到托盘 ──────────────────────────────────
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.window_visible = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.save_config();
        }

        // ── 首帧：应用已保存的置顶状态 ────────────────────────────────────
        if self.first_frame {
            self.first_frame = false;
            if self.always_on_top {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                    egui::viewport::WindowLevel::AlwaysOnTop,
                ));
            }
        }

        // ── 更新状态文本 ──────────────────────────────────────────────────
        let running = self.is_running.load(Ordering::Relaxed);
        if running {
            if let Some(t) = self.start_time {
                let elapsed = t.elapsed().as_secs();
                if elapsed < self.start_delay_secs {
                    self.status_text =
                        format!("倒计时：{} 秒…", self.start_delay_secs - elapsed);
                } else {
                    self.status_text = String::from("输入中…");
                }
            }
        } else if self.prev_running {
            // 刚刚完成或被停止
            let vmrun_err = self.vmrun_error.lock().ok()
                .map(|s| s.clone())
                .unwrap_or_default();
            self.status_text = if !vmrun_err.is_empty() {
                vmrun_err
            } else if self.stop_flag.load(Ordering::Relaxed) {
                String::from("已停止")
            } else {
                String::from("输入完成 ✓")
            };
            self.start_time = None;
        }
        self.prev_running = running;

        // ── 渲染界面 ──────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("⌨  Auto Input — 自动输入工具");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let prev = self.always_on_top;
                    ui.checkbox(&mut self.always_on_top, "📍 置顶");
                    if self.always_on_top != prev {
                        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                            if self.always_on_top {
                                egui::viewport::WindowLevel::AlwaysOnTop
                            } else {
                                egui::viewport::WindowLevel::Normal
                            },
                        ));
                    }
                });
            });
            ui.separator();

            // ── 输入来源 ─────────────────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("输入来源").strong());
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.use_clipboard, false, "自定义文本");
                    ui.radio_value(&mut self.use_clipboard, true, "剪切板内容");
                });
                ui.add_space(4.0);

                if self.use_clipboard {
                    let preview = clipboard_preview();
                    ui.label(format!("剪切板预览：{preview}"));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.custom_text)
                                    .hint_text("在此输入要模拟键盘输入的文本…")
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(5),
                            );
                        });
                }
            });

            ui.add_space(6.0);

            // ── 输入方式 ─────────────────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("输入方式").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut self.input_mode,
                        crate::input::MODE_CHAR,
                        "逐字模拟按键",
                    );
                    ui.radio_value(
                        &mut self.input_mode,
                        crate::input::MODE_UNICODE,
                        "Unicode 按键",
                    );
                    ui.radio_value(
                        &mut self.input_mode,
                        crate::input::MODE_WM_CHAR,
                        "消息注入（飞书远控）",
                    );
                    ui.radio_value(
                        &mut self.input_mode,
                        crate::input::MODE_PASTE,
                        "粘贴 Ctrl+V",
                    );
                    ui.radio_value(
                        &mut self.input_mode,
                        crate::input::MODE_VMRUN,
                        "VMware 虚拟机",
                    );
                });
                let hint = match self.input_mode {
                    crate::input::MODE_WM_CHAR =>
                        "WM_KEYDOWN+WM_CHAR+WM_KEYUP 直接向前台窗口投递，ASCII 字符使用真实 VK 码，适用于从消息队列捕获键盘的远控软件（飞书/向日葵）",
                    crate::input::MODE_UNICODE =>
                        "Win32 KEYEVENTF_UNICODE 逐字发送，绕过键盘布局，适用于本地禁止粘贴的窗口",
                    crate::input::MODE_PASTE =>
                        "将文本写入剪贴板后发送 Ctrl+V；一次性粘贴，字符间延迟无效",
                    crate::input::MODE_VMRUN =>
                        "通过 vmrun typeTextInGuest 直接在 VMware 客户机内输入（需 VMware Tools）",
                    _ =>
                        "逐字模拟键盘输入，支持延迟控制",
                };
                ui.label(egui::RichText::new(hint).small().weak());
            });

            // ── VMware 设置（仅 VMware 模式显示）────────────────────────
            if self.input_mode == crate::input::MODE_VMRUN {
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.label(egui::RichText::new("VMware 设置").strong());
                    egui::Grid::new("vm_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("vmrun 路径：");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.vmrun_path)
                                    .hint_text("vmrun.exe 路径（自动探测）")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.end_row();

                            ui.label("VMX 文件：");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.vmx_path)
                                    .hint_text(r"如 D:\VMs\Win10\Win10.vmx")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.end_row();

                            ui.label("客户机用户名：");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.guest_user)
                                    .hint_text("Guest OS 登录用户名")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.end_row();

                            ui.label("客户机密码：");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.guest_pass)
                                    .hint_text("Guest OS 登录密码")
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.end_row();
                        });

                    if self.vmrun_path.is_empty() {
                        ui.label(
                            egui::RichText::new("⚠ 未找到 vmrun.exe，请手动指定路径")
                                .color(egui::Color32::from_rgb(240, 150, 60)),
                        );
                    }
                });
            }

            ui.add_space(6.0);

            // ── 时间设置 ─────────────────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("时间设置").strong());
                egui::Grid::new("timing_grid")
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        if self.input_mode == crate::input::MODE_CHAR
                            || self.input_mode == crate::input::MODE_UNICODE
                            || self.input_mode == crate::input::MODE_WM_CHAR
                            || self.input_mode == crate::input::MODE_VMRUN
                        {
                            ui.label("字符间延迟（ms）：");
                            ui.add(
                                egui::DragValue::new(&mut self.char_delay_ms)
                                    .range(0u64..=10_000u64)
                                    .speed(5.0),
                            );
                            let cps_label = if self.char_delay_ms == 0 {
                                String::from("（无限制，最快速度）")
                            } else {
                                format!("≈ {:.1} 字/秒", 1000.0 / self.char_delay_ms as f64)
                            };
                            ui.label(cps_label);
                            ui.end_row();
                        }

                        ui.label("开始延迟（秒）：");
                        ui.add(
                            egui::DragValue::new(&mut self.start_delay_secs)
                                .range(0u64..=60u64)
                                .speed(1.0),
                        );
                        ui.label("（开始前请切换到目标窗口）");
                        ui.end_row();
                    });
            });

            ui.add_space(6.0);

            // ── 全局热键 ─────────────────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("全局热键").strong());
                ui.label(
                    egui::RichText::new(
                        "⚠ 焦点在虚拟机窗口内时，宿主机热键无法被接收，请先点击虚拟机外部再触发。"
                    )
                    .small()
                    .color(egui::Color32::from_rgb(220, 170, 60)),
                );

                let mut hk_cb_changed = false;
                let mut hk_cu_changed = false;

                // 渲染一行热键设置 —— 返回是否有任何控件发生变化
                let render_row = |ui: &mut egui::Ui,
                                  row_id: &str,
                                  label: &str,
                                  def: &mut HotkeyDef,
                                  reg_ok: bool| -> bool {
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut def.enabled, label).changed();

                        // 修饰键复选框
                        let enabled = def.enabled;
                        ui.add_enabled_ui(enabled, |ui| {
                            let mut ctrl  = def.modifiers & MOD_CTRL  != 0;
                            let mut alt   = def.modifiers & MOD_ALT   != 0;
                            let mut shift = def.modifiers & MOD_SHIFT != 0;
                            let mut win   = def.modifiers & MOD_WIN   != 0;
                            if ui.checkbox(&mut ctrl,  "Ctrl").changed()  {
                                def.modifiers = (def.modifiers & !MOD_CTRL)  | if ctrl  { MOD_CTRL  } else { 0 };
                                changed = true;
                            }
                            if ui.checkbox(&mut alt,   "Alt").changed()   {
                                def.modifiers = (def.modifiers & !MOD_ALT)   | if alt   { MOD_ALT   } else { 0 };
                                changed = true;
                            }
                            if ui.checkbox(&mut shift, "Shift").changed() {
                                def.modifiers = (def.modifiers & !MOD_SHIFT) | if shift { MOD_SHIFT } else { 0 };
                                changed = true;
                            }
                            if ui.checkbox(&mut win,   "Win").changed()   {
                                def.modifiers = (def.modifiers & !MOD_WIN)   | if win   { MOD_WIN   } else { 0 };
                                changed = true;
                            }

                            // 键选择下拉框
                            let sel_label = if def.vk == 0 { "(未设置)" }
                                            else { crate::hotkey::vk_name(def.vk) };
                            egui::ComboBox::from_id_salt(row_id)
                                .selected_text(sel_label)
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    if ui.selectable_value(&mut def.vk, 0, "(未设置)").changed() {
                                        changed = true;
                                    }
                                    for &(vk, name) in KEY_OPTIONS {
                                        if ui.selectable_value(&mut def.vk, vk, name).changed() {
                                            changed = true;
                                        }
                                    }
                                });
                        });

                        // 当前快捷键标签 + 冲突警告
                        let disp = def.display();
                        ui.label(egui::RichText::new(&disp).small().weak());
                        if def.enabled && def.vk != 0 && !reg_ok {
                            ui.label(
                                egui::RichText::new("⚠ 快捷键冲突")
                                    .small()
                                    .color(egui::Color32::from_rgb(240, 150, 60)),
                            );
                        }
                    });
                    changed
                };

                egui::Grid::new("hotkey_grid")
                    .num_columns(1)
                    .spacing([0.0, 4.0])
                    .show(ui, |ui| {
                        hk_cb_changed = render_row(
                            ui, "hk_cb", "输入剪切板",
                            &mut self.hotkey_clipboard, self.hotkey_clipboard_ok,
                        );
                        ui.end_row();
                        hk_cu_changed = render_row(
                            ui, "hk_cu", "输入自定义",
                            &mut self.hotkey_custom, self.hotkey_custom_ok,
                        );
                        ui.end_row();
                    });

                if hk_cb_changed {
                    self.hotkey_worker.update(HK_CLIPBOARD, self.hotkey_clipboard.clone());
                }
                if hk_cu_changed {
                    self.hotkey_worker.update(HK_CUSTOM, self.hotkey_custom.clone());
                }
            });

            ui.add_space(6.0);

            // ── 控制按钮 ─────────────────────────────────────────────────
            ui.horizontal(|ui| {
                let has_text = self.use_clipboard || !self.custom_text.is_empty();
                let vm_ready = self.input_mode != crate::input::MODE_VMRUN
                    || (!self.vmrun_path.is_empty()
                        && !self.vmx_path.is_empty()
                        && !self.guest_user.is_empty());
                let can_start = !running && has_text && vm_ready;

                if ui
                    .add_enabled(can_start, egui::Button::new("▶  开始输入"))
                    .clicked()
                {
                    self.start_input();
                }

                if ui
                    .add_enabled(running, egui::Button::new("⏹  停止"))
                    .clicked()
                {
                    self.stop_input();
                }
            });

            ui.add_space(4.0);

            // ── 状态栏 ───────────────────────────────────────────────────
            let status_color = if running {
                egui::Color32::from_rgb(80, 200, 100)
            } else if self.status_text.contains('✓') {
                egui::Color32::from_rgb(100, 180, 255)
            } else if self.status_text.starts_with("vmrun") || self.status_text.starts_with("无法执行") {
                egui::Color32::from_rgb(240, 60, 60)
            } else if self.status_text == "已停止" {
                egui::Color32::from_rgb(240, 150, 60)
            } else {
                egui::Color32::GRAY
            };

            ui.label(
                egui::RichText::new(format!("状态：{}", self.status_text))
                    .color(status_color),
            );

            ui.separator();
            ui.label(
                egui::RichText::new(
                    "提示：关闭窗口后程序最小化到系统托盘；右键托盘图标可彻底退出",
                )
                .small()
                .weak(),
            );
        });

        // 定期重绘以轮询托盘事件并刷新倒计时
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ─── 工具函数 ─────────────────────────────────────────────────────────────────

/// 生成一个简单的 32×32 键盘样式托盘图标（纯 RGBA，无外部资源）
/// 生成图标 RGBA 像素数据（32×32），供窗口图标和托盘图标共用
pub fn make_icon_rgba() -> (Vec<u8>, u32, u32) {
    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgba = vec![0u8; (W * H * 4) as usize];

    for y in 0..H {
        for x in 0..W {
            let idx = ((y * W + x) * 4) as usize;

            // 键盘主体区域：x ∈ [3, 28)，y ∈ [9, 23)
            if x >= 3 && x < 29 && y >= 9 && y < 23 {
                let is_border = x == 3 || x == 28 || y == 9 || y == 22;

                if is_border {
                    // 外框：亮蓝白
                    rgba[idx] = 200;
                    rgba[idx + 1] = 220;
                    rgba[idx + 2] = 255;
                    rgba[idx + 3] = 255;
                } else {
                    // 背景：深蓝
                    rgba[idx] = 55;
                    rgba[idx + 1] = 85;
                    rgba[idx + 2] = 155;
                    rgba[idx + 3] = 230;

                    // 按键点阵（每 5×4 像素一个按键）
                    let lx = (x - 4) % 5;
                    let ly = (y - 10) % 4;
                    if lx <= 2 && ly <= 1 {
                        rgba[idx] = 175;
                        rgba[idx + 1] = 200;
                        rgba[idx + 2] = 240;
                        rgba[idx + 3] = 255;
                    }
                }
            }
        }
    }

    (rgba, W, H)
}

fn make_tray_icon() -> tray_icon::Icon {
    let (rgba, w, h) = make_icon_rgba();
    tray_icon::Icon::from_rgba(rgba, w, h).expect("创建托盘图标失败")
}

/// 读取剪切板文字并返回前 60 个字符的预览
fn clipboard_preview() -> String {
    match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
        Ok(text) => {
            let preview: String = text.chars().take(60).collect();
            if text.chars().count() > 60 {
                format!("{preview}…")
            } else {
                preview
            }
        }
        Err(_) => String::from("（无法读取剪切板）"),
    }
}
