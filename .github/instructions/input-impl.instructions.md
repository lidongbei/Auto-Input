---
description: "修改 src/input.rs 时使用：输入模式实现规范、VMware SendInput 脚本模式、特殊字符处理约束。"
applyTo: "src/input.rs"
---

# 输入实现规范

## 核心约束

**禁止在任何输入模式中改用剪贴板粘贴**。`MODE_PASTE` 是用户主动选择的模式，不是其他模式的回退方案。

## MODE_CHAR — 本机逐字模拟

- 使用 `enigo::text(&ch.to_string())` 发送普通字符
- `\n`/`\r` → `Key::Return`，`\t` → `Key::Tab`
- `char_delay_ms == 0` 时跳过 sleep

## MODE_VMRUN — VMware 客户机键盘模拟

脚本必须使用 **Win32 `SendInput` + `KEYEVENTF_UNICODE`**，不得改用 `SendKeys` 或剪贴板：

```csharp
// 必须保持这个 struct 布局（与 Win32 ABI 对齐）
struct INPUT {
    uint type;       // INPUT_KEYBOARD = 1
    KEYBDINPUT ki;
    uint pad1; uint pad2;  // 不可省略，保证 sizeof 正确
}
```

- 换行符 `\n`（或 `\r\n` 中的 `\r` 跳过）→ 发送虚拟键 `0x0D`（VK_RETURN），而不是 Unicode 值
- `char_delay_ms` 最小值 10ms（`char_delay_ms.max(10)`）
- 脚本文件临时路径：客户机 `C:\Users\Public\auto_input_type.ps1`，文本文件 `C:\Users\Public\auto_input_text.txt`
- 脚本末尾自删除两个临时文件

## Rust format! 模板注意

PowerShell 脚本用 `format!()` 生成：
- `{variable}` → Rust 插值
- `{{` / `}}` → PowerShell 或 C# 中的字面 `{` / `}`
- PowerShell here-string `@"..."@` 内的 `$` 不需要额外转义（Rust raw string）

## 错误处理

- `run_vmrun` 闭包失败时写入 `error_msg`，立即 `return`
- 所有 `return` 路径必须先 `is_running.store(false, ...)`
- 临时文件在 return 前清理（`let _ = std::fs::remove_file(...)`）
