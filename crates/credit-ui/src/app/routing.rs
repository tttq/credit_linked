use super::{
    mix_color, AppView, MyFluent,
    accent, green, red, subtle,
};
use crate::backend;
use crate::i18n::{t, tf};
use crate::platform;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    switch::Switch,
    v_flex, Disableable as _, Sizable as _,
};
use gpui_kit::*;
// =====================================================================
// 账号路由：网关卡 / 请求平台 / 账号切换 / 模型测试 / 请求日志
// =====================================================================
impl AppView {
    pub fn refresh_gateway(&mut self, cx: &mut Context<Self>) {
        // busy 态立即反馈：按钮变「刷新中…」并禁用（清空按钮共用该标志）
        self.gw_logs_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (st, accts) = backend::bg(async {
                (
                    backend::gateway_status().await,
                    backend::gateway_accounts().await,
                )
            })
            .await
            .unwrap_or_else(|_| (Err(String::new()), Err(String::new())));
            this.update(cx, |v, cx| {
                v.gw_status = st.ok();
                v.gw_accounts = accts.ok();
                v.gw_logs_busy = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn gw_platform_label(channel: &str) -> String {
        for (name, key, _) in platform::GW_PLATFORMS {
            if *name == channel {
                let s = t(key);
                if s != *key {
                    return s.to_string();
                }
            }
        }
        channel.to_string()
    }

    pub(crate) fn render_routing(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(st) = self.gw_status.clone() else {
            return v_flex()
                .gap(px(12.))
                .child(self.page_header(t("route_title"), ""))
                .child(
                    div()
                        .py(px(22.))
                        .text_center()
                        .text_size(px(12.))
                        .text_color(rgb(subtle()))
                        .child(t("loading")),
                )
                .into_any_element();
        };
        let base_url = st
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let strict = st.get("strict").and_then(|v| v.as_bool()).unwrap_or(true);
        let cur_platform = st
            .get("platform")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let accounts_map = st.get("accounts").cloned().unwrap_or_default();
        let log_stats = st.get("logStats").cloned().unwrap_or_default();
        let logs = st
            .get("logs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let gw_accts = self.gw_accounts.clone();
        let model_list = self.gw_models.clone();
        let test_status = self.gw_test_status.clone();
        let test_result = self.gw_test_result.clone();
        let log_total = log_stats.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        // 注意：绝不在 render 内调用 cx.new()——会触发 gpui 重入导致栈溢出。
        // 端口/RPM/测试消息用只读文本展示，编辑走对话框（在回调中创建 InputState）。

        v_flex()
            .gap(px(12.))
            .min_w_0()
            .max_w_full()
            .child(self.page_header(t("route_title"), ""))
            // 网关卡
            .child(
                self.surface_card().p(px(16.)).child(
                    v_flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t("gw_title")),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(8.))
                                .flex_wrap()
                                .child(
                                    div()
                                        .px(px(14.))
                                        .py(px(8.))
                                        .rounded(px(9.))
                                        .bg(mix_color(accent(), 0.10))
                                        .text_color(rgb(accent()))
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .overflow_hidden()
                                        .truncate()
                                        .child(format!("{base_url}/v1")),
                                )
                                .child(Button::new("gw-copy").label(t("copy")).small().on_click(
                                    cx.listener(move |this, _, _w, cx| {
                                        let url = format!("{base_url}/v1");
                                        this.copy_text(&url, cx);
                                    }),
                                ))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(subtle()))
                                        .child("GET /v1/models · POST /v1/chat/completions"),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(subtle()))
                                .child(t("models_follow_platform")),
                        )
                        .child(
                            h_flex()
                                .gap(px(14.))
                                .flex_wrap()
                                .pt(px(6.))
                                .child(
                                    v_flex()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(subtle()))
                                                .child(format!(
                                                    "{} ({})",
                                                    t("port"),
                                                    t("port_restart_hint")
                                                )),
                                        )
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .children(self.gw_port_input.clone().map(|e| vec![Input::new(&e).w(px(100.)).small().into_any_element()]).unwrap_or_default())
                                                .child(
                                                    Button::new("gw-port-save")
                                                        .label(t("save"))
                                                        .primary()
                                                        .small()
                                                        .on_click(cx.listener(|this, _, _w, cx| this.save_gw_port(cx))),
                                                ),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(subtle()))
                                                .child(format!(
                                                    "{} ({})",
                                                    t("rate_limit"),
                                                    t("rate_limit_hint")
                                                )),
                                        )
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .children(self.gw_rpm_input.clone().map(|e| vec![Input::new(&e).w(px(100.)).small().into_any_element()]).unwrap_or_default())
                                                .child(
                                                    Button::new("gw-rpm-save")
                                                        .label(t("save"))
                                                        .primary()
                                                        .small()
                                                        .on_click(cx.listener(|this, _, _w, cx| this.save_gw_rpm(cx))),
                                                ),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(subtle()))
                                                .child(t("strict_current_only")),
                                        )
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .child(
                                                    Switch::new("gw-strict")
                                                        .checked(strict)
                                                        .on_change(cx.listener(
                                                            |this, checked: &bool, _w, cx| {
                                                                this.set_gw_strict(*checked, cx)
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(rgb(subtle()))
                                                        .child(t("strict_hint")),
                                                ),
                                        ),
                                ),
                        ),
                ),
            )
            // 请求平台卡
            .child(
                self.surface_card().p(px(16.)).child(
                    v_flex()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!(
                                    "{} - {}",
                                    t("request_platform"),
                                    t("bare_model_hint")
                                )),
                        )
                        .child(
                            h_flex()
                                .gap(px(2.))
                                .p(px(3.))
                                .rounded(px(9.))
                                .border_1()
                                .border_color(hsla(0., 0., 0.5, 0.12))
                                .flex_wrap()
                                .min_w_0()
                                .children(vec![super::actions::simple_select(
                                    "sel-gwplatform",
                                    platform::GW_PLATFORMS.iter().map(|(name, key, chat)| {
                                        let n = accounts_map.get(*name).and_then(|v| v.as_i64()).unwrap_or(0);
                                        let mut label = t(key).to_string();
                                        label.push_str(&tf("n_accounts_suffix", &[("n", n.to_string())]));
                                        if !chat { label.push_str(t("signin_only")); }
                                        ((*name).to_string(), label)
                                    }).collect(),
                                    &cur_platform,
                                    "gwplatform",
                                    px(200.),
                                )]),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(12.))
                                .flex_wrap()
                                .px(px(14.))
                                .py(px(12.))
                                .rounded(px(11.))
                                .bg(mix_color(accent(), 0.08))
                                .child(self.picon(&cur_platform, px(26.)))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(Self::gw_platform_label(&cur_platform)),
                                        )
                                        .child(
                                            div().text_size(px(12.)).text_color(rgb(subtle())).child(
                                                format!(
                                                    "{}{}{}",
                                                    t("provider_label"),
                                                    cur_platform,
                                                    tf(
                                                        "available_accounts",
                                                        &[(
                                                            "n",
                                                            accounts_map
                                                                .get(&cur_platform)
                                                                .and_then(|v| v.as_i64())
                                                                .unwrap_or(0)
                                                                .to_string()
                                                        )]
                                                    )
                                                ),
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(8.))
                                        .py(px(3.))
                                        .rounded(px(7.))
                                        .bg(rgb(accent()))
                                        .text_color(rgb(0xffffff))
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(t("current")),
                                ),
                        ),
                ),
            )
            // 账号切换卡
            .child(
                self.surface_card().p(px(16.)).child(
                    v_flex()
                        .gap(px(10.))
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .flex_wrap()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!(
                                            "{} - {}",
                                            t("account_switch"),
                                            t("account_switch_hint")
                                        )),
                                )
                                .child(
                                    Button::new("gw-acct-next")
                                        .label(t("switch_next"))
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            this.switch_next_account(cx)
                                        })),
                                ),
                        )
                        .children(match &gw_accts {
                            None => vec![div()
                                .text_size(px(12.))
                                .text_color(rgb(subtle()))
                                .child(t("loading"))
                                .into_any_element()],
                            Some(v)
                                if v.get("accounts")
                                    .and_then(|a| a.as_array())
                                    .map(|a| a.is_empty())
                                    .unwrap_or(true) =>
                            {
                                vec![div()
                                    .text_size(px(12.))
                                    .text_color(rgb(subtle()))
                                    .child(tf(
                                        "no_accounts_platform",
                                        &[(
                                            "p",
                                            v.get("platform")
                                                .and_then(|x| x.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        )],
                                    ))
                                    .into_any_element()]
                            }
                            Some(v) => v
                                .get("accounts")
                                .and_then(|a| a.as_array())
                                .unwrap_or(&vec![])
                                .iter()
                                .map(|a| {
                                    let r#ref = a
                                        .get("ref")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let name = a
                                        .get("name")
                                        .and_then(|x| x.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(String::from)
                                        .unwrap_or_else(|| {
                                            a.get("uid")
                                                .and_then(|x| x.as_str())
                                                .unwrap_or("")
                                                .to_string()
                                        });
                                    let active =
                                        a.get("active").and_then(|x| x.as_bool()).unwrap_or(false);
                                    let remaining = a.get("remaining").and_then(|x| x.as_f64());
                                    let bal = remaining
                                        .map(|r| {
                                            format!("{}{}", t("balance"), crate::i18n::fmt_num(r))
                                        })
                                        .unwrap_or_default();
                                    let failed =
                                        a.get("status").and_then(|x| x.as_str()) == Some("failed");
                                    let r2 = r#ref.clone();
                                    // 整行可点：点击账号即切换（不必非要点右侧按钮）
                                    h_flex()
                                        .id(SharedString::from(format!("gw-row-{}", r#ref)))
                                        .items_center()
                                        .gap(px(8.))
                                        .px(px(10.))
                                        .py(px(8.))
                                        .rounded(px(9.))
                                        .cursor_pointer()
                                        .border_1()
                                        .border_color(if active {
                                            rgb(accent()).into()
                                        } else {
                                            hsla(0., 0., 0.5, 0.12)
                                        })
                                        .bg(if active {
                                            mix_color(accent(), 0.10)
                                        } else {
                                            let c: gpui_kit::Hsla = rgb(if crate::theme::is_dark() { 0x26262f } else { 0xffffff }).into();
                                            c
                                        })
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.pick_account(r2.clone(), cx)
                                        }))
                                        .child(
                                            div()
                                                .text_color(if active {
                                                    rgb(accent())
                                                } else {
                                                    rgb(subtle())
                                                })
                                                .child(if active { "●" } else { "○" }),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .truncate()
                                                .text_size(px(12.5))
                                                .child(format!("{name}{bal}")),
                                        )
                                        .when(failed, |el| {
                                            el.child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(red()))
                                                    .child(t("quota_exhausted")),
                                            )
                                        })
                                        .child(
                                            div()
                                                .px(px(8.)).py(px(3.)).rounded(px(7.))
                                                .bg(if active { mix_color(accent(), 0.18) } else { mix_color(subtle(), 0.14) })
                                                .text_size(px(11.))
                                                .text_color(if active { rgb(accent()) } else { rgb(subtle()) })
                                                .child(if active { t("current") } else { t("switch_account") }),
                                        )
                                        .into_any_element()
                                })
                                .collect(),
                        }),
                ),
            )
            // 模型测试卡
            .child(
                self.surface_card().p(px(16.)).child(
                    v_flex()
                        .gap(px(10.))
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!(
                                            "{} - {}",
                                            t("model_test"),
                                            t("model_test_hint")
                                        )),
                                )
                                .child(
                                    Button::new("gw-models-refresh")
                                        .label(t("refresh_models"))
                                        .small()
                                        .on_click(
                                            cx.listener(|this, _, _w, cx| this.fetch_models(cx)),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(8.))
                                .flex_wrap()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(subtle()))
                                        .child(t("model_placeholder")),
                                )
                                .children(vec![super::actions::simple_select(
                                    "sel-gwmodel",
                                    model_list
                                        .iter()
                                        .map(|m| (m.clone(), m.clone()))
                                        .collect::<Vec<_>>(),
                                    self.gw_test_model.as_deref().unwrap_or(""),
                                    "gwmodel",
                                    px(200.),
                                )]),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .children(self.gw_test_input.clone().map(|e| {
                                            vec![Input::new(&e).w_full().into_any_element()]
                                        }).unwrap_or_default()),
                                )
                                .child(
                                    Button::new("gw-test-send")
                                        .label(t("send_test"))
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            this.send_test_request(cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(subtle()))
                                .child(test_status.clone()),
                        )
                        .when(!test_result.is_empty(), |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .p(px(14.))
                                    .rounded(px(11.))
                                    .bg(hsla(0., 0., 0.5, 0.06))
                                    .text_size(px(13.))
                                    .child(test_result.clone()),
                            )
                        }),
                ),
            )
            // 请求日志卡
            .child(
                self.surface_card().p(px(16.)).child(
                    v_flex()
                        .gap(px(10.))
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(t("request_log")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(subtle()))
                                                .child(log_stats_text(&log_stats, logs.len())),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(8.))
                                        .child(
                                            Button::new("gw-logs-refresh")
                                                // busy 态：文案换成「刷新中…」并禁用，
                                                // 让点击有即时可感知的状态反馈
                                                .label(if self.gw_logs_busy { t("refreshing") } else { t("refresh") })
                                                .small()
                                                .disabled(self.gw_logs_busy)
                                                .on_click(cx.listener(|this, _, _w, cx| {
                                                    this.refresh_gateway(cx)
                                                })),
                                        )
                                        .when(log_total > 0, |el| {
                                            el.child(
                                                Button::new("gw-logs-clear")
                                                    .label(if self.gw_logs_busy { t("clearing") } else { t("clear_log") })
                                                    .small()
                                                    .text()
                                                    .text_color(rgb(red()))
                                                    .disabled(self.gw_logs_busy)
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.clear_gw_logs(cx)
                                                    })),
                                            )
                                        }),
                                ),
                        )
                        .children(if logs.is_empty() {
                            vec![div()
                                .py(px(16.))
                                .text_center()
                                .text_size(px(12.))
                                .text_color(rgb(subtle()))
                                .child(t("no_logs"))
                                .into_any_element()]
                        } else {
                            logs.iter()
                                .take(10)
                                .map(|l| {
                                    let ok = l.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                                    let model = l
                                        .get("model")
                                        .and_then(|x| x.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(String::from)
                                        .unwrap_or_else(|| t("no_model").to_string());
                                    let at = l
                                        .get("at")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let dur =
                                        l.get("duration").and_then(|x| x.as_i64()).unwrap_or(0);
                                    let platform =
                                        l.get("platform").and_then(|x| x.as_str()).unwrap_or("");
                                    let account =
                                        l.get("account").and_then(|x| x.as_str()).unwrap_or("");
                                    let message =
                                        l.get("message").and_then(|x| x.as_str()).unwrap_or("");
                                    h_flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .py(px(9.))
                                        .px(px(12.))
                                        .border_b_1()
                                        .border_color(hsla(0., 0., 0.5, 0.08))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(if ok { rgb(green()) } else { rgb(red()) })
                                                .child(if ok { "✓" } else { "✗" }),
                                        )
                                        .child(
                                            v_flex()
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_size(px(12.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(model),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(rgb(subtle()))
                                                        .overflow_hidden()
                                                        .truncate()
                                                        .child(format!(
                                                            "{}{} · {}{}{}",
                                                            fmt_log_time(&at),
                                                            fmt_log_duration(dur),
                                                            platform,
                                                            if account.is_empty() {
                                                                String::new()
                                                            } else {
                                                                format!(" · {account}")
                                                            },
                                                            if !ok && !message.is_empty() {
                                                                format!(" · {message}")
                                                            } else {
                                                                String::new()
                                                            }
                                                        )),
                                                ),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        }),
                ),
            )
            .into_any_element()
    }
}

fn log_stats_text(stats: &serde_json::Value, n: usize) -> String {
    let total = stats.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    let ok = stats.get("ok").and_then(|v| v.as_i64()).unwrap_or(0);
    let fail = stats.get("fail").and_then(|v| v.as_i64()).unwrap_or(0);
    tf(
        "log_stats",
        &[
            ("n", n.to_string()),
            ("t", total.to_string()),
            ("ok", ok.to_string()),
            ("fail", fail.to_string()),
        ],
    )
}

fn fmt_log_time(at: &str) -> String {
    if at.is_empty() {
        return "—".into();
    }
    at.replace('T', " ")
        .split('.')
        .next()
        .unwrap_or(at)
        .to_string()
}

fn fmt_log_duration(ms: i64) -> String {
    if ms <= 0 {
        return "—".into();
    }
    if ms < 1000 {
        format!(" · 耗时 {}ms", ms)
    } else {
        format!(" · 耗时 {:.2}s", ms as f64 / 1000.0)
    }
}
