---
description: "为 auto-input 添加新的输入模式。包含完整的实现清单：input.rs 新增逻辑、app.rs UI 和配置更新。"
---

# 添加新输入模式

## 参数

- **模式名称**：$modeName
- **模式描述**：$modeDescription

## 实现清单

### 1. `src/input.rs`

- [ ] 在文件顶部添加 `pub const MODE_XXX: u8 = N;`（N 取当前最大值 +1）
- [ ] 实现 `pub fn run_xxx_input(...)` 函数，参数规范参考现有函数
- [ ] 函数所有 `return` 路径必须先 `is_running.store(false, Ordering::Relaxed)`
- [ ] **不得使用剪贴板粘贴**作为输入实现（除非模式本身就是 MODE_PASTE）

### 2. `src/app.rs`

**UI：**
- [ ] 在输入方式 radio group 中添加新选项
- [ ] 如需额外配置项，在 `if self.input_mode == MODE_XXX` 块内添加设置面板
- [ ] 更新"开始输入"按钮的 `can_start` 条件（如有必填字段）

**配置：**
- [ ] `AppConfig` 添加新模式专属字段（如有）
- [ ] `load_config()` 默认值
- [ ] `save_config()` 赋值

**分发：**
- [ ] `start_input()` 中添加 `else if input_mode == MODE_XXX { ... }` 分支

### 3. 验证

```shell
cargo build
cargo run
```

确认：新模式在 UI 选择后显示正确配置面板，开始/停止功能正常。
