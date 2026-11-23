//! app — 根视图 + 导航 + 六个页面（首页 / 账号管理 / 签到中心 / 积分统计 / 账号路由 / 设置）。
//!
//! 基于 gpui_kit 0.10 重建，视觉对齐原 Tauri 版（index.html / app.js）：
//! - 主色 #6862ed，窗口 560×680，圆角卡片，亮/暗双主题
//! - 所有 backend 异步调用经 `backend::bg()` 派发到全局 tokio runtime
//!   （gpui 执行器无 tokio reactor，直调会 panic "no reactor running"）

use crate::backend;
use crate::i18n::{t, tf};
use crate::platform;
use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    chart::{BarChart, LineChart},
    input::{Input, InputState},
    switch::Switch,
    TitleBar,
    h_flex, v_flex,
    Disableable as _, Selectable as _,
    Sizable as _,
};
use gpui_kit::*;
use gpui_kit::component::WindowExt as _;
use std::sync::Arc;

/// 关闭确认弹框是否已弹出（防止重复弹窗）
static CLOSE_PROMPT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub mod overview;
pub mod accounts;
pub mod checkin;
pub mod statistics;
pub mod routing;
pub mod settings;
pub mod actions;

// 主题色：随当前亮/暗模式动态取值（只读一个原子量 + 常量元组，开销极小）
pub fn accent() -> u32 { crate::theme::current_palette().0 }
pub fn accent_soft() -> u32 { crate::theme::current_palette().1 }
pub fn line_color() -> u32 { crate::theme::current_palette().2 }
pub fn subtle() -> u32 { crate::theme::current_palette().3 }
pub fn text_color() -> u32 { crate::theme::current_palette().4 }
pub fn green() -> u32 { crate::theme::current_palette().5 }
pub fn amber() -> u32 { crate::theme::current_palette().6 }
pub fn red() -> u32 { crate::theme::current_palette().7 }
pub fn token_blue() -> u32 { 0x20b9ec }

