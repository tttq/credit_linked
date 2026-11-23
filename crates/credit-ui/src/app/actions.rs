use super::{
    accent, mix_color, MyFluent, AppView, SELECT_TX,
};
use crate::backend;
use crate::i18n::{t, tf};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    h_flex, v_flex,
    Sizable as _,
};
use gpui_kit::*;
// =====================================================================
// 动作方法：全部业务交互（backend 调用一律经 backend::bg 派发到全局 runtime）
// =====================================================================
impl AppView {
    pub fn checkin_all(&mut self, cx: &mut Context<Self>) {
        self.toast(t("checkin_running"), cx);
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async { backend::checkin(String::new(), String::new(), true).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => {
                        let results = res.get("results").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                        let ok = results.iter().filter(|x| x.get("status").and_then(|s| s.as_str()) == Some("success")).count();
                        let al = results.iter().filter(|x| x.get("status").and_then(|s| s.as_str()) == Some("already")).count();
                        let bad = results.iter().filter(|x| x.get("status").and_then(|s| s.as_str()) == Some("failed")).count();
                        v.toast(tf("checkin_done_stats", &[("ok", ok.to_string()), ("al", al.to_string()), ("bad", bad.to_string())]), cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("checkin_failed"), e), cx),
                }
                v.refresh_all(cx);
            }).ok();
        }).detach();
    }

    pub fn checkin_one(&mut self, provider: String, uid: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::checkin(provider, uid, false).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => {
                        if let Some(o) = res.get("results").and_then(|x| x.as_array()).and_then(|a| a.first()) {
                            let status = o.get("status").and_then(|s| s.as_str()).unwrap_or("");
                            let amount = o.get("amount").and_then(|a| a.as_f64());
                            let msg = o.get("message").and_then(|m| m.as_str()).unwrap_or("");
                            let mut text = match status {
                                "success" => format!("{} +{}", t("claim_ok"), amount.map(crate::i18n::fmt_num).unwrap_or_default()),
                                "already" => t("already_claimed").to_string(),
                                s => s.to_string(),
                            };
                            if !msg.is_empty() { text.push_str(&format!(" · {msg}")); }
                            v.toast(text, cx);
                        }
                    }
                    Err(e) => v.toast(format!("{}{}", t("op_failed"), e), cx),
                }
                v.refresh_all(cx);
            }).ok();
        }).detach();
    }

    pub fn quota_all(&mut self, cx: &mut Context<Self>) {
        self.toast(t("refreshing"), cx);
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async { backend::quota_refresh_all().await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => v.toast(t("balance_refreshed"), cx),
                    Err(e) => v.toast(format!("{}{}", t("refresh_failed"), e), cx),
                }
                v.refresh_all(cx);
            }).ok();
        }).detach();
    }

    pub fn refresh_quota(&mut self, provider: String, uid: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::quota_refresh(provider, uid).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => { v.toast(t("quota_refreshed"), cx); v.refresh_all(cx); }
                    Err(e) => v.toast(format!("{}{}", t("op_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    pub fn toggle_auto(&mut self, provider: String, uid: String, auto: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::account_toggle_auto(provider, uid, auto).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => v.toast(if auto { t("auto_on") } else { t("auto_off") }, cx),
                    Err(e) => v.toast(format!("{}{}", t("set_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    pub fn import_local(&mut self, platform: &'static str, cx: &mut Context<Self>) {
        self.toast(t("scanning"), cx);
        cx.spawn(async move |this, cx| {
            let plat = if platform == "workbuddy" { "wb" } else { platform };
            let r = backend::bg(async move { backend::import(plat.to_string(), false).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => {
                        let who = res.get("name").and_then(|n| n.as_str())
                            .or_else(|| res.get("uid").and_then(|u| u.as_str())).unwrap_or("");
                        v.toast(format!("{}{}", t("import_ok"), who), cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("import_failed"), e), cx),
                }
                v.refresh_all(cx);
            }).ok();
        }).detach();
    }

    pub fn confirm_delete(&mut self, provider: String, uid: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::account_delete(provider, uid).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => { v.toast(t("deleted"), cx); v.refresh_all(cx); }
                    Err(e) => v.toast(format!("{}{}", t("op_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    pub fn start_login(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async { backend::login_start(provider.to_string()).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            if let Ok(res) = r {
                let sid = res.get("sessionId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let url = res.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // Chrome/Edge app 模式小窗（480×680 无地址栏）；退回系统浏览器时也能用（引号已修复）
                let _ = backend::bg(async move { backend::login_open_window(url).await }).await;
                // 轮询至终态（sleep 放进 runtime，避免阻塞 gpui 执行器）
                loop {
                    let sid_loop = sid.clone();
                    let poll = backend::bg(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        backend::login_poll(sid_loop).await
                    }).await.unwrap_or_else(|_| Err("task aborted".into()));
                    let mut done = false;
                    let mut ok = false;
                    if let Ok(p) = &poll {
                        match p.get("status").and_then(|s| s.as_str()).unwrap_or("") {
                            "ready" => { done = true; ok = true; }
                            "error" | "expired" => { done = true; }
                            _ => {}
                        }
                    } else { done = true; }
                    if done {
                        // 终态：自动关闭登录小窗（成功取到凭据 / 失败 / 超时都关）
                        let _ = backend::bg(async { backend::login_close_window().await }).await;
                    }
                    let poll_v = poll.ok();
                    this.update(cx, move |v, cx| {
                        if done {
                            if ok {
                                let acct = poll_v.as_ref().and_then(|p| p.get("account")).cloned();
                                let name = acct.as_ref().and_then(|a| a.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                                let (prov2, uid2) = acct.as_ref().map(|a| (
                                    a.get("provider").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                    a.get("uid").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                )).unwrap_or_default();
                                v.toast(tf("login_ok_toast", &[("n", name)]), cx);
                                if !prov2.is_empty() {
                                    cx.spawn(async move |this, cx| {
                                        let _ = backend::bg(async move { backend::quota_refresh(prov2, uid2).await }).await;
                                        this.update(cx, |v, cx| v.refresh_all(cx)).ok();
                                    }).detach();
                                }
                            } else {
                                let msg = poll_v.as_ref().and_then(|p| p.get("error")).and_then(|e| e.as_str()).unwrap_or("").to_string();
                                v.toast(if msg.is_empty() { t("auth_timeout").to_string() } else { format!("{}: {msg}", t("login_failed")) }, cx);
                            }
                            v.login_seq = None;
                            v.refresh_all(cx);
                        }
                    }).ok();
                    if done { break; }
                }
            } else {
                // 发起失败：关掉可能已开的登录窗
                let _ = backend::bg(async { backend::login_close_window().await }).await;
                this.update(cx, |v, cx| {
                    let msg = r.err().unwrap_or_else(|| "task aborted".into());
                    v.toast(format!("{}: {msg}", t("login_failed")), cx);
                }).ok();
            }
        }).detach();
    }

    /// Trae 设备号弹框：列出本机已注册设备号（可一键选用），也可手动输入。
    /// InputState 在回调内创建（绝不在 render 内 cx.new）。
    /// Trae 设备号弹框：输入或点选本机已注册设备号。
    /// 设备列表来自 AppView 缓存（启动/刷新账号页时预取），此处**不做 await**。
    pub fn open_device_dialog(&mut self, provider: String, uid: String, window: &mut Window, cx: &mut Context<Self>) {
        let devs = self.dev_devices.clone();
        let input = cx.new(|cx| InputState::new(window, cx));
        let weak = cx.weak_entity();
        let (p_back, u_back) = (provider.clone(), uid.clone());
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let inp = input.clone();
            let w1 = weak.clone();
            let (p1, u1) = (p_back.clone(), u_back.clone());
            let mut chips: Vec<AnyElement> = Vec::new();
            for d in devs.iter().take(12) {
                let d2 = d.clone();
                let inp2 = inp.clone();
                chips.push(
                    Button::new(SharedString::from(format!("dev-{d}")))
                        .label(d.clone())
                        .small()
                        .ghost()
                        .on_click(move |_, window, cx| {
                            inp2.update(cx, |st, cx| st.set_value(d2.clone(), window, cx));
                        })
                        .into_any_element(),
                );
            }
            let inp_clear = inp.clone();
            let w2 = w1.clone();
            let (p2, u2) = (p1.clone(), u1.clone());
            dialog
                .title(t("device_btn"))
                .child(
                    v_flex().gap(px(8.)).w_full()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(crate::theme::current_palette().3))
                                .child(t("device_btn_tip")),
                        )
                        .child(Input::new(&inp).w_full())
                        .when(!chips.is_empty(), |el| {
                            el.child(
                                v_flex().gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(crate::theme::current_palette().3))
                                            .child(t("device_local_list")),
                                    )
                                    .child(h_flex().gap(px(4.)).flex_wrap().children(chips)),
                            )
                        }),
                )
                .footer(
                    h_flex().gap(px(8.)).justify_end()
                        .child(
                            Button::new("dev-clear").label(t("device_clear")).small().on_click(
                                move |_, window, cx| {
                                    inp_clear.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                                },
                            ),
                        )
                        .child(
                            Button::new("dev-cancel").label(t("cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("dev-save").label(t("save")).primary().on_click(
                                move |_, window, cx| {
                                    let val = inp.read(cx).value().trim().to_string();
                                    let (pp, uu) = (p2.clone(), u2.clone());
                                    let _ = w2.update(cx, |v, cx| v.apply_device(pp, uu, val, cx));
                                    window.close_dialog(cx);
                                },
                            ),
                        ),
                )
        });
    }

    /// 预取本机 Trae 已注册设备号（供设备号弹框点选；避免弹框内 await）。
    /// 在 refresh_all 与账号页打开时调用，保证弹框始终有可用列表。
    pub fn refresh_local_devices(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let devices = backend::bg(async { backend::trae_local_devices().await })
                .await
                .unwrap_or(Ok(Vec::new()))
                .unwrap_or_default();
            this.update(cx, |v, cx| {
                v.dev_devices = devices;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 保存 Trae 设备号（后台执行，完成后刷新账号列表）
    pub fn apply_device(&mut self, provider: String, uid: String, device_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::trae_set_device(provider, uid, device_id).await })
                .await
                .unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => { v.toast(t("device_saved"), cx); v.refresh_all(cx); }
                    Err(e) => v.toast(format!("{}{}", t("save_failed"), e), cx),
                }
            })
            .ok();
        })
        .detach();
    }
    pub fn set_setting_bool(&mut self, key: &'static str, value: bool, cx: &mut Context<Self>) {
        let v = if value { "1" } else { "0" };
        self.set_setting_raw(key, v.to_string(), cx);
    }

    pub fn set_setting_raw(&mut self, key: &'static str, value: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = backend::bg(async move { backend::settings_set(key.to_string(), value).await }).await;
            this.update(cx, |v, cx| { v.refresh_all(cx); }).ok();
        }).detach();
    }

    pub fn set_theme_mode(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        use gpui_kit::component::{Theme, ThemeMode};
        // system → 跟随 Windows 个性化开关（winreg 直读 DWORD，见 theme::resolve_dark）
        let dark = crate::theme::resolve_dark(mode);
        let m = if dark { ThemeMode::Dark } else { ThemeMode::Light };
        Theme::update(cx, |theme| { theme.mode = m; });
        crate::theme::apply(cx);
        // 记住当前选择（system/light/dark），供下拉框显示与启动恢复
        let mode_s = mode.to_string();
        cx.spawn(async move |this, cx| {
            let _ = backend::bg(async move { backend::settings_set("theme".into(), mode_s).await }).await;
            this.update(cx, |v, cx| v.refresh_all(cx)).ok();
        }).detach();
        cx.notify();
    }

    // ---- 网关动作 ----

    pub fn save_gw_port(&mut self, cx: &mut Context<Self>) {
        let port_opt = self.gw_port_input.as_ref()
            .map(|e| e.read(cx).value().trim().to_string())
            .and_then(|v| v.parse::<i64>().ok());
        if port_opt.is_none() || !(1..=65535).contains(port_opt.as_ref().unwrap()) { self.toast(t("port_invalid"), cx); return; }
        let port = *port_opt.as_ref().unwrap();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::gateway_set_limits(Some(port), None, None).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => v.toast(if res.get("needsRestart").and_then(|x| x.as_bool()).unwrap_or(false) {
                        t("port_saved_restart") } else { t("port_saved") }, cx),
                    Err(e) => v.toast(format!("{}{}", t("save_failed"), e), cx),
                }
                v.refresh_gateway(cx);
            }).ok();
        }).detach();
    }
    pub fn save_gw_rpm(&mut self, cx: &mut Context<Self>) {
        let rpm = self.gw_rpm_input.as_ref()
            .map(|e| e.read(cx).value().trim().to_string())
            .and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::gateway_set_limits(None, Some(rpm), None).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r { Ok(_) => v.toast(t("rate_saved"), cx), Err(e) => v.toast(format!("{}{}", t("save_failed"), e), cx) }
                v.refresh_gateway(cx);
            }).ok();
        }).detach();
    }

    pub fn set_gw_strict(&mut self, strict: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::gateway_set_limits(None, None, Some(strict)).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => v.toast(if strict { t("strict_on") } else { t("strict_off") }, cx),
                    Err(e) => v.toast(format!("{}{}", t("set_failed"), e), cx),
                }
                v.refresh_gateway(cx);
            }).ok();
        }).detach();
    }

    pub fn switch_gw_platform(&mut self, platform: &str, cx: &mut Context<Self>) {
        let ch = platform.to_string();
        let label = Self::gw_platform_label(&ch);
        cx.spawn(async move |this, cx| {
            let (er, r) = backend::bg(async move {
                let er = backend::platform_ensure_running(ch.clone()).await;
                let r = backend::gateway_set_platform(ch).await;
                (er, r)
            }).await.unwrap_or_else(|_| (Ok(serde_json::Value::Null), Err("task aborted".into())));
            this.update(cx, |v, cx| {
                if let Ok(e) = er {
                    if e.get("launched").and_then(|x| x.as_bool()).unwrap_or(false) {
                        v.toast(tf("client_not_running", &[("p", label.clone())]), cx);
                    }
                }
                match r {
                    Ok(_) => {
                        v.toast(format!("{}{}", t("switched_platform"), label), cx);
                        // 切换平台后，同时刷新网关状态和主账号列表（确保所有平台数据同步）
                        v.refresh_gateway(cx);
                        v.refresh_all(cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("switch_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    pub fn pick_account(&mut self, account_ref: String, cx: &mut Context<Self>) {
        let ref_parse = account_ref.clone();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::gateway_switch_account(Some(account_ref)).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            let mut verify = String::new();
            if r.is_ok() {
                let mut it = ref_parse.splitn(2, ':');
                let (prov, uid) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
                if let Ok(Ok(q)) = backend::bg(async move { backend::quota_refresh(prov, uid).await }).await {
                    if let Some(rem) = q.get("remaining").and_then(|x| x.as_f64()) {
                        verify = format!("{}{}", t("balance"), crate::i18n::fmt_num(rem));
                    }
                }
            }
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => { 
                        v.toast(format!("{}{}", t("switched_account"), verify), cx); 
                        // 切换账号后，同时刷新网关状态和主账号列表（确保所有平台数据同步）
                        v.refresh_gateway(cx);
                        v.refresh_all(cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("switch_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    pub fn switch_next_account(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.gw_accounts.clone() else { return };
        let accounts = d.get("accounts").and_then(|a| a.as_array()).cloned().unwrap_or_default();
        if accounts.is_empty() { self.toast(t("no_accounts_current"), cx); return; }
        let cur = accounts.iter().position(|a| a.get("active").and_then(|x| x.as_bool()).unwrap_or(false));
        let next_idx = cur.map(|c| (c + 1) % accounts.len()).unwrap_or(0);
        let r#ref = accounts[next_idx].get("ref").and_then(|x| x.as_str()).unwrap_or("").to_string();
        self.pick_account(r#ref, cx);
    }

    pub fn fetch_models(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.gw_status.clone() else { return };
        let base = st.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
        self.gw_test_status = t("fetching_models").to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let models: Vec<String> = backend::bg(async move {
                let client = reqwest::Client::new();
                let resp = client.get(format!("{base}/v1/models")).send().await;
                let json = match resp {
                    Ok(r) => r.json::<serde_json::Value>().await.ok(),
                    Err(_) => None,
                };
                json.and_then(|j| j.get("data").and_then(|d| d.as_array()).cloned())
                    .map(|arr| arr.iter().filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(String::from)).collect())
                    .unwrap_or_default()
            }).await.unwrap_or_default();
            this.update(cx, |v, cx| {
                if models.is_empty() {
                    v.gw_test_status = t("no_accounts_or_expired").to_string();
                } else {
                    v.gw_test_status = tf("n_models", &[("n", models.len().to_string())]);
                }
                v.gw_models = models;
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn send_test_request(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.gw_test_model.clone() else { self.toast(t("select_model_first"), cx); return; };
        let Some(input) = self.gw_test_input.clone() else { self.toast(t("input_test_msg"), cx); return; };
        let msg = input.read(cx).value().to_string();
        if msg.trim().is_empty() { self.toast(t("input_test_msg"), cx); return; }
        let Some(st) = self.gw_status.clone() else { return };
        let base = st.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let api_key = st.get("apiKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
        self.gw_test_status = t("requesting").to_string();
        self.gw_test_result = t("waiting_response").to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (secs, payload) = backend::bg(async move {
                let t0 = std::time::Instant::now();
                let client = reqwest::Client::new();
                let mut req = client.post(format!("{base}/v1/chat/completions"))
                    .json(&serde_json::json!({
                        "model": model, "messages": [{"role": "user", "content": msg}], "stream": false
                    }));
                if !api_key.is_empty() { req = req.bearer_auth(&api_key); }
                let payload = match req.send().await {
                    Ok(r) => match r.json::<serde_json::Value>().await {
                        Ok(v) => Ok(v),
                        Err(e) => Err(format!("{}{}", t("request_error"), e)),
                    },
                    Err(e) => Err(format!("{}{}", t("request_error"), e)),
                };
                (t0.elapsed().as_secs_f64(), payload)
            }).await.unwrap_or_else(|_| (0.0, Err(format!("{}task aborted", t("request_error")))));
            this.update(cx, |v, cx| {
                match payload {
                    Ok(data) => {
                        if let Some(err) = data.get("error") {
                            v.gw_test_result = format!("{}{}", tf("test_failed", &[("s", format!("{secs:.1}"))]),
                                err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string());
                        } else {
                            let content = data.pointer("/choices/0/message/content")
                                .and_then(|c| c.as_str()).unwrap_or("").to_string();
                            let usage = data.get("usage").cloned().unwrap_or_default();
                            let comp = usage.get("completion_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                            let speed = if comp > 0 && secs > 0.0 { format!("{:.1} tok/s", comp as f64 / secs) } else { "—".into() };
                            let usage_text = if usage.get("total_tokens").is_some() {
                                let p = usage.get("prompt_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                                let t_ = usage.get("total_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                                format!("{}{}", tf("usage_tokens", &[("in", p.to_string()), ("out", comp.to_string()), ("total", t_.to_string())]), speed)
                            } else { String::new() };
                            v.gw_test_result = format!("{}{}\n\n{}", tf("test_ok", &[("s", format!("{secs:.1}")), ("usage", usage_text)]), "", content);
                        }
                        v.gw_test_status = String::new();
                    }
                    Err(e) => { v.gw_test_result = e; v.gw_test_status = String::new(); }
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn clear_gw_logs(&mut self, cx: &mut Context<Self>) {
        // busy 态立即反馈：清空按钮变「清空中…」并禁用（刷新按钮共用该标志）
        self.gw_logs_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async { backend::gateway_clear_logs().await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                v.gw_logs_busy = false;
                match r {
                    Ok(res) => {
                        let n = res.get("cleared").and_then(|x| x.as_i64()).unwrap_or(0);
                        v.toast(tf("cleared_n", &[("n", n.to_string())]), cx);
                        v.refresh_gateway(cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("clear_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    // ---- 工具安装 ----
    pub fn load_tools(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let tools = backend::bg(async {
                let _ = backend::tools_refresh_minimax_url().await;
                backend::tools_info().await
            }).await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                v.tools = tools.ok();
                cx.notify();
            }).ok();
        }).detach();
    }

    /// 下载进度轮询：只要有平台处于 downloading，就每 800ms 刷新一次工具信息。
    /// 全部结束后自动停止（最多轮询 15 分钟，避免异常状态常驻）。
    pub fn tools_progress_poll(&mut self, cx: &mut Context<Self>) {
        if self.tools_poll_running { return; }
        self.tools_poll_running = true;
        let weak = cx.weak_entity();
        cx.spawn(async move |_this, cx| {
            for _ in 0..1125 {
                let slept = backend::bg(async {
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                }).await;
                if slept.is_err() { break; }
                let still = weak.update(cx, |v, cx| {
                    v.load_tools(cx);
                    let running = v.tools.as_ref()
                        .and_then(|t| t.get("downloads"))
                        .and_then(|d| d.as_array())
                        .map(|arr| arr.iter().any(|d| {
                            d.get("status").and_then(|s| s.as_str()) == Some("downloading")
                        }))
                        .unwrap_or(false);
                    if !running { v.tools_poll_running = false; }
                    running
                });
                match still {
                    Ok(true) => continue,
                    _ => break,
                }
            }
        }).detach();
    }

    pub fn download_tool(&mut self, platform: String, cx: &mut Context<Self>) {
        self.toast(t("processing"), cx);
        // 开始下载即启动进度轮询：每 800ms 拉一次 tools_info，进度条实时更新
        self.tools_progress_poll(cx);
        let install_dir = self.tools.as_ref()
            .and_then(|t| t.get("installDir")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let platform_late = platform.clone();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::tools_download(platform, install_dir, None).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            let open_page: Option<String> = match &r {
                Ok(res) if !res.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) => {
                    let page = res.get("openPage").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if page.is_empty() { None } else { Some(page) }
                }
                _ => None,
            };
            if let Some(page) = open_page {
                let _ = backend::bg(async move { backend::open_url(page).await }).await;
            }
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => {
                        if res.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
                            let file = res.get("file").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let name = t(match platform_late.as_str() {
                                "trae" => "tool_trae", "workbuddy" => "tool_workbuddy", "minimax" => "tool_minimax",
                                "qoder" => "tool_qoder", "zcode" => "tool_zcode", _ => "tool_codearts",
                            }).to_string();
                            let name2 = name.clone();
                            let p3 = platform_late.clone();
                            cx.spawn(async move |this, cx| {
                                let ir = backend::bg(async move { backend::tools_install(p3, file).await })
                                    .await.unwrap_or_else(|_| Err("task aborted".into()));
                                this.update(cx, |v, cx| {
                                    match ir {
                                        Ok(_) => v.toast(tf("silent_install_started", &[("name", name2)]), cx),
                                        Err(e) => v.toast(format!("{}{}", t("dl_install_failed"), e), cx),
                                    }
                                    v.load_tools(cx);
                                }).ok();
                            }).detach();
                        } else {
                            v.toast(t("no_direct_open_page"), cx);
                        }
                        v.load_tools(cx);
                    }
                    Err(e) => v.toast(format!("{}{}", t("dl_install_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    /// 保存安装包下载目录（输入框路径 → settings，供后续下载使用）
    pub fn save_tools_dir(&mut self, cx: &mut Context<Self>) {
        let dir = self.input_tools_dir.as_ref()
            .map(|e| e.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if dir.is_empty() { self.toast(t("specify_dir_tip"), cx); return; }
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::settings_set("toolsInstallDir".into(), dir).await }).await;
            this.update(cx, |v, cx| {
                match r {
                    Ok(Ok(_)) => { v.toast(t("settings_saved"), cx); v.load_tools(cx); }
                    _ => v.toast(t("save_failed"), cx),
                }
            }).ok();
        }).detach();
    }

    /// 打开安装包下载目录（系统文件管理器）
    pub fn open_installer_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        if dir.trim().is_empty() {
            self.toast(t("tools_dl_path_ph"), cx);
            return;
        }
        cx.spawn(async move |_this, _cx| {
            let _ = backend::bg(async move { backend::open_url(dir).await }).await;
        }).detach();
    }

    pub fn open_tool_page(&mut self, platform: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::tools_open_page(platform).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r { Ok(_) => v.toast(t("dl_page_opened"), cx), Err(e) => v.toast(format!("{}{}", t("open_failed"), e), cx) }
            }).ok();
        }).detach();
    }

    pub fn set_tool_dir(&mut self, platform: String, cx: &mut Context<Self>) {
        let p = platform.clone();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::tools_set_install_dir(p, String::new()).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(_) => { v.toast(t("install_dir_cleared"), cx); v.load_tools(cx); }
                    Err(e) => v.toast(format!("{}{}", t("import_failed"), e), cx),
                }
            }).ok();
        }).detach();
    }

    // ---- 自动更新 ----
    pub fn update_check(&mut self, cx: &mut Context<Self>) {
        self.update_status = t("checking_update").to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async { backend::check_update().await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(info) => {
                        let avail = info.get("updateAvailable").and_then(|x| x.as_bool()).unwrap_or(false);
                        let latest = info.get("latestVersion").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let cur = info.get("currentVersion").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        v.update_info = Some(info);
                        v.update_status = if avail {
                            tf("new_version_found", &[("latest", latest), ("current", cur)])
                        } else {
                            tf("already_latest", &[("v", cur)])
                        };
                    }
                    Err(e) => v.update_status = format!("{}{}", t("check_failed"), e),
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn update_download(&mut self, url: String, cx: &mut Context<Self>) {
        self.update_status = t("downloading").to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::download_update(url, None).await })
                .await.unwrap_or_else(|_| Err("task aborted".into()));
            this.update(cx, |v, cx| {
                match r {
                    Ok(res) => {
                        let file = res.get("file").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let bytes = res.get("bytes").and_then(|x| x.as_i64()).unwrap_or(0);
                        v.update_file = Some(file);
                        v.update_status = tf("download_done_toast", &[("mb", (bytes / 1024 / 1024).to_string())]);
                    }
                    Err(e) => v.update_status = format!("{}{}", t("download_failed"), e),
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn update_install(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.update_file.clone() else { return };
        cx.spawn(async move |this, cx| {
            let r = backend::bg(async move { backend::install_update(file).await }).await;
            this.update(cx, |v, cx| {
                if let Err(e) = r { v.update_status = format!("{}{}", t("install_failed"), e); cx.notify(); }
            }).ok();
        }).detach();
    }

    pub fn open_release(&mut self, url: String, cx: &mut Context<Self>) {
        cx.spawn(async move |_this, _cx| {
            let _ = backend::bg(async move { backend::open_url(url).await }).await;
        }).detach();
    }

    pub fn copy_text(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_string()));
        self.toast(t("copied_url"), cx);
    }
}

// =====================================================================
// SimpleSelect — 轻量下拉框（Popover + 选项列表），用于设置页语言/外观
// =====================================================================

/// 渲染一个下拉选择器：触发按钮（当前值 + 箭头），点开后浮层列出全部选项。
/// 选择事件经 SELECT_TX 全局通道送回 AppView（render 中 poll）。
pub(crate) fn simple_select(
    id: &'static str,
    options: Vec<(String, String)>,
    selected_value: &str,
    tag: &'static str,
    min_width: Pixels,
) -> AnyElement {
    let current_label = options.iter()
        .find(|(v, _)| v == selected_value)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| selected_value.to_string());
    let opts = options;
    let sv = selected_value.to_string();
    gpui_kit::component::popover::Popover::new(id)
        .trigger(
            Button::new(SharedString::from(format!("{id}-trigger")))
                .label(SharedString::from(current_label))
                .small()
                .outline()
                .dropdown_caret(true)
                .min_w(min_width),
        )
        .content(move |_, _, _| {
            v_flex().min_w(px(min_width.as_f32() + 24.)).max_h(px(220.)).id(SharedString::from(format!("{id}-menu-list"))).overflow_y_scroll()
                .rounded(px(9.))
                .border_1()
                .border_color(hsla(0., 0., 0.5, 0.15))
                .bg(if crate::theme::is_dark() { rgb(0x24242f) } else { rgb(0xffffff) })
                .py(px(4.))
                .id(SharedString::from(format!("{id}-menu")))
                .children(opts.iter().map(|(v, label)| {
                    let selected = *v == sv;
                    let v2 = v.clone();
                    h_flex()
                        .id(SharedString::from(format!("{tag}-{v2}")))
                        .px(px(10.)).py(px(6.))
                        .text_size(px(12.5))
                        .rounded(px(6.))
                        .when(selected, |el| el.bg(mix_color(accent(), 0.12)).text_color(rgb(accent())).font_weight(FontWeight::SEMIBOLD))
                        .hover(|s| s.bg(mix_color(accent(), 0.06)))
                        .on_click(move |_, window, cx| {
                            if let Ok(tx) = SELECT_TX.lock() {
                                if let Some(tx) = tx.as_ref() {
                                    let _ = tx.send((tag, v2.clone()));
                                }
                            }
                            let _ = window; let _ = cx;
                        })
                        .child(label.clone())
                }))
                .into_any_element()
        })
        .into_any_element()
}
