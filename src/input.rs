use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// 输入模式
/// - 0：逐字模拟按键（enigo，适用于普通窗口）
/// - 1：粘贴 Ctrl+V（先写剪贴板再发 Ctrl+V）
/// - 2：VMware vmrun（通过 VIX API 直接在客户机内输入）
/// - 3：Unicode SendInput（Win32 KEYEVENTF_UNICODE，适用于飞书/远程控制等场景）
/// - 4：WM_CHAR 消息注入（PostMessage 直接注入到焦点窗口，绕过键盘钩子过滤）
pub const MODE_CHAR: u8 = 0;
pub const MODE_PASTE: u8 = 1;
pub const MODE_VMRUN: u8 = 2;
pub const MODE_UNICODE: u8 = 3;
pub const MODE_WM_CHAR: u8 = 4;

// ── Unicode SendInput 模式：KEYEVENTF_UNICODE ───────────────────────────────
// 直接发送 Unicode 码点，完全绕过键盘布局映射，适用于飞书远程控制等场景。
// 对于代理对（Surrogate Pair）字符，会发送两个 UTF-16 编码单元。
#[cfg(windows)]
fn send_unicode_char(c: char) {
    use winapi::shared::minwindef::WORD;
    use winapi::um::winuser::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_RETURN, VK_TAB,
    };

    if c == '\r' {
        return;
    }

    // 换行 / Tab 使用虚拟键，避免远控协议对控制字符的特殊处理
    if c == '\n' || c == '\t' {
        let vk = if c == '\n' { VK_RETURN as WORD } else { VK_TAB as WORD };
        let mut inputs: [INPUT; 2] = unsafe { std::mem::zeroed() };
        unsafe {
            inputs[0].type_ = INPUT_KEYBOARD;
            inputs[0].u.ki_mut().wVk = vk;
            inputs[1].type_ = INPUT_KEYBOARD;
            inputs[1].u.ki_mut().wVk = vk;
            inputs[1].u.ki_mut().dwFlags = KEYEVENTF_KEYUP;
            SendInput(2, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
        return;
    }

    // 普通字符：UTF-16 编码后逐码元发送 KEYEVENTF_UNICODE
    let mut buf = [0u16; 2];
    let encoded = c.encode_utf16(&mut buf);
    unsafe {
        for &code_unit in encoded.iter() {
            let mut inputs: [INPUT; 2] = std::mem::zeroed();
            inputs[0].type_ = INPUT_KEYBOARD;
            inputs[0].u.ki_mut().wScan = code_unit;
            inputs[0].u.ki_mut().dwFlags = KEYEVENTF_UNICODE;
            inputs[1].type_ = INPUT_KEYBOARD;
            inputs[1].u.ki_mut().wScan = code_unit;
            inputs[1].u.ki_mut().dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
            SendInput(2, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }
}

// ── WM_CHAR 消息注入模式（飞书/向日葵等远程控制）─────────────────────────────
// SendInput 产生的事件携带 LLKHF_INJECTED 标志，飞书等软件可能拦截过滤。
// 此模式直接向前台窗口投递 WM_KEYDOWN + WM_CHAR + WM_KEYUP 完整消息序列，
// ASCII 字符映射真实虚拟键码并附带 Shift 修饰，让远控软件能识别完整按键事件并转发。

/// ASCII 字符 → (VK 码, 是否需要 Shift)；仅覆盖英文键盘标准布局上有直接映射的字符
#[cfg(windows)]
fn char_to_vk_shift(c: char) -> Option<(u32, bool)> {
    let r = match c {
        'a'..='z' => (c as u32 - b'a' as u32 + 0x41, false),
        'A'..='Z' => (c as u32 - b'A' as u32 + 0x41, true),
        '0'..='9' => (c as u32, false),
        ' '  => (0x20, false),
        '!'  => (0x31, true),  '@'  => (0x32, true),  '#'  => (0x33, true),
        '$'  => (0x34, true),  '%'  => (0x35, true),  '^'  => (0x36, true),
        '&'  => (0x37, true),  '*'  => (0x38, true),  '('  => (0x39, true),
        ')'  => (0x30, true),
        '-'  => (0xBD, false), '_'  => (0xBD, true),
        '='  => (0xBB, false), '+'  => (0xBB, true),
        '['  => (0xDB, false), '{'  => (0xDB, true),
        ']'  => (0xDD, false), '}'  => (0xDD, true),
        '\\' => (0xDC, false), '|'  => (0xDC, true),
        ';'  => (0xBA, false), ':'  => (0xBA, true),
        '\'' => (0xDE, false), '"'  => (0xDE, true),
        ','  => (0xBC, false), '<'  => (0xBC, true),
        '.'  => (0xBE, false), '>'  => (0xBE, true),
        '/'  => (0xBF, false), '?'  => (0xBF, true),
        '`'  => (0xC0, false), '~'  => (0xC0, true),
        _    => return None,
    };
    Some(r)
}

/// 向前台窗口投递 WM_KEYDOWN + WM_CHAR + WM_KEYUP 完整按键消息序列。
/// ASCII 字符使用真实 VK 码（含 Shift 修饰），非 ASCII 回退到 WM_CHAR。
#[cfg(windows)]
fn send_key_to_foreground(c: char) {
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
        MapVirtualKeyW, PostMessageW, GUITHREADINFO,
        WM_CHAR, WM_KEYDOWN, WM_KEYUP, VK_SHIFT, VK_RETURN, VK_TAB,
    };
    // MAPVK_VK_TO_VSC = 0
    const MAPVK_VK_TO_VSC: u32 = 0;

    if c == '\r' { return; }

    unsafe {
        let hwnd_fg: HWND = GetForegroundWindow();
        if hwnd_fg.is_null() { return; }

        // 辅助闭包：构造 lParam
        let make_lp = |scan: u32, is_up: bool| -> isize {
            let base = 1u32 | (scan << 16);
            if is_up { (0xC000_0000u32 | base) as i32 as isize }
            else      { base as i32 as isize }
        };

        // \n / \t → 对应虚拟键
        let ctrl_vk: Option<u32> = match c {
            '\n' => Some(VK_RETURN as u32),
            '\t' => Some(VK_TAB    as u32),
            _ => None,
        };
        if let Some(vk) = ctrl_vk {
            let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            PostMessageW(hwnd_fg, WM_KEYDOWN, vk as usize, make_lp(scan, false));
            PostMessageW(hwnd_fg, WM_KEYUP,   vk as usize, make_lp(scan, true));
            return;
        }

        // ASCII 字符 → 真实 VK + Shift
        if let Some((vk, need_shift)) = char_to_vk_shift(c) {
            let scan   = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            let shscan = MapVirtualKeyW(VK_SHIFT as u32, MAPVK_VK_TO_VSC);
            if need_shift {
                PostMessageW(hwnd_fg, WM_KEYDOWN, VK_SHIFT as usize, make_lp(shscan, false));
            }
            PostMessageW(hwnd_fg, WM_KEYDOWN, vk as usize, make_lp(scan, false));
            PostMessageW(hwnd_fg, WM_CHAR,    c  as usize, make_lp(scan, false));
            PostMessageW(hwnd_fg, WM_KEYUP,   vk as usize, make_lp(scan, true));
            if need_shift {
                PostMessageW(hwnd_fg, WM_KEYUP, VK_SHIFT as usize, make_lp(shscan, true));
            }
            return;
        }

        // 非 ASCII → WM_CHAR 到焦点子窗口
        let tid = GetWindowThreadProcessId(hwnd_fg, std::ptr::null_mut());
        let mut gui_info: GUITHREADINFO = std::mem::zeroed();
        gui_info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        let target: HWND = if GetGUIThreadInfo(tid, &mut gui_info) != 0
            && !gui_info.hwndFocus.is_null()
        {
            gui_info.hwndFocus
        } else {
            hwnd_fg
        };
        let mut buf = [0u16; 2];
        let len = c.encode_utf16(&mut buf).len();
        for i in 0..len {
            PostMessageW(target, WM_CHAR, buf[i] as usize, 1);
        }
    }
}

// ── 粘贴模式：Ctrl+V ─────────────────────────────────────────────────────────
#[cfg(windows)]
fn send_ctrl_v() {
    use winapi::shared::minwindef::WORD;
    use winapi::um::winuser::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, VK_CONTROL,
    };

    const VK_V: WORD = 0x56;
    let mut inputs: [INPUT; 4] = unsafe { std::mem::zeroed() };

    unsafe {
        inputs[0].type_ = INPUT_KEYBOARD;
        inputs[0].u.ki_mut().wVk = VK_CONTROL as WORD;
        inputs[1].type_ = INPUT_KEYBOARD;
        inputs[1].u.ki_mut().wVk = VK_V;
        inputs[2].type_ = INPUT_KEYBOARD;
        inputs[2].u.ki_mut().wVk = VK_V;
        inputs[2].u.ki_mut().dwFlags = KEYEVENTF_KEYUP;
        inputs[3].type_ = INPUT_KEYBOARD;
        inputs[3].u.ki_mut().wVk = VK_CONTROL as WORD;
        inputs[3].u.ki_mut().dwFlags = KEYEVENTF_KEYUP;
        SendInput(4, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// 在后台线程中执行键盘输入模拟。
///
/// - `use_clipboard`：为 true 时从剪切板读取文本，否则使用 `custom_text`
/// - `input_mode`：MODE_CHAR / MODE_PASTE
/// - `char_delay_ms`：每个字符之间的等待时间（毫秒，粘贴模式无效）
/// - `start_delay_secs`：开始前的延迟（秒）
/// - `is_running` / `stop_flag`：运行控制
pub fn run_input(
    use_clipboard: bool,
    input_mode: u8,
    custom_text: String,
    char_delay_ms: u64,
    start_delay_secs: u64,
    is_running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
) {
    // ── 倒计时阶段（可中断）─────────────────────────────────────────────
    if start_delay_secs > 0 {
        let deadline = Instant::now() + Duration::from_secs(start_delay_secs);
        while Instant::now() < deadline {
            if stop_flag.load(Ordering::Relaxed) {
                is_running.store(false, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    if stop_flag.load(Ordering::Relaxed) {
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 获取待输入文本 ─────────────────────────────────────────────────
    let text = if use_clipboard {
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.get_text() {
                Ok(t) => t,
                Err(_) => {
                    is_running.store(false, Ordering::Relaxed);
                    return;
                }
            },
            Err(_) => {
                is_running.store(false, Ordering::Relaxed);
                return;
            }
        }
    } else {
        custom_text
    };

    if text.is_empty() {
        is_running.store(false, Ordering::Relaxed);
        return;
    }
    // ── WM_CHAR 消息注入模式 ────────────────────────────────────────────────────
    if input_mode == MODE_WM_CHAR {
        let delay = Duration::from_millis(char_delay_ms);
        for ch in text.chars() {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            #[cfg(windows)]
            send_key_to_foreground(ch);
            if char_delay_ms > 0 {
                std::thread::sleep(delay);
            }
        }
        is_running.store(false, Ordering::Relaxed);
        return;
    }
    // ── Unicode SendInput 模式 ──────────────────────────────────────────
    if input_mode == MODE_UNICODE {
        let delay = Duration::from_millis(char_delay_ms);
        for ch in text.chars() {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            #[cfg(windows)]
            send_unicode_char(ch);
            if char_delay_ms > 0 {
                std::thread::sleep(delay);
            }
        }
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 粘贴模式 Ctrl+V ────────────────────────────────────────────────
    if input_mode == MODE_PASTE {
        if !use_clipboard {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(&text);
            } else {
                is_running.store(false, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        #[cfg(windows)]
        send_ctrl_v();
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 逐字模拟按键模式 ───────────────────────────────────────────────
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(_) => {
            is_running.store(false, Ordering::Relaxed);
            return;
        }
    };

    let delay = Duration::from_millis(char_delay_ms);

    for ch in text.chars() {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }
        let _ = match ch {
            '\n' | '\r' => enigo.key(Key::Return, Direction::Click),
            '\t' => enigo.key(Key::Tab, Direction::Click),
            _ => enigo.text(&ch.to_string()),
        };
        if char_delay_ms > 0 {
            std::thread::sleep(delay);
        }
    }

    is_running.store(false, Ordering::Relaxed);
}

// ─── VMware vmrun 模式 ───────────────────────────────────────────────────────

/// 自动探测 vmrun.exe 路径
pub fn detect_vmrun() -> Option<String> {
    let candidates = [
        r"C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe",
        r"C:\Program Files\VMware\VMware Workstation\vmrun.exe",
        r"C:\Program Files (x86)\VMware\VMware VIX\vmrun.exe",
    ];
    for p in &candidates {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    // 尝试 PATH
    if let Ok(output) = std::process::Command::new("where").arg("vmrun").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = s.lines().next() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// 通过 vmrun 将文本输入到 VMware 客户机
/// 流程：写临时文件 → copyFileFromHostToGuest → runProgramInGuest
/// 在客户机内用 PowerShell SendKeys 逐字模拟键盘输入（非剪贴板粘贴）
pub fn run_vmrun_input(
    vmrun_path: String,
    vmx_path: String,
    guest_user: String,
    guest_pass: String,
    use_clipboard: bool,
    custom_text: String,
    char_delay_ms: u64,
    start_delay_secs: u64,
    is_running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    error_msg: Arc<std::sync::Mutex<String>>,
) {
    // ── 倒计时阶段 ──────────────────────────────────────────────────────
    if start_delay_secs > 0 {
        let deadline = Instant::now() + Duration::from_secs(start_delay_secs);
        while Instant::now() < deadline {
            if stop_flag.load(Ordering::Relaxed) {
                is_running.store(false, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    if stop_flag.load(Ordering::Relaxed) {
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 获取待输入文本 ──────────────────────────────────────────────────
    let text = if use_clipboard {
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.get_text() {
                Ok(t) => t,
                Err(_) => {
                    is_running.store(false, Ordering::Relaxed);
                    return;
                }
            },
            Err(_) => {
                is_running.store(false, Ordering::Relaxed);
                return;
            }
        }
    } else {
        custom_text
    };

    if text.is_empty() {
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 先验证 vmrun 是否可用 ──────────────────────────────────────────
    if !std::path::Path::new(&vmrun_path).exists() {
        if let Ok(mut s) = error_msg.lock() {
            *s = format!("vmrun 不存在：{}", vmrun_path);
        }
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── 辅助闭包：执行 vmrun 命令并检查结果 ─────────────────────────────
    let run_vmrun = |args: &[&str]| -> Result<String, String> {
        let mut cmd_args = vec!["-T", "ws", "-gu", &guest_user, "-gp", &guest_pass];
        cmd_args.extend_from_slice(args);
        let output = std::process::Command::new(&vmrun_path)
            .args(&cmd_args)
            .output()
            .map_err(|e| format!("无法执行 vmrun：{}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            return Err(format!("vmrun 错误：{}", detail));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    };

    // ── Step 1：将文本写入宿主机临时文件 ────────────────────────────────
    let host_text_file = std::env::temp_dir().join("auto_input_text.txt");
    // 写 UTF-8 BOM + 内容，确保客户机 PowerShell 正确读取中文
    let bom_text = format!("\u{FEFF}{}", text);
    if let Err(e) = std::fs::write(&host_text_file, &bom_text) {
        if let Ok(mut s) = error_msg.lock() {
            *s = format!("写入临时文件失败：{}", e);
        }
        is_running.store(false, Ordering::Relaxed);
        return;
    }
    let host_text_str = host_text_file.to_string_lossy().to_string();

    let guest_text_path = r"C:\Users\Public\auto_input_text.txt";
    let guest_script_path = r"C:\Users\Public\auto_input_type.ps1";

    // ── Step 2：将文本文件复制到客户机 ──────────────────────────────────
    if let Err(e) = run_vmrun(&["copyFileFromHostToGuest", &vmx_path, &host_text_str, guest_text_path]) {
        if let Ok(mut s) = error_msg.lock() { *s = e; }
        let _ = std::fs::remove_file(&host_text_file);
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── Step 3：生成 PowerShell 脚本（SendInput KEYEVENTF_UNICODE，逐字发送）──
    // 使用内联 C# 调用 Win32 SendInput，以 KEYEVENTF_UNICODE 标志发送每个字符。
    // 该方式直接发送 Unicode 码点，与键盘布局和 Shift 状态完全无关，
    // 能正确输入 !@#$%^&*() 等所有需要 Shift 的符号，以及中文、特殊字符等。
    let delay_ms = char_delay_ms.max(10);
    let ps_script = format!(
r#"$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class UnicodeKeyboard {{
    [DllImport("user32.dll", SetLastError=true)]
    static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
    [StructLayout(LayoutKind.Sequential)]
    struct INPUT {{
        public uint type;
        public KEYBDINPUT ki;
        uint pad1; uint pad2;
    }}
    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT {{
        public ushort wVk;
        public ushort wScan;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }}
    const uint INPUT_KEYBOARD = 1;
    const uint KEYEVENTF_UNICODE = 0x0004;
    const uint KEYEVENTF_KEYUP   = 0x0002;
    const uint KEYEVENTF_EXTENDEDKEY = 0x0001;
    public static void TypeChar(char c) {{
        if (c == '\r') return;
        ushort scan = (ushort)c;
        uint flags = KEYEVENTF_UNICODE;
        if (c == '\n') {{ scan = 0x0D; flags = 0; TypeVK(0x0D); return; }}
        var inputs = new INPUT[2];
        inputs[0].type = INPUT_KEYBOARD;
        inputs[0].ki.wScan = scan;
        inputs[0].ki.dwFlags = flags;
        inputs[1].type = INPUT_KEYBOARD;
        inputs[1].ki.wScan = scan;
        inputs[1].ki.dwFlags = flags | KEYEVENTF_KEYUP;
        SendInput(2, inputs, Marshal.SizeOf(typeof(INPUT)));
    }}
    static void TypeVK(ushort vk) {{
        var inputs = new INPUT[2];
        inputs[0].type = INPUT_KEYBOARD;
        inputs[0].ki.wVk = vk;
        inputs[1].type = INPUT_KEYBOARD;
        inputs[1].ki.wVk = vk;
        inputs[1].ki.dwFlags = KEYEVENTF_KEYUP;
        SendInput(2, inputs, Marshal.SizeOf(typeof(INPUT)));
    }}
}}
"@
$text = [IO.File]::ReadAllText('{guest_text}', [Text.Encoding]::UTF8)
foreach ($ch in $text.ToCharArray()) {{
    [UnicodeKeyboard]::TypeChar($ch)
    Start-Sleep -Milliseconds {delay}
}}
Remove-Item '{guest_text}' -ErrorAction SilentlyContinue
Remove-Item $MyInvocation.MyCommand.Path -ErrorAction SilentlyContinue
"#,
        guest_text = guest_text_path,
        delay = delay_ms,
    );

    let host_script_file = std::env::temp_dir().join("auto_input_type.ps1");
    if let Err(e) = std::fs::write(&host_script_file, &ps_script) {
        if let Ok(mut s) = error_msg.lock() {
            *s = format!("写入脚本文件失败：{}", e);
        }
        let _ = std::fs::remove_file(&host_text_file);
        is_running.store(false, Ordering::Relaxed);
        return;
    }
    let host_script_str = host_script_file.to_string_lossy().to_string();

    if let Err(e) = run_vmrun(&["copyFileFromHostToGuest", &vmx_path, &host_script_str, guest_script_path]) {
        if let Ok(mut s) = error_msg.lock() { *s = e; }
        let _ = std::fs::remove_file(&host_text_file);
        let _ = std::fs::remove_file(&host_script_file);
        is_running.store(false, Ordering::Relaxed);
        return;
    }

    // ── Step 4：在客户机内运行脚本（逐字键盘模拟）─────────────────────
    let result = run_vmrun(&[
        "runProgramInGuest", &vmx_path,
        "-interactive",
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        "-STA", "-WindowStyle", "Hidden",
        "-ExecutionPolicy", "Bypass",
        "-File", guest_script_path,
    ]);
    if let Err(e) = result {
        if let Ok(mut s) = error_msg.lock() { *s = e; }
    }

    // ── 清理宿主机临时文件 ──────────────────────────────────────────────
    let _ = std::fs::remove_file(&host_text_file);
    let _ = std::fs::remove_file(&host_script_file);

    is_running.store(false, Ordering::Relaxed);
}