// ---- FluentBuilder 本地实现（gpui-pre 未在根导出 when/when_some） ----
pub trait MyFluent {
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self where Self: Sized {
        if condition { then(self) } else { self }
    }
    fn when_some<T>(self, opt: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self where Self: Sized {
        match opt { Some(v) => then(self, v), None => self }
    }
}
impl MyFluent for gpui_kit::Div {}
impl MyFluent for gpui_kit::Stateful<gpui_kit::Div> {}
impl MyFluent for gpui_kit::component::button::Button {}
impl MyFluent for gpui_kit::Img {}
impl MyFluent for gpui_kit::component::input::Input {}

// 设置页下拉选择事件通道：SimpleSelect 选项点击 → AppView 处理
// (tag, value)；tag 区分语言 / 外观
pub(crate) static SELECT_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<(&'static str, String)>>> = std::sync::Mutex::new(None);
// 删除账号确认事件通道
pub(crate) static DELETE_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<(String, String)>>> = std::sync::Mutex::new(None);
// 设备号保存事件通道
pub(crate) static DEVICE_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<(String, String, String)>>> = std::sync::Mutex::new(None);

/// UI 快照中的一个账号行（从 ui_snapshot JSON 解出）
#[derive(Clone, Debug, Default)]
pub struct AccountRow {
    pub provider: String,
    pub uid: String,
    pub name: String,
    pub status: String,
    pub message: String,
    pub auto: bool,
    pub remaining: Option<f64>,
    pub last_checkin: String,
    pub expires: String,
}

impl AccountRow {
    pub fn from_json(v: &serde_json::Value) -> Self {
        // 额度可能以两种形态出现：
        // 1) 快照里 quota 是数据库原始 JSON 字符串（{"remaining":..,"parts":..,"sampled_at":..}）
        // 2) 单账号刷新接口直接返回 Quota 对象（顶层 remaining）
        let quota_json = v
            .get("quota")
            .and_then(|q| q.as_str())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .or_else(|| v.get("quota").filter(|q| q.is_object()).cloned());
        let remaining = v
            .get("remaining")
            .and_then(|x| x.as_f64())
            .or_else(|| {
                quota_json
                    .as_ref()
                    .and_then(|q| q.get("remaining"))
                    .and_then(|x| x.as_f64())
            });
        Self {
            provider: v.get("provider").and_then(|x| x.as_str()).unwrap_or("").into(),
            uid: v.get("uid").and_then(|x| x.as_str()).unwrap_or("").into(),
            name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
            status: v.get("status").and_then(|x| x.as_str()).unwrap_or("").into(),
            message: v.get("message").and_then(|x| x.as_str()).unwrap_or("").into(),
            auto: v.get("auto").and_then(|x| x.as_bool()).unwrap_or(false),
            remaining,
            last_checkin: v.get("last_checkin").and_then(|x| x.as_str()).unwrap_or("").into(),
            expires: v.get("expires").and_then(|x| x.as_str()).unwrap_or("").into(),
        }
    }
    /// 今日已签到（success / already 视为完成）
    pub fn done(&self) -> bool { self.status == "success" || self.status == "already" }
}

/// 状态样式映射（对齐原版 stInfo）：kind 决定颜色，key 为 i18n 文案
pub fn st_kind(s: &str) -> (&'static str, &'static str) {
    match s {
        "success" | "already" => ("ok", "st_success"),
        "failed" | "auth-expired" => ("bad", "st_failed"),
        "pending" | "needs-action" => ("warn", "st_pending"),
        _ => ("idle", "st_idle"),
    }
}

pub fn st_color(kind: &str) -> Rgba {
    match kind {
        "ok" => rgb(green()),
        "bad" => rgb(red()),
        "warn" => rgb(amber()),
        _ => rgb(subtle()),
    }
}

/// 页面枚举（顺序与原版导航一致）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page { Overview, Accounts, Checkin, Statistics, Routing, Settings }

impl Page {
    pub fn label(&self) -> &'static str {
        match self {
            Page::Overview => "nav_home",
            Page::Accounts => "nav_accounts",
            Page::Checkin => "nav_checkin",
            Page::Statistics => "nav_stats",
            Page::Routing => "nav_route",
            Page::Settings => "nav_settings",
        }
    }
    pub fn all() -> [Page; 6] {
        [Page::Overview, Page::Accounts, Page::Checkin, Page::Statistics, Page::Routing, Page::Settings]
    }
}

/// 首页柱状图数据点
#[derive(Clone, Debug, Default)]
pub struct DayBar {
    pub day: String,
    pub income: f64,
    pub by_provider: Vec<(String, f64)>,
}

/// 根视图：持有全部页面状态
pub struct AppView {
    pub page: Page,
    pub snap: Option<serde_json::Value>,
    pub accounts: Vec<AccountRow>,
    pub settings: Option<serde_json::Value>,
    pub loading: bool,
    pub toast: Option<(String, std::time::Instant)>,
    // 首页
    pub overview_range: usize,
    pub chart_days: Option<Vec<DayBar>>,
    pub chart_summary_text: Option<String>,
    // 统计页
    pub stat_period: String,
    pub stat_account: String,
    pub stat_items: Vec<(String, f64, i64)>,
    pub stat_tokens: Vec<f64>,
    pub stat_note: String,
    // 签到中心
    pub checkin_filter: String,
    // 路由页
    pub gw_status: Option<serde_json::Value>,
    pub gw_accounts: Option<serde_json::Value>,
    pub gw_models: Vec<String>,
    pub gw_test_model: Option<String>,
    pub gw_test_status: String,
    pub gw_test_result: String,
    pub gw_test_input: Option<Entity<InputState>>,
    pub gw_port_input: Option<Entity<InputState>>,
    pub gw_rpm_input: Option<Entity<InputState>>,
    // 设置页
    pub settings_tab: String,
    pub tools: Option<serde_json::Value>,
    pub update_info: Option<serde_json::Value>,
    pub update_status: String,
    pub update_file: Option<String>,
    pub update_progress: String,
    pub input_tools_dir: Option<Entity<InputState>>,
    // 登录会话
    pub login_seq: Option<(String, String)>,
    // 设备号弹窗状态
    pub pending_dev: Option<(String, String)>,
    pub dev_devices: Vec<String>,

