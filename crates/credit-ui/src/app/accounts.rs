use super::{
    accent, red, st_color, subtle, MyFluent, AccountRow, AppView,
    platform_name, mix_color,
};
use crate::i18n::{t, tf};
use crate::platform;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    switch::Switch,
    h_flex, v_flex,
    Disableable as _,
    Sizable as _,
};
use gpui_kit::*;
// =====================================================================
// 账号管理：平台分组卡片 + 账号行操作
// =====================================================================
impl AppView {
    pub(crate) fn render_accounts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let accs = self.accounts.clone();
        let all_plats: Vec<&'static str> = platform::PLATS.iter()
            .filter(|m| m.provider != "doubao").map(|m| m.provider).collect();
        let added: Vec<String> = accs.iter().map(|a| a.provider.clone()).collect::<std::collections::HashSet<_>>().into_iter().collect();
        let mut plats: Vec<&'static str> = all_plats.iter().filter(|p| added.contains(&p.to_string())).copied().collect();
        plats.extend(all_plats.iter().filter(|p| !added.contains(&p.to_string())).copied());
        let n_added = added.len();
        let m_total = accs.len();

        v_flex().gap(px(12.))
            .child(
                h_flex().justify_between().items_center()
                    .child(self.page_header(t("nav_accounts"),
                        &tf("accounts_connected", &[("n", n_added.to_string()), ("m", m_total.to_string())])))
                    .child(
                        h_flex().gap(px(8.))
                            .child(Button::new("ac-quota").label(t("refresh_balance")).small()
                                .on_click(cx.listener(|this, _, _w, cx| this.quota_all(cx))))
                            .child(Button::new("ac-checkin").label(t("one_click_checkin")).primary().small()
                                .on_click(cx.listener(|this, _, _w, cx| this.checkin_all(cx)))),
                    ),
            )
            .children(plats.iter().map(|p| self.render_platform_group(p, &accs, cx)))
    }

    pub(crate) fn render_platform_group(&self, p: &'static str, accs: &[AccountRow], cx: &mut Context<Self>) -> AnyElement {
        let m = platform::meta(p);
        let list: Vec<&AccountRow> = accs.iter().filter(|a| a.provider == p).collect();
        let cap = if m.multi { t("multi_account") } else { t("single_account") };

        v_flex()
            .rounded(px(12.))
            .border_1()
            .border_color(hsla(0., 0., 0.5, 0.12))
            .overflow_hidden()
            .child(
                h_flex().justify_between().items_center().flex_wrap()
                    .px(px(12.)).py(px(10.))
                    .border_b_1().border_color(hsla(0., 0., 0.5, 0.12))
                    .child(
                        h_flex().items_center().gap(px(8.)).flex_wrap()
                            .child(self.picon(p, px(24.)))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(platform_name(p)))
                            .child(
                                div().px(px(6.)).py(px(2.)).rounded(px(6.))
                                    .text_size(px(10.5))
                                    .bg(mix_color(accent(), 0.09)).text_color(rgb(accent()))
                                    .child(cap),
                            )
                            .child(div().text_size(px(11.)).text_color(rgb(subtle()))
                                .child(tf("n_accounts", &[("n", list.len().to_string())]))),
                    )
                    .child(
                        h_flex().gap(px(6.))
                            .when(m.login != "none", |el| el.child(
                                Button::new(SharedString::from(format!("add-{p}")))
                                    .label(t("add_account")).primary().small()
                                    .on_click(cx.listener(move |this, _, _w, cx| this.start_login(p, cx))),
                            ))
                            .when(platform::can_import(p), |el| el.child(
                                Button::new(SharedString::from(format!("import-{p}")))
                                    .label(t("scan_local")).small()
                                    .on_click(cx.listener(move |this, _, _w, cx| this.import_local(p, cx))),
                            )),
                    ),
            )
            .children(if list.is_empty() {
                vec![div().p(px(14.)).text_size(px(12.)).text_color(rgb(subtle()))
                    .child(t("empty_no_accounts")).into_any_element()]
            } else {
                list.iter().map(|a| self.render_account_row(a, p, cx)).collect::<Vec<_>>()
            })
            .into_any_element()
    }

    pub(crate) fn render_account_row(&self, a: &AccountRow, p: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let done = a.done();
        let (state_key, state_kind) = if done {
            ("today_claimed", "ok")
        } else if a.status == "failed" || a.status == "auth-expired" {
            ("checkin_error", "bad")
        } else {
            ("today_not_claimed", "")
        };
        let state_color = if state_kind.is_empty() { rgb(subtle()) } else { st_color(state_kind) };
        let quota_text = a.remaining.map(crate::i18n::fmt_num).unwrap_or_else(|| "—".into());
        let expires = if a.expires.len() >= 10 { format!("{}{}", t("cred_expires"), &a.expires[..10]) } else { String::new() };
        let msg = if a.message.is_empty() { String::new() } else { format!(" · {}", a.message) };
        let provider = a.provider.clone();
        let uid = a.uid.clone();
        // 预生成按钮 id 与独立克隆（避免 move 冲突）
        let id_switch = format!("auto-{}-{}", provider, uid);
        let id_ck = format!("ck-{}-{}", provider, uid);
        let id_q = format!("q-{}-{}", provider, uid);
        let id_dev = format!("dev-{}-{}", provider, uid);
        let id_del = format!("del-{}-{}", provider, uid);
        let (p_auto, u_auto) = (provider.clone(), uid.clone());
        let (p_ck, u_ck) = (provider.clone(), uid.clone());
        let (p_q, u_q) = (provider.clone(), uid.clone());
        let (p_dev, u_dev) = (provider.clone(), uid.clone());
        let (p_del, u_del) = (provider, uid);

        div().w_full()
            .px(px(12.)).py(px(10.))
            .child(
                v_flex().gap(px(8.))
                    .child(
                        h_flex().justify_between().items_center().gap(px(10.))
                            .child(
                                v_flex().min_w_0()
                                    .child(
                                        h_flex().items_center().gap(px(4.))
                                            .child(div().text_size(px(13.)).overflow_hidden().truncate()
                                                .child(if a.name.is_empty() { platform_name(p) } else { a.name.clone() }))
                                            .child(div().text_size(px(11.)).text_color(state_color).child(t(state_key))),
                                    )
                                    .child(div().text_size(px(11.)).text_color(rgb(subtle()))
                                        .overflow_hidden().truncate()
                                        .child(format!("{}{}", a.uid, expires))),
                            )
                            .child(
                                v_flex().items_end()
                                    .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(quota_text))
                                    .child(div().text_size(px(10.5)).text_color(rgb(subtle())).max_w(px(130.))
                                        .overflow_hidden().truncate().child(format!("{}{}", t("available_balance"), msg))),
                            ),
                    )
                    .child(
                        h_flex().gap(px(5.)).justify_end().flex_wrap().items_center()
                            .child(
                                Switch::new(SharedString::from(id_switch))
                                    .checked(a.auto)
                                    .on_change(cx.listener(move |this, checked: &bool, _w, cx| {
                                        this.toggle_auto(p_auto.clone(), u_auto.clone(), *checked, cx);
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(id_ck))
                                    .label(if done { t("today_claimed") } else { t("checkin") })
                                    .small().disabled(done)
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.checkin_one(p_ck.clone(), u_ck.clone(), cx);
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(id_q))
                                    .label(t("quota_btn")).small()
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.refresh_quota(p_q.clone(), u_q.clone(), cx);
                                    })),
                            )
                            .when(p == "trae", |el| el.child(
                                Button::new(SharedString::from(id_dev))
                                    .label(t("device_btn")).small()
                                    .tooltip(t("device_btn_tip"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_device_dialog(p_dev.clone(), u_dev.clone(), window, cx);
                                    })),
                            ))
                            .child(
                                Button::new(SharedString::from(id_del))
                                    .label(t("delete")).small().text().text_color(rgb(red()))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.confirm_delete(p_del.clone(), u_del.clone(), cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
