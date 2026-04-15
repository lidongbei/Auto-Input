---
description: "修改 src/app.rs 时使用：UI 组件规范、配置持久化字段、状态机约束、eframe 生命周期注意事项。"
applyTo: "src/app.rs"
---

# UI & App 规范

## 配置持久化

`AppConfig` 保存路径：`%APPDATA%\auto-input\config.json`

**必须保存的字段**（跨次有意义的设置）：
`vmrun_path`、`vmx_path`、`guest_user`、`guest_pass`、`input_mode`、`char_delay_ms`、`start_delay_secs`、`always_on_top`

**不保存**：`custom_text`、`use_clipboard`

保存时机：`start_input()` 开始前、close 事件（最小化到托盘）时。

新增字段时：同步更新 `AppConfig` 结构体、`load_config()` 默认值、`save_config()` 赋值。

## 状态机

```
就绪 → [开始] → 倒计时 → 输入中 → 输入完成 ✓
                              ↓ [停止]
                           已停止
```

- `is_running` + `stop_flag` 均为 `Arc<AtomicBool>`，线程安全
- `prev_running` 用于检测"刚完成"转换，在 `update()` 末尾更新
- `start_time` 仅在运行期间有值，用于倒计时显示

## VMware 设置面板

仅当 `input_mode == MODE_VMRUN` 时渲染 VMware 设置 section。
"开始输入"按钮在 VM 模式下的额外 disable 条件：`vmrun_path` 或 `vmx_path` 或 `guest_user` 为空。

## 托盘行为

- 关闭窗口 → 最小化到托盘（拦截 `close_requested`，`CancelClose`）
- 双击托盘图标 → 通过 Win32 `FindWindowW` + `ShowWindow` 直接还原（不依赖 egui 事件循环）
- 右键退出 → `std::process::exit(0)`

## eframe 注意事项

- `first_frame` 标志用于首帧应用置顶状态（`WindowLevel` 命令必须在帧内发送）
- `ctx.request_repaint_after(150ms)` 保持托盘事件和倒计时刷新
- `ctx_holder` 供托盘 handler 跨线程调用 `request_repaint`
