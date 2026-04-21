use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use iced::widget::{
    button, checkbox, column, container, horizontal_rule, horizontal_space,
    pick_list, radio, row, scrollable, text, text_editor, text_input,
};
use iced::{Color, Element, Font, Length, Subscription, Task};

use crate::hotkey::{
    HotkeyDef, HotkeyEvent, HotkeyWorker, HK_CLIPBOARD, HK_CUSTOM, KEY_OPTIONS,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN,
};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

// Segoe UI Symbol 包含 ▶ ⏹ ⌨ ⚠ 等几何符号，显式指定以绕过 cosmic-text fallback 不稳定的问题
const SYM: Font = Font::with_name("Segoe UI Symbol");

// ─── TrayCmd ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum TrayCmd {
    ShowWindow,
    ToggleAlwaysOnTop,
}

// ─── KeyOpt ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOpt {
    pub vk: u32,
    pub name: &'static str,
}

impl std::fmt::Display for KeyOpt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

fn all_key_opts() -> Vec<KeyOpt> {
    let mut opts = vec![KeyOpt { vk: 0, name: "(未设置)" }];
    opts.extend(KEY_OPTIONS.iter().map(|&(vk, name)| KeyOpt { vk, name }));
    opts
}

// ─── Message ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    UseClipboardToggled(bool),
    TextEditorAction(text_editor::Action),
    InputModeChanged(u8),
    VmrunPathChanged(String),
    VmxPathChanged(String),
    GuestUserChanged(String),
    GuestPassChanged(String),
    CharDelayChanged(String),
    StartDelayChanged(String),
    AlwaysOnTopToggled(bool),
    // Hotkey
    HkClipboardEnabled(bool),
    HkClipboardMod(u32, bool),
    HkClipboardKey(KeyOpt),
    HkCustomEnabled(bool),
    HkCustomMod(u32, bool),
    HkCustomKey(KeyOpt),
    // Control
    StartInput,
    StopInput,
    // Background polling
    Tick,
    // Window
    CloseRequested,
}

