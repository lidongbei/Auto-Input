fn main() {
    // 仅在 Windows 目标上嵌入资源（exe 文件图标、版本信息）
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        // 如果 assets/icon.ico 存在则嵌入；否则跳过（不影响编译）
        if std::path::Path::new("assets/icon.ico").exists() {
            res.set_icon("assets/icon.ico");
        }
        // 忽略 winres 错误，避免没有 rc.exe / llvm-rc 时构建中断
        let _ = res.compile();
    }
}
