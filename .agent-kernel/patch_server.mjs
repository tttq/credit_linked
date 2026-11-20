// Patch credit-rs server.rs: token stats + context-usage endpoint.
// Run: node patch_server.mjs
import fs from 'node:fs';

const F = 'E:/auto_change/credit-rs/crates/credit-gateway/src/server.rs';
let t = fs.readFileSync(F, 'utf8');
const nl = t.includes('\r\n') ? '\r\n' : '\n';
const J = (lines) => lines.join(nl);

function rep(oldStr, newStr, tag) {
  if (t.includes(newStr)) { console.log('skip (already applied):', tag); return; }
  if (!t.includes(oldStr)) throw new Error('pattern not found: ' + tag);
  t = t.replace(oldStr, newStr);
}

// 1. stream log -> full signature (tokens backfilled by StreamGuard)
rep(J([
  'let log_id = st.store.log_gateway_request(',
  '                            platform.as_str(), &account_ref, &model, true, Some(200), None, None,',
  '                        ).unwrap_or(0);',
]), J([
  'let log_id = st.store.log_gateway_request_full(',
  '                            platform.as_str(), &account_ref, &model, true, Some(200), None, None, None, None,',
  '                        ).unwrap_or(0);',
]), 'stream-log');

// 2. blocking call captures usage
rep('let (resp, has_content) = blocking_response(&model, platform.as_str(), events).await;',
    'let (resp, has_content, usage) = blocking_response(&model, platform.as_str(), events).await;',
    'blocking-call');

rep(J([
  'let log_id = st.store.log_gateway_request(',
  '                            platform.as_str(),',
  '                            &format!("{}:{}", attempt_cred.provider, attempt_cred.uid),',
  '                            &model, has_content, Some(if has_content { 200 } else { 502 }),',
  '                            if has_content { None } else { Some("empty stream (no finish_reason/content)") },',
  '                            None,',
  '                        ).unwrap_or(0);',
]), J([
  'let (pt, ct) = usage.map(|(p, c, _)| (Some(p as i64), Some(c as i64))).unwrap_or((None, None));',
  '                        let log_id = st.store.log_gateway_request_full(',
  '                            platform.as_str(),',
  '                            &format!("{}:{}", attempt_cred.provider, attempt_cred.uid),',
  '                            &model, has_content, Some(if has_content { 200 } else { 502 }),',
  '                            if has_content { None } else { Some("empty stream (no finish_reason/content)") },',
  '                            None, pt, ct,',
  '                        ).unwrap_or(0);',
]), 'blocking-log');

// 3. blocking_response signature + returns
rep(J([
  ') -> (axum::response::Response, bool) {',
]), J([
  ') -> (axum::response::Response, bool, Option<(u64, u64, Option<u64>)>) {',
]), 'blocking-sig');

rep('return (err_response(StatusCode::BAD_GATEWAY, "upstream", &msg), false);',
    'return (err_response(StatusCode::BAD_GATEWAY, "upstream", &msg), false, usage);',
    'blocking-earlyret');

rep('    (resp, !empty_stream)', '    (resp, !empty_stream, usage)', 'blocking-ret');

// 4. StreamGuard: carry token pair
rep(J([
  '    finished: Arc<std::sync::atomic::AtomicBool>,',
  '    error: Arc<std::sync::Mutex<Option<String>>>,',
  '}',
]), J([
  '    finished: Arc<std::sync::atomic::AtomicBool>,',
  '    error: Arc<std::sync::Mutex<Option<String>>>,',
  '    tokens: Arc<std::sync::Mutex<(Option<i64>, Option<i64>)>>,',
  '}',
]), 'guard-struct');

rep(J([
  '        if finished {',
  '            let _ = self.store.finish_gateway_log(self.log_id, elapsed, None, None, None);',
  '        } else {',
]), J([
  '        if finished {',
  '            let (pt, ct) = *self.tokens.lock().unwrap_or_else(|e| e.into_inner());',
  '            if pt.is_some() || ct.is_some() {',
  '                let _ = self.store.update_gateway_log_tokens(self.log_id, pt, ct);',
  '            }',
  '            let _ = self.store.finish_gateway_log(self.log_id, elapsed, None, None, None);',
  '        } else {',
]), 'guard-drop');

