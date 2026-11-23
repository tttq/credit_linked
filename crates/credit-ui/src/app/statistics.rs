use super::{
    accent, green, red, token_blue, subtle, MyFluent, AccountRow, Page, DayBar, AppView,
    st_color, st_kind, platform_name, mix_color, aggregate_tokens, short_day, SELECT_TX,
};
use crate::backend;
use crate::i18n::{t, tf};
use crate::platform;
use gpui_kit::component::{
    chart::{BarChart, LineChart},
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    switch::Switch,
    h_flex, v_flex,
    Disableable as _, Selectable as _,
    Sizable as _,
};
use gpui_kit::*;
use std::sync::Arc;
// =====================================================================
// 积分统计：账号/周期 pills + 指标卡 + 官方 LineChart
// =====================================================================
impl AppView {
    pub(crate) fn render_statistics(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.stat_items.clone();
        let tokens = self.stat_tokens.clone();
        let note = self.stat_note.clone();
        let period = self.stat_period.clone();
        let total_sum: f64 = items.iter().map(|i| i.1).sum();
        let total_count: i64 = items.iter().map(|i| i.2).sum();
        let token_sum: f64 = tokens.iter().sum();
        let show_token = self.stat_account == "all";

        v_flex().gap(px(12.))
            .child(self.page_header(t("nav_stats"), t("stats_sub")))
            .child(
                h_flex().justify_between().items_center().flex_wrap().gap(px(10.))
                    .child(div().text_size(px(12.)).text_color(rgb(subtle())).child(t("select_account")))
                    .child(
                        h_flex().gap(px(2.)).p(px(3.)).rounded(px(9.))
                            .border_1().border_color(hsla(0., 0., 0.5, 0.12))
                            .children([("day", "daily"), ("week", "weekly"), ("month", "monthly"), ("year", "yearly")]
                                .iter().map(|(p, k)| {
                                    Button::new(SharedString::from(format!("stat-p-{p}")))
                                        .label(t(k)).small()
                                        .when(period == *p, |b| b.primary().selected(true))
                                        .when(period != *p, |b| b.ghost())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.stat_period = p.to_string();
                                            this.load_statistics(cx);
                                        }))
                                })),
                    ),
            )
            .child(
                div().w_full().rounded(px(12.)).border_1().border_color(hsla(0., 0., 0.5, 0.12))
                    .py(px(14.)).px(px(8.))
                    .child(
                        h_flex().w_full()
                            .child(self.stat_metric(t("period_points_short"), &crate::i18n::fmt_num(total_sum)))
                            .child(self.stat_metric(t("token_period"), &crate::i18n::fmt_num(token_sum)))
                            .child(self.stat_metric(t("records"), &total_count.to_string())),
                    ),
            )
            .child(
                self.surface_card().p(px(16.))
                    .child(
                        h_flex().justify_between().items_center()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(t("trend")))
                            .child(
                                h_flex().gap(px(15.)).items_center()
                                    .child(self.legend_dot(accent(), t("credit_in")))
                                    .when(show_token && tokens.iter().any(|x| *x > 0.0), |el| el.child(self.legend_dot(token_blue(), t("token_usage")))),
                            ),
                    )
                    // 图表自身已带高度（chart_line 内部按 show_token 定高），外层只约束宽度
                    .child(div().w_full().child(Self::chart_line(items, tokens, show_token, &period, cx)))
                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).pt(px(8.)).child(note)),
            )
    }

    pub(crate) fn stat_metric(&self, label: &str, value: &str) -> impl IntoElement {
        div().flex_1().text_center().py(px(6.))
            .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(label.to_string()))
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(value.to_string()))
    }

    pub(crate) fn legend_dot(&self, color: u32, label: &str) -> impl IntoElement {
        h_flex().items_center().gap(px(6.))
            .child(div().w(px(11.)).h(px(3.)).rounded(px(2.)).bg(rgb(color)))
            .child(div().text_size(px(12.)).text_color(rgb(subtle())).child(label.to_string()))
    }

    pub fn load_statistics(&mut self, cx: &mut Context<Self>) {
        let period = self.stat_period.clone();
        let account = self.stat_account.clone();
        let (provider, uid) = if account == "all" || account.is_empty() {
            (None, None)
        } else {
            let mut it = account.splitn(2, '|');
            (it.next().map(String::from), it.next().map(String::from))
        };
        cx.spawn(async move |this, cx| {
            let days_n = match period.as_str() { "day" => 14, "week" => 56, "month" => 180, _ => 730 };
            let all_view = provider.is_none() && uid.is_none();
            let period2 = period.clone();
            let (agg, toks) = backend::bg(async move {
                let agg = backend::statistics(provider, uid, Some(period.clone())).await;
                let toks = if all_view { backend::daily_tokens(Some(days_n)).await.ok() } else { None };
                (agg, toks)
            }).await.unwrap_or_else(|_| (Err(String::new()), None));
            this.update(cx, |v, cx| {
                if let Ok(agg) = agg { v.fill_stat_from_json(&agg); }
                if let Some(tk) = toks {
                    v.stat_tokens = aggregate_tokens(&v.stat_items, &tk, &period2);
                    v.stat_note = t("token_stats_all").to_string();
                } else {
                    v.stat_tokens = Vec::new();
                    v.stat_note = t("token_stats_single").to_string();
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn load_chart(&mut self, cx: &mut Context<Self>) {
        let range = self.overview_range;
        cx.spawn(async move |this, cx| {
            let data = backend::bg(async move { backend::daily_income(Some(range as i64)).await })
                .await.ok().and_then(|r| r.ok());
            this.update(cx, |v, cx| {
                if let Some(d) = data { v.fill_chart_from_income(&d); }
                cx.notify();
            }).ok();
        }).detach();
    }

    /// 首页柱状图（官方 BarChart，按主平台品牌色着色）
    pub(crate) fn chart_bars(data: Vec<DayBar>) -> AnyElement {
        if data.is_empty() {
            return div().w_full().h_full().flex().items_center().justify_center()
                .text_size(px(12.)).text_color(rgb(subtle())).child("—").into_any_element();
        }
        let bars = data.iter().map(|d| {
            let (color, _) = dominant_provider_color(d);
            ChartDatum { label: short_day(&d.day, data.len()), value: d.income, color }
        }).collect::<Vec<_>>();
        BarChart::new(bars)
            .id("chart-overview")
            .band(|d| d.label.clone())
            .value(|d| d.value)
            .fill(|d, _, _, _| d.color)
            .max_band_width(px(22.))
            .corner_radii(px(4.))
            .grid(true)
            .value_tick_count(4)
            .into_any_element()
    }

    /// 统计折线图（官方 LineChart：积分 + token 双曲线同图）
    /// period 决定 x 轴标签粒度：按天只显示「月-日」，按周显示「W03」，按月「01月」，按年「2025」。
    ///
    /// gpui_kit 的 LineChart 是单序列：两个曲线用**两个 LineChart 绝对定位叠加**实现。
    /// 积分（主色）画在下层带轴，token（蓝）画在上层隐藏轴，两层共享同一绘图区；
    /// 各自按自身数据归一化（各自 y 缩放），曲线形态都是真实的相对趋势。
    pub(crate) fn chart_line(items: Vec<(String, f64, i64)>, tokens: Vec<f64>, show_token: bool, period: &str, cx: &mut Context<Self>) -> AnyElement {
        let dark = matches!(cx.theme().mode, gpui_kit::component::ThemeMode::Dark);
        let (accent, _, _, _, _, _, _, _, _, _, _, _, _) = crate::theme::palette(dark);
        if items.is_empty() {
            return div().w_full().h_full().flex().items_center().justify_center()
                .text_size(px(12.)).text_color(rgb(subtle())).child("—").into_any_element();
        }
        // 短标签；若区间跨年导致重复，则回退为完整 bucket 保证 x 唯一
        let mut labels: Vec<String> = items.iter().map(|(b, _, _)| short_bucket(b, period)).collect();
        let has_dup = {
            let mut seen = std::collections::HashSet::new();
            labels.iter().any(|l| !seen.insert(l.clone()))
        };
        if has_dup {
            labels = items.iter().map(|(b, _, _)| b.clone()).collect();
        }
        let has_token = tokens.iter().any(|t| *t > 0.0);
        let show_token = show_token && has_token;
        let series: Vec<StatDatum> = items.iter().enumerate().map(|(i, (_b, points, _))| StatDatum {
            x: gpui_kit::SharedString::from(labels.get(i).cloned().unwrap_or_default()),
            points: *points,
            tokens: tokens.get(i).copied().unwrap_or(0.0),
        }).collect();
        let peak = items.iter().map(|(_, p, _)| *p).fold(0.0f64, f64::max);
        let total: f64 = items.iter().map(|(_, p, _)| *p).sum();
        let height = if show_token { 150. } else { 180. };
        v_flex().w_full().gap(px(4.))
            // 数值摘要：替代 Y 轴刻度（官方 LineChart 在小尺寸下不画刻度数字）
            .child(
                h_flex().gap(px(12.))
                    .child(div().text_size(px(10.5)).text_color(rgb(subtle()))
                        .child(format!("{} {}", t("peak"), crate::i18n::fmt_num(peak))))
                    .child(div().text_size(px(10.5)).text_color(rgb(subtle()))
                        .child(format!("{} {}", t("total_points"), crate::i18n::fmt_num(total)))),
            )
            .child(
                // 双曲线容器：relative 定位，两张图绝对定位铺满同一绘图区
                div().w_full().h(px(height)).relative()
                    .child(
                        // 下层：积分曲线（带 x 轴与网格）
                        div().absolute().inset_0()
                            .child(
                                LineChart::new(series.clone())
                                    .id("chart-points")
                                    .x(|d| d.x.clone())
                                    .y(|d| d.points)
                                    .stroke(rgb(accent))
                                    .grid(true)
                                    .x_tick_count(if series.len() > 8 { 6 } else { series.len().max(1) })
                                    .dot()
                            ),
                    )
                    .when(show_token, |el| el.child(
                        // 上层：token 曲线（隐藏轴与网格，避免与下层重叠；交互层让给下层）
                        div().absolute().inset_0()
                            .child(
                                LineChart::new(series.clone())
                                    .id("chart-tokens")
                                    .x(|d| d.x.clone())
                                    .y(|d| d.tokens)
                                    .stroke(rgb(token_blue()))
                                    .grid(false)
                                    .x_axis(false)
                                    .x_tick_count(if series.len() > 8 { 6 } else { series.len().max(1) })
                                    .interactive(false)
                            ),
                    )),
            )
            .into_any_element()
    }
}

fn short_bucket(bucket: &str, period: &str) -> String {
    match period {
        // "2025-01-15" → "15"（按天只显示日）
        "day" => bucket.get(8..).map(|s| s.to_string()).unwrap_or_else(|| bucket.to_string()),
        // "2025-W03" → "W03"
        "week" => bucket.rsplit("-W").next().map(|w| format!("W{w}")).unwrap_or_else(|| bucket.to_string()),
        // "2025-01" → "01月"
        "month" => bucket.get(5..).map(|m| format!("{m}月")).unwrap_or_else(|| bucket.to_string()),
        // "2025" → "2025"
        _ => bucket.to_string(),
    }
}

struct ChartDatum { label: String, value: f64, color: Rgba }
#[derive(Clone)]
struct StatDatum { x: gpui_kit::SharedString, points: f64, tokens: f64 }

fn dominant_provider_color(d: &DayBar) -> (Rgba, String) {
    let best = d.by_provider.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    if let Some((provider, _)) = best {
        (rgb(platform::meta(provider).color_hex_u32()), provider.clone())
    } else {
        (rgb(accent()), "checkin".into())
    }
}
