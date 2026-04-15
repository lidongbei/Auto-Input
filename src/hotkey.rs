use std::sync::mpsc;
use std::time::Duration;

// Windows 修饰键常量
pub const MOD_ALT:      u32 = 0x0001;
pub const MOD_CTRL:     u32 = 0x0002;
pub const MOD_SHIFT:    u32 = 0x0004;
pub const MOD_WIN:      u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000; // Win8+，旧系统忽略此标志

/// 热键 ID（传给 Win32 RegisterHotKey）
pub const HK_CLIPBOARD: i32 = 1;
pub const HK_CUSTOM:    i32 = 2;

/// 热键定义
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct HotkeyDef {
    #[serde(default)]
    pub enabled: bool,
    /// 修饰键组合，OR 合并 MOD_CTRL / MOD_ALT / MOD_SHIFT / MOD_WIN
    #[serde(default)]
    pub modifiers: u32,
    /// Windows 虚拟键码，0 表示未设置
    #[serde(default)]
    pub vk: u32,
}

impl HotkeyDef {
    pub fn display(&self) -> String {
        if !self.enabled || self.vk == 0 {
            return String::from("(未启用)");
        }
        let mut parts = Vec::<&str>::new();
        if self.modifiers & MOD_CTRL  != 0 { parts.push("Ctrl"); }
        if self.modifiers & MOD_ALT   != 0 { parts.push("Alt"); }
        if self.modifiers & MOD_SHIFT != 0 { parts.push("Shift"); }
        if self.modifiers & MOD_WIN   != 0 { parts.push("Win"); }
        let mut s = parts.join("+");
        if !parts.is_empty() { s.push('+'); }
        s.push_str(vk_name(self.vk));
        s
    }
}

/// UI ComboBox 可选键列表
pub const KEY_OPTIONS: &[(u32, &str)] = &[
    (0x70, "F1"),  (0x71, "F2"),  (0x72, "F3"),  (0x73, "F4"),
    (0x74, "F5"),  (0x75, "F6"),  (0x76, "F7"),  (0x77, "F8"),
    (0x78, "F9"),  (0x79, "F10"), (0x7A, "F11"), (0x7B, "F12"),
    (0x41,"A"), (0x42,"B"), (0x43,"C"), (0x44,"D"), (0x45,"E"),
    (0x46,"F"), (0x47,"G"), (0x48,"H"), (0x49,"I"), (0x4A,"J"),
    (0x4B,"K"), (0x4C,"L"), (0x4D,"M"), (0x4E,"N"), (0x4F,"O"),
    (0x50,"P"), (0x51,"Q"), (0x52,"R"), (0x53,"S"), (0x54,"T"),
    (0x55,"U"), (0x56,"V"), (0x57,"W"), (0x58,"X"), (0x59,"Y"),
    (0x5A,"Z"),
    (0x30,"0"), (0x31,"1"), (0x32,"2"), (0x33,"3"), (0x34,"4"),
    (0x35,"5"), (0x36,"6"), (0x37,"7"), (0x38,"8"), (0x39,"9"),
];

pub fn vk_name(vk: u32) -> &'static str {
    for &(v, name) in KEY_OPTIONS {
        if v == vk { return name; }
    }
    "?"
}

// ─── 后台工作线程通信 ─────────────────────────────────────────────────────────

/// 从工作线程发向主线程的事件
pub enum HotkeyEvent {
    /// 热键被触发
    Triggered(i32),
    /// 注册成功
    RegisterOk(i32),
    /// 注册失败（与其他程序冲突）
    RegisterFailed(i32),
}

enum WorkerCmd {
    Update { id: i32, def: HotkeyDef },
    Shutdown,
}

pub struct HotkeyWorker {
    cmd_tx: mpsc::Sender<WorkerCmd>,
    pub event_rx: mpsc::Receiver<HotkeyEvent>,
}

impl HotkeyWorker {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (event_tx, event_rx) = mpsc::channel::<HotkeyEvent>();
        std::thread::spawn(move || worker_loop(cmd_rx, event_tx));
        Self { cmd_tx, event_rx }
    }

    /// 更新（或注销）一个热键。启用且 vk != 0 时注册，否则注销。
    pub fn update(&self, id: i32, def: HotkeyDef) {
        let _ = self.cmd_tx.send(WorkerCmd::Update { id, def });
    }
}

impl Drop for HotkeyWorker {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
    }
}

fn worker_loop(cmd_rx: mpsc::Receiver<WorkerCmd>, event_tx: mpsc::Sender<HotkeyEvent>) {
    use winapi::um::winuser::{PeekMessageW, RegisterHotKey, UnregisterHotKey, PM_REMOVE, WM_HOTKEY};

    // 已成功注册的热键 ID 集合
    let mut registered: std::collections::HashSet<i32> = std::collections::HashSet::new();

    loop {
        // ── 处理命令 ──────────────────────────────────────────────────────
        loop {
            match cmd_rx.try_recv() {
                Ok(WorkerCmd::Update { id, def }) => {
                    // 先注销旧注册
                    if registered.contains(&id) {
                        unsafe { UnregisterHotKey(std::ptr::null_mut(), id); }
                        registered.remove(&id);
                    }
                    // 满足条件时重新注册
                    if def.enabled && def.vk != 0 {
                        let mods = def.modifiers | MOD_NOREPEAT;
                        let ok = unsafe {
                            RegisterHotKey(std::ptr::null_mut(), id, mods, def.vk)
                        };
                        if ok != 0 {
                            registered.insert(id);
                            let _ = event_tx.send(HotkeyEvent::RegisterOk(id));
                        } else {
                            let _ = event_tx.send(HotkeyEvent::RegisterFailed(id));
                        }
                    }
                }
                Ok(WorkerCmd::Shutdown) => {
                    for &id in &registered {
                        unsafe { UnregisterHotKey(std::ptr::null_mut(), id); }
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // ── 轮询 WM_HOTKEY 消息 ──────────────────────────────────────────
        unsafe {
            let mut msg: winapi::um::winuser::MSG = std::mem::zeroed();
            while PeekMessageW(
                &mut msg,
                std::ptr::null_mut(),
                WM_HOTKEY,
                WM_HOTKEY,
                PM_REMOVE,
            ) != 0 {
                let _ = event_tx.send(HotkeyEvent::Triggered(msg.wParam as i32));
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}
