# auto-input — 项目说明

## 核心需求（必读）

这是一个 Windows 桌面工具，核心目标是**在禁止粘贴的场景下模拟键盘逐字输入**。
典型用途：银行/金融系统登录框、虚拟机内的受限输入框、远程桌面密码框等。

> **关键约束**：任何涉及输入的修改，绝对不能改为剪贴板粘贴方案。粘贴在目标场景中是被禁止的。

## 技术栈

| 层 | 技术 |
|----|------|
| UI | eframe / egui 0.29 |
| 系统托盘 | tray-icon 0.19 |
| 本机键盘模拟 | enigo 0.2 |
| 剪贴板读取 | arboard 3（仅用于读取，不用于输入） |
| Win32 API | winapi 0.3 |
| 配置持久化 | serde + serde_json |
| 运行时 | Windows only，edition 2024 |

## 输入模式

| 模式 | 常量 | 实现方式 | 适用场景 |
|------|------|----------|----------|
| `MODE_CHAR` (0) | `enigo::text()` / key click | 本机窗口逐字键盘模拟 | 普通 Windows 程序 |
| `MODE_PASTE` (1) | 写剪贴板 + Ctrl+V | 剪贴板粘贴 | 允许粘贴的场景 |
| `MODE_VMRUN` (2) | vmrun + PowerShell SendInput | VMware 客户机内键盘模拟 | VMware 虚拟机内禁止粘贴的场景 |
| `MODE_UNICODE` (3) | Win32 `SendInput + KEYEVENTF_UNICODE` | 本机逐字 Unicode 按键 | 本地禁止粘贴的窗口 |
| `MODE_WM_CHAR` (4) | `PostMessage WM_KEYDOWN+WM_CHAR+WM_KEYUP` | 消息队列注入，绕过 INJECTED 钩子 | 飞书/向日葵等远程控制；⚠ **不支持中文** |

## MODE_WM_CHAR 关键实现

- ASCII 字母/数字/符号：`char_to_vk_shift()` 映射真实 VK 码，`PostMessage` 发 `WM_KEYDOWN + WM_CHAR + WM_KEYUP`，大写及 `!@#` 等附加 `WM_KEYDOWN(VK_SHIFT)`
- 回车/Tab：`WM_KEYDOWN(VK_RETURN/VK_TAB)` + `WM_KEYUP`
- 非 ASCII（中文等）：尝试 `WM_IME_CHAR`，但飞书远控协议**不转发此消息到远端**，中文无法输入

**已知限制**：`MODE_WM_CHAR` 不支持中文。需要输入中文时应改用 `MODE_PASTE`（需开启飞书剪贴板共享）。

## VMware 模式关键实现

1. 宿主机写临时文本文件（UTF-8 BOM）
2. `vmrun copyFileFromHostToGuest` 传入客户机
3. 客户机运行 PowerShell 脚本，使用**内联 C# + Win32 `SendInput` + `KEYEVENTF_UNICODE`** 逐字发送每个 Unicode 字符
4. `KEYEVENTF_UNICODE` 直接发送 Unicode 码点，**不依赖键盘布局，不需要 Shift**，正确处理 `!@#$%^&*()` 等所有符号

**为什么不用 `SendKeys`**：`SendKeys` 依赖键盘布局映射，在 VM 环境下 Shift 修饰键不可靠，`!@#` 会变成 `123`。
**为什么不用剪贴板粘贴**：目标场景禁止粘贴。

## 代码结构

```
src/
  main.rs     — 入口，eframe::run_native
  app.rs      — AutoInputApp：UI 渲染、配置持久化、托盘管理
  input.rs    — 所有输入逻辑：run_input / run_vmrun_input / detect_vmrun
```

## 配置持久化

保存路径：`%APPDATA%\auto-input\config.toml`

保存时机：点击"开始输入"时、最小化到托盘时。

保存字段：`vmrun_path`、`vmx_path`、`guest_user`、`guest_pass`、`input_mode`、`char_delay_ms`、`start_delay_secs`、`always_on_top`。

不保存：`custom_text`、`use_clipboard`（每次使用可能不同）。

## 构建 & 运行

```shell
cargo build          # debug
cargo build --release
cargo run
```

## 修改注意事项

- 修改 `run_vmrun_input` 时，脚本模板使用 Rust `format!` 生成，`{{}}` 是字面花括号，`{}` 是插值。
- PowerShell 内联 C# 的 `struct` 布局需与 Win32 ABI 对齐，不要随意增减字段。
- `char_delay_ms` 在 VMware 模式下也有效，是客户机脚本的每字符等待时间（最小 10ms）。