// ─── AppConfig ───────────────────────────────────────────────────────────────

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
    HotkeyDef { enabled: false, modifiers: MOD_CTRL | MOD_ALT, vk: 0x70 }
}
fn default_hotkey_custom() -> HotkeyDef {
    HotkeyDef { enabled: false, modifiers: MOD_CTRL | MOD_ALT, vk: 0x71 }
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

// ─── AutoInputApp ─────────────────────────────────────────────────────────────

pub struct AutoInputApp {
    use_clipboard: bool,
    custom_text_content: text_editor::Content,

    char_delay_ms: u64,
    char_delay_str: String,
    start_delay_secs: u64,
    start_delay_str: String,

    input_mode: u8,

    vmrun_path: String,
    vmx_path: String,
    guest_user: String,
    guest_pass: String,

    is_running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    vmrun_error: Arc<Mutex<String>>,
    prev_running: bool,
    start_time: Option<Instant>,

    _tray_icon: TrayIcon,
    tray_rx: mpsc::Receiver<TrayCmd>,

    hotkey_clipboard: HotkeyDef,
    hotkey_custom: HotkeyDef,
    hotkey_clipboard_ok: bool,
    hotkey_custom_ok: bool,
    hotkey_worker: HotkeyWorker,

    window_visible: bool,
    always_on_top: bool,
    needs_topmost_init: bool,
    status_text: String,
}

impl AutoInputApp {
    fn new() -> (Self, Task<Message>) {
        let (tray_tx, tray_rx) = mpsc::channel::<TrayCmd>();
        let cfg = Self::load_config();

        // Build tray menu
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

        let pin_id = pin_item.id().clone();
        let quit_id = quit_item.id().clone();

        // Menu event handler — no ctx needed, Tick subscription handles repaint
        {
            let tx = tray_tx.clone();
            tray_icon::menu::MenuEvent::set_event_handler(Some(
                move |e: tray_icon::menu::MenuEvent| {
                    if e.id == quit_id {
                        std::process::exit(0);
                    } else if e.id == pin_id {
                        let _ = tx.send(TrayCmd::ToggleAlwaysOnTop);
                    }
                },
            ));
        }

        // Tray icon event: double-click → restore via Win32
        {
            let tx = tray_tx.clone();
            tray_icon::TrayIconEvent::set_event_handler(Some(
                move |e: tray_icon::TrayIconEvent| {
                    if let tray_icon::TrayIconEvent::DoubleClick { .. } = e {
                        unsafe {
                            let title = "Auto Input — 自动输入";
                            let title_wide: Vec<u16> =
                                title.encode_utf16().chain(std::iter::once(0)).collect();
                            let hwnd = winapi::um::winuser::FindWindowW(
                                std::ptr::null(),
                                title_wide.as_ptr(),
                            );
                            if !hwnd.is_null() {
                                winapi::um::winuser::ShowWindow(
                                    hwnd,
                                    winapi::um::winuser::SW_SHOW,
                                );
                                winapi::um::winuser::ShowWindow(
                                    hwnd,
                                    winapi::um::winuser::SW_RESTORE,
                                );
                                winapi::um::winuser::SetForegroundWindow(hwnd);
                            }
                        }
                        let _ = tx.send(TrayCmd::ShowWindow);
                    }
                },
            ));
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

        let always_on_top = cfg.always_on_top;
        // We apply always-on-top on the first Tick after the window is shown
        let needs_topmost_init = always_on_top;

        let hotkey_worker = {
            let w = HotkeyWorker::spawn();
            w.update(HK_CLIPBOARD, cfg.hotkey_clipboard.clone());
            w.update(HK_CUSTOM, cfg.hotkey_custom.clone());
            w
        };

        let app = Self {
            use_clipboard: false,
            custom_text_content: text_editor::Content::new(),
            char_delay_ms: cfg.char_delay_ms,
            char_delay_str: cfg.char_delay_ms.to_string(),
            start_delay_secs: cfg.start_delay_secs,
            start_delay_str: cfg.start_delay_secs.to_string(),
            input_mode: cfg.input_mode,
            vmrun_path,
            vmx_path: cfg.vmx_path,
            guest_user: cfg.guest_user,
            guest_pass: cfg.guest_pass,
            is_running: Arc::new(AtomicBool::new(false)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            vmrun_error: Arc::new(Mutex::new(String::new())),
            prev_running: false,
            start_time: None,
            _tray_icon: tray_icon,
            tray_rx,
            hotkey_clipboard: cfg.hotkey_clipboard,
            hotkey_custom: cfg.hotkey_custom,
            hotkey_clipboard_ok: true,
            hotkey_custom_ok: true,
            hotkey_worker,
            window_visible: true,
            always_on_top,
            needs_topmost_init,
            status_text: String::from("就绪"),
        };

        (app, Task::none())
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
        if let Ok(mut s) = self.vmrun_error.lock() {
            s.clear();
        }
        self.start_time = Some(Instant::now());
        self.status_text = format!("倒计时：{} 秒…", self.start_delay_secs);

        let is_running = self.is_running.clone();
        let stop_flag = self.stop_flag.clone();
        let use_clipboard = self.use_clipboard;
        let input_mode = self.input_mode;
        let custom_text = self.custom_text_content.text();
        let char_delay_ms = self.char_delay_ms;
        let start_delay_secs = self.start_delay_secs;

        if input_mode == crate::input::MODE_VMRUN {
            let vmrun_path = self.vmrun_path.clone();
            let vmx_path = self.vmx_path.clone();
            let guest_user = self.guest_user.clone();
            let guest_pass = self.guest_pass.clone();
            let error_msg = self.vmrun_error.clone();
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

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UseClipboardToggled(v) => {
                self.use_clipboard = v;
                Task::none()
            }
            Message::TextEditorAction(action) => {
                self.custom_text_content.perform(action);
                Task::none()
            }
            Message::InputModeChanged(mode) => {
                self.input_mode = mode;
                Task::none()
            }
            Message::VmrunPathChanged(s) => {
                self.vmrun_path = s;
                Task::none()
            }
            Message::VmxPathChanged(s) => {
                self.vmx_path = s;
                Task::none()
            }
            Message::GuestUserChanged(s) => {
                self.guest_user = s;
                Task::none()
            }
            Message::GuestPassChanged(s) => {
                self.guest_pass = s;
                Task::none()
            }
            Message::CharDelayChanged(s) => {
                if let Ok(v) = s.parse::<u64>() {
                    if v <= 10_000 {
                        self.char_delay_ms = v;
                    }
                }
                self.char_delay_str = s;
                Task::none()
            }
            Message::StartDelayChanged(s) => {
                if let Ok(v) = s.parse::<u64>() {
                    if v <= 60 {
                        self.start_delay_secs = v;
                    }
                }
                self.start_delay_str = s;
                Task::none()
            }
            Message::AlwaysOnTopToggled(v) => {
                self.always_on_top = v;
                self.save_config();
                set_always_on_top(v);
                Task::none()
            }
            Message::HkClipboardEnabled(v) => {
                self.hotkey_clipboard.enabled = v;
                self.hotkey_worker.update(HK_CLIPBOARD, self.hotkey_clipboard.clone());
                Task::none()
            }
            Message::HkClipboardMod(mask, checked) => {
                if checked {
                    self.hotkey_clipboard.modifiers |= mask;
                } else {
                    self.hotkey_clipboard.modifiers &= !mask;
                }
                self.hotkey_worker.update(HK_CLIPBOARD, self.hotkey_clipboard.clone());
                Task::none()
            }
            Message::HkClipboardKey(k) => {
                self.hotkey_clipboard.vk = k.vk;
                self.hotkey_worker.update(HK_CLIPBOARD, self.hotkey_clipboard.clone());
                Task::none()
            }
            Message::HkCustomEnabled(v) => {
                self.hotkey_custom.enabled = v;
                self.hotkey_worker.update(HK_CUSTOM, self.hotkey_custom.clone());
                Task::none()
            }
            Message::HkCustomMod(mask, checked) => {
                if checked {
                    self.hotkey_custom.modifiers |= mask;
                } else {
                    self.hotkey_custom.modifiers &= !mask;
                }
                self.hotkey_worker.update(HK_CUSTOM, self.hotkey_custom.clone());
                Task::none()
            }
            Message::HkCustomKey(k) => {
                self.hotkey_custom.vk = k.vk;
                self.hotkey_worker.update(HK_CUSTOM, self.hotkey_custom.clone());
                Task::none()
            }
            Message::StartInput => {
                self.start_input();
                Task::none()
            }
            Message::StopInput => {
                self.stop_input();
                Task::none()
            }
            Message::Tick => {
                // Apply initial always-on-top on first tick after window is shown
                if self.needs_topmost_init {
                    self.needs_topmost_init = false;
                    set_always_on_top(true);
                }

                // Poll tray channel
                while let Ok(cmd) = self.tray_rx.try_recv() {
                    match cmd {
                        TrayCmd::ShowWindow => {
                            self.window_visible = true;
                        }
                        TrayCmd::ToggleAlwaysOnTop => {
                            self.always_on_top = !self.always_on_top;
                            self.save_config();
                            if !self.window_visible {
                                self.window_visible = true;
                                unsafe {
                                    let title = "Auto Input — 自动输入";
                                    let w: Vec<u16> =
                                        title.encode_utf16().chain(std::iter::once(0)).collect();
                                    let hwnd =
                                        winapi::um::winuser::FindWindowW(std::ptr::null(), w.as_ptr());
                                    if !hwnd.is_null() {
                                        winapi::um::winuser::ShowWindow(
                                            hwnd,
                                            winapi::um::winuser::SW_SHOW,
                                        );
                                    }
                                }
                            }
                            set_always_on_top(self.always_on_top);
                        }
                    }
                }

                // Poll hotkey worker
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
                            if id == HK_CLIPBOARD {
                                self.hotkey_clipboard_ok = true;
                            }
                            if id == HK_CUSTOM {
                                self.hotkey_custom_ok = true;
                            }
                        }
                        HotkeyEvent::RegisterFailed(id) => {
                            if id == HK_CLIPBOARD {
                                self.hotkey_clipboard_ok = false;
                            }
                            if id == HK_CUSTOM {
                                self.hotkey_custom_ok = false;
                            }
                        }
                    }
                }

                // Update status text
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
                    let vmrun_err = self
                        .vmrun_error
                        .lock()
                        .ok()
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

                Task::none()
            }
            Message::CloseRequested => {
                self.window_visible = false;
                self.save_config();
                unsafe {
                    let title = "Auto Input — 自动输入";
                    let w: Vec<u16> =
                        title.encode_utf16().chain(std::iter::once(0)).collect();
                    let hwnd =
                        winapi::um::winuser::FindWindowW(std::ptr::null(), w.as_ptr());
                    if !hwnd.is_null() {
                        winapi::um::winuser::ShowWindow(hwnd, winapi::um::winuser::SW_HIDE);
                    }
                }
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let running = self.is_running.load(Ordering::Relaxed);

        // ── Header ──────────────────────────────────────────────────────────
        let header = row![
            row![
                text("⌨").font(SYM).size(18),
                text("  Auto Input — 自动输入工具").size(18),
            ].align_y(iced::Alignment::Center),
            horizontal_space(),
            checkbox("置顶", self.always_on_top).on_toggle(Message::AlwaysOnTopToggled),
        ]
        .align_y(iced::Alignment::Center);

        // ── Input source ────────────────────────────────────────────────────
        let source_body: Element<'_, Message> = if self.use_clipboard {
            text(format!("剪切板预览：{}", clipboard_preview())).into()
        } else {
            text_editor(&self.custom_text_content)
                .on_action(Message::TextEditorAction)
                .placeholder("在此输入要模拟键盘输入的文本…")
                .height(130)
                .into()
        };
        let source_section = section(
            "输入来源",
            column![
                row![
                    radio(
                        "自定义文本",
                        false,
                        Some(self.use_clipboard),
                        Message::UseClipboardToggled
                    ),
                    radio(
                        "剪切板内容",
                        true,
                        Some(self.use_clipboard),
                        Message::UseClipboardToggled
                    ),
                ]
                .spacing(16),
                source_body,
            ]
            .spacing(6),
        );

        // ── Input mode ──────────────────────────────────────────────────────
        let hint = match self.input_mode {
            crate::input::MODE_WM_CHAR =>
                "WM_KEYDOWN+WM_CHAR+WM_KEYUP 直接向前台窗口投递，ASCII 字符使用真实 VK 码，适用于从消息队列捕获键盘的远控软件（飞书/向日葵）",
            crate::input::MODE_UNICODE =>
                "Win32 KEYEVENTF_UNICODE 逐字发送，绕过键盘布局，适用于本地禁止粘贴的窗口",
            crate::input::MODE_PASTE =>
                "将文本写入剪贴板后发送 Ctrl+V；一次性粘贴，字符间延迟无效",
            crate::input::MODE_VMRUN =>
                "通过 vmrun 和 PowerShell SendInput 直接在 VMware 客户机内逐字输入（需 VMware Tools）",
            _ => "逐字模拟键盘输入，支持延迟控制",
        };
        let mode_section = section(
            "输入方式",
            column![
                column![
                    radio("逐字模拟按键", crate::input::MODE_CHAR, Some(self.input_mode), Message::InputModeChanged),
                    radio("Unicode 按键", crate::input::MODE_UNICODE, Some(self.input_mode), Message::InputModeChanged),
                    radio("消息注入（飞书远控）", crate::input::MODE_WM_CHAR, Some(self.input_mode), Message::InputModeChanged),
                    radio("粘贴 Ctrl+V", crate::input::MODE_PASTE, Some(self.input_mode), Message::InputModeChanged),
                    radio("VMware 虚拟机", crate::input::MODE_VMRUN, Some(self.input_mode), Message::InputModeChanged),
                ]
                .spacing(4),
                text(hint).size(12),
            ]
            .spacing(6),
        );

        // ── WM_CHAR warning ─────────────────────────────────────────────────
        let wm_warning: Option<Element<'_, Message>> =
            if self.input_mode == crate::input::MODE_WM_CHAR {
                Some(
                    row![
                        text("⚠").font(SYM).size(12).color(Color::from_rgb8(240, 150, 60)),
                        text(" 消息注入模式不支持中文输入。如需发送中文，请开启飞书剪贴板共享后改用「粘贴 Ctrl+V」模式。").size(12).color(Color::from_rgb8(240, 150, 60)),
                    ]
                    .align_y(iced::Alignment::Center)
                    .into(),
                )
            } else {
                None
            };

        // ── VMware settings (conditional) ────────────────────────────────────
        let vm_section: Option<Element<'_, Message>> =
            if self.input_mode == crate::input::MODE_VMRUN {
                let mut vm_col = column![
                    row![
                        text("vmrun 路径：").width(130),
                        text_input("vmrun.exe 路径（自动探测）", &self.vmrun_path)
                            .on_input(Message::VmrunPathChanged)
                            .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("VMX 文件：").width(130),
                        text_input(r"如 D:\VMs\Win10\Win10.vmx", &self.vmx_path)
                            .on_input(Message::VmxPathChanged)
                            .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("客户机用户名：").width(130),
                        text_input("Guest OS 登录用户名", &self.guest_user)
                            .on_input(Message::GuestUserChanged)
                            .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("客户机密码：").width(130),
                        text_input("Guest OS 登录密码", &self.guest_pass)
                            .on_input(Message::GuestPassChanged)
                            .secure(true)
                            .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(6);

                if self.vmrun_path.is_empty() {
                    vm_col = vm_col.push(
                        row![
                            text("⚠").font(SYM).size(12).color(Color::from_rgb8(240, 150, 60)),
                            text(" 未找到 vmrun.exe，请手动指定路径").size(12).color(Color::from_rgb8(240, 150, 60)),
                        ]
                        .align_y(iced::Alignment::Center),
                    );
                }

                Some(section("VMware 设置", vm_col))
            } else {
                None
            };

        // ── Timing settings ──────────────────────────────────────────────────
        let show_char_delay = matches!(
            self.input_mode,
            crate::input::MODE_CHAR
                | crate::input::MODE_UNICODE
                | crate::input::MODE_WM_CHAR
                | crate::input::MODE_VMRUN
        );
        let mut timing_col = column![].spacing(6);
        if show_char_delay {
            let cps = if self.char_delay_ms == 0 {
                String::from("（无限制）")
            } else {
                format!("≈ {:.1} 字/秒", 1000.0 / self.char_delay_ms as f64)
            };
            timing_col = timing_col.push(
                row![
                    text("字符间延迟（ms）：").width(150),
                    text_input("50", &self.char_delay_str)
                        .on_input(Message::CharDelayChanged)
                        .width(70),
                    text(cps).size(12),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
        timing_col = timing_col.push(
            row![
                text("开始延迟（秒）：").width(150),
                text_input("3", &self.start_delay_str)
                    .on_input(Message::StartDelayChanged)
                    .width(70),
                text("（开始前请切换到目标窗口）").size(12),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
        let timing_section = section("时间设置", timing_col);

        // ── Hotkey settings ──────────────────────────────────────────────────
        let key_opts = all_key_opts();
        let hk_cb_row = build_hotkey_row(
            "输入剪切板",
            &self.hotkey_clipboard,
            self.hotkey_clipboard_ok,
            &key_opts,
            true,
        );
        let hk_cu_row = build_hotkey_row(
            "输入自定义",
            &self.hotkey_custom,
            self.hotkey_custom_ok,
            &key_opts,
            false,
        );
        let hotkey_section = section(
            "全局热键",
            column![
                row![
                    text("⚠").font(SYM).size(12).color(Color::from_rgb8(220, 170, 60)),
                    text(" 焦点在虚拟机窗口内时，宿主机热键无法被接收，请先点击虚拟机外部再触发。").size(12).color(Color::from_rgb8(220, 170, 60)),
                ]
                .align_y(iced::Alignment::Center),
                hk_cb_row,
                hk_cu_row,
            ]
            .spacing(6),
        );

        // ── Control buttons ──────────────────────────────────────────────────
        let has_text = self.use_clipboard || !self.custom_text_content.text().is_empty();
        let vm_ready = self.input_mode != crate::input::MODE_VMRUN
            || (!self.vmrun_path.is_empty()
                && !self.vmx_path.is_empty()
                && !self.guest_user.is_empty());
        let can_start = !running && has_text && vm_ready;

        let controls = row![
            button(
                row![
                    text("▶").font(SYM),
                    text("  开始输入"),
                ].align_y(iced::Alignment::Center).spacing(2)
            )
            .on_press_maybe(if can_start { Some(Message::StartInput) } else { None }),
            button(
                row![
                    text("⏹").font(SYM),
                    text("  停止"),
                ].align_y(iced::Alignment::Center).spacing(2)
            )
            .on_press_maybe(if running { Some(Message::StopInput) } else { None }),
        ]
        .spacing(8);

        // ── Status ───────────────────────────────────────────────────────────
        let status_color = if running {
            Color::from_rgb8(80, 200, 100)
        } else if self.status_text.contains('✓') {
            Color::from_rgb8(100, 180, 255)
        } else if self.status_text.starts_with("vmrun")
            || self.status_text.starts_with("无法执行")
        {
            Color::from_rgb8(240, 60, 60)
        } else if self.status_text == "已停止" {
            Color::from_rgb8(240, 150, 60)
        } else {
            Color::from_rgb8(150, 150, 150)
        };

        // ── Assemble ─────────────────────────────────────────────────────────
        let mut content = column![
            header,
            horizontal_rule(1),
            source_section,
            mode_section,
        ]
        .spacing(8);

        if let Some(w) = wm_warning {
            content = content.push(w);
        }
        if let Some(vm) = vm_section {
            content = content.push(vm);
        }

        content = content
            .push(timing_section)
            .push(hotkey_section)
            .push(controls)
            .push(
                text(format!("状态：{}", self.status_text))
                    .color(status_color),
            )
            .push(horizontal_rule(1))
            .push(
                text("提示：关闭窗口后程序最小化到系统托盘；右键托盘图标可彻底退出")
                    .size(11)
                    .color(Color::from_rgb8(150, 150, 150)),
            );

        scrollable(
            container(content.spacing(8))
                .padding(16)
                .width(Length::Fill),
        )
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let tick =
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick);
        let events = iced::event::listen_with(|event, _status, _id| {
            if let iced::event::Event::Window(iced::window::Event::CloseRequested) = event {
                Some(Message::CloseRequested)
            } else {
                None
            }
        });
        Subscription::batch([tick, events])
    }
}

// ─── Hotkey row builder ──────────────────────────────────────────────────────

fn build_hotkey_row<'a>(
    label: &str,
    def: &HotkeyDef,
    reg_ok: bool,
    key_opts: &[KeyOpt],
    is_clipboard: bool,
) -> Element<'a, Message> {
    let enabled = def.enabled;

    let enable_cb: Element<'_, Message> = if is_clipboard {
        checkbox(label, enabled)
            .on_toggle(Message::HkClipboardEnabled)
            .into()
    } else {
        checkbox(label, enabled)
            .on_toggle(Message::HkCustomEnabled)
            .into()
    };

    // Modifier checkboxes — only interactive when the hotkey is enabled
    let make_mod_cb = |lbl: &'static str, mask: u32, checked: bool| -> Element<'a, Message> {
        let cb = checkbox(lbl, checked);
        if enabled {
            if is_clipboard {
                cb.on_toggle(move |v| Message::HkClipboardMod(mask, v)).into()
            } else {
                cb.on_toggle(move |v| Message::HkCustomMod(mask, v)).into()
            }
        } else {
            cb.into()
        }
    };

    let cb_ctrl = make_mod_cb("Ctrl", MOD_CTRL, def.modifiers & MOD_CTRL != 0);
    let cb_alt = make_mod_cb("Alt", MOD_ALT, def.modifiers & MOD_ALT != 0);
    let cb_shift = make_mod_cb("Shift", MOD_SHIFT, def.modifiers & MOD_SHIFT != 0);
    let cb_win = make_mod_cb("Win", MOD_WIN, def.modifiers & MOD_WIN != 0);

    // Key pick-list — only shown when enabled
    let selected_key = key_opts.iter().find(|k| k.vk == def.vk).cloned();
    let key_pick: Element<'_, Message> = if enabled {
        if is_clipboard {
            pick_list(key_opts.to_vec(), selected_key, Message::HkClipboardKey)
                .placeholder("(未设置)")
                .width(90)
                .into()
        } else {
            pick_list(key_opts.to_vec(), selected_key, Message::HkCustomKey)
                .placeholder("(未设置)")
                .width(90)
                .into()
        }
    } else {
        text(if def.vk == 0 {
            "(未设置)".to_owned()
        } else {
            crate::hotkey::vk_name(def.vk).to_owned()
        })
        .size(13)
        .into()
    };

    let disp = def.display();
    let mut r: iced::widget::Row<'_, Message> = row![
        enable_cb,
        cb_ctrl,
        cb_alt,
        cb_shift,
        cb_win,
        key_pick,
        text(disp)
            .size(12)
            .color(Color::from_rgb8(120, 120, 120)),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    if enabled && def.vk != 0 && !reg_ok {
        r = r.push(
            row![
                text("⚠").font(SYM).size(12).color(Color::from_rgb8(240, 150, 60)),
                text(" 快捷键冲突").size(12).color(Color::from_rgb8(240, 150, 60)),
            ]
            .align_y(iced::Alignment::Center),
        );
    }

    r.into()
}

// ─── Win32 window helpers ────────────────────────────────────────────────────

fn set_always_on_top(enable: bool) {
    use winapi::um::winuser::{
        FindWindowW, SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
    };
    unsafe {
        let title = "Auto Input — 自动输入";
        let w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = FindWindowW(std::ptr::null(), w.as_ptr());
        if !hwnd.is_null() {
            let insert_after = if enable { HWND_TOPMOST } else { HWND_NOTOPMOST };
            SetWindowPos(hwnd, insert_after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        }
    }
}

// ─── Section layout helper ───────────────────────────────────────────────────

fn section<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    column![
        text(title)
            .size(14),
        container(content)
            .padding(8)
            .width(Length::Fill)
            .style(container::rounded_box),
    ]
    .spacing(4)
    .into()
}

// ─── App entry point ─────────────────────────────────────────────────────────

/// 尝试加载 Windows 自带符号/Emoji 字体（Segoe UI Emoji / Segoe UI Symbol）。
/// cosmic-text 支持多字体 fallback，注册后当主字体缺少某字形时自动在已注册字体中查找。
/// 加载所有可用的 Windows 符号/Emoji fallback 字体。
/// - seguisym.ttf ：Segoe UI Symbol，包含 ▶ ⏹ ⌨ ⚠ 等几何符号
/// - seguiemj.ttf ：Segoe UI Emoji，包含 📍 等彩色 Emoji
/// 两个字体都必须加载，前者存皮符号，后者存 Emoji。
fn load_fallback_fonts() -> Vec<&'static [u8]> {
    let candidates = &[
        r"C:\Windows\Fonts\seguisym.ttf", // Segoe UI Symbol — ▶ ⏹ ⌨ ⚠
        r"C:\Windows\Fonts\seguiemj.ttf", // Segoe UI Emoji  — 📍
    ];
    let mut result = Vec::new();
    for &path in candidates {
        if let Ok(data) = std::fs::read(path) {
            result.push(Box::leak(data.into_boxed_slice()) as &'static [u8]);
        }
    }
    result
}

/// 从 Windows 系统字体目录读取首个可用的 CJK 字体，
/// 返回 (字节数据, 字体族名称)。
/// 用 `Box::leak` 将 Vec<u8> 提升为 `'static`，生命周期与进程相同。
fn try_load_cjk_font() -> Option<(&'static [u8], Font)> {
    // (文件路径, PostScript/GDI 字族名)
    let candidates: &[(&str, &'static str)] = &[
        (r"C:\Windows\Fonts\msyh.ttc",  "Microsoft YaHei"), // Win7+
        (r"C:\Windows\Fonts\simhei.ttf", "SimHei"),
        (r"C:\Windows\Fonts\simsun.ttc", "SimSun"),
    ];
    for &(path, family) in candidates {
        if let Ok(data) = std::fs::read(path) {
            let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
            return Some((bytes, Font::with_name(family)));
        }
    }
    None
}

pub fn run() -> iced::Result {
    let (icon_rgba, icon_w, icon_h) = make_icon_rgba();
    let icon = iced::window::icon::from_rgba(icon_rgba, icon_w, icon_h).unwrap();

    let base = iced::application(
        "Auto Input — 自动输入",
        AutoInputApp::update,
        AutoInputApp::view,
    )
    .window(iced::window::Settings {
        size: iced::Size::new(560.0, 560.0),
        min_size: Some(iced::Size::new(440.0, 420.0)),
        resizable: true,
        icon: Some(icon),
        exit_on_close_request: false,
        ..Default::default()
    })
    .subscription(AutoInputApp::subscription)
    .theme(|_| iced::Theme::Light);

    // 必须同时调用 .font() + .default_font()：
    //   .font(bytes)       → 把字体字节注册进 cosmic-text 字体数据库
    //   .default_font(f)   → 告诉 iced 用该字体族渲染所有文本（含汉字）
    // 仅调用 .font() 而不设置 default_font，iced 仍会使用内置英文字体渲染，
    // 导致 CJK 字符因在默认字体中不存在而显示为方块。
    match try_load_cjk_font() {
        Some((cjk_bytes, font)) => {
            let mut app = base.font(cjk_bytes).default_font(font);
            for sym_bytes in load_fallback_fonts() {
                app = app.font(sym_bytes);
            }
            app.run_with(|| AutoInputApp::new())
        }
        None => base.run_with(|| AutoInputApp::new()),
    }
}

// ─── Icon helpers ─────────────────────────────────────────────────────────────

pub fn make_icon_rgba() -> (Vec<u8>, u32, u32) {
    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgba = vec![0u8; (W * H * 4) as usize];

    for y in 0..H {
        for x in 0..W {
            let idx = ((y * W + x) * 4) as usize;
            if x >= 3 && x < 29 && y >= 9 && y < 23 {
                let is_border = x == 3 || x == 28 || y == 9 || y == 22;
                if is_border {
                    rgba[idx] = 200;
                    rgba[idx + 1] = 220;
                    rgba[idx + 2] = 255;
                    rgba[idx + 3] = 255;
                } else {
                    rgba[idx] = 55;
                    rgba[idx + 1] = 85;
                    rgba[idx + 2] = 155;
                    rgba[idx + 3] = 230;
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
        Err(_) => String::from("（剪切板为空或无法读取）"),
    }
}