    // 事件接收器（SELECT_TX / DELETE_TX / DEVICE_TX 的对端）
    pub select_rx: Option<std::sync::mpsc::Receiver<(&'static str, String)>>,
    /// 首帧加载标记：启动直接落在某个页面时，也要拉取该页数据
    pub page_loaded_once: bool,
    /// 内容区滚动句柄（换页时回到顶部）
    pub scroll: gpui_kit::ScrollHandle,
    /// 自动刷新循环是否已启动
    pub auto_refresh_started: bool,
    /// 安装包下载进度轮询是否进行中
    pub tools_poll_running: bool,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 建立 select 事件通道（本视图为唯一消费者）
        let (tx, rx) = std::sync::mpsc::channel();
        if let Ok(mut g) = SELECT_TX.lock() { *g = Some(tx); }
        // 在构造期创建输入框（绝不在 render 内 cx.new，否则栈溢出）
        let gw_port_input = cx.new(|cx| InputState::new(window, cx).default_value("8756"));
        let gw_rpm_input = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        let gw_test_input = cx.new(|cx| InputState::new(window, cx).placeholder(t("test_msg_placeholder")));
        let input_tools_dir = cx.new(|cx| {
            let default_dir = std::env::temp_dir().join("credit-rs-downloads");
            let _ = std::fs::create_dir_all(&default_dir);
            gpui_kit::component::input::InputState::new(window, cx)
                .default_value(default_dir.to_string_lossy().to_string())
        });
        Self {
            page: Page::Overview,
            snap: None,
            accounts: Vec::new(),
            settings: None,
            loading: true,
            toast: None,
            overview_range: 7,
            chart_days: None,
            chart_summary_text: None,
            stat_period: "day".into(),
            stat_account: "all".into(),
            stat_items: Vec::new(),
            stat_tokens: Vec::new(),
            stat_note: String::new(),
            checkin_filter: "all".into(),
            gw_status: None,
            gw_accounts: None,
            gw_models: Vec::new(),
            gw_test_model: None,
            gw_test_status: String::new(),
            gw_test_result: String::new(),
            gw_test_input: Some(gw_test_input),
            gw_port_input: Some(gw_port_input),
            gw_rpm_input: Some(gw_rpm_input),
            settings_tab: "tasks".into(),
            tools: None,
            update_info: None,
            update_status: String::new(),
            update_file: None,
            update_progress: String::new(),
            input_tools_dir: Some(input_tools_dir),
            login_seq: None,
            pending_dev: None,
            dev_devices: Vec::new(),
            select_rx: Some(rx),
            page_loaded_once: false,
            scroll: gpui_kit::ScrollHandle::new(),
            auto_refresh_started: false,
            tools_poll_running: false,

        }
    }

    /// toast 消息（3 秒自动消失）
    pub fn toast(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
        cx.notify();
    }

