# Auto Input — 自动输入工具

> 在禁止粘贴的场景下，通过**模拟键盘逐字输入**将文本发送到目标窗口。
>
> 典型用途：银行/金融系统登录框、虚拟机内的受限输入框、远程桌面密码框等。

---

## 功能特性

- **三种输入模式**：逐字键盘模拟 / 剪贴板粘贴 / VMware 虚拟机内键盘模拟
- **全局热键**：后台运行时一键触发输入，支持自定义快捷键组合，可独立启用/禁用
- **输入来源**：自定义文本框 或 读取系统剪贴板
- **延迟控制**：可设置开始前延迟（用于切换焦点）和字符间延迟（控制输入速度）
- **系统托盘**：关闭窗口后最小化到托盘，双击还原；右键菜单含置顶开关和退出
- **配置持久化**：所有设置自动保存到 `%APPDATA%\auto-input\config.toml`，重启后恢复
- **窗口置顶**：可通过 UI 复选框或托盘右键菜单切换

---

## 输入模式

| 模式 | 适用场景 | 原理 |
|------|----------|------|
| **逐字模拟按键** | 普通 Windows 程序 | `enigo` 逐字发送键盘事件 |
| **粘贴 Ctrl+V** | 允许粘贴的场景 | 写入剪贴板后发送 `Ctrl+V` |
| **VMware 虚拟机** | VM 内禁止粘贴的场景 | `vmrun` 传文件 + 客户机 PowerShell 内联 C# `SendInput + KEYEVENTF_UNICODE` |

> **VMware 模式**直接以 Unicode 码点发送每个字符，不依赖键盘布局，`!@#$%^&*()` 等符号均可正确输入。

---

## 全局热键

在"全局热键"面板中配置：

| 热键 | 默认（未启用） | 行为 |
|------|--------------|------|
| 输入剪切板 | Ctrl+Alt+F1 | 触发后输入系统剪贴板内容 |
| 输入自定义 | Ctrl+Alt+F2 | 触发后输入文本框中的自定义内容 |

支持 Ctrl / Alt / Shift / Win 任意组合 + F1~F12 / A~Z / 0~9。

> ⚠ **焦点在虚拟机窗口内时，宿主机热键无法被接收**，请先点击虚拟机外部再触发。

---

## 配置文件

路径：`%APPDATA%\auto-input\config.toml`

```toml
input_mode = 0          # 0=逐字 1=粘贴 2=VMware
char_delay_ms = 50      # 字符间延迟（毫秒）
start_delay_secs = 3    # 开始前延迟（秒）
always_on_top = false

vmrun_path = ""         # vmrun.exe 路径（自动探测）
vmx_path = ""           # 虚拟机 .vmx 文件路径
guest_user = ""
guest_pass = ""

[hotkey_clipboard]
enabled = false
modifiers = 6           # Ctrl+Alt = 0x02 | 0x01
vk = 112                # F1

[hotkey_custom]
enabled = false
modifiers = 6
vk = 113                # F2
```

---

## 构建

**环境要求**：Windows，Rust 1.80+（edition 2024）

```powershell
git clone <repo-url>
cd auto-input
cargo build --release
```

可执行文件输出到 `target/release/auto-input.exe`，单文件，无需额外依赖。

**自定义 exe 图标**：将 256×256 的 `.ico` 图标放到 `assets/icon.ico`，重新编译即可嵌入。

> 如遇 SSL 证书撤销检查错误，在项目根目录添加 `.cargo/config.toml`：
> ```toml
> [http]
> check-revoke = false
> ```

---

## 技术栈

| 组件 | 库 |
|------|----|
| UI 框架 | [eframe](https://github.com/emilk/egui) / egui 0.29 |
| 系统托盘 | tray-icon 0.19 |
| 键盘模拟（本机） | enigo 0.2 |
| 剪贴板读取 | arboard 3 |
| Win32 API | winapi 0.3 |
| 配置持久化 | serde + toml 0.8 |
| 全局热键 | Win32 `RegisterHotKey` (winapi) |

---

## 注意事项

- 仅支持 **Windows**（Vista+）
- VMware 模式需要安装 **VMware Workstation** 并已安装 **VMware Tools**
- 全局热键在虚拟机窗口获得焦点时失效（系统限制）
- 配置中不保存 `custom_text` 和 `use_clipboard`（每次使用可能不同）
