use super::actions::simple_select;
use super::{
    mix_color, platform_name, st_color, st_kind, AccountRow, AppView, DayBar, MyFluent, Page,
    accent, green, red, SELECT_TX, subtle, token_blue,
};
use crate::backend;
use crate::i18n::{t, tf};
use crate::platform;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    switch::Switch,
    v_flex, ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
};
use gpui_kit::*;
use std::sync::Arc;
// =====================================================================
// 设置页：任务 / 通用（语言与外观用下拉框）/ 工具安装 / 数据与版本
// =====================================================================
impl AppView {
    pub(crate) fn render_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.settings_tab.clone();
        let st = self.settings.clone().unwrap_or_default();
        v_flex()
            .gap(px(12.))
            .child(self.page_header(t("nav_settings"), ""))
            // 开源仓库地址 + 免责说明：放在「设置」标题正下方
            .child(
                v_flex()
                    .gap(px(3.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div().text_size(px(11.5)).text_color(rgb(subtle()))
                                    .child(format!("{}：", t("open_source_repo"))),
                            )
                            .child(
                                div().text_size(px(11.5)).text_color(rgb(accent()))
                                    .child("https://github.com/tttq/credit_linked"),
                            ),
                    )
                    .child(
                        div().text_size(px(11.)).text_color(rgb(subtle()))
                            .child(t("disclaimer")),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(hsla(0., 0., 0.5, 0.12))
                    .flex_wrap()
                    .children(
                        [
                            ("tasks", "tab_tasks"),
                            ("general", "tab_general"),
                            ("tools", "tab_tools"),
                            ("data", "tab_data"),
                        ]
                        .iter()
                        .map(|(k, key)| {
                            Button::new(SharedString::from(format!("tab-{k}")))
                                .label(t(key))
                                .small()
                                .when(tab == *k, |b| b.primary().selected(true))
                            .when(tab != *k, |b| b.ghost())
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.settings_tab = k.to_string();
                                    if *k == "tools" {
                                        this.load_tools(cx);
                                    }
                                    if *k == "data" {
                                        this.update_check(cx);
                                    }
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .child(self.surface_card().p(px(18.)).child(match tab.as_str() {
                "tasks" => self.render_settings_tasks(&st, cx).into_any_element(),
                "general" => self.render_settings_general(cx).into_any_element(),
                "tools" => self.render_settings_tools(cx).into_any_element(),
                _ => self.render_settings_data(cx).into_any_element(),
            }))
            .into_any_element()
    }

    pub(crate) fn setting_row(
        &self,
        title: &str,
        desc: &str,
        control: impl gpui_kit::IntoElement,
    ) -> AnyElement {
        h_flex()
            .items_center()
            .gap(px(12.))
            .py(px(14.))
            .border_b_1()
            .border_color(hsla(0., 0., 0.5, 0.08))
            .child(
                v_flex()
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(subtle()))
                            .pt(px(4.))
                            .child(desc.to_string()),
                    ),
            )
            .child(control.into_any_element())
            .into_any_element()
    }

    pub(crate) fn render_settings_tasks(
        &mut self,
        st: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let auto = st.get("auto").and_then(|v| v.as_bool()).unwrap_or(true);
        let catchup = st.get("catchup").and_then(|v| v.as_bool()).unwrap_or(true);
        let random = st
            .get("randomTime")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let refresh = st
            .get("refreshMinutes")
            .and_then(|v| v.as_i64())
            .unwrap_or(60);
        let time = st
            .get("time")
            .and_then(|v| v.as_str())
            .unwrap_or("09:00")
            .to_string();
        let time_v = if random {
            t("random").to_string()
        } else {
            time.clone()
        };

        v_flex()
            .max_w_full()
            .child(self.setting_row(
                t("auto_checkin"),
                t("auto_checkin_desc"),
                Switch::new("set-auto").checked(auto).on_change(
                    cx.listener(|this, v: &bool, _w, cx| this.set_setting_bool("auto", *v, cx)),
                ),
            ))
            .child(self.setting_row(
                t("catchup"),
                t("catchup_desc"),
                Switch::new("set-catchup").checked(catchup).on_change(
                    cx.listener(|this, v: &bool, _w, cx| this.set_setting_bool("catchup", *v, cx)),
                ),
            ))
            .child(
                self.setting_row(
                    t("refresh_interval"),
                    t("refresh_interval_desc"),
                    h_flex()
                        .gap(px(2.))
                        .p(px(3.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(hsla(0., 0., 0.5, 0.12))
                        .children([(30i64, "m30"), (60, "m60"), (120, "m120")].iter().map(
                            |(n, key)| {
                                Button::new(SharedString::from(format!("ri-{n}")))
                                    .label(t(key))
                                    .small()
                                    .when(refresh == *n, |b| b.selected(true))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_setting_raw("refreshMinutes", n.to_string(), cx);
                                    }))
                            },
                        )),
                ),
            )
            .child(
                self.setting_row(
                    t("sched_time"),
                    t("sched_time_desc"),
                    h_flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(px(9.))
                                .border_1()
                                .border_color(hsla(0., 0., 0.5, 0.15))
                                .text_size(px(13.))
                                .child(time.clone()),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().text_size(px(11.)).child(t("random_time")))
                                .child(Switch::new("set-random").checked(random).on_change(
                                    cx.listener(|this, v: &bool, _w, cx| {
                                        this.set_setting_bool("randomTime", *v, cx)
                                    }),
                                )),
                        ),
                ),
            )
            .child(
                v_flex()
                    .pt(px(10.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("exec_preview")),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(subtle()))
                            .child(tf("saved_time_is", &[("v", time_v.clone())])),
                    )
                    .children(self.accounts.iter().take(8).map(|a| {
                        h_flex()
                            .items_center()
                            .gap(px(8.))
                            .py(px(6.))
                            .child(self.picon(&a.provider, px(22.)))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(platform_name(&a.provider)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(subtle()))
                                    .child(tf("every_day", &[("v", time_v.clone())])),
                            )
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .mt(px(8.))
                            .p(px(10.))
                            .px(px(13.))
                            .rounded(px(10.))
                            .bg(mix_color(accent(), 0.09))
                            .text_size(px(11.5))
                            .text_color(rgb(subtle()))
                            .child(t("notice_beijing")),
                    ),
            )
            .into_any_element()
    }

    /// 通用设置：语言与外观均为下拉框（对齐原版 select）
    pub(crate) fn render_settings_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let loc = crate::i18n::locale().to_string();
        let theme_mode = self
            .settings
            .as_ref()
            .map(|s| {
                s.get("theme")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system")
                    .to_string()
            })
            .unwrap_or_else(|| "system".into());
        let telemetry = self
            .settings
            .as_ref()
            .map(|s| {
                s.get("telemetry")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .unwrap_or(false);

        v_flex()
            .max_w_full()
            .child(self.setting_row(
                t("language"),
                "",
                simple_select(
                    "sel-language",
                    vec![
                        ("zh-CN".into(), t("lang_zh").to_string()),
                        ("en-US".into(), t("lang_en").to_string()),
                    ],
                    &loc,
                    "lang",
                    px(120.),
                ),
            ))
            .child(self.setting_row(
                t("appearance"),
                "",
                simple_select(
                    "sel-appearance",
                    vec![
                        ("system".into(), t("theme_system").to_string()),
                        ("light".into(), t("theme_light").to_string()),
                        ("dark".into(), t("theme_dark").to_string()),
                    ],
                    &theme_mode,
                    "theme",
                    px(100.),
                ),
            ))
            .child(self.setting_row(
                t("base_usage_stats"),
                "",
                Switch::new("set-telemetry").checked(telemetry).on_change(
                    cx.listener(|this, v: &bool, _w, cx| {
                        this.set_setting_bool("telemetry", *v, cx)
                    }),
                ),
            ))
            .child(
                div()
                    .mt(px(8.))
                    .p(px(10.))
                    .px(px(13.))
                    .rounded(px(10.))
                    .bg(mix_color(accent(), 0.09))
                    .text_size(px(11.5))
                    .text_color(rgb(subtle()))
                    .child(t("privacy_notice")),
            )
            .into_any_element()
    }

    pub(crate) fn render_settings_tools(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(tools) = self.tools.clone() else {
            return v_flex()
                .py(px(22.))
                .child(
                    div()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(rgb(subtle()))
                        .child(t("detect_state")),
                )
                .into_any_element();
        };
        let items = tools
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let dl_dir = tools
            .get("installDir")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let downloads = tools
            .get("downloads")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut rows: Vec<AnyElement> = Vec::new();
        // 安装包下载路径：标题在上，输入框在下（对齐要求）
        let path_display = if dl_dir.is_empty() {
            t("tools_dl_path_ph").to_string()
        } else {
            dl_dir.clone()
        };
        let dir_for_open = dl_dir.clone();
        rows.push(
            v_flex()
                .w_full()
                .gap(px(6.))
                .py(px(12.))
                .border_b_1()
                .border_color(hsla(0., 0., 0.5, 0.08))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(t("tools_dl_path")),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(6.))
                        .w_full()
                        .child(
                            self.input_tools_dir.clone().map(|e| Input::new(&e).flex_1().min_w_0().into_any_element()).unwrap_or_else(|| div().into_any_element()),
                        )
                        .child(
                            Button::new("tools-dir-open")
                                .label(t("open_dir"))
                                .small()
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    let d = dir_for_open.clone();
                                    this.open_installer_dir(d, cx);
                                })),
                        )
                        .child(
                            Button::new("tools-dir-change")
                                .label(t("save"))
                                .primary()
                                .small()
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.save_tools_dir(cx);
                                })),
                        ),
                )
                .into_any_element(),
        );
        for it in &items {
            let p = it
                .get("platform")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let direct = it.get("direct").and_then(|x| x.as_bool()).unwrap_or(false);
            let installed = it
                .get("installed")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let accounts_n = it.get("accounts").and_then(|x| x.as_i64()).unwrap_or(0);
            let quota_total = it.get("quotaTotal").and_then(|x| x.as_f64());
            let dl = downloads
                .iter()
                .find(|d| d.get("platform").and_then(|x| x.as_str()) == Some(p.as_str()));
            let dl_status = dl
                .and_then(|d| d.get("status").and_then(|x| x.as_str()))
                .unwrap_or("");
            let dl_prog = dl
                .and_then(|d| d.get("progress").and_then(|x| x.as_i64()))
                .unwrap_or(0);
            let st_txt = match dl_status {
                "downloading" => tf("dl_state_downloading", &[("n", dl_prog.to_string())]),
                "downloaded" => t("dl_state_downloaded").to_string(),
                _ => String::new(),
            };
            let qtxt = if accounts_n > 0 {
                format!(
                    "{}{}",
                    tf("installed_accounts", &[("n", accounts_n.to_string())]),
                    quota_total
                        .map(|q| format!("{}{}", t("remaining_quota"), crate::i18n::fmt_num(q)))
                        .unwrap_or_default()
                )
            } else {
                t("no_account_added").to_string()
            };
            let name_key = format!("tool_{p}");
            let name = if t(&name_key) == name_key {
                p.clone()
            } else {
                t(&name_key).to_string()
            };
            let p_dl = p.clone();
            let p_page = p.clone();
            let p_dir = p.clone();
            rows.push(
                h_flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(14.))
                    .flex_wrap()
                    .border_b_1()
                    .border_color(hsla(0., 0., 0.5, 0.08))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_1()
                            .min_w(px(200.))
                            .child(self.picon(&p, px(24.)))
                            .child(
                                v_flex().min_w_0().child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(name),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(6.))
                            .flex_wrap()
                            .justify_end()
                            .child(
                                v_flex().items_end().gap(px(3.)).min_w(px(96.))
                                    // 下载中：进度条 + 百分比，用户能直观看到进度
                                    .when(dl_status == "downloading", |el| {
                                        el.child(
                                            div().w(px(96.)).child(
                                                gpui_kit::component::progress::Progress::new(
                                                    SharedString::from(format!("dl-p-{p}")),
                                                )
                                                .value(dl_prog.max(0).min(100) as f32 / 100.0),
                                            ),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if installed { rgb(green()) } else { rgb(subtle()) })
                                            .child(format!(
                                                "{}{}",
                                                if installed {
                                                    t("installed_yes")
                                                } else {
                                                    t("not_detected")
                                                },
                                                st_txt
                                            )),
                                    ),
                            )
                            .when(direct, |el| {
                                el.child(
                                    Button::new(SharedString::from(format!("tdl-{}", p.clone())))
                                        .label(if installed {
                                            t("reinstall")
                                        } else {
                                            t("one_click_install")
                                        })
                                        .small()
                                        .when(!installed, |b| b.primary())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.download_tool(p_dl.clone(), cx)
                                        })),
                                )
                            })
                            .when(!direct, |el| {
                                el.child(
                                    Button::new(SharedString::from(format!("tpage-{}", p.clone())))
                                        .label(t("open_dl_page"))
                                        .small()
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.open_tool_page(p_page.clone(), cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new(SharedString::from(format!("tdir-{}", p.clone())))
                                    .label(t("specify_dir"))
                                    .small()
                                    .tooltip(t("specify_dir_tip"))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_tool_dir(p_dir.clone(), cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .mt(px(8.))
                .p(px(10.))
                .px(px(13.))
                .rounded(px(10.))
                .bg(mix_color(accent(), 0.09))
                .text_size(px(11.5))
                .text_color(rgb(subtle()))
                .child(t("tools_notice"))
                .into_any_element(),
        );
        v_flex().max_w_full().children(rows).into_any_element()
    }

    pub(crate) fn render_settings_data(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let upd = self.update_info.clone();
        let cur_ver = upd
            .as_ref()
            .and_then(|u| u.get("currentVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string();
        let has_update = upd
            .as_ref()
            .map(|u| {
                u.get("updateAvailable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        let asset = upd
            .as_ref()
            .and_then(|u| u.get("assetUrl"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let release_url = upd
            .as_ref()
            .and_then(|u| u.get("releaseUrl"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = self.update_status.clone();
        let downloaded = self.update_file.is_some();
        let has_asset = !asset.is_empty();
        let asset_dl = asset.clone();
        let release_empty = release_url.is_empty();
        let release_open = release_url.clone();

        v_flex()
            .max_w_full()
            .child(
                self.setting_row(
                    &format!("{} v{}", t("app_name"), cur_ver),
                    t("app_version"),
                    div()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(7.))
                        .bg(mix_color(accent(), 0.10))
                        .text_color(rgb(accent()))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("local_edition")),
                ),
            )
            .child(
                self.setting_row(
                    t("software_update"),
                    &status,
                    h_flex()
                        .gap(px(6.))
                        .items_center()
                        .flex_wrap()
                        .justify_end()
                        .when_some(
                            (!self.update_progress.is_empty())
                                .then(|| self.update_progress.clone()),
                            |el, p| {
                                el.child(div().text_size(px(11.5)).text_color(rgb(subtle())).child(p))
                            },
                        )
                        .child(
                            Button::new("upd-check")
                                .label(t("recheck"))
                                .small()
                                .on_click(cx.listener(|this, _, _w, cx| this.update_check(cx))),
                        )
                        .when(has_update && has_asset && !downloaded, |el| {
                            el.child(
                                Button::new("upd-dl")
                                    .label(t("download_update"))
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.update_download(asset_dl.clone(), cx)
                                    })),
                            )
                        })
                        .when(downloaded, |el| {
                            el.child(
                                Button::new("upd-inst")
                                    .label(t("restart_install"))
                                    .primary()
                                    .small()
                                    .on_click(
                                        cx.listener(|this, _, _w, cx| this.update_install(cx)),
                                    ),
                            )
                        })
                        .when(has_update && !has_asset && !release_empty, |el| {
                            el.child(
                                Button::new("upd-open")
                                    .label(t("goto_release"))
                                    .small()
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.open_release(release_open.clone(), cx)
                                    })),
                            )
                        }),
                ),
            )
            .child(
                self.setting_row(
                    t("data_dir"),
                    t("data_dir_desc"),
                    Button::new("db-open")
                        .label(t("open_dir"))
                        .small()
                        .on_click(cx.listener(|this, _, _w, cx| {
                            let n = this.accounts.len();
                            this.toast(tf("accounts_records", &[("n", n.to_string())]), cx);
                        })),
                ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .p(px(10.))
                    .px(px(13.))
                    .rounded(px(10.))
                    .bg(mix_color(accent(), 0.09))
                    .text_size(px(11.5))
                    .text_color(rgb(subtle()))
                    .child(t("db_notice")),
            )
            .into_any_element()
    }
}