    /// 全量刷新：并行拉快照 / 设置 / 网关 / 首页图 / 统计（全部经 bg 派发）
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (snap, settings, gw, income, stat, tokens) = backend::bg(async {
                let snap = backend::snapshot().await;
                let settings = backend::settings_get().await;
                let gw = backend::gateway_status().await;
                let income = backend::daily_income(Some(7)).await;
                let stat = backend::statistics(None, None, Some("day".into())).await;
                let tokens = backend::daily_tokens(Some(14)).await;
                (snap, settings, gw, income, stat, tokens)
            }).await.unwrap_or_else(|_| (
                Err(String::new()), Ok(serde_json::Value::Null), Ok(serde_json::Value::Null),
                Ok(serde_json::Value::Null), Ok(serde_json::Value::Null), Ok(serde_json::Value::Null),
            ));
            this.update(cx, |v, cx| {
                v.snap = snap.ok();
                v.settings = settings.ok();
                v.gw_status = gw.ok();
                v.accounts = v.snap.as_ref().and_then(|s| s.get("accounts"))
                    .and_then(|a| a.as_array()).map(|arr| arr.iter().map(AccountRow::from_json).collect())
                    .unwrap_or_default();
                if let Some(st) = &v.settings {
                    if let Some(loc) = st.get("locale").and_then(|x| x.as_str()) {
                        if !loc.is_empty() { crate::i18n::set_locale(loc); }
                    }
                }
                // 首页柱状图数据
                if let Ok(d) = income {
                    v.fill_chart_from_income(&d);
                }
                // 统计折线数据
                if let Ok(s) = stat {
                    v.fill_stat_from_json(&s);
                }
                if let Ok(tk) = tokens {
                    v.stat_tokens = aggregate_tokens(&v.stat_items, &tk, "day");
                    v.stat_note = t("token_stats_all").to_string();
                }
                v.loading = false;
                cx.notify();
            }).ok();
        }).detach();
    }

    /// 从 daily_income JSON 填充首页柱状图
    pub(crate) fn fill_chart_from_income(&mut self, d: &serde_json::Value) {
        let days = d.get("days").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let bars: Vec<DayBar> = days.iter().map(|day| {
            let mut by_provider = Vec::new();
            if let Some(bp) = day.get("by_provider").and_then(|x| x.as_object()) {
                for (k, val) in bp { if let Some(f) = val.as_f64() { by_provider.push((k.clone(), f)); } }
            }
            DayBar {
                day: day.get("day").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                income: day.get("income").and_then(|x| x.as_f64()).unwrap_or(0.0),
                by_provider,
            }
        }).collect();
        let sum: f64 = bars.iter().map(|b| b.income).sum();
        self.chart_days = Some(bars);
        self.chart_summary_text = Some(format!("{}{}", t("period_points"), crate::i18n::fmt_num(sum)));
    }

    /// 从 statistics JSON 填充统计桶
    pub(crate) fn fill_stat_from_json(&mut self, s: &serde_json::Value) {
        self.stat_items = s.get("items").and_then(|x| x.as_array()).map(|arr|
            arr.iter().map(|it| (
                it.get("bucket").and_then(|b| b.as_str()).unwrap_or("").to_string(),
                it.get("sum").and_then(|b| b.as_f64()).unwrap_or(0.0),
                it.get("count").and_then(|c| c.as_i64()).unwrap_or(0),
            )).collect()).unwrap_or_default();
    }

    /// 消费 SimpleSelect 下拉选择事件（在 render 前调用）
    pub(crate) fn poll_select_events(&mut self, cx: &mut Context<Self>) {
        let mut hit: Option<(&'static str, String)> = None;
        if let Some(rx) = &self.select_rx {
            while let Ok(ev) = rx.try_recv() { hit = Some(ev); }
        }
        if let Some((tag, value)) = hit {
            match tag {
                "lang" => { let loc2 = value.clone(); crate::i18n::set_locale(&loc2); self.set_setting_raw("locale", value, cx); }
                "theme" => { let m: &'static str = if value == "dark" { "dark" } else if value == "light" { "light" } else { "system" }; self.set_theme_mode(m, cx); }
                "gwplatform" => { self.switch_gw_platform(&value, cx); }
                "gwmodel" => { self.gw_test_model = Some(value); cx.notify(); }
                _ => {}
            }
        }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_select_events(cx);
        // 首帧：为当前落点页面拉取数据（启动直接进统计/路由等页面时也要有内容）
        if !self.page_loaded_once {
            self.page_loaded_once = true;
            self.scroll.set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
            // 关闭窗口 → 弹框让用户选择「最小化到后台」或「退出程序」。
            // 必须用 on_window_should_close 拦截：标题栏 ✕ 是 gpui 自己画的控件，
            // 组件级 on_close_window 对它不生效（会直接 remove_window）。
            window.on_window_should_close(cx, |window, cx| {
                if CLOSE_PROMPT.load(std::sync::atomic::Ordering::Relaxed) {
                    return false;
                }
                CLOSE_PROMPT.store(true, std::sync::atomic::Ordering::Relaxed);
                window.open_dialog(cx, |dialog, _window, _cx| {
                    dialog
                        .title(t("close_title"))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(crate::theme::current_palette().3))
                                .child(t("close_desc")),
                        )
                        .footer(
                            h_flex()
                                .gap(px(8.))
                                .justify_end()
                                .child(
                                    Button::new("close-bg")
                                        .label(t("close_to_background"))
                                        .on_click(|_, window, cx| {
                                            CLOSE_PROMPT.store(false, std::sync::atomic::Ordering::Relaxed);
                                            window.close_dialog(cx);
                                            window.minimize_window();
                                        }),
                                )
                                .child(
                                    Button::new("close-quit")
                                        .label(t("quit_app"))
                                        .danger()
                                        .on_click(|_, window, cx| {
                                            CLOSE_PROMPT.store(false, std::sync::atomic::Ordering::Relaxed);
                                            window.close_dialog(cx);
                                            cx.quit();
                                        }),
                                ),
                        )
                });
                false // 阻止本次关闭
            });
            // 快照/账号/设置：所有页面都要用，启动即拉一次
            self.refresh_all(cx);
            // 后台定时刷新：首页报表与额度默认自动更新（60s）
            if !self.auto_refresh_started {
                self.auto_refresh_started = true;
                let weak = cx.weak_entity();
                cx.spawn(async move |_this, cx| loop {
                    let slept = backend::bg(async {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }).await;
                    if slept.is_err() { break; }
                    let alive = weak.update(cx, |v, cx| {
                        v.refresh_all(cx);
                        if v.page == Page::Overview {
                            v.load_chart(cx);
                        }
                    });
                    if alive.is_err() { break; }
                }).detach();
            }
            match self.page {
                Page::Overview => self.load_chart(cx),
                Page::Statistics => self.load_statistics(cx),
                Page::Routing => self.refresh_gateway(cx),
                Page::Settings => {
                    if self.settings_tab == "tools" { self.load_tools(cx); }
                    if self.settings_tab == "data" { self.update_check(cx); }
                }
                _ => {}
            }
        }
        let theme = cx.theme().clone();
        let fg = theme.foreground;
        let dark = matches!(cx.theme().mode, gpui_kit::component::ThemeMode::Dark);
        let (_, _, _, _, _, _, _, _, _, shell_bg, _, _, _) = crate::theme::palette(dark);

        // 原版 #shell：圆角 16 + 边框；顶栏 / 导航 / 滚动内容区
        v_flex()
            .size_full()
            .bg(rgb(shell_bg))
            .text_color(fg)
            .font_family(".SystemUIFont")
            .rounded(px(16.))
            .border_1()
            .border_color(rgb(if dark { 0x33324a } else { 0xe4e2f0 }))
            .overflow_hidden()
            .child(self.render_title_bar(window, cx))
            .child(self.render_nav(cx))
            .child(
                div()
                    .id("workspace")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .pb(px(14.))
                    .child(self.render_page(window, cx)),
            )
            .child(self.render_toast())
    }
}