rep(J([
  '    let guard = StreamGuard {',
  '        store: st.store.clone(),',
  '        log_id,',
  '        t0,',
  '        finished,',
  '        error,',
  '    };',
]), J([
  '    let tokens_seen: Arc<std::sync::Mutex<(Option<i64>, Option<i64>)>> = Arc::default();',
  '    let tokens_seen2 = tokens_seen.clone();',
  '    let guard = StreamGuard {',
  '        store: st.store.clone(),',
  '        log_id,',
  '        t0,',
  '        finished,',
  '        error,',
  '        tokens: tokens_seen,',
  '    };',
]), 'guard-init');

// 5. Usage event: capture into guard + emit chunk
rep(J([
  '            Ok(StreamEvent::Usage { input, output, total }) => out.push(Ok(Event::default().data(json!({',
  '                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,',
  '                "choices": [],',
  '                "usage": {',
  '                    "prompt_tokens": input,',
  '                    "completion_tokens": output,',
  '                    "total_tokens": total.unwrap_or(input + output),',
  '                },',
  '            }).to_string()))),',
]), J([
  '            Ok(StreamEvent::Usage { input, output, total }) => {',
  '                if let Ok(mut g) = tokens_seen2.lock() {',
  '                    *g = (Some(input as i64), Some(output as i64));',
  '                }',
  '                out.push(Ok(Event::default().data(json!({',
  '                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,',
  '                "choices": [],',
  '                "usage": {',
  '                    "prompt_tokens": input,',
  '                    "completion_tokens": output,',
  '                    "total_tokens": total.unwrap_or(input + output),',
  '                },',
  '            }).to_string())));',
  '            }',
]), 'usage-event');

// 6. context-usage endpoint for agent compaction decisions
rep('        .route("/v1/chat/completions", post(chat_completions))',
    '        .route("/v1/chat/completions", post(chat_completions))' + nl +
    '        .route("/v1/context-usage", get(context_usage))',
    'route-ctx');

const handler = J([
  '/// GET /v1/context-usage: context consumption snapshot for agent-side compaction.',
  '/// Agent asks this before each turn: usageRatio >= compactThreshold -> compact now.',
  'async fn context_usage(',
  '    State(st): State<Arc<GatewayState>>,',
  '    headers: axum::http::HeaderMap,',
  ') -> Response {',
  '    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {',
  '        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");',
  '    }',
  '    // 模型提示：取最近一条网关日志的模型（没有则为空 → 用默认窗口）',
  '    let model_hint = st.store.list_gateway_logs(1).ok().and_then(|l| l.first().map(|x| x.model.clone())).unwrap_or_default();',
  '    let (platform, model) = resolve_model(&model_hint, st.router.current_platform())',
  '        .unwrap_or((st.router.current_platform(), model_hint));',
  '    let raw = st.router.current_platform_raw();',
  '    let (context_window, _mt) = if model.is_empty() {',
  '        (DEFAULT_CONTEXT_WINDOW, None)',
  '    } else {',
  '        model_limits(&st, platform, &raw, &model).await',
  '    };',
  '    let rows = st.store.token_stats_daily(1).unwrap_or_default();',
  '    let (pt, ct, total, reqs) = rows.first().map(|r| (r.1, r.2, r.3, r.4)).unwrap_or((0, 0, 0, 0));',
  '    let ratio = if context_window > 0 { (total as f64 / context_window as f64).min(1.0) } else { 0.0 };',
  '    Json(json!({',
  '        "model": model,',
  '        "platform": platform.as_str(),',
  '        "contextWindow": context_window,',
  '        "usedToday": { "promptTokens": pt, "completionTokens": ct, "totalTokens": total, "requests": reqs },',
  '        "estimatedUsed": total,',
  '        "usageRatio": (ratio * 10000.0).round() / 10000.0,',
  '        "shouldCompact": ratio >= 0.85,',
  '        "compactThreshold": 0.85,',
  '        "policy": "agent_decides: gateway publishes usage; agent summarizes and trims history when shouldCompact=true"',
  '    })).into_response()',
  '}',
  '',
]);
{
  const anchor = 'async fn list_models(';
  const idx = t.indexOf(anchor);
  if (idx < 0) throw new Error('anchor list_models not found');
  const docStart = t.lastIndexOf('/// ', idx);
  t = t.slice(0, docStart < 0 ? idx : docStart) + handler + nl + t.slice(docStart < 0 ? idx : docStart);
}

fs.writeFileSync(F, t, 'utf8');
console.log('OK newLines=' + t.split('\n').length);
