---
description: "调试 VMware 输入失败问题。提供诊断步骤、常见原因检查表和 PowerShell 脚本验证方法。"
---

# 调试 VMware 输入失败

## 诊断流程

### 1. 确认 vmrun 可达

```powershell
# 在宿主机运行
& "vmrun路径" -T ws listRunningVMs
```

应输出正在运行的 VMX 路径列表。若失败：检查路径、VMware Workstation 版本。

### 2. 确认客户机凭据

```powershell
& "vmrun路径" -T ws -gu 用户名 -gp 密码 runProgramInGuest 'vmx路径' -interactive cmd.exe /c whoami
```

若输出用户名则凭据正确。常见问题：密码含特殊字符被 shell 转义。

### 3. 手动测试脚本

将以下脚本复制到客户机内执行，验证 `SendInput` 是否正常工作：

```powershell
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class UnicodeKeyboard {
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] p, int sz);
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public KEYBDINPUT ki; uint p1; uint p2; }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr extra; }
    public static void Type(char c) {
        var inp = new INPUT[2];
        inp[0].type = 1; inp[0].ki.wScan = (ushort)c; inp[0].ki.dwFlags = 4;
        inp[1].type = 1; inp[1].ki.wScan = (ushort)c; inp[1].ki.dwFlags = 6;
        SendInput(2, inp, System.Runtime.InteropServices.Marshal.SizeOf(typeof(INPUT)));
    }
}
"@
Start-Sleep -Seconds 3  # 切到目标窗口
foreach ($ch in "Hello !@#123".ToCharArray()) {
    [UnicodeKeyboard]::Type($ch); Start-Sleep -Milliseconds 50
}
```

### 4. 常见原因检查表

| 症状 | 可能原因 | 解法 |
|------|----------|------|
| `!@#` 变成 `123` | 用了 `SendKeys` 而非 `SendInput+KEYEVENTF_UNICODE` | 确保脚本使用 `UnicodeKeyboard` 类 |
| vmrun 报认证错误 | 密码包含 `"` 或空格 | 检查程序传参方式 |
| 脚本执行但无输出 | 目标窗口焦点丢失 | 增大 `start_delay_secs` |
| 中文乱码 | 文本文件编码问题 | 确认写入 UTF-8 BOM（`\u{FEFF}` 前缀） |
| struct 大小错误 | `INPUT` 缺少 padding 字段 | 保留 `uint pad1; uint pad2;` |