impl AppView {
    /// 顶栏：原版渐变背景 + FR 渐变标 + 标题 + 今日周期
    pub(crate) fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cycle = self.snap.as_ref()
            .and_then(|s| s.get("cycle")).and_then(|c| c.as_str()).unwrap_or("")
            .to_string();
        let dark = matches!(cx.theme().mode, gpui_kit::component::ThemeMode::Dark);
        let (_, _, _, subtle, text, _, _, _, _, _, topbar_a, topbar_b, _) = crate::theme::palette(dark);
        let grad = gpui_kit::linear_gradient(145.0,
            gpui_kit::linear_color_stop(rgb(topbar_a), 0.0),
            gpui_kit::linear_color_stop(rgb(topbar_b), 1.0));
        let weak = cx.weak_entity();
        // 自定义标题栏：只保留「最小化」「关闭」两个按钮（不要最大化）。
        // 组件 TitleBar 会无条件画出最大化按钮，所以这里自己渲染。
        h_flex()
            .items_center()
            .w_full()
            .bg(grad)
            .border_b_1()
            .border_color(rgb(if dark { 0x3a3949 } else { 0xeae9f1 }))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(14.)).py(px(9.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(24.)).h(px(24.))
                            .rounded(px(8.))
                            .bg(gpui_kit::linear_gradient(135.0,
                                gpui_kit::linear_color_stop(rgb(0x6862ed), 0.0),
                                gpui_kit::linear_color_stop(rgb(0x9d7bf0), 1.0)))
                            .flex().items_center().justify_center()
                            .text_color(rgb(0xffffff))
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .child("FR"),
                    )
                    .child(div().font_weight(FontWeight::BOLD).text_size(px(13.)).text_color(rgb(text)).child(t("app_name")))
                    // 当前工作台状态：一眼看出自己在哪个页面
                    .child(
                        div().px(px(8.)).py(px(2.)).rounded(px(6.))
                            .bg(mix_color(accent(), 0.14))
                            .text_size(px(11.)).font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(accent()))
                            .child(t(self.page.label())),
                    )
                    .child(
                        div().text_size(px(11.5)).text_color(rgb(subtle))
                            .child(format!("{} {}", t("today_cycle"), cycle)),
                    ),
            )
            // 窗口控制：最小化 + 关闭（无最大化）
            .child(
                h_flex().items_center().h_full().flex_shrink_0()
                    .child(
                        window_control_button("wc-min", "—", false)
                            .on_click(|_, window, _cx| window.minimize_window()),
                    )
                    .child(
                        window_control_button("wc-close", "✕", true)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |_v, cx| {
                                    show_close_prompt(window, cx);
                                });
                            }),
                    ),
            )
    }
    pub(crate) fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cur = self.page;
        let pages = Page::all();
        h_flex()
            .flex_wrap()
            .gap(px(4.))
            .px(px(12.))
            .py(px(8.))
            .children(pages.into_iter().map(|p| {
                let selected = cur == p;
                // 选中态要一眼可辨：实心主色胶囊 + 白字；未选中为透明 ghost
                Button::new(SharedString::from(format!("nav-{:?}", p)))
                    .label(t(p.label()))
                    .small()
                    .when(selected, |b| b.primary().selected(true))
                    .when(!selected, |b| b.ghost())
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        if this.page == p { return; }
                        this.page = p;
                        // 换页后回到内容区顶部（避免沿用上一页滚动位置）
                        this.scroll.set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
                        if p == Page::Statistics { this.load_statistics(cx); }
                        if p == Page::Routing { this.refresh_gateway(cx); }
                        if p == Page::Settings && this.settings_tab == "tools" { this.load_tools(cx); }
                        cx.notify();
                    }))
            }))
    }

    /// 页面分发
    pub(crate) fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Overview => self.render_overview(cx).into_any_element(),
            Page::Accounts => self.render_accounts(cx).into_any_element(),
            Page::Checkin => self.render_checkin(cx).into_any_element(),
            Page::Statistics => self.render_statistics(cx).into_any_element(),
            Page::Routing => self.render_routing(window, cx).into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        }
    }

    /// 页头：标题 + 副标题
    pub(crate) fn page_header(&self, title: &str, sub: &str) -> impl IntoElement {
        v_flex()
            .gap(px(3.))
            .pb(px(10.))
            .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title.to_string()))
            .when(!sub.is_empty(), |el| el.child(
                div().text_size(px(11.5)).text_color(rgb(subtle())).child(sub.to_string())
            ))
    }

    /// 卡片容器（对齐 .surface）
    /// 卡片容器：背景/描边随亮暗主题变化
    pub(crate) fn surface_card(&self) -> Div {
        let dark = crate::theme::is_dark();
        div()
            .w_full()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(line_color()))
            .bg(if dark { rgb(0x20202c) } else { rgb(0xffffff) })
    }

    /// toast（底部居中，3 秒消失）
    pub(crate) fn render_toast(&self) -> impl IntoElement {
        let Some((msg, at)) = &self.toast else {
            return div().absolute().bottom_0().w(px(0.)).h(px(0.));
        };
        if at.elapsed() > std::time::Duration::from_secs(3) {
            return div().absolute().bottom_0().w(px(0.)).h(px(0.));
        }
        // 屏幕中下方居中展示（整行容器 + justify_center，避免 left:50% 不带位移导致偏右）
        div()
            .absolute()
            .bottom(px(16.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .max_w(px(460.))
                    .px(px(16.))
                    .py(px(10.))
                    .rounded(px(10.))
                    .bg(rgb(0x2c2c38))
                    .text_color(rgb(0xffffff))
                    .text_size(px(12.))
                    .shadow_lg()
                    .child(msg.clone()),
            )
    }

    /// 平台图标：嵌入位图 → 圆标首字母兜底
    pub(crate) fn picon(&self, provider: &str, size: Pixels) -> AnyElement {
        let m = platform::meta(provider);
        let color = m.color_hex_u32();
        let initial = platform_name(provider).chars().next().unwrap_or('?').to_string();
        match platform::icon_image(provider) {
            Some(image) => img(ImageSource::Image(Arc::new(image)))
                .size(size).rounded(px(7.)).into_any_element(),
            None => div()
                .size(size)
                .rounded(px(7.))
                .bg(rgb(color))
                .flex().items_center().justify_center()
                .text_color(rgb(0xffffff))
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .child(initial)
                .into_any_element(),
        }
    }

    /// 状态徽标（圆点 + 文案）
    pub(crate) fn status_badge(&self, status: &str) -> AnyElement {
        let (kind, key) = st_kind(status);
        let c = st_color(kind);
        h_flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(c)
            .child(div().size(px(7.)).rounded_full().bg(c))
            .child(t(key).to_string())
            .into_any_element()
    }
}

