//! AI Free Router — gpui_kit 原生 UI。
//! 模块划分：
//! - backend   : 全部业务命令
//! - platform  : 平台元数据与嵌入图标
//! - i18n      : 双语文案（平移自 ui/i18n.js）
//! - app       : 根视图 + 导航 + 六个页面（首页/账号/签到/统计/路由/设置）

// 纯 GUI 程序：不要附带控制台窗口（否则启动时会闪一个黑框）。
// 诊断信息统一写 credit-ui-panic.log，不依赖 stdout/stderr。
#![windows_subsystem = "windows"]

mod app;
mod backend;
mod i18n;
mod platform;
mod theme;

fn main() {
    // panic 日志（崩溃排查，对齐原版）：带 backtrace，写文件 + stderr
    std::panic::set_hook(Box::new(|info| {
        let bt = format!("{:?}", backtrace::Backtrace::new());
        let log = format!("[PANIC] {info}\ncontext: credit-ui gpui_kit\nbacktrace:\n{bt}");
        let _ = std::fs::write("credit-ui-panic.log", &log);
        eprintln!("{log}");
        let _ = std::fs::write("C:/temp/credit-ui-panic.log", &log);
    }));

    // gpui 的 render/layout 是递归的；路由页等复杂页面在默认 1MB 栈上会溢出。
    // 用 64MB 栈的专用线程运行整个 UI 消息循环（Windows 允许非主线程拥有窗口）。
    let ui = std::thread::Builder::new()
        .name("credit-ui-main".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(run_app)
        .expect("failed to spawn UI thread");
    let _ = ui.join();
}

fn run_app() {
    gpui_kit::application()
        .with_assets(platform::AppAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            // 覆盖设计稿配色（#6862ed 主色、浅色背景等）
            theme::apply(cx);
            // 恢复用户保存的主题模式（system/light/dark）
            {
                use gpui_kit::component::ThemeMode;
                // 保存的设置：system / light / dark；system 跟随 Windows 个性化开关
                let saved = credit_core::db::Store::open_default()
                    .ok()
                    .and_then(|st| st.get_setting("theme", "system").ok())
                    .unwrap_or_else(|| "system".into());
                let mode = if theme::resolve_dark(&saved) { ThemeMode::Dark } else { ThemeMode::Light };
                gpui_kit::component::Theme::update(cx, |t| { t.mode = mode; });
                theme::apply(cx);
            }

            // 后台服务：模型网关 + 自动签到调度（独立 tokio runtime 线程）
            backend::spawn_gateway();
            backend::spawn_auto_tick();

            // 窗口尺寸：560×680，最小 360×420，居中
            let opts = gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
                    None,
                    gpui_kit::size(gpui_kit::px(560.), gpui_kit::px(680.)),
                    cx,
                ))),
                titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
                app_owns_titlebar_drag: true,
                window_min_size: Some(gpui_kit::size(gpui_kit::px(360.), gpui_kit::px(420.))),
                ..Default::default()
            };
            gpui_kit::open_window(opts, cx, |window, cx| gpui_kit::AppContext::new(cx, |cx| app::AppView::new(window, cx)))
                .expect("failed to open main window");
        });
}
