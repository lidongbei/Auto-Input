// 在 release 模式下隐藏 Windows 控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod hotkey;
mod input;

fn setup_fonts(ctx: &eframe::egui::Context) {
    // 运行时读取 Windows 系统字体，不内嵌字体文件，大幅缩小二进制体积。
    // 按优先级依次尝试：微软雅黑 → 黑体 → 宋体。均不存在时保持 egui 默认字体。
    let candidates: &[(&str, u32)] = &[
        (r"C:\Windows\Fonts\msyh.ttc",   0), // 微软雅黑 Regular（Win7+）
        (r"C:\Windows\Fonts\msyhbd.ttc", 0), // 微软雅黑 Bold
        (r"C:\Windows\Fonts\simhei.ttf", 0), // 黑体
        (r"C:\Windows\Fonts\simsun.ttc", 0), // 宋体
    ];

    let mut fonts = eframe::egui::FontDefinitions::default();

    for (path, index) in candidates {
        if let Ok(data) = std::fs::read(path) {
            let mut font_data = eframe::egui::FontData::from_owned(data);
            font_data.index = *index;
            fonts.font_data.insert("chinese".to_owned(), font_data);
            // 将系统中文字体追加为回退字体（英文字符仍优先用默认字体）
            fonts
                .families
                .entry(eframe::egui::FontFamily::Proportional)
                .or_default()
                .push("chinese".to_owned());
            fonts
                .families
                .entry(eframe::egui::FontFamily::Monospace)
                .or_default()
                .push("chinese".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let (icon_rgba, icon_w, icon_h) = app::make_icon_rgba();
    let icon = eframe::egui::IconData { rgba: icon_rgba, width: icon_w, height: icon_h };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([560.0, 560.0])
            .with_min_inner_size([440.0, 420.0])
            .with_resizable(true)
            .with_title("Auto Input — 自动输入")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Auto Input",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::AutoInputApp::new()))
        }),
    )
}