pub(crate) fn platform_name(p: &str) -> String {
    let k = format!("pname_{}", p);
    let s = t(&k);
    if s == k { p.to_string() } else { s.to_string() }
}

pub(crate) fn mix_color(base: u32, ratio: f32) -> Hsla {
    let c: Hsla = Rgba::from(rgb(base)).into();
    c.opacity(ratio)
}

/// 按周期聚合每日 token（对齐原版 keyOf）
pub(crate) fn aggregate_tokens(items: &[(String, f64, i64)], daily: &serde_json::Value, period: &str) -> Vec<f64> {
    let days = daily.get("days").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let mut acc: std::collections::BTreeMap<String, f64> = Default::default();
    for d in &days {
        let day = d.get("day").and_then(|x| x.as_str()).unwrap_or("");
        let total = d.get("totalTokens").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let key = bucket_key(day, period);
        *acc.entry(key).or_insert(0.0) += total;
    }
    items.iter().map(|(bucket, _, _)| acc.get(bucket).copied().unwrap_or(0.0)).collect()
}

pub(crate) fn bucket_key(day: &str, period: &str) -> String {
    match period {
        "month" => day.get(..7).unwrap_or(day).to_string(),
        "year" => day.get(..4).unwrap_or(day).to_string(),
        _ => day.to_string(),
    }
}

