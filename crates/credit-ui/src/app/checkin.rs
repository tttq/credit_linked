use super::{
    accent, amber, subtle, MyFluent, AccountRow, AppView,
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
// 签到中心：进度 hero + 筛选 pills + 账号行（顶部按钮与账号管理一致）
// =====================================================================
impl AppView {
    pub(crate) fn render_checkin(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let accs = self.accounts.clone();
        let done_n = accs.iter().filter(|a| a.done()).count();
        let pending_n = accs.iter().filter(|a| a.status.is_empty() || a.status == "idle" || a.status == "no-activity").count();
        let confirm_n = accs.iter().filter(|a| a.status == "pending").count();
        let attention_n = accs.iter().filter(|a| a.status == "failed" || a.status == "needs-action" || a.status == "auth-expired").count();
        let sched = self.settings.as_ref()
            .map(|s| if s.get("randomTime").and_then(|v| v.as_bool()).unwrap_or(false) {
                t("random").to_string()
            } else {
                s.get("time").and_then(|v| v.as_str()).unwrap_or("09:00").to_string()
            })
            .unwrap_or_else(|| "--:--".into());
        let filter = self.checkin_filter.clone();

        let match_fn = |a: &AccountRow| -> bool {
            match filter.as_str() {
                "done" => a.done(),
                "pending" => a.status.is_empty() || a.status == "idle" || a.status == "no-activity",
                "confirm" => a.status == "pending",
                "attention" => a.status == "failed" || a.status == "needs-action" || a.status == "auth-expired",
                _ => true,
            }
        };
        let filters: [(&str, &str, usize); 5] = [
            ("all", "filter_all", accs.len()),
            ("done", "filter_done", done_n),
            ("pending", "filter_pending", pending_n),
            ("confirm", "filter_confirm", confirm_n),
            ("attention", "filter_attention", attention_n),
        ];
        let list: Vec<&AccountRow> = accs.iter().filter(|a| match_fn(a)).collect();
        let pct = if accs.is_empty() { 0.0 } else { done_n as f32 / accs.len() as f32 };

        v_flex().gap(px(12.))
            .child(
                h_flex().justify_between().items_center()
                    .child(self.page_header(t("nav_checkin"), ""))
                    .child(
                        h_flex().gap(px(8.))
                            .child(Button::new("ck-quota-all").label(t("refresh_balance")).small()
                                .on_click(cx.listener(|this, _, _w, cx| this.quota_all(cx))))
                            .child(Button::new("ck-checkin-all").label(t("one_click_checkin")).primary().small()
                                .on_click(cx.listener(|this, _, _w, cx| this.checkin_all(cx)))),
                    ),
            )
            // 进度 hero
            .child(
                div().w_full().rounded(px(14.)).p(px(18.))
                    .bg(mix_color(accent(), 0.10))
                    .child(
                        h_flex().gap(px(22.)).items_center()
                            .child(
                                v_flex().flex_1().gap(px(8.))
                                    .child(
                                        h_flex().items_end().gap(px(4.))
                                            .child(div().text_size(px(28.)).font_weight(FontWeight::BOLD).child(done_n.to_string()))
                                            .child(div().text_size(px(20.)).text_color(rgb(subtle())).child(format!(" / {}", accs.len()))),
                                    )
                                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(t("cycle_done")))
                                    .child(
                                        div().w_full().h(px(6.)).rounded(px(10.))
                                            .bg(mix_color(accent(), 0.15))
                                            .child(div().h_full().rounded(px(10.)).bg(rgb(accent())).w(relative(pct))),
                                    ),
                            )
                            .child(self.metric_col(pending_n, t("waiting_claim"), None))
                            .child(self.metric_col(confirm_n, t("st_pending"), None))
                            .child(self.metric_col(attention_n, t("st_needs_action"), Some(if attention_n > 0 { rgb(amber()) } else { rgb(subtle()) }))),
                    ),
            )
            // 计划时间条
            .child(
                h_flex().items_center().gap(px(14.)).flex_wrap()
                    .px(px(16.)).py(px(10.))
                    .rounded(px(12.))
                    .border_1().border_color(hsla(0., 0., 0.5, 0.12))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).child(sched))
                    .child(div().text_size(px(11.)).child(t("sched_checkin_label")))
                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(t("beijing_note"))),
            )
            // 筛选 pills
            .child(
                h_flex().gap(px(2.)).p(px(3.)).rounded(px(9.))
                    .border_1().border_color(hsla(0., 0., 0.5, 0.12)).flex_wrap()
                    .children(filters.into_iter().map(|(k, key, n)| {
                        Button::new(SharedString::from(format!("cf-{k}")))
                            .label(format!("{} {}", t(key), n))
                            .small()
                            .when(filter == k, |b| b.selected(true))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.checkin_filter = k.to_string();
                                cx.notify();
                            }))
                    })),
            )
            .children(if list.is_empty() {
                vec![div().py(px(22.)).text_center().text_size(px(12.)).text_color(rgb(subtle()))
                    .child(t("no_accounts_in_filter")).into_any_element()]
            } else {
                list.iter().map(|a| self.render_checkin_row(a, cx)).collect::<Vec<_>>()
            })
    }

    pub(crate) fn metric_col(&self, n: usize, label: &str, color: Option<Rgba>) -> impl IntoElement {
        v_flex().items_center()
            .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD)
                .text_color(color.unwrap_or(rgb(subtle()))).child(n.to_string()))
            .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(label.to_string()))
    }

    pub(crate) fn render_checkin_row(&self, a: &AccountRow, cx: &mut Context<Self>) -> AnyElement {
        let m = platform::meta(&a.provider);
        let quota_text = a.remaining.map(crate::i18n::fmt_num).unwrap_or_else(|| "—".into());
        let last = if a.last_checkin.len() >= 16 {
            a.last_checkin[..16].replace('T', " ")
        } else { "—".to_string() };
        let msg = if a.message.is_empty() { String::new() } else { format!(" · {}", a.message) };
        let provider = a.provider.clone();
        let uid = a.uid.clone();
        let id_cknow = format!("ck-now-{}-{}", provider, uid);
        let id_qnow = format!("q-now-{}-{}", provider, uid);
        let (p_cknow, u_cknow) = (provider.clone(), uid.clone());
        let (p_qnow, u_qnow) = (provider, uid);

        div().w_full()
            .rounded(px(10.)).border_1().border_color(mix_color(m.color_hex_u32(), 0.42))
            .px(px(12.)).py(px(10.))
            .child(
                h_flex().items_center().gap(px(10.)).flex_wrap()
                    .child(
                        h_flex().items_center().gap(px(10.)).min_w(px(140.)).flex_1()
                            .child(self.picon(&a.provider, px(22.)))
                            .child(
                                v_flex().min_w_0()
                                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(platform_name(&a.provider)))
                                    .child(div().text_size(px(11.)).text_color(rgb(subtle())).max_w(px(150.))
                                        .overflow_hidden().truncate()
                                        .child(if a.name.is_empty() { a.uid.chars().take(14).collect() } else { a.name.clone() })),
                            ),
                    )
                    .child(
                        v_flex()
                            .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(t("status")))
                            .child(self.status_badge(&a.status)),
                    )
                    .child(
                        v_flex().items_end()
                            .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(quota_text))
                            .child(div().text_size(px(10.5)).text_color(rgb(subtle()))
                                .child(format!("{}{}", t("available_balance"), msg))),
                    )
                    .child(
                        v_flex()
                            .child(div().text_size(px(11.)).text_color(rgb(subtle())).child(t("last_time")))
                            .child(div().text_size(px(12.5)).child(last)),
                    )
                    .child(
                        h_flex().gap(px(6.))
                            .child(Button::new(SharedString::from(id_cknow))
                                .label(t("checkin_now")).small()
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.checkin_one(p_cknow.clone(), u_cknow.clone(), cx);
                                })))
                            .child(Button::new(SharedString::from(id_qnow))
                                .label(t("quota_btn")).small()
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.refresh_quota(p_qnow.clone(), u_qnow.clone(), cx);
                                }))),
                    ),
            )
            .into_any_element()
    }
}
