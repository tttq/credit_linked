//! 平台元数据与图标 — 对齐原版 app.js 的 PLAT 常量与 picon 逻辑。
//! 图标经 rust-embed 嵌入 assets/platforms/，缺图标用首字母彩色圆标兜底。
//! AssetSource 与 gpui_kit 内置图标合并（缺内置资源会导致所有 IconName 渲染空白）。

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/platforms/"]
pub struct PlatformAssets;

/// 平台清单与登录方式（login: browser | device | none），与 PLAT 完全一致
pub struct PlatMeta {
    pub provider: &'static str,
    pub color: &'static str, // #RRGGBB
    pub login: &'static str,
    pub icon_file: Option<&'static str>,
    pub multi: bool,
}

pub const PLATS: &[PlatMeta] = &[
    PlatMeta { provider: "trae",              color: "#20b978", login: "browser", icon_file: Some("trae.png"),              multi: true },
    PlatMeta { provider: "workbuddy",         color: "#00ba9c", login: "browser", icon_file: Some("workbuddy.jpg"),         multi: true },
    PlatMeta { provider: "minimax",           color: "#20b9ec", login: "device",  icon_file: Some("minimax.png"),           multi: true },
    PlatMeta { provider: "qoder",             color: "#606570", login: "browser", icon_file: Some("qoder.png"),             multi: true },
    PlatMeta { provider: "codearts",          color: "#e93442", login: "browser", icon_file: Some("codearts.svg"),          multi: true },
    PlatMeta { provider: "codearts-desktop",  color: "#854cf5", login: "browser", icon_file: Some("codearts-desktop.png"),  multi: true },
    PlatMeta { provider: "zcode",             color: "#2e7a6f", login: "none",    icon_file: None,                          multi: false },
    PlatMeta { provider: "doubao",            color: "#c78f79", login: "none",    icon_file: None,                          multi: false },
];

/// 查平台元数据（未收录返回兜底灰色）
pub fn meta(provider: &str) -> &'static PlatMeta {
    PLATS.iter().find(|m| m.provider == provider)
        .unwrap_or(&FALLBACK)
}

static FALLBACK: PlatMeta = PlatMeta { provider: "", color: "#456", login: "none", icon_file: None, multi: false };

/// 平台登录方式
pub fn login_mode(provider: &str) -> &'static str {
    meta(provider).login
}

/// 支持扫描本机导入的平台（对齐 renderAccounts 的 data-import 清单）
pub fn can_import(provider: &str) -> bool {
    matches!(provider, "trae" | "workbuddy" | "qoder" | "zcode" | "minimax" | "codearts" | "codearts-desktop")
}

/// 平台图标二进制（嵌入资源），用于 gpui 渲染
pub fn icon_bytes(provider: &str) -> Option<&'static [u8]> {
    let m = meta(provider);
    m.icon_file.and_then(|f| PlatformAssets::get(f).map(|d| Box::leak(d.data.to_vec().into_boxed_slice()) as &'static [u8]))
}

/// 平台图标 → gpui Image（按文件扩展名推断格式）。
/// 仅返回 gpui 能解码的位图格式（PNG/JPEG/GIF/WEBP/BMP/ICO）；SVG 走首字母兜底。
pub fn icon_image(provider: &str) -> Option<gpui_kit::Image> {
    let m = meta(provider);
    let file = m.icon_file?;
    let bytes = PlatformAssets::get(file)?.data.to_vec();
    let ext = file.rsplit('.').next().unwrap_or("").to_lowercase();
    let format = match ext.as_str() {
        "png" => gpui_kit::ImageFormat::Png,
        "jpg" | "jpeg" => gpui_kit::ImageFormat::Jpeg,
        "gif" => gpui_kit::ImageFormat::Gif,
        "webp" => gpui_kit::ImageFormat::Webp,
        "bmp" => gpui_kit::ImageFormat::Bmp,
        "ico" => gpui_kit::ImageFormat::Ico,
        // SVG 由 gpui 内置图标管线处理；此处不构造位图
        _ => return None,
    };
    Some(gpui_kit::Image::from_bytes(format, bytes))
}

/// 网关平台通道清单（对齐 GW_PLATFORMS；chat 标注是否有对话通道）
pub const GW_PLATFORMS: &[(&str, &str, bool)] = &[
    ("workbuddy",        "gw_workbuddy",         true),
    ("workbuddy-intl",   "gw_workbuddy_intl",    true),
    ("trae",             "gw_trae",              true),
    ("trae-enterprise",  "gw_trae_enterprise",   true),
    ("trae-global",      "gw_trae_global",       true),
    ("zcode",            "gw_zcode",             true),
    ("minimax",          "gw_minimax",           true),
    ("qoder",            "gw_qoder",             true),
    ("codearts",         "gw_codearts",          false),
    ("codearts-desktop", "gw_codearts_desktop",  false),
];

pub fn gw_chat(channel: &str) -> bool {
    GW_PLATFORMS.iter().find(|(n, _, _)| *n == channel).map(|(_, _, c)| *c).unwrap_or(false)
}

/// 通道 → 账号 provider（与原版前端映射一致）
pub fn gw_provider(channel: &str) -> String {
    match channel {
        "workbuddy-intl" => "workbuddy-intl".into(),
        "trae" | "trae-enterprise" | "trae-global" => "trae".into(),
        _ => channel.to_string(),
    }
}

// ===== gpui AssetSource =====
// 先查本 crate 嵌入的平台图标，再回落 gpui_kit 内置图标资源。

/// 供 application().with_assets(...) 使用的合并资源源
pub struct AppAssets;

impl gpui_kit::AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(file) = PlatformAssets::get(path) {
            return Ok(Some(file.data));
        }
        match gpui_kit::assets::Assets.load(path) {
            Ok(v) => Ok(v),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut out: Vec<gpui_kit::SharedString> = PlatformAssets::iter()
            .filter(|name| name.starts_with(path))
            .map(|name| name.into())
            .collect();
        if let Ok(kit_list) = gpui_kit::assets::Assets.list(path) {
            out.extend(kit_list);
        }
        Ok(out)
    }
}

impl PlatMeta {
    /// "#RRGGBB" → u32
    pub fn color_hex_u32(&self) -> u32 {
        u32::from_str_radix(self.color.trim_start_matches('#'), 16).unwrap_or(0x6862ed)
    }
}