pub(crate) fn short_day(day: &str, total: usize) -> String {
    if total > 100 { day.get(2..).unwrap_or(day).to_string() } else { day.get(5..).unwrap_or(day).to_string() }
}

/// 标题栏窗口控制按钮（hover 变底色；close=true 时用危险色）
fn window_control_button(id: &'static str, glyph: &'static str, close: bool) -> gpui_kit::component::button::Button {
    // Button 自带 hover 样式，外部再 .hover() 会 panic（hover style already set）
    let color = if close { red() } else { subtle() };
    gpui_kit::component::button::Button::new(id)
        .label(glyph)
        .ghost()
        .small()
        .text_size(px(12.))
        .text_color(rgb(color))
}

/// 关闭确认弹框（标题栏 ✕ 与系统关闭共用）
fn show_close_prompt(window: &mut Window, cx: &mut App) {
    if CLOSE_PROMPT.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    CLOSE_PROMPT.store(true, std::sync::atomic::Ordering::Relaxed);
    window.open_dialog(cx, |dialog, _window, _cx| {
        dialog
            .title(t("close_title"))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(crate::theme::current_palette().3))
                    .child(t("close_desc")),
            )
            .footer(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        Button::new("close-bg")
                            .label(t("close_to_background"))
                            .on_click(|_, window, cx| {
                                CLOSE_PROMPT.store(false, std::sync::atomic::Ordering::Relaxed);
                                window.close_dialog(cx);
                                window.minimize_window();
                            }),
                    )
                    .child(
                        Button::new("close-quit")
                            .label(t("quit_app"))
                            .danger()
                            .on_click(|_, window, cx| {
                                CLOSE_PROMPT.store(false, std::sync::atomic::Ordering::Relaxed);
                                window.close_dialog(cx);
                                cx.quit();
                            }),
                    ),
            )
    });
}
