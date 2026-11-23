//! theme.rs — 把 gpui_kit 0.10 主题 token 覆盖成设计稿配色（亮/暗双主题）。
//!
//! 色值集中在 light / dark 两个模块里；页面代码统一通过 `current_palette()`
//! 与 `is_dark()` 取值，切换主题时全站自动跟随。

use gpui_kit::component::Theme;
use gpui_kit::component::ThemeMode;
use gpui_kit::{px, rgb};

/// 亮色配色
pub mod light {
    pub const ACCENT: u32 = 0x6862ed;
    pub const ACCENT_SOFT: u32 = 0xeeebff;
    pub const LINE: u32 = 0xeae9f1;
    pub const SUBTLE: u32 = 0x7f7d91;
    pub const TEXT: u32 = 0x25252f;
    pub const GREEN: u32 = 0x20966c;
    pub const AMBER: u32 = 0xb97826;
    pub const RED: u32 = 0xd5455c;
    pub const BG: u32 = 0xf7f7fb;
    pub const SHELL_BG: u32 = 0xfafafe;
    #[allow(dead_code)]
    pub const SHELL_BORDER: u32 = 0xe4e2f0;
    pub const TOPBAR_A: u32 = 0xeeebff;
    pub const TOPBAR_B: u32 = 0xf4f1ff;
    pub const PROGRESS_TRACK: u32 = 0xe2def0;
}

/// 暗色配色
pub mod dark {
    pub const ACCENT: u32 = 0x7b76f0;
    pub const ACCENT_SOFT: u32 = 0x7b76f0;
    pub const LINE: u32 = 0x3a3949;
    pub const SUBTLE: u32 = 0x9d9bb0;
    pub const TEXT: u32 = 0xe8e7f2;
    pub const GREEN: u32 = 0x2db684;
    pub const AMBER: u32 = 0xd69a44;
    pub const RED: u32 = 0xe9607a;
    pub const BG: u32 = 0x1b1b26;
    pub const SHELL_BG: u32 = 0x181824;
    #[allow(dead_code)]
    pub const SHELL_BORDER: u32 = 0x33324a;
    pub const TOPBAR_A: u32 = 0x2c2a4a;
    pub const TOPBAR_B: u32 = 0x232238;
    pub const PROGRESS_TRACK: u32 = 0x34334a;
}

/// 当前是否为暗色（全局）
static DARK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_dark() -> bool {
    DARK_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 读取 Windows「应用使用浅色主题」开关；true = 系统当前是**深色**。
///
/// 用 winreg 直接读 DWORD，避免解析 `reg query` 的十六进制输出
/// （`0x0` 用 `parse::<u32>()` 必然失败，会让"跟随系统"永远退化成浅色）。
#[cfg(windows)]
pub fn system_prefers_dark() -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_READ) else {
        return false;
    };
    match key.get_value::<u32, _>("AppsUseLightTheme") {
        Ok(v) => v == 0, // 0 = 深色，1 = 浅色
        Err(_) => false,
    }
}

#[cfg(not(windows))]
pub fn system_prefers_dark() -> bool {
    false
}

/// 按用户设置（system / light / dark）解析出实际是否为深色
pub fn resolve_dark(setting: &str) -> bool {
    match setting {
        "dark" => true,
        "light" => false,
        _ => system_prefers_dark(),
    }
}

/// 当前色板：
/// (accent, accent_soft, line, subtle, text, green, amber, red, bg, shell_bg, topbar_a, topbar_b, progress_track)
pub fn current_palette() -> (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) {
    palette(is_dark())
}

/// 指定模式的色板（同上元组顺序）
pub fn palette(dark_mode: bool) -> (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) {
    if dark_mode {
        (
            dark::ACCENT, dark::ACCENT_SOFT, dark::LINE, dark::SUBTLE, dark::TEXT,
            dark::GREEN, dark::AMBER, dark::RED, dark::BG, dark::SHELL_BG,
            dark::TOPBAR_A, dark::TOPBAR_B, dark::PROGRESS_TRACK,
        )
    } else {
        (
            light::ACCENT, light::ACCENT_SOFT, light::LINE, light::SUBTLE, light::TEXT,
            light::GREEN, light::AMBER, light::RED, light::BG, light::SHELL_BG,
            light::TOPBAR_A, light::TOPBAR_B, light::PROGRESS_TRACK,
        )
    }
}

/// 应用到 gpui_kit 全局主题：覆盖主色/背景/边框/成功警告危险色/次要文字，
/// 使内置组件（按钮、开关、图表坐标轴等）与设计稿一致。
pub fn apply(cx: &mut gpui_kit::App) {
    Theme::update(cx, |theme| {
        let dark = matches!(theme.mode, ThemeMode::Dark);
        DARK_MODE.store(dark, std::sync::atomic::Ordering::Relaxed);
        let (accent, accent_soft, line, subtle, text, green, amber, red, bg, _shell, _, _, _track) =
            palette(dark);
        theme.colors.primary = rgb(accent).into();
        theme.colors.button_primary = rgb(accent).into();
        theme.colors.button_primary_hover = rgb(if dark { 0x8d88f5 } else { 0x7a75f5 }).into();
        theme.colors.button_primary_active = rgb(if dark { 0x6a65d8 } else { 0x5650d8 }).into();
        theme.colors.accent = rgb(accent).into();
        theme.colors.accent_foreground = rgb(0xffffff).into();
        theme.colors.background = rgb(bg).into();
        theme.colors.border = rgb(line).into();
        theme.colors.foreground = rgb(text).into();
        theme.colors.success = rgb(green).into();
        theme.colors.warning = rgb(amber).into();
        theme.colors.danger = rgb(red).into();
        // 图表刻度/次要文字：不显式设置的话，暗色下坐标轴数字会与背景同色而"看不见"
        theme.colors.muted_foreground = rgb(subtle).into();
        theme.colors.muted = rgb(accent_soft).into();
        theme.colors.secondary = rgb(line).into();
        theme.colors.progress_bar = rgb(accent).into();

        // 字号与圆角：对齐设计稿（正文 14px、按钮 9px）
        theme.font_size = px(14.);
        theme.radius = px(9.);
    });
}

/// 顶栏渐变色（亮/暗各一对）
#[allow(dead_code)]
pub fn topbar_gradient(dark: bool) -> (u32, u32) {
    if dark {
        (dark::TOPBAR_A, dark::TOPBAR_B)
    } else {
        (light::TOPBAR_A, light::TOPBAR_B)
    }
}
