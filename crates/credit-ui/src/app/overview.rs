use super::{
    accent, subtle, MyFluent, AccountRow, AppView,
    platform_name, mix_color,
};
use crate::i18n::t;
use crate::platform;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    Selectable as _,
    Sizable as _,
};
use gpui_kit::*;
// =====================================================================
// 首页（overview）：积分趋势柱状图 + 我的 AI 工具卡
// =====================================================================
impl AppView {
    pub(crate) fn render_overview(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let accounts = self.accounts.clone();
        let range = self.overview_range;
        let sum_text = self.chart_summary_text.clone().unwrap_or_default();

        v_flex()
            .gap(px(10.))
            .child(self.page_header(t("nav_home"), ""))
            // 日期范围 pills
            .child(
                h_flex()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(hsla(0., 0., 0.5, 0.12))
                    .children([
                        (7usize, "weekly"), (30, "monthly"), (365, "yearly"),
                    ].into_iter().map(|(n, k)| {
                        Button::new(SharedString::from(format!("range-{n}")))
                            .label(t(k))
                            .small()
                            .when(range == n, |b| b.primary().selected(true))
                            .when(range != n, |b| b.ghost())
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.overview_range = n;
                                this.load_chart(cx);
                                cx.notify();
                            }))
                    })),
            )
            // 趋势卡（官方 BarChart）
            .child(
                self.surface_card()
                    .p(px(16.))
                    .child(
                        h_flex().justify_between().items_center()
                            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t("points_trend")))
                            .child(div().text_size(px(11.5)).text_color(rgb(subtle())).child(sum_text))
                    )
                    .child(div().w_full().h(px(210.)).child(Self::chart_bars(self.chart_days.clone().unwrap_or_default()))),
            )
            // 我的 AI 工具
            .child(
                h_flex().justify_between().items_center().pt(px(4.))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t("my_ai_tools")))
                    .child(div().text_size(px(12.)).text_color(rgb(accent())).child(t("manage_accounts"))),
            )
            .child(
                if accounts.is_empty() {
                    self.surface_card().p(px(22.)).child(
                        div().text_center().text_size(px(12.)).text_color(rgb(subtle())).child(t("empty_no_tools"))
                    ).into_any_element()
                } else {
                    v_flex().gap(px(8.))
                        .children(accounts.iter().map(|a| self.render_tool_card(a)))
                        .into_any_element()
                }
            )
    }

    pub(crate) fn render_tool_card(&self, a: &AccountRow) -> AnyElement {
        let m = platform::meta(&a.provider);
        let color = m.color_hex_u32();
        let quota_text = a.remaining.map(crate::i18n::fmt_num).unwrap_or_else(|| "—".into());
        let last = if a.last_checkin.len() >= 10 { format!("{}{}", t("last"), &a.last_checkin[5..10]) } else { String::new() };

        div()
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(mix_color(color, 0.42))
            .p(px(12.))
            .child(
                h_flex().items_center().gap(px(10.))
                    .child(self.picon(&a.provider, px(26.)))
                    .child(
                        v_flex().min_w_0()
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(platform_name(&a.provider)))
                            .child(div().text_size(px(11.)).text_color(rgb(subtle())).overflow_hidden().truncate().child(a.name.clone())),
                    ),
            )
            .child(
                h_flex().justify_between().items_center().pt(px(6.))
                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(t("available")))
                    .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(quota_text)),
            )
            .child(
                h_flex().justify_between().items_center()
                    .child(self.status_badge(&a.status))
                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(last)),
            )
            .into_any_element()
    }
}
