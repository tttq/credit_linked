// AI路由助手 前端逻辑
const _T = window.__TAURI__;
const invoke = _T?.core?.invoke || _T?.invoke;
window.invoke = invoke; // i18n.js 等外部脚本经 window.invoke 调后端（settings locale 持久化）
const openUrl = _T?.opener?.openUrl || _T?.shell?.open || null;

const PLAT = {
  trae:      { color: '#20b978', login: 'browser' },
  workbuddy: { color: '#00ba9c', login: 'browser' },
  minimax:   { color: '#20b9ec', login: 'device'  },
  qoder:     { color: '#606570', login: 'browser' },
  codearts:  { color: '#e93442', login: 'browser' },
  'codearts-desktop': { color: '#854cf5', login: 'browser' },
  zcode:     { color: '#2e7a6f', login: 'none'    },
  doubao:    { color: '#c78f79', login: 'none'    },
};
const NAV = [
  ['overview', 'nav_home', icon('home')],
  ['accounts', 'nav_accounts', icon('users')],
  ['checkin', 'nav_checkin', icon('check')],
  ['statistics', 'nav_stats', icon('chart')],
  ['routing', 'nav_route', icon('route')],
  ['settings', 'nav_settings', icon('gear')],
];
function icon(k) {
  const P = {
    home: 'M3 10.5 12 3l9 7.5M5 9.5V21h5v-6h4v6h5V9.5',
    users: 'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm13 10v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75',
    check: 'M20 6 9 17l-5-5',
    chart: 'M3 3v18h18M8 17V9m5 8V5m5 12v-6',
    route: 'M4 19h6a4 4 0 0 0 0-8H8a4 4 0 0 1 0-8h8M18 7l3-4 3 4M17 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z',
    gear: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm7.4-3a7.4 7.4 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7.6 7.6 0 0 0-2-1.2L14.4 3h-4l-.5 2.6a7.6 7.6 0 0 0-2 1.2l-2.4-1-2 3.4 2 1.6a7.4 7.4 0 0 0 0 2.4l-2 1.6 2 3.4 2.4-1a7.6 7.6 0 0 0 2 1.2l.5 2.6h4l.5-2.6a7.6 7.6 0 0 0 2-1.2l2.4 1 2-3.4-2-1.6c.1-.4.1-.8.1-1.2Z',
  }[k];
  return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="' + P + '"/></svg>';
}

let SNAP = null, SETTINGS = null;
let curPage = 'overview';
let overviewRange = 7;
let statPeriod = 'day', statAccount = 'all';
let checkinFilter = 'all';
let loginTimer = null, curLoginSid = null;
let charts = { ov: null, st: null };
let statTokens = []; // token consume per day for statistics chart

// ===== 外观主题（system | light | dark，默认跟随系统） =====
// html[data-theme] 驱动 CSS 暗色变量；system 模式实时跟随 OS 深浅色切换。
let THEME_MODE = 'system';
const themeMedia = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;
function resolveTheme(mode) {
  return mode === 'system' ? ((themeMedia && themeMedia.matches) ? 'dark' : 'light') : mode;
}
function applyTheme(mode, save) {
  THEME_MODE = (mode === 'dark' || mode === 'light') ? mode : 'system';
  document.documentElement.dataset.theme = resolveTheme(THEME_MODE);
  if (save) {
    try { localStorage.setItem('ui-theme', THEME_MODE); } catch {}
    try { const r = invoke('settings_set', { key: 'theme', value: THEME_MODE }); if (r && r.catch) r.catch(() => {}); } catch {}
  }
  // 坐标轴文字/分隔线颜色是 JS 画的，换主题后重绘当前页图表
  try { showPage(curPage); } catch {}
}
// 启动恢复：localStorage（本机偏好）> DB settings.theme > 跟随系统
function restoreTheme(dbTheme) {
  let saved = null;
  try { saved = localStorage.getItem('ui-theme'); } catch {}
  applyTheme(saved || dbTheme || 'system', false);
}
if (themeMedia && themeMedia.addEventListener) themeMedia.addEventListener('change', () => {
  if (THEME_MODE === 'system') { document.documentElement.dataset.theme = resolveTheme('system'); try { showPage(curPage); } catch {} }
});
// 脚本加载即应用一次（避免暗色用户启动时白闪）；DB 恢复在 refreshAll 里再校正
try { document.documentElement.dataset.theme = resolveTheme(localStorage.getItem('ui-theme') || 'system'); } catch {}
// 图表配色跟随主题（echarts 选项不支持 CSS 变量）
function chartAxisColor() { return document.documentElement.dataset.theme === 'dark' ? '#9d9bb0' : '#7f7d91'; }
function chartSplitColor() { return document.documentElement.dataset.theme === 'dark' ? '#34334a' : '#eae9f1'; }

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const esc = (v) => { const d = document.createElement('div'); d.textContent = v == null ? '' : String(v); return d.innerHTML; };
function toast(msg, ms = 2800) { const t = $('#toast'); t.textContent = msg; t.style.display = 'block'; clearTimeout(t._h); t._h = setTimeout(() => t.style.display = 'none', ms); }

// 居中确认弹框（替代原生 confirm/alert）
function confirmBox(title, message, { okText = t('ok'), cancelText = t('cancel'), danger = false, onlyOk = false } = {}) {
  return new Promise(resolve => {
    const mask = $('#confirm-mask');
    if (!mask) { resolve(window.confirm(message)); return; }
    $('#confirm-title').textContent = title;
    $('#confirm-desc').textContent = message;
    const yes = $('#confirm-yes'), no = $('#confirm-no');
    yes.textContent = okText;
    yes.className = 'primary' + (danger ? ' danger-ghost' : '');
    no.textContent = cancelText;
    no.style.display = onlyOk ? 'none' : '';
    mask.style.display = 'flex';
    const done = (v) => { mask.style.display = 'none'; yes.onclick = null; no.onclick = null; resolve(v); };
    yes.onclick = () => done(true);
    no.onclick = () => done(false);
  });
}
function platName(p) { const k = 'pname_' + p; const s = t(k); return s === k ? p : s; }
function platDesc(p) { const k = 'desc_' + p; const s = t(k); return s === k ? '' : s; }
function platMeta(p) { return PLAT[p] || { color: '#456' }; }
function gwLabel(p) { const k = 'gw_' + p; const s = t(k); return s === k ? p : s; }
function picon(p, cls) {
  const m = platMeta(p);
  const iconFile = { trae: 'trae.png', workbuddy: 'workbuddy.jpg', minimax: 'minimax.png', qoder: 'qoder.png', codearts: 'codearts.svg', 'codearts-desktop': 'codearts-desktop.png', doubao: 'doubao.ico', zcode: null }[p];
  // svg 无固有位图尺寸，object-fit:cover 不生效会撑破容器 —— svg 用 contain
  const fit = iconFile && iconFile.endsWith('.svg') ? 'contain' : 'cover';
  if (iconFile) return '<img class="picon ' + (cls || '') + '" style="width:26px;height:26px;border-radius:7px;object-fit:' + fit + ';flex:0 0 auto" src="platforms/' + iconFile + '" onerror="this.outerHTML=piconFallback(\'' + p + '\',\'' + (cls || '') + '\')">';
  return piconFallback(p, cls);
}
function piconFallback(p, cls) {
  const m = platMeta(p);
  return '<span class="picon ' + (cls || '') + '" style="background:' + m.color + ';width:26px;height:26px;border-radius:7px;display:inline-flex;align-items:center;justify-content:center;color:#fff;font-weight:700;font-size:12px;flex:0 0 auto">' + esc((platName(p) || '?')[0]) + '</span>';
}
function stInfo(s) {
  const m = { success: ['st-ok','st_success'], already: ['st-ok','st_already'], failed: ['st-bad','st_failed'],
    pending: ['st-warn','st_pending'], 'needs-action': ['st-warn','st_needs_action'], 'no-activity': ['st-idle','st_no_activity'], idle: ['st-idle','st_idle'], 'auth-expired': ['st-bad','st_auth_expired'] };
  const hit = m[s];
  const [cls, txt] = hit ? [hit[0], t(hit[1])] : ['st-idle', s || '—'];
  return '<span class="status ' + cls + '"><span class="dot"></span>' + esc(txt) + '</span>';
}
function fmt(n) { return I18n.fmtNumber(n); }
function parseQuota(a) {
  if (!a.quota) return null;
  try { return typeof a.quota === 'string' ? JSON.parse(a.quota) : a.quota; } catch { return null; }
}
// 启动/刷新后：给没有额度缓存的账号静默补拉一次（逐个，避免并发打爆平台）
async function backfillQuotas() {
  const accs = (SNAP?.accounts || []).filter(a => !parseQuota(a));
  for (const a of accs) {
    try { await invoke('quota_refresh', { provider: a.provider, uid: a.uid }); } catch {}
    SNAP = await invoke('snapshot');
    renderOverview(); renderAccounts(); renderCheckin();
  }
}

// ===== 导航 =====
function renderNav() {
  $('#nav').innerHTML = NAV.map(([k, labelKey, ic]) =>
    '<button data-page="' + k + '" class="' + (curPage === k ? 'selected' : '') + '">' + ic + '<span>' + esc(t(labelKey)) + '</span></button>').join('');
  $$('#nav button').forEach(b => b.addEventListener('click', () => showPage(b.dataset.page)));
}
function showPage(p) {
  curPage = p;
  $$('.page').forEach(s => s.classList.toggle('active', s.id === 'page-' + p));
  $$('#nav button').forEach(b => b.classList.toggle('selected', b.dataset.page === p));
  // 顶部工具条仅签到中心保留；账号管理的刷新/签到在页面内页头
  const toolbar = document.querySelector('.account-toolbar');
  if (toolbar) toolbar.style.display = (p === 'checkin') ? 'flex' : 'none';
  if (p === 'statistics') refreshStatistics();
  if (p === 'overview') drawOverview();
}

// ===== 数据 =====
async function refreshAll() {
  renderNav(); // 菜单永远渲染，不依赖数据
  const step = async (name, fn) => { try { await fn(); } catch (e) { console.error('[refresh:' + name + ']', e); } };
  try {
    SNAP = await invoke('snapshot');
    SETTINGS = await invoke('settings_get');
    try { if (window.I18n && I18n.restoreFromDb) I18n.restoreFromDb(SETTINGS.locale); } catch {}
    try { restoreTheme(SETTINGS.theme); } catch {}
    $('#cycle-label').textContent = t('today_cycle') + ' ' + SNAP.cycle;
    $('#today-str').textContent = I18n.fmtDate(new Date(), { weekday: 'long', month: 'long', day: 'numeric' });
    $('#sched-time').textContent = SETTINGS.randomTime ? t('random') : SETTINGS.time;
  } catch (e) { console.error('[refresh:snapshot]', e); toast(t('data_load_failed') + e, 4200); }
  await step('gateway', async () => { await refreshGateway(); });
  await step('overview', () => { renderOverview(); });
  await step('accounts', () => { renderAccounts(); });
  await step('checkin', () => { renderCheckin(); });
  await step('statistics', () => { refreshStatistics(); });
  await step('settings', () => { renderSettings(); });
  await step('routing', () => { renderRouting(); });
  if (curPage === 'overview') { try { drawOverview(); } catch (e) { console.error('[refresh:chart]', e); } }
  try { backfillQuotas(); } catch (e) { console.error('[refresh:backfill]', e); }
  try { applyRefreshInterval(); } catch (e) { console.error('[refresh:interval]', e); }
}

// ===== 首页 =====
function renderOverview() {
  const accs = SNAP.accounts || [];
  const grid = $('#tool-grid');
  if (!accs.length) { grid.innerHTML = '<div class="empty" style="grid-column:1/-1">' + t('empty_no_tools') + '</div>'; return; }
  grid.innerHTML = accs.map(a => {
    const m = platMeta(a.provider);
    const done = a.status === 'success' || a.status === 'already';
    const q = parseQuota(a);
    return '<section class="tool-card" data-provider="' + a.provider + '">' +
      '<div class="tool-identity">' + picon(a.provider) + '<div><h3>' + esc(platName(a.provider)) + '</h3><p class="subtle" title="' + esc(a.name) + '">' + esc(a.name || a.uid.slice(0, 10)) + '</p></div></div>' +
      '<div class="tool-balance"><span class="subtle">' + t('available') + '</span><strong>' + (q && q.remaining != null ? fmt(q.remaining) : '—') + '</strong></div>' +
      '<div class="tool-result">' + (done ? '<span class="checkin-done">' + t('checked_in') + '</span>' : stInfo(a.status)) +
      '<span class="subtle">' + esc(a.last_checkin ? t('last') + a.last_checkin.slice(5, 10) : '') + '</span></div></section>';
  }).join('');
}
async function drawOverview() {
  try {
    const data = await invoke('daily_income', { days: overviewRange });
    const el = $('#overview-chart');
    charts.ov ||= echarts.init(el);
    charts.ov.resize();
    const days = data.days || [];
    const provs = [...new Set(days.flatMap(d => Object.keys(d.by_provider || {})))];
    const colors = { trae: '#20b978', workbuddy: '#00ba9c', minimax: '#20b9ec', qoder: '#606570', codearts: '#e93442', 'codearts-desktop': '#854cf5', doubao: '#c78f79', zcode: '#2e7a6f' };
    charts.ov.setOption({
      tooltip: { trigger: 'axis' },
      legend: provs.length > 1 ? { data: provs.map(platName), bottom: 0, textStyle: { fontSize: 11, color: chartAxisColor() } } : undefined,
      grid: { left: 44, right: 14, top: 20, bottom: provs.length > 1 ? 42 : 26 },
      xAxis: { type: 'category', data: days.map(d => d.day.slice(overviewRange > 100 ? 2 : 5)), axisLabel: { fontSize: 10, color: chartAxisColor() } },
      yAxis: { type: 'value', axisLabel: { fontSize: 10, color: chartAxisColor() }, splitLine: { lineStyle: { color: chartSplitColor() } } },
      series: (provs.length ? provs : ['_']).map(p => ({
        name: p === '_' ? t('checkin_points') : platName(p), type: 'bar', stack: 'income',
        itemStyle: { color: p === '_' ? '#6862ed' : (colors[p] || '#6862ed'), borderRadius: p === '_' ? 4 : undefined, borderColor: '#fff', borderWidth: p === '_' ? 0 : 1 },
        data: days.map(d => p === '_' ? d.income : (d.by_provider?.[p] || 0)),
        barMaxWidth: 22,
      })),
    });
    const sum = days.reduce((s, d) => s + d.income, 0);
    $('#chart-sum').textContent = t('period_points') + fmt(sum);
  } catch (e) { toast(t('chart_load_failed') + e); }
}
$$('#range-pills button').forEach(b => b.addEventListener('click', () => {
  overviewRange = Number(b.dataset.range);
  $$('#range-pills button').forEach(x => x.classList.toggle('active', x === b));
  drawOverview();
}));

// ===== 账号管理 =====
function renderAccounts() {
  const accs = SNAP.accounts || [];
  // 固定平台清单：已添加平台在前，未添加平台也渲染（保证删除后添加入口不消失）
  const ALL_PLATS = Object.keys(PLAT).filter(p => p !== 'doubao');
  const added = [...new Set(accs.map(a => a.provider))];
  const plats = [...added, ...ALL_PLATS.filter(p => !added.includes(p))];
  $('#accounts-sub').textContent = t('accounts_connected', { n: added.length, m: accs.length });
  const el = $('#account-list');
  el.innerHTML = plats.map(p => {
    const list = accs.filter(a => a.provider === p);
    const m = platMeta(p);
    const multi = ['trae', 'workbuddy', 'minimax', 'qoder', 'codearts', 'codearts-desktop'].includes(p);
    return '<section class="account-platform-group" data-provider="' + p + '">' +
      '<header class="account-platform-header">' +
      '<div class="account-platform-heading">' + picon(p) + '<h2>' + esc(platName(p)) + '</h2>' +
      '<span class="account-capability ' + (multi ? 'multi' : '') + '">' + (multi ? t('multi_account') : t('single_account')) + '</span>' +
      '<span class="account-platform-count subtle">' + t('n_accounts', { n: list.length }) + '</span></div>' +
      '<div class="actions">' +
      (m.login !== 'none' ? '<button data-add="' + p + '" class="small primary">' + t('add_account') + '</button>' : '') +
      (['trae','workbuddy','qoder','zcode','minimax','codearts','codearts-desktop'].includes(p) ? '<button data-import="' + p + '" class="small">' + t('scan_local') + '</button>' : '') +
      '</div></header>' +
      (list.length ? list.map(a => acctRow(a, p)).join('') : '<div class="empty" style="padding:14px">' + t('empty_no_accounts') + '</div>') +
      '</section>';
  }).join('');
  el.querySelectorAll('[data-add]').forEach(b => b.addEventListener('click', () => startLogin(b.dataset.add)));
  el.querySelectorAll('[data-import]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true; b.textContent = t('scanning');
    try {
      const r = await invoke('import', { platform: b.dataset.import === 'workbuddy' ? 'wb' : b.dataset.import, intl: false });
      toast(t('import_ok') + (r.name || r.uid));
      await refreshAll();
    } catch (e) { toast(t('import_failed') + e, 4200); }
    b.disabled = false; b.textContent = t('scan_local');
  }));
  el.querySelectorAll('.acct-actions button, .acct-actions input').forEach(wire);
}
function acctRow(a, p) {
  const q = parseQuota(a);
  const done = a.status === 'success' || a.status === 'already';
  const stateText = done ? t('today_claimed') : (a.status === 'failed' || a.status === 'auth-expired' ? t('checkin_error') : t('today_not_claimed'));
  const stateCls = done ? 'done' : (a.status === 'failed' || a.status === 'auth-expired' ? 'bad' : '');
  return '<div class="account-group-row" data-p="' + esc(a.provider) + '" data-u="' + esc(a.uid) + '">' +
    '<div class="single-account-identity"><strong>' + esc(a.name || platName(p)) + ' <span class="acct-state ' + stateCls + '">(' + stateText + ')</span></strong><small>' + esc(a.uid) + (a.expires ? t('cred_expires') + a.expires.slice(0, 10) : '') + '</small></div>' +
    '<div class="single-account-balance"><strong>' + (q && q.remaining != null ? fmt(q.remaining) : '—') + '</strong><small>' + t('available_balance') + (a.message ? ' · ' + esc(a.message) : '') + '</small></div>' +
    '<div class="account-row-actions acct-actions">' +
    '<label class="switch" title="' + t('auto_checkin') + '"><input type="checkbox" data-auto ' + (a.auto ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label>' +
    '<button class="small" data-act="checkin"' + (done ? ' disabled title="' + t('today_claimed') + '"' : '') + '>' + (done ? t('today_claimed') : t('checkin')) + '</button>' +
    '<button class="small" data-act="quota">' + t('quota_btn') + '</button>' +
    (p === 'trae' ? '<button class="small" data-act="device" title="' + t('device_btn_tip') + '">' + t('device_btn') + '</button>' : '') +
    '<button class="small danger-ghost" data-act="del">' + t('delete') + '</button>' +
    '</div></div>';
}
function wire(el) {
  if (el.type === 'checkbox') {
    el.addEventListener('change', async () => {
      const row = el.closest('.account-group-row');
      try { await invoke('account_toggle_auto', { provider: row.dataset.p, uid: row.dataset.u, auto: el.checked }); toast(el.checked ? t('auto_on') : t('auto_off')); }
      catch (e) { toast(t('set_failed') + e); }
    });
    return;
  }
  el.addEventListener('click', async () => {
    const row = el.closest('.account-group-row');
    const p = row.dataset.p, uid = row.dataset.u;
    el.disabled = true; const old = el.textContent;
    try {
      if (el.dataset.act === 'checkin') {
        el.innerHTML = '<span class="spin"></span>';
        const r = await invoke('checkin', { provider: p, uid, all: false });
        const o = r.results[0];
        toast((o.status === 'success' ? t('claim_ok') : o.status === 'already' ? t('already_claimed') : o.status) + (o.amount != null ? ' +' + o.amount : '') + (o.message ? ' · ' + o.message : ''));
        await refreshAll();
      } else if (el.dataset.act === 'quota') {
        el.textContent = '…';
        const q = await invoke('quota_refresh', { provider: p, uid });
        const bal = row.querySelector('.single-account-balance strong');
        if (bal) bal.textContent = q.remaining != null ? fmt(q.remaining) : '—';
      } else if (el.dataset.act === 'device') {
        el.disabled = false; el.textContent = old;
        await openDeviceDialog(p, uid);
        return;
      } else if (el.dataset.act === 'del') {
        if (!await confirmBox(t('delete_account'), t('delete_confirm_msg', { u: uid.slice(0, 12) }), { okText: t('delete'), danger: true })) { el.disabled = false; return; }
        await invoke('account_delete', { provider: p, uid });
        toast(t('deleted'));
        await refreshAll();
      }
    } catch (e) { toast(t('op_error') + e, 4200); }
    el.disabled = false; el.textContent = old;
  });
}

// ===== Trae 设备号设置弹窗（居中模态） =====
async function openDeviceDialog(p, uid) {
  const mask = document.getElementById('dev-mask');
  if (!mask) { toast(t('dev_unavailable')); return; }
  const body = document.getElementById('dev-body');
  document.getElementById('dev-desc').textContent = t('dev_desc');
  body.innerHTML = '<div style="padding:14px 0"><span class="spin"></span>' + t('dev_loading') + '</div>';
  mask.style.display = 'flex';
  let local = [];
  try { local = await invoke('trae_local_devices'); } catch {}
  body.innerHTML =
    (local.length ? '<div style="font-size:12.5px;margin-bottom:8px">' + t('dev_local_found', { n: local.length }) + '</div>' +
      '<div style="display:grid;gap:6px;margin-bottom:12px">' +
      local.map((d, i) => '<label style="display:flex;align-items:center;gap:8px;font-size:13px;cursor:pointer"><input type="radio" name="dev-pick" value="' + esc(d) + '"> <code style="font-size:12.5px">' + esc(d) + '</code><span class="subtle">' + t('dev_local_n', { n: i + 1 }) + '</span></label>').join('') +
      '<label style="display:flex;align-items:center;gap:8px;font-size:13px;cursor:pointer"><input type="radio" name="dev-pick" value=""> <span class="subtle">' + t('dev_auto_option') + '</span></label></div>'
    : '<div class="subtle" style="margin-bottom:10px">' + t('dev_none_found') + '</div>') +
    '<input id="dev-manual" type="text" placeholder="' + t('dev_manual_ph') + '" style="width:100%;padding:9px;border-radius:9px;border:1px solid var(--line);font:inherit;font-size:13px">';
  document.getElementById('dev-cancel').onclick = () => { mask.style.display = 'none'; };
  document.getElementById('dev-save').onclick = async () => {
    const picked = (body.querySelector('input[name=dev-pick]:checked') || {}).value;
    const manual = document.getElementById('dev-manual').value.trim();
    const val = manual || picked || '';
    try {
      await invoke('trae_set_device', { provider: p, uid, deviceId: val });
      toast(val ? t('dev_set_ok') + val : t('dev_cleared'));
      mask.style.display = 'none';
    } catch (e) { toast(t('save_failed') + e, 4200); }
  };
}

// ===== 签到中心 =====
function renderCheckin() {
  const accs = SNAP.accounts || [];
  const done = accs.filter(a => a.status === 'success' || a.status === 'already').length;
  const pending = accs.filter(a => !a.status || a.status === 'idle' || a.status === 'no-activity').length;
  const confirmN = accs.filter(a => a.status === 'pending').length;
  const attention = accs.filter(a => a.status === 'failed' || a.status === 'needs-action' || a.status === 'auth-expired').length;
  $('#progress-hero').innerHTML =
    '<div><strong>' + done + ' <span style="font-size:20px;color:var(--subtle)">/ ' + accs.length + '</span></strong><p>' + t('cycle_done') + '</p>' +
    '<div class="progress"><span style="width:' + (accs.length ? done / accs.length * 100 : 0) + '%"></span></div></div>' +
    '<div><b style="font-size:24px">' + pending + '</b><p>' + t('waiting_claim') + '</p></div>' +
    '<div><b style="font-size:24px">' + confirmN + '</b><p>' + t('st_pending') + '</p></div>' +
    '<div><b style="font-size:24px;color:' + (attention ? 'var(--amber)' : 'inherit') + '">' + attention + '</b><p>' + t('st_needs_action') + '</p></div>';
  const FILTERS = [['all', t('filter_all')],['done', t('filter_done')],['pending', t('filter_pending')],['confirm', t('filter_confirm')],['attention', t('filter_attention')]];
  const cnt = k => ({ all: accs.length, done, pending, confirm: confirmN, attention }[k]);
  $('#checkin-filters').innerHTML = FILTERS.map(([k, l]) =>
    '<button data-filter="' + k + '" class="' + (checkinFilter === k ? 'active' : '') + '">' + l + ' <span class="checkin-filter-count">' + cnt(k) + '</span></button>').join('');
  $$('#checkin-filters button').forEach(b => b.addEventListener('click', () => { checkinFilter = b.dataset.filter; renderCheckin(); }));
  const match = a => {
    const isDone = a.status === 'success' || a.status === 'already';
    switch (checkinFilter) {
      case 'done': return isDone;
      case 'pending': return !a.status || a.status === 'idle' || a.status === 'no-activity';
      case 'confirm': return a.status === 'pending';
      case 'attention': return a.status === 'failed' || a.status === 'needs-action' || a.status === 'auth-expired';
      default: return true;
    }
  };
  const list = accs.filter(match);
  $('#checkin-list').innerHTML = list.length ? list.map(a => {
    const q = parseQuota(a);
    return '<div class="checkin-row" data-provider="' + a.provider + '">' +
    '<div class="checkin-identity">' + picon(a.provider) + '<div><strong>' + esc(platName(a.provider)) + '</strong><small>' + esc(a.name || a.uid.slice(0, 14)) + '</small></div></div>' +
    '<div><small class="subtle">' + t('status') + '</small><br>' + stInfo(a.status) + '</div>' +
    '<div class="single-account-balance"><strong>' + (q && q.remaining != null ? fmt(q.remaining) : '—') + '</strong><small>' + t('available_balance') + (a.message ? ' · ' + esc(a.message) : '') + '</small></div>' +
    '<div><small class="subtle">' + t('last_time') + '</small><br><span style="font-size:12.5px">' + esc(a.last_checkin ? a.last_checkin.slice(0, 16).replace('T', ' ') : '—') + '</span></div>' +
    '<div style="margin-left:auto;display:flex;gap:6px">' +
    '<button class="small" data-ck-p="' + esc(a.provider) + '" data-ck-u="' + esc(a.uid) + '">' + t('checkin_now') + '</button>' +
    '<button class="small" data-qk-p="' + esc(a.provider) + '" data-qk-u="' + esc(a.uid) + '">' + t('quota_btn') + '</button>' +
    '</div></div>';
  }).join('') : '<div class="empty">' + t('no_accounts_in_filter') + '</div>';
  $$('#checkin-list [data-ck-p]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true; b.innerHTML = '<span class="spin"></span>';
    try {
      const r = await invoke('checkin', { provider: b.dataset.ckP, uid: b.dataset.ckU, all: false });
      const o = r.results[0];
      toast((o.status === 'success' ? t('claim_ok') : o.status === 'already' ? t('already_claimed') : o.status) + (o.amount != null ? ' +' + o.amount : ''));
      await refreshAll();
    } catch (e) { toast(t('failed') + ': ' + e, 4200); }
    b.disabled = false; b.textContent = t('checkin_now');
  }));
  $$('#checkin-list [data-qk-p]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true;
    try { await invoke('quota_refresh', { provider: b.dataset.qkP, uid: b.dataset.qkU }); toast(t('quota_refreshed')); await refreshAll(); }
    catch (e) { toast(t('failed') + ': ' + e, 4200); }
    b.disabled = false;
  }));
}

// ===== 积分统计 =====
async function refreshStatistics() {
  const sel = $('#stat-account');
  const accs = SNAP?.accounts || [];
  const cur = sel.value;
  sel.innerHTML = '<option value="all">' + t('all_accounts') + '</option>' + accs.map(a =>
    '<option value="' + esc(a.provider + '|' + a.uid) + '">' + esc(platName(a.provider) + ' · ' + (a.name || a.uid.slice(0, 10))) + '</option>').join('');
  if (cur) sel.value = cur;
  if (SNAP) await drawStatistics();
}
async function drawStatistics() {
  if (!SNAP) return;
  const [provider, uid] = statAccount === 'all' ? [null, null] : statAccount.split('|');
  try {
    const agg = await invoke('statistics', { provider, uid, period: statPeriod });
    const items = agg.items || [];
    // token 消耗统计（按天粒度；周期=week/month/year 时直接聚合日桶）
    let toks = [];
    if (!provider && !uid) {
      const daily = await invoke('daily_tokens', { days: statPeriod === 'day' ? 14 : (statPeriod === 'week' ? 56 : (statPeriod === 'month' ? 180 : 730)) }).catch(() => ({ days: [] }));
      const days = daily.days || [];
      const keyOf = (day) => {
        if (statPeriod === 'day') return day;
        if (statPeriod === 'week') { const d = new Date(day + 'T00:00:00'); const p = new Date(d); p.setDate(d.getDate() - ((d.getDay() + 6) % 7)); return isoWeekKey(p); }
        if (statPeriod === 'month') return day.slice(0, 7);
        return day.slice(0, 4);
      };
      const acc = new Map();
      for (const d of days) {
        const k = keyOf(d.day);
        const e = acc.get(k) || { prompt: 0, completion: 0, total: 0 };
        e.prompt += d.promptTokens || 0; e.completion += d.completionTokens || 0; e.total += d.totalTokens || 0;
        acc.set(k, e);
      }
      // 对齐统计图的桶顺序
      toks = items.map(i => acc.get(i.bucket) || { prompt: 0, completion: 0, total: 0 });
      $('#stat-token-note').textContent = t('token_stats_all');
    } else {
      $('#stat-token-note').textContent = t('token_stats_single');
    }
    const totalSum = items.reduce((s, i) => s + i.sum, 0);
    const totalCount = items.reduce((s, i) => s + i.count, 0);
    const tokenSum = toks.reduce((s, x) => s + x.total, 0);
    $('#stat-metrics').innerHTML =
      '<div><p>' + t('period_points_short') + '</p><strong>' + fmt(totalSum) + '</strong></div>' +
      '<div><p>' + t('token_period') + '</p><strong>' + fmt(tokenSum) + '</strong></div>' +
      '<div><p>' + t('records') + '</p><strong>' + totalCount + '</strong></div>' +
      '<div><p>' + t('buckets') + '</p><strong>' + items.length + '</strong></div>';
    const el = $('#stat-chart');
    charts.st ||= echarts.init(el);
    charts.st.resize();
    // 双序列折线图：签到积分（左轴）+ token 消耗（右轴）；全部账号时同时统计
    const showToken = !provider && !uid;
    const series = [{
      name: t('checkin_points'), type: 'line', smooth: true, symbolSize: 5,
      itemStyle: { color: '#6862ed' }, lineStyle: { width: 2.4, color: '#6862ed' },
      areaStyle: { color: 'rgba(104, 98, 237, .12)' },
      data: items.map(i => Math.round(i.sum * 100) / 100),
    }];
    if (showToken) {
      series.push({
        name: t('token_usage'), type: 'line', smooth: true, symbolSize: 5, yAxisIndex: 1,
        itemStyle: { color: '#20b9ec' }, lineStyle: { width: 2.4, color: '#20b9ec', type: 'dashed' },
        data: toks.map(x => x.total),
      });
    }
    charts.st.setOption({
      tooltip: { trigger: 'axis' },
      legend: showToken ? { data: [t('checkin_points'), t('token_usage')], bottom: 0, textStyle: { fontSize: 11, color: chartAxisColor() } } : undefined,
      grid: { left: 50, right: showToken ? 56 : 18, top: 24, bottom: showToken ? 44 : 28 },
      xAxis: { type: 'category', data: items.map(i => i.bucket), axisLabel: { fontSize: 10, color: chartAxisColor(), rotate: items.length > 12 ? 30 : 0 } },
      yAxis: [
        { type: 'value', name: t('checkin_points'), nameTextStyle: { fontSize: 10, color: chartAxisColor() }, axisLabel: { fontSize: 10, color: chartAxisColor() }, splitLine: { lineStyle: { color: chartSplitColor() } } },
        ...(showToken ? [{ type: 'value', name: t('token_usage'), nameTextStyle: { fontSize: 10, color: chartAxisColor() }, axisLabel: { fontSize: 10, color: chartAxisColor() }, splitLine: { show: false } }] : []),
      ],
      series,
    }, true);
  } catch (e) { toast(t('stat_read_failed') + e, 4200); }
}
// ISO 周桶键（周一为界）：YYYY-Www
function isoWeekKey(mondayDate) {
  const d = new Date(mondayDate);
  const target = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const dayNum = (target.getUTCDay() + 6) % 7;
  target.setUTCDate(target.getUTCDate() - dayNum + 3);
  const firstThursday = new Date(Date.UTC(target.getUTCFullYear(), 0, 4));
  const fDayNum = (firstThursday.getUTCDay() + 6) % 7;
  firstThursday.setUTCDate(firstThursday.getUTCDate() - fDayNum + 3);
  const week = 1 + Math.round((target - firstThursday) / (7 * 86400000));
  const y = target.getUTCFullYear();
  const m = new Date(target); m.setUTCDate(m.getUTCDate() - 3);
  const yr = m.getUTCFullYear();
  void y;
  return yr + '-W' + String(week).padStart(2, '0');
}
$$('#stat-period button').forEach(b => b.addEventListener('click', () => {
  statPeriod = b.dataset.period;
  $$('#stat-period button').forEach(x => x.classList.toggle('active', x === b));
  drawStatistics();
}));
$('#stat-account').addEventListener('change', () => { statAccount = $('#stat-account').value || 'all'; drawStatistics(); });

// ===== 账号路由 + 模型网关 =====
// 平台清单（与 credit-gateway 的 Platform 对齐；chat 标注是否有对话通道）
const GW_PLATFORMS = {
  'workbuddy':        { label: () => t('gw_workbuddy'), provider: 'workbuddy', chat: true },
  'workbuddy-intl':   { label: () => t('gw_workbuddy_intl'), provider: 'workbuddy-intl', chat: true },
  'trae':             { label: () => t('gw_trae'), provider: 'trae', chat: true },
  'trae-enterprise':  { label: () => t('gw_trae_enterprise'), provider: 'trae', chat: true },
  'trae-global':      { label: () => t('gw_trae_global'), provider: 'trae', chat: true },
  'zcode':            { label: () => t('gw_zcode'), provider: 'zcode', chat: true },
  'minimax':          { label: () => t('gw_minimax'), provider: 'minimax', chat: true },
  'qoder':            { label: () => t('gw_qoder'), provider: 'qoder', chat: true },
  'codearts':         { label: () => t('gw_codearts'), provider: 'codearts', chat: false },
  'codearts-desktop': { label: () => t('gw_codearts_desktop'), provider: 'codearts-desktop', chat: false },
};
let GW_STATUS = null;

async function refreshGateway() {
  try {
    GW_STATUS = await invoke('gateway_status');
  } catch (e) { GW_STATUS = null; }
}

async function refreshGatewayAccounts() {
  const box = document.getElementById('gw-acct-list');
  if (!box) return;
  const d = await invoke('gateway_accounts').catch(() => null);
  if (!d || !d.accounts?.length) {
    box.innerHTML = '<div class="empty" style="padding:12px">' + t('no_accounts_platform', { p: esc(d?.platform || '') }) + '</div>';
    return;
  }
  box.innerHTML = d.accounts.map(a => {
    const bal = a.remaining != null ? (t('balance') + fmt(a.remaining)) : '';
    return '<button class="small gw-acct-pick" data-ref="' + esc(a.ref) + '" style="text-align:left;display:flex;align-items:center;gap:8px;' + (a.active ? 'border-color:var(--accent);background:var(--accent-soft)' : '') + '">' +
      '<span style="font-weight:700;color:' + (a.active ? 'var(--accent)' : 'var(--subtle)') + '">' + (a.active ? '●' : '○') + '</span>' +
      '<span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + esc(a.name || a.uid) + bal + '</span>' +
      (a.status === 'failed' ? '<span class="subtle" style="color:var(--red)">' + t('quota_exhausted') + '</span>' : '') + '</button>';
  }).join('');
  box.querySelectorAll('.gw-acct-pick').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true;
    try {
      await invoke('gateway_switch_account', { accountRef: b.dataset.ref });
      // 切换后立即用新账号拉一次余额，验证凭据可用 + 让用户看到切换生效
      const [prov, uid] = b.dataset.ref.split(':');
      let verify = '';
      try {
        const q = await invoke('quota_refresh', { provider: prov, uid });
        verify = q.remaining != null ? (t('balance') + fmt(q.remaining)) : '';
      } catch (e) { verify = t('cred_check_failed') + String(e).slice(0, 80); }
      toast(t('switched_account') + verify, 4200);
      await refreshGatewayAccounts();
    } catch (e) { toast(t('switch_failed') + e, 4200); b.disabled = false; }
  }));
}

function bindGwSave(btnId, inputId, field) {
  const btn = document.getElementById(btnId);
  const input = document.getElementById(inputId);
  if (!btn || !input) return;
  btn.addEventListener('click', async () => {
    btn.disabled = true;
    try {
      const val = Number(input.value);
      if (field === 'port' && !(val >= 1 && val <= 65535)) { toast(t('port_invalid'), 4200); return; }
      const r = await invoke('gateway_set_limits', { [field]: val });
      if (field === 'port') toast(r.needsRestart ? t('port_saved_restart') : t('port_saved'));
      else toast(t('rate_saved'));
      await refreshGateway();
    } catch (e) { toast(t('save_failed') + e, 4200); }
    btn.disabled = false;
  });
}

// 请求日志：时间格式化 / 耗时格式化 / 列表与统计渲染（刷新按钮复用）
function fmtLogTime(at) {
  if (!at) return '—';
  return String(at).replace('T', ' ').replace(/\.\d+/, '').replace(/([+-]\d{2}:\d{2}|Z)$/, '');
}
function fmtLogDuration(ms) {
  if (ms == null || ms === '') return '—';
  const n = Number(ms);
  if (!Number.isFinite(n)) return '—';
  return n < 1000 ? n + 'ms' : (n / 1000).toFixed(2) + 's';
}
function buildLogRows(logs) {
  return (logs || []).slice(0, 30).map(l => {
    const detail = t('log_time') + fmtLogTime(l.at) + t('log_duration') + fmtLogDuration(l.duration) + ' · ' + l.platform +
      (l.account ? ' · ' + l.account : '') + (l.message ? ' · ' + l.message : '');
    return '<div class="route-row" style="font-size:12.5px" title="' + esc(detail) + '">' +
      '<div style="flex:1;min-width:0;display:flex;align-items:center;gap:8px">' +
      '<span style="font-weight:700;flex:0 0 auto">' + (l.ok ? '<span style="color:var(--green,#16a34a)">✓</span>' : '<span style="color:var(--red,#dc2626)">✗</span>') + '</span>' +
      '<div style="min-width:0"><strong style="font-size:12.5px"><code style="font-size:12px">' + esc(l.model || t('no_model')) + '</code></strong>' +
      '<div class="subtle" style="margin-top:2px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' +
      t('log_time') + esc(fmtLogTime(l.at)) +
      t('log_duration') + esc(fmtLogDuration(l.duration)) +
      ' · ' + esc(l.platform) +
      (l.account ? ' · ' + esc(l.account) : '') + (l.message && !l.ok ? ' · <span style="color:var(--red,#dc2626)">' + esc(l.message) + '</span>' : '') + '</div></div></div>' +
      '</div>';
  }).join('') || '<div class="empty" style="padding:16px">' + t('no_logs') + '</div>';
}
function buildLogStats(st) {
  const s = st?.logStats || { total: 0, ok: 0, fail: 0 };
  return t('log_stats', { n: Math.min(st?.logs?.length || 0, 30), t: s.total, ok: s.ok, fail: s.fail });
}

function renderRouting() {
  const el = $('#route-matrix');
  if (!GW_STATUS) { el.innerHTML = '<div class="empty">' + t('gw_read_failed') + '</div>'; return; }
  const st = GW_STATUS;
  const logStats = st.logStats || { total: 0, ok: 0, fail: 0 };
  const curMeta = GW_PLATFORMS[st.platform] || {};

  const logRows = buildLogRows(st.logs);

  el.innerHTML =
    // ===== 网关卡 =====
    '<section class="surface route-card">' +
    '<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap">' +
    '<div style="flex:1;min-width:200px"><h3 style="font-size:14px">' + t('gw_title') + '</h3>' +
    '<div style="display:flex;align-items:center;gap:8px;margin-top:8px;flex-wrap:wrap">' +
    '<code style="background:var(--accent-soft);color:var(--accent);padding:8px 14px;border-radius:9px;font-size:13.5px;font-weight:600;max-width:100%;overflow-wrap:anywhere">' + esc(st.baseUrl) + '/v1</code>' +
    '<button class="small" id="gw-copy-url">' + t('copy') + '</button>' +
    '<span class="subtle" style="font-size:11.5px">GET /v1/models · POST /v1/chat/completions</span></div>' +
    '<p class="subtle" style="font-size:11.5px;margin-top:8px">' + t('models_follow_platform') + '</p></div></div>' +
    '<div class="route-grid">' +
    '<div class="route-field"><label class="subtle route-field-label">' + t('port') + ' <span class="route-field-hint">' + t('port_restart_hint') + '</span></label>' +
    '<div style="display:flex;gap:6px"><input id="gw-port" type="number" min="1" max="65535" value="' + st.port + '" style="flex:1;min-width:0;padding:7px 10px;border-radius:8px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-port-save" class="small primary">' + t('save') + '</button></div></div>' +
    '<div class="route-field"><label class="subtle route-field-label">' + t('rate_limit') + ' <span class="route-field-hint">' + t('rate_limit_hint') + '</span></label>' +
    '<div style="display:flex;gap:6px"><input id="gw-rpm" type="number" min="0" value="' + (st.rpm || 0) + '" style="flex:1;min-width:0;padding:7px 10px;border-radius:8px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-rpm-save" class="small primary">' + t('save') + '</button></div></div>' +
    '<div class="route-field"><label class="subtle route-field-label">' + t('strict_current_only') + '</label>' +
    '<div style="display:flex;align-items:center;gap:10px;margin-top:6px"><label class="switch"><input type="checkbox" id="gw-strict" ' + (st.strict !== false ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label>' +
    '<span class="subtle" style="font-size:11px">' + t('strict_hint') + '</span></div></div>' +
    '</div></section>' +

    // ===== 请求平台卡（全部整合进下拉框）=====
    '<section class="surface route-card">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">' +
    '<h3 style="font-size:14px">' + t('request_platform') + ' <span class="subtle" style="font-size:11.5px;font-weight:400">' + t('bare_model_hint') + '</span></h3>' +
    '<select id="route-plat-select" style="padding:8px 12px;border-radius:9px;border:1px solid var(--line);font:inherit;min-width:0;flex:1 1 220px;max-width:100%">' +
    Object.entries(GW_PLATFORMS).map(([name, meta]) => '<option value="' + name + '"' + (st.platform === name ? ' selected' : '') + '>' + esc(meta.label()) + ' · ' + t('n_accounts_suffix', { n: (st.accounts || {})[name] || 0 }) + (meta.chat ? '' : t('signin_only')) + '</option>').join('') +
    '</select></div>' +
    '<div style="display:flex;align-items:center;gap:12px;margin-top:14px;padding:12px 14px;background:var(--accent-soft,#eef6ff);border-radius:11px">' +
    picon(st.platform) +
    '<div style="flex:1"><strong style="font-size:13.5px">' + esc(curMeta.label ? curMeta.label() : st.platform) + '</strong>' +
    '<div class="subtle" style="margin-top:3px;font-size:12px">' + t('provider_label') + esc(curMeta.provider || st.platform) + ' · ' + t('available_accounts', { n: (st.accounts || {})[st.platform] || 0 }) +
    (curMeta.chat ? '' : t('no_chat_protocol')) + '</div></div>' +
    '<span class="tag" style="background:var(--accent);color:#fff">' + t('current') + '</span></div>' +
    '</section>' +

    // ===== 账号切换卡 =====
    '<section class="surface route-card">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">' +
    '<h3 style="font-size:14px">' + t('account_switch') + ' <span class="subtle" style="font-size:11.5px;font-weight:400">' + t('account_switch_hint') + '</span></h3>' +
    '<button id="gw-acct-next" class="small primary">' + t('switch_next') + '</button></div>' +
    '<div id="gw-acct-list" style="display:grid;gap:6px;margin-top:12px"><span class="subtle">' + t('loading') + '</span></div>' +
    '</section>' +

    // ===== 模型测试卡 =====
    '<section class="surface route-card">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">' +
    '<h3 style="font-size:14px">' + t('model_test') + ' <span class="subtle" style="font-size:11.5px;font-weight:400">' + t('model_test_hint') + '</span></h3>' +
    '<button id="gw-test-refresh" class="small">' + t('refresh_models') + '</button></div>' +
    '<div style="display:flex;gap:8px;margin-top:12px;align-items:center;flex-wrap:wrap">' +
    '<select id="gw-test-model" style="flex:1 1 240px;min-width:0;max-width:100%;padding:8px 12px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">' + t('model_placeholder') + '</option></select>' +
    '</div>' +
    '<textarea id="gw-test-msg" rows="2" placeholder="' + t('test_msg_placeholder') + '" style="width:100%;margin-top:10px;padding:10px 12px;border-radius:9px;border:1px solid var(--line);font:inherit;resize:vertical"></textarea>' +
    '<div style="display:flex;align-items:center;gap:10px;margin-top:10px">' +
    '<button id="gw-test-send" class="small primary">' + t('send_test') + '</button>' +
    '<span id="gw-test-status" class="subtle" style="font-size:12px"></span></div>' +
    '<div id="gw-test-result" style="margin-top:12px;padding:12px 14px;border-radius:11px;background:var(--surface-2,#f7f7fa);min-height:44px;font-size:13px;white-space:pre-wrap;word-break:break-word;display:none"></div>' +
    '</section>' +

    // ===== 请求日志卡 =====
    '<section class="surface route-card">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px"><h3 style="font-size:14px">' + t('request_log') +
    '<span class="subtle" id="gw-logs-stats" style="font-size:11.5px;font-weight:400">' + buildLogStats(st) + '</span></h3>' +
    '<div style="display:flex;align-items:center;gap:8px">' +
    '<button id="gw-logs-refresh" class="small">' + t('refresh') + '</button>' +
    ((logStats.total || 0) > 0 ? '<button id="gw-logs-clear" class="small danger-ghost">' + t('clear_log') + '</button>' : '') + '</div></div>' +
    '<div id="gw-logs-list" style="margin-top:10px">' + logRows + '</div></section>';

  // ===== 账号切换卡 =====
  refreshGatewayAccounts();
  el.querySelector('#gw-acct-next')?.addEventListener('click', async () => {
    const d = await invoke('gateway_accounts').catch(() => null);
    if (!d || !d.accounts?.length) { toast(t('no_accounts_current')); return; }
    const cur = d.accounts.findIndex(a => a.active);
    const next = d.accounts[(cur + 1) % d.accounts.length];
    try {
      await invoke('gateway_switch_account', { accountRef: next.ref });
      // 同步拉余额验证新账号
      const [prov, uid] = next.ref.split(':');
      let verify = '';
      try { const q = await invoke('quota_refresh', { provider: prov, uid }); verify = q.remaining != null ? (t('balance') + fmt(q.remaining)) : ''; }
      catch (e) { verify = t('cred_check_failed') + String(e).slice(0, 80); }
      toast(t('switched_account') + next.name + verify, 4200);
      await refreshGatewayAccounts();
    } catch (e) { toast(t('switch_failed') + e, 4200); }
  });

  // ===== 绑定 =====
  el.querySelector('#gw-copy-url')?.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(st.baseUrl + '/v1'); toast(t('copied_url')); } catch (e) { toast(t('copy_failed') + e); }
  });
  const platSelect = el.querySelector('#route-plat-select');
  if (platSelect) platSelect.addEventListener('change', async () => {
    const p = platSelect.value;
    platSelect.disabled = true;
    try {
      // 切换前检查客户端是否运行，未运行自动启动
      const er = await invoke('platform_ensure_running', { platform: p }).catch(() => null);
      if (er && er.launched) toast(t('client_not_running', { p: GW_PLATFORMS[p] ? GW_PLATFORMS[p].label() : p }));
      await invoke('gateway_set_platform', { platform: p });
      toast(t('switched_platform') + (GW_PLATFORMS[p] ? GW_PLATFORMS[p].label() : p));
      await refreshGateway(); renderRouting();
    } catch (e) { toast(t('switch_failed') + e, 4200); renderRouting(); }
  });
  // 请求日志刷新：只重取最新日志并就地更新列表/统计，不重渲染整页（保留输入状态）
  const refreshLogsBtn = el.querySelector('#gw-logs-refresh');
  if (refreshLogsBtn) refreshLogsBtn.addEventListener('click', async () => {
    refreshLogsBtn.disabled = true;
    const old = refreshLogsBtn.textContent;
    refreshLogsBtn.textContent = t('refreshing_logs');
    try {
      await refreshGateway();
      if (!GW_STATUS) { toast(t('gw_read_failed'), 4200); return; }
      const listEl = el.querySelector('#gw-logs-list');
      const statsEl = el.querySelector('#gw-logs-stats');
      if (listEl) listEl.innerHTML = buildLogRows(GW_STATUS.logs);
      if (statsEl) statsEl.innerHTML = buildLogStats(GW_STATUS);
      toast(t('logs_refreshed'));
    } catch (e) { toast(t('refresh_failed') + e, 4200); }
    finally {
      refreshLogsBtn.disabled = false;
      refreshLogsBtn.textContent = old;
    }
  });
  const clearBtn = el.querySelector('#gw-logs-clear');
  if (clearBtn) clearBtn.addEventListener('click', async () => {
    if (!await confirmBox(t('clear_logs_title'), t('clear_logs_msg'), { okText: t('clear_btn'), danger: true })) return;
    clearBtn.disabled = true;
    try {
      const r = await invoke('gateway_clear_logs');
      toast(t('cleared_n', { n: r.cleared || 0 }));
      await refreshGateway(); renderRouting();
    } catch (e) { toast(t('clear_failed') + e, 4200); }
    clearBtn.disabled = false;
  });
  bindGwSave('gw-port-save', 'gw-port', 'port');
  bindGwSave('gw-rpm-save', 'gw-rpm', 'rpm');
  const strictChk = el.querySelector('#gw-strict');
  if (strictChk) strictChk.addEventListener('change', async () => {
    strictChk.disabled = true;
    try {
      await invoke('gateway_set_limits', { strict: strictChk.checked });
      toast(strictChk.checked ? t('strict_on') : t('strict_off'));
      await refreshGateway();
    } catch (e) { strictChk.checked = !strictChk.checked; toast(t('set_failed') + e, 4200); }
    strictChk.disabled = false;
  });

  // ===== 模型测试 =====
  const modelSel = el.querySelector('#gw-test-model');
  const statusEl = el.querySelector('#gw-test-status');
  const resultEl = el.querySelector('#gw-test-result');
  const fetchModels = async () => {
    statusEl.textContent = t('fetching_models');
    try {
      const r = await fetch(st.baseUrl + '/v1/models').then(x => x.json());
      // /v1/models 只返回当前请求平台的模型（跟随「请求平台」下拉框）——直接用返回的 id 作为请求模型
      const models = (r.data || []).map(m => m.id).filter(Boolean);
      if (!models.length) {
        modelSel.innerHTML = '<option value="">' + t('no_models') + '</option>';
        // 码道：STS 可能过期 —— 提供一键同步客户端最新会话
        if (st.platform === 'codearts' || st.platform === 'codearts-desktop') {
          statusEl.innerHTML = t('codearts_expired') + '<button id="gw-ca-sync" class="small primary" style="margin-left:6px">' + t('sync_from_client') + '</button>';
          statusEl.querySelector('#gw-ca-sync')?.addEventListener('click', async (ev) => {
            ev.target.disabled = true; ev.target.textContent = t('syncing');
            try {
              const sr = await invoke('codearts_sync_local');
              if (sr.synced?.length) {
                toast(t('sync_ok', { names: sr.synced.join(' / ') }));
                fetchModels();
              } else {
                toast(t('sync_no_login'));
              }
            } catch (e) { toast(t('sync_failed') + e, 4200); }
            ev.target.disabled = false; ev.target.textContent = t('sync_from_client');
          });
        } else {
          statusEl.textContent = t('no_accounts_or_expired');
        }
        return;
      }
      modelSel.innerHTML = models.map(m => '<option value="' + esc(m) + '">' + esc(m) + '</option>').join('');
      statusEl.textContent = t('n_models', { n: models.length });
    } catch (e) { statusEl.textContent = t('fetch_models_failed') + e; }
  };
  el.querySelector('#gw-test-refresh')?.addEventListener('click', fetchModels);
  // 进页面自动获取一次
  fetchModels();
  el.querySelector('#gw-test-send')?.addEventListener('click', async () => {
    const model = modelSel.value;
    const msg = (el.querySelector('#gw-test-msg')?.value || '').trim();
    const sendBtn = el.querySelector('#gw-test-send');
    if (!model) { toast(t('select_model_first'), 4200); return; }
    if (!msg) { toast(t('input_test_msg'), 4200); return; }
    sendBtn.disabled = true;
    statusEl.textContent = t('requesting');
    resultEl.style.display = 'block';
    resultEl.style.color = 'var(--subtle,#7f7d91)';
    resultEl.textContent = t('waiting_response');
    const t0 = Date.now();
    try {
      const resp = await fetch(st.baseUrl + '/v1/chat/completions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...(st.apiKey ? { 'Authorization': 'Bearer ' + st.apiKey } : {}) },
        body: JSON.stringify({ model, messages: [{ role: 'user', content: msg }], stream: false }),
      });
      const data = await resp.json().catch(() => ({}));
      const secs = ((Date.now() - t0) / 1000).toFixed(1);
      if (data.error) {
        resultEl.style.color = 'var(--red,#dc2626)';
        resultEl.textContent = t('test_failed', { s: secs }) + (data.error.message || JSON.stringify(data.error));
        statusEl.textContent = '';
      } else {
        const content = data.choices?.[0]?.message?.content || JSON.stringify(data).slice(0, 500);
        const u = data.usage || {};
        const comp = u.completion_tokens ?? 0;
        const speed = comp > 0 && Number(secs) > 0 ? (comp / Number(secs)).toFixed(1) + ' tok/s' : '—';
        const usage = u.total_tokens != null ? t('usage_tokens', { in: u.prompt_tokens ?? 0, out: comp, total: u.total_tokens }) + speed : '';
        resultEl.style.color = 'var(--ink,#25252f)';
        resultEl.textContent = t('test_ok', { model, s: secs, usage }) + '\n\n' + content;
        statusEl.textContent = '';
      }
    } catch (e) {
      resultEl.style.color = 'var(--red,#dc2626)';
      resultEl.textContent = t('request_error') + e;
      statusEl.textContent = '';
    }
    sendBtn.disabled = false;
  });
}
function showRuleModal() {
  const accs = SNAP?.accounts || [];
  const accOptions = accs.map(a => '<option value="' + esc(a.provider + ':' + a.uid) + '">' + esc(platName(a.provider) + ' · ' + (a.name || a.uid.slice(0, 10))) + '</option>').join('');
  const platOptions = Object.entries(GW_PLATFORMS).map(([k, v]) => '<option value="' + k + '">' + esc(v.label()) + '</option>').join('');
  const mask = document.createElement('div');
  mask.className = 'modal-mask'; mask.style.display = 'flex';
  mask.innerHTML = '<div class="modal"><h3>' + t('rule_modal_title') + '</h3>' +
    '<div class="desc">' + t('rule_modal_desc') + '</div>' +
    '<div style="display:grid;gap:10px;margin:12px 0">' +
    '<label class="subtle">' + t('rule_model_prefix') + '<input id="rule-model" placeholder="' + t('rule_model_ph') + '" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"></label>' +
    '<label class="subtle">' + t('rule_target_platform') + '<select id="rule-platform" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">' + t('rule_follow_platform') + '</option>' + platOptions + '</select></label>' +
    '<label class="subtle">' + t('rule_account') + '<select id="rule-account" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">' + t('rule_default_account') + '</option>' + accOptions + '</select></label>' +
    '</div><div class="foot"><button id="rule-cancel">' + t('cancel') + '</button><button id="rule-save" class="primary">' + t('save') + '</button></div></div>';
  document.body.appendChild(mask);
  mask.querySelector('#rule-cancel').addEventListener('click', () => mask.remove());
  mask.querySelector('#rule-save').addEventListener('click', async () => {
    const modelPattern = mask.querySelector('#rule-model').value.trim() || '*';
    const platform = mask.querySelector('#rule-platform').value;
    const account = mask.querySelector('#rule-account').value;
    try {
      await invoke('gateway_add_rule', { modelPattern, platform, account, weight: 1, enabled: true });
      toast(t('rule_saved'));
      mask.remove();
      await refreshGateway(); renderRouting();
    } catch (e) { toast(t('save_failed') + e, 4200); }
  });
}

// ===== 设置 =====
let settingsDraft = null;
function renderSettings() {
  settingsDraft = { ...SETTINGS };
  const body = $('#settings-body');
  const tab = document.querySelector('#settings-tabs button.active')?.dataset.tab || 'tasks';
  // 保存/取消按钮只在「数据与版本」显示，其余页改动即保存
  const footer = document.querySelector('.save-footer');
  if (footer) footer.style.display = (tab === 'data') ? 'flex' : 'none';
  if (tab === 'tasks') {
    body.innerHTML = '<div class="task-settings"><div>' +
      '<div class="setting-row"><div><h3>' + t('auto_checkin') + '</h3><p>' + t('auto_checkin_desc') + '</p></div>' + sw('auto', settingsDraft.auto) + '</div>' +
      '<div class="setting-row"><div><h3>' + t('catchup') + '</h3><p>' + t('catchup_desc') + '</p></div>' + sw('catchup', settingsDraft.catchup) + '</div>' +
      '<div class="setting-row"><div><h3>' + t('refresh_interval') + '</h3><p>' + t('refresh_interval_desc') + '</p></div>' +
      '<select data-set="refreshMinutes"><option value="30"' + (settingsDraft.refreshMinutes === 30 ? ' selected' : '') + '>' + t('m30') + '</option>' +
      '<option value="60"' + (settingsDraft.refreshMinutes === 60 ? ' selected' : '') + '>' + t('m60') + '</option>' +
      '<option value="120"' + (settingsDraft.refreshMinutes === 120 ? ' selected' : '') + '>' + t('m120') + '</option></select></div>' +
      '<div class="setting-row"><div><h3>' + t('sched_time') + '</h3><p>' + t('sched_time_desc') + '</p></div>' +
      '<input type="time" data-set="time" value="' + esc(settingsDraft.time) + '" style="padding:7px 10px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
      '<label class="switch" title="' + t('random_time') + '"><input type="checkbox" data-setb="randomTime" ' + (settingsDraft.randomTime ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label></div>' +
      '</div><div class="settings-preview"><h3 class="spaced" style="margin-bottom:6px">' + t('exec_preview') + '</h3>' +
      '<p class="subtle">' + t('saved_time_is', { v: esc(SETTINGS.randomTime ? t('random') : SETTINGS.time) }) + '</p>' +
      (SNAP?.accounts || []).slice(0, 8).map(a => '<div class="preview-row">' + picon(a.provider) + '<strong>' + esc(platName(a.provider)) + '</strong><span class="subtle" style="margin-left:auto">' + t('every_day', { v: esc(settingsDraft.randomTime ? t('random') : settingsDraft.time) }) + '</span></div>').join('') +
      '<p class="notice">' + t('notice_beijing') + '</p>' +
      '</div></div>';
  } else if (tab === 'tools') {
    renderToolsTab(body);
  } else if (tab === 'general') {
    const loc = (window.I18n && I18n.getLocale()) || 'zh-CN';
    body.innerHTML =
      '<div class="setting-row"><div><h3>' + t('language') + '</h3><p>' + t('language_desc') + '</p></div>' +
      '<select id="locale-select"><option value="zh-CN"' + (loc === 'zh-CN' ? ' selected' : '') + '>' + t('lang_zh') + '</option>' +
      '<option value="en-US"' + (loc === 'en-US' ? ' selected' : '') + '>' + t('lang_en') + '</option></select></div>' +
      '<div class="setting-row"><div><h3>' + t('appearance') + '</h3><p>' + t('appearance_desc') + '</p></div>' +
      '<select id="theme-select"><option value="system"' + (THEME_MODE === 'system' ? ' selected' : '') + '>' + t('theme_system') + '</option>' +
      '<option value="light"' + (THEME_MODE === 'light' ? ' selected' : '') + '>' + t('theme_light') + '</option>' +
      '<option value="dark"' + (THEME_MODE === 'dark' ? ' selected' : '') + '>' + t('theme_dark') + '</option></select></div>' +
      '<div class="setting-row"><div><h3>' + t('base_usage_stats') + '</h3><p>' + t('base_usage_desc') + '</p></div>' + sw('telemetry', settingsDraft.telemetry) + '</div>' +
      '<p class="notice">' + t('privacy_notice') + '</p>';
    const locSel = body.querySelector('#locale-select');
    if (locSel) locSel.addEventListener('change', () => {
      if (window.I18n) I18n.setLocale(locSel.value);
      renderSettings(); // 用新语言重绘当前 tab
    });
    const themeSel = body.querySelector('#theme-select');
    if (themeSel) themeSel.addEventListener('change', () => { applyTheme(themeSel.value, true); });
  } else {
    body.innerHTML =
      '<div class="setting-row"><div><h3>' + t('app_name') + ' <span id="ver-current">…</span></h3><p>' + t('app_version') + '</p></div><span class="tag">' + t('local_edition') + '</span></div>' +
      '<div class="setting-row"><div><h3>' + t('software_update') + '</h3><p id="upd-status">' + t('checking_release') + '</p></div>' +
      '<div class="actions" style="display:flex;gap:6px;align-items:center;justify-content:flex-end;flex-shrink:0">' +
      '<span class="subtle" id="upd-progress"></span>' +
      '<button id="upd-check" class="small">' + t('recheck') + '</button>' +
      '<button id="upd-download" class="small primary" style="display:none">' + t('download_update') + '</button>' +
      '<button id="upd-install" class="small primary" style="display:none">' + t('restart_install') + '</button>' +
      '</div></div>' +
      '<div class="setting-row"><div><h3>' + t('data_dir') + '</h3><p>' + t('data_dir_desc') + '</p></div>' +
      '<button data-open-db class="small">' + t('open_dir') + '</button></div>' +
      '<p class="notice">' + t('db_notice') + '</p>';
    body.querySelector('[data-open-db]').addEventListener('click', async () => {
      try { await invoke('account_list_all'); toast(t('accounts_records', { n: SNAP?.accounts?.length || 0 })); } catch {}
    });
    // ===== 自动更新流程 =====
    (async () => {
      const statusEl = body.querySelector('#upd-status');
      const dlBtn = body.querySelector('#upd-download');
      const instBtn = body.querySelector('#upd-install');
      const progEl = body.querySelector('#upd-progress');
      let updateInfo = null;
      const run = async () => {
        try {
          statusEl.textContent = t('checking_update');
          updateInfo = await invoke('check_update');
          body.querySelector('#ver-current').textContent = 'v' + updateInfo.currentVersion;
          if (updateInfo.updateAvailable) {
            statusEl.innerHTML = t('new_version_found', { latest: esc(updateInfo.latestVersion), current: esc(updateInfo.currentVersion) });
            if (updateInfo.assetUrl) {
              dlBtn.style.display = '';
            } else {
              statusEl.innerHTML += ' · <a href="' + esc(updateInfo.releaseUrl) + '" target="_blank" style="color:var(--accent)">' + t('goto_release') + '</a>';
            }
          } else {
            statusEl.textContent = t('already_latest', { v: updateInfo.currentVersion });
          }
        } catch (e) { statusEl.textContent = t('check_failed') + e; }
      };
      run();
      body.querySelector('#upd-check')?.addEventListener('click', run);
      dlBtn?.addEventListener('click', async () => {
        if (!updateInfo?.assetUrl) return;
        dlBtn.disabled = true; dlBtn.textContent = t('downloading');
        progEl.textContent = '';
        try {
          const r = await invoke('download_update', { url: updateInfo.assetUrl });
          window.__updFile = r.file;
          instBtn.style.display = '';
          dlBtn.style.display = 'none';
          toast(t('download_done_toast', { mb: Math.round(r.bytes / 1024 / 1024) }));
        } catch (e) { toast(t('download_failed') + e, 5000); }
        dlBtn.disabled = false; dlBtn.textContent = t('download_update');
      });
      instBtn?.addEventListener('click', async () => {
        const file = (await invoke('check_update').then(i => null).catch(() => null), null) || (window.__updFile || '');
        // 文件路径从下载结果拿（暂存全局）
        instBtn.disabled = true;
        try {
          await invoke('install_update', { file: window.__updFile || '' });
        } catch (e) { toast(t('install_failed') + e, 5000); instBtn.disabled = false; }
      });
      // 下载进度事件
      try {
        const listen = _T?.event?.listen;
        if (listen) await listen('update-download', (e) => {
          const d = e.payload || {};
          progEl.textContent = d.progress + '%（' + Math.round((d.downloaded || 0) / 1024 / 1024) + ' MB）';
        });
      } catch {}
    })();
  }
  body.querySelectorAll('input[data-setb], input[data-sw]').forEach(wireSetting);
  body.querySelectorAll('[data-set]').forEach(el => el.addEventListener('change', () => {
    settingsDraft[el.dataset.set] = el.dataset.set === 'refreshMinutes' ? Number(el.value) : el.value;
  }));
}
let TOOLS = null;
let TOOLS_QUOTA_REFRESHED = false; // 防止 quota 刷新 → 重绘 → 再刷新 的死循环
async function renderToolsTab(body) {
  // 立即渲染骨架（每平台一行占位），数据回来后原地更新 —— 不再整页"加载平台信息…"
  const SKELETON_META = { trae: t('tool_trae'), workbuddy: t('tool_workbuddy'), minimax: t('tool_minimax'), qoder: t('tool_qoder'), zcode: t('tool_zcode'), codearts: t('tool_codearts') };
  body.innerHTML = Object.keys(SKELETON_META).map(p =>
    '<div class="setting-row" data-tool="' + p + '"><div><h3>' + picon(p) + ' ' + SKELETON_META[p] + '</h3><p class="subtle">' + t('detect_state') + '</p></div>' +
    '<div class="actions"><span class="subtle" style="font-size:11.5px">…</span></div></div>').join('') +
    '<div class="setting-row"><div><h3>' + t('tools_dl_path') + '</h3><p class="subtle">…</p></div></div>';
  // 后台探测 MiniMax 直链缓存，不阻塞渲染
  invoke('tools_refresh_minimax_url').catch(() => {});
  if (!TOOLS) {
    try { TOOLS = await invoke('tools_info'); } catch (e) { body.innerHTML = '<div class="empty">' + t('tools_load_failed') + esc(String(e)) + '</div>'; return; }
  }
  // 每次进入页面只后台刷一次额度，完成后仅更新卡片额度文字（不重绘整个列表，避免打断操作）
  if (!TOOLS_QUOTA_REFRESHED) {
    TOOLS_QUOTA_REFRESHED = true;
    invoke('quota_refresh_all').then(async () => {
      try {
        TOOLS = await invoke('tools_info');
        SNAP = await invoke('snapshot').catch(() => SNAP);
        const tabsEl = document.querySelector('#settings-tabs');
        if (tabsEl?.querySelector('[data-tab="tools"]')?.classList.contains('active')) {
          const body2 = document.querySelector('#settings-body');
          if (body2) { const keep = TOOLS; TOOLS = null; renderToolsTab(body2); TOOLS = keep; }
        }
      } catch {}
    }).catch(() => {});
  }
  const dlDir = TOOLS.installDir || '';
  const items = TOOLS.items || [];
  const meta = SKELETON_META;
  body.innerHTML = '<div class="setting-row"><div><h3>' + t('tools_one_click') + '</h3><p>' + t('tools_one_click_desc') + '</p></div></div>' +
    '<div class="setting-row"><div><h3>' + t('tools_dl_path') + '</h3><p>' + t('tools_dl_path_desc') + '</p></div>' +
    '<div style="display:flex;gap:6px;min-width:340px"><input id="tools-dir" type="text" value="' + esc(dlDir) + '" placeholder="' + t('tools_dl_path_ph') + '" style="flex:1;padding:8px 10px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
    '<button id="tools-dir-save" class="small primary">' + t('save') + '</button></div></div>' +
    items.map(it => {
      const dl = (TOOLS.downloads || []).find(d => d.platform === it.platform) || {};
      const stMap = { downloading: t('dl_state_downloading', { n: dl.progress || 0 }), downloaded: t('dl_state_downloaded'), installing: t('dl_state_installing'), installed: t('dl_state_installed'), install_timeout: t('dl_state_timeout') };
      const st = stMap[dl.status] || '';
      const qtxt = it.accounts > 0 ? t('installed_accounts', { n: it.accounts }) + (it.quotaTotal != null ? t('remaining_quota') + fmt(it.quotaTotal) : '') : t('no_account_added');
      return '<div class="setting-row" data-tool="' + it.platform + '"><div><h3>' + picon(it.platform) + ' ' + (meta[it.platform] || it.platform) + '</h3>' +
        '<p>' + (it.direct ? t('direct_link') + esc(it.url.split('/').pop() || '') : t('no_direct_link')) + qtxt + '</p></div>' +
        '<div class="actions" style="display:flex;gap:6px;align-items:center;flex-wrap:wrap;justify-content:flex-end">' +
        '<span data-status style="font-size:11.5px;font-weight:600;' + (it.installed ? 'color:var(--green,#16a34a)' : 'color:var(--subtle,#7f7d91)') + '">' + (it.installed ? t('installed_yes') : t('not_detected')) + st + '</span>' +
        (it.direct ? '<button data-dl="' + it.platform + '" class="small ' + (it.installed ? '' : 'primary') + '">' + (it.installed ? t('reinstall') : t('one_click_install')) + '</button>' : '<button data-page="' + it.platform + '" class="small">' + t('open_dl_page') + '</button>') +
        '<button data-setdir="' + it.platform + '" class="small" title="' + t('specify_dir_tip') + '">' + t('specify_dir') + '</button>' +
        '</div></div>';
    }).join('') +
    '<p class="notice">' + t('tools_notice') + '</p>' +
    '<div class="setting-row" style="flex-direction:column;align-items:flex-start;gap:6px">' +
    '<p style="font-size:12.5px;margin:0">' + t('open_source_repo') + '：<a id="repo-link" href="https://github.com/tttq/credit_linked" style="color:var(--accent,#4b7bec);word-break:break-all;cursor:pointer">https://github.com/tttq/credit_linked</a></p>' +
    '<p class="subtle" style="font-size:12px;line-height:1.7;margin:0">' + t('disclaimer') + '</p>' +
    '</div>';
  // 绑定
  // 开源仓库链接：Tauri webview 内 <a> 默认导航会被拦，走 opener 插件用系统浏览器打开
  const repoLink = body.querySelector('#repo-link');
  if (repoLink) repoLink.addEventListener('click', (e) => {
    if (openUrl) { e.preventDefault(); try { Promise.resolve(openUrl('https://github.com/tttq/credit_linked')).catch(() => {}); } catch {} }
  });
  body.querySelectorAll('[data-setdir]').forEach(b => b.addEventListener('click', async () => {
    const p = b.dataset.setdir;
    const dir = prompt(t('input_install_dir', { name: meta[p] || p }), '');
    if (dir == null) return;
    if (!dir.trim()) {
      try { await invoke('tools_set_install_dir', { platform: p, dir: '' }); toast(t('install_dir_cleared')); TOOLS = null; renderToolsTab(body); } catch (e) { toast(t('op_error') + e, 4200); }
      return;
    }
    b.disabled = true;
    try {
      const r = await invoke('tools_set_install_dir', { platform: p, dir: dir.trim() });
      toast(r.installed ? (t('imported') + r.exe) : t('install_dir_cleared'));
      TOOLS = null;
      renderToolsTab(body);
    } catch (e) { toast(t('import_failed') + e, 4200); }
    b.disabled = false;
  }));
  body.querySelectorAll('[data-dl]').forEach(b => b.addEventListener('click', async () => {
    const p = b.dataset.dl;
    const dir = body.querySelector('#tools-dir')?.value || '';
    b.disabled = true; b.textContent = t('processing');
    try {
      const r = await invoke('tools_download', { platform: p, installDir: dir });
      if (r.ok) {
        await invoke('tools_install', { platform: p, file: r.file });
        toast(t('silent_install_started', { name: meta[p] || p }));
      } else if (r.openPage) {
        await invoke('tools_open_page', { platform: p });
        toast(t('no_direct_open_page'));
      }
    } catch (e) { toast(t('dl_install_failed') + e, 5000); }
    b.disabled = false; b.textContent = t('one_click_install');
  }));
  body.querySelectorAll('[data-page]').forEach(b => b.addEventListener('click', async () => {
    try { await invoke('tools_open_page', { platform: b.dataset.page }); toast(t('dl_page_opened')); } catch (e) { toast(t('open_failed') + e); }
  }));
  // 下载进度事件（5% 步进）
  try {
    const ev = _T?.event?.listen || null;
    if (ev) await ev('tool-download', (e) => {
      const d = e.payload || {};
      if (!TOOLS) TOOLS = {};
      TOOLS.downloads = (TOOLS.downloads || []).filter(x => x.platform !== d.platform).concat([d]);
      const row = document.querySelector('#settings-body [data-tool="' + d.platform + '"] [data-status]');
      if (row) {
        const stMap2 = { downloading: t('dl_state_downloading', { n: d.progress || 0 }), downloaded: t('dl_state_downloaded'), installing: t('dl_state_installing'), installed: t('dl_state_installed'), install_timeout: t('dl_state_timeout') };
        row.textContent = stMap2[d.status] || '';
        if (d.status === 'installed' || d.status === 'install_timeout') toast((d.platform === 'qoder' ? 'Qoder' : d.platform) + t('install_flow_done'));
      }
    });
  } catch {}
}

function sw(key, checked) {
  return '<label class="switch"><input type="checkbox" data-setb="' + key + '" ' + (checked ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label>';
}
function wireSetting(el) {
  el.addEventListener('change', () => {
    settingsDraft[el.dataset.setb] = el.checked;
    autoSaveSettings();
  });
}
// 非「数据与版本」页：改动即保存
async function autoSaveSettings() {
  const tab = document.querySelector('#settings-tabs button.active')?.dataset.tab || 'tasks';
  if (tab === 'data') return;
  try {
    for (const [k, v] of Object.entries(settingsDraft)) {
      if (SETTINGS[k] !== v) { await invoke('settings_set', { key: k, value: String(v) }); SETTINGS[k] = v; }
    }
    applyRefreshInterval();
    toast(t('settings_saved'));
  } catch (e) { toast(t('save_failed') + e, 4200); }
}
$$('#settings-tabs button').forEach(b => b.addEventListener('click', () => {
  $$('#settings-tabs button').forEach(x => x.classList.toggle('active', x === b));
  renderSettings();
}));
$('#settings-cancel').addEventListener('click', () => { renderSettings(); toast(t('settings_reverted')); });
$('#settings-save').addEventListener('click', async () => {
  try {
    for (const [k, v] of Object.entries(settingsDraft)) {
      if (SETTINGS[k] !== v) await invoke('settings_set', { key: k, value: String(v) });
    }
    toast(t('settings_saved_sched'));
    applyRefreshInterval();
    await refreshAll();
    // 补跑：开启 catchup 且今日计划时间已过 → 后台调度器 60s 内接管（Rust auto_tick）
  } catch (e) { toast(t('save_failed') + e, 4200); }
});

// ===== 登录模态 =====
async function startLogin(p) {
  const meta = PLAT[p] || {};
  $('#login-title').textContent = t('login_title', { p: platName(p) });
  $('#login-desc').textContent = meta.desc || '';
  $('#login-body').innerHTML = '<div style="padding:18px 0"><span class="spin"></span>' + t('starting_auth') + '</div>';
  $('#login-mask').classList.add('show');
  clearInterval(loginTimer);
  try {
    const r = await invoke('login_start', { provider: p });
    $('#login-body').innerHTML =
      '<div style="font-size:12.5px;color:var(--subtle);margin-bottom:10px">' + t('auth_link_ready') + '</div>' +
      '<div style="word-break:break-all;font-size:12px;background:var(--bg);border-radius:8px;padding:10px;max-height:120px;overflow:auto">' + esc(r.url) + '</div>' +
      '<div id="login-status" style="margin-top:12px;font-size:13px"><span class="spin"></span> ' + t('waiting_auth') + '</div>';
    // 应用内授权子窗口（不是系统浏览器）：iframe 会被平台 X-Frame-Options 拒绝，Tauri 子窗口不受限
    await invoke('login_open_window', { sessionId: r.sessionId, url: r.url, provider: p });
    curLoginSid = r.sessionId;
    $('#login-open').textContent = t('refresh_auth_window');
    $('#login-open').onclick = async () => {
      await invoke('login_open_window', { sessionId: r.sessionId, url: r.url, provider: p });
    };
    loginTimer = setInterval(async () => {
      try {
        const s = await invoke('login_poll', { sessionId: r.sessionId });
        if (s.status === 'ready') {
          clearInterval(loginTimer);
          invoke('login_close_window', { sessionId: r.sessionId }).catch(() => {});
          curLoginSid = null;
          $('#login-body').innerHTML = '<div style="color:var(--green);font-weight:600;padding:16px 0">' + t('login_ok') + esc(s.account?.name || '') + ' · ' + t('fetching_quota') + '</div>';
          toast(t('login_ok_toast', { n: s.account?.name || '' }));
          // 登录后立即获取：余额 + 状态
          try { await invoke('quota_refresh', { provider: s.account.provider, uid: s.account.uid }); } catch {}
          await refreshAll();
          $('#login-mask').classList.remove('show');
          showPage('accounts');
        } else if (s.status === 'expired') {
          clearInterval(loginTimer);
          $('#login-status').innerHTML = '<span style="color:var(--amber)">' + t('auth_timeout') + '</span>';
        }
      } catch (e) {
        clearInterval(loginTimer);
        $('#login-status').innerHTML = '<span style="color:var(--red)">' + esc(String(e)) + '</span>';
      }
    }, 3000);
  } catch (e) {
    $('#login-body').innerHTML = '<div style="color:var(--red);padding:12px 0">' + esc(String(e)) + '</div>';
  }
}
$('#login-cancel').addEventListener('click', () => {
    clearInterval(loginTimer);
    invoke('login_cancel').catch(() => {});
    if (curLoginSid) { invoke('login_close_window', { sessionId: curLoginSid }).catch(() => {}); curLoginSid = null; }
    $('#login-open').textContent = t('open_auth_page');
    $('#login-mask').classList.remove('show');
  });

// ===== 顶部 =====
async function checkinAll(btn) {
  btn.disabled = true; const old = btn.textContent; btn.innerHTML = '<span class="spin"></span>' + t('checkin_running');
  try {
    const r = await invoke('checkin', { provider: '', uid: '', all: true });
    const ok = r.results.filter(x => x.status === 'success').length;
    const al = r.results.filter(x => x.status === 'already').length;
    const bad = r.results.filter(x => x.status === 'failed').length;
    toast(t('checkin_done_stats', { ok, al, bad }));
    await refreshAll();
  } catch (e) { toast(t('checkin_failed') + e); }
  btn.disabled = false; btn.textContent = old;
}
$('#btn-checkin-all').addEventListener('click', (e) => checkinAll(e.currentTarget));
$('#btn-checkin-all2').addEventListener('click', (e) => checkinAll(e.currentTarget));
async function quotaAll(btn) {
  btn.disabled = true; const old = btn.textContent; btn.textContent = t('refreshing');
  try {
    const accs = SNAP?.accounts || [];
    for (const a of accs) { try { await invoke('quota_refresh', { provider: a.provider, uid: a.uid }); } catch {} }
    toast(t('balance_refreshed'));
    await refreshAll();
  } catch (e) { toast(t('refresh_failed') + e); }
  btn.disabled = false; btn.textContent = old;
}
$('#btn-quota-all3').addEventListener('click', (e) => quotaAll(e.currentTarget));
$('#btn-quota-all2').addEventListener('click', (e) => quotaAll(e.currentTarget));
$$('[data-goto]').forEach(b => b.addEventListener('click', () => showPage(b.dataset.goto)));
window.addEventListener('resize', () => { charts.ov?.resize(); charts.st?.resize(); });
window.piconFallback = piconFallback;

// ===== 面板顶栏：最小化到托盘 / 关闭确认（模态在屏幕中间弹出） =====
(function setupPanelBar() {
  const curWin = () => { try { return _T?.window?.getCurrent?.() || _T?.window?.getCurrentWindow?.(); } catch { return null; } };
  const min = document.getElementById('panel-min');
  if (min) min.addEventListener('click', () => { try { curWin()?.hide?.(); } catch {} });
  const close = document.getElementById('panel-close');
  if (close) close.addEventListener('click', () => { document.getElementById('quit-mask').style.display = 'flex'; });
  // 右下角缩放手柄
  const handle = document.getElementById('resize-handle');
  if (handle) handle.addEventListener('mousedown', (e) => {
    e.preventDefault(); e.stopPropagation();
    const w = curWin();
    if (!w) return;
    try { if (typeof w.startResize === 'function') { w.startResize('SouthEast'); return; } } catch {}
    try { if (typeof w.startResizeDragging === 'function') { w.startResizeDragging('SouthEast'); } } catch {}
  });
})();

// ===== 关闭确认：退至后台 / 退出程序 =====
(function setupQuitDialog() {
  const mask = document.getElementById('quit-mask');
  if (!mask) return;
  const show = () => { mask.style.display = 'flex'; };
  const hide = () => { mask.style.display = 'none'; };
  mask.querySelector('#quit-cancel').addEventListener('click', hide);
  mask.querySelector('#quit-to-tray').addEventListener('click', () => {
    hide();
    try { _T?.window?.getCurrent?.()?.hide?.(); } catch { try { _T?.window?.getCurrentWindow?.()?.hide?.(); } catch {} }
  });
  mask.querySelector('#quit-exit').addEventListener('click', async () => {
    hide();
    try { await invoke('quit_app'); } catch {}
  });
  try {
    const listen = _T?.event?.listen;
    if (listen) listen('window-close-requested', show);
  } catch {}
})();

document.querySelector('.account-toolbar').style.display = 'none'; // 默认首页隐藏
refreshAll();
function applyRefreshInterval() {
  if (window.__refreshTimer) clearInterval(window.__refreshTimer);
  const mins = (SETTINGS && SETTINGS.refreshMinutes) || 60;
  window.__refreshTimer = setInterval(() => { if (SNAP) refreshAll(); }, Math.max(1, mins) * 60 * 1000);
}
window.__refreshTimer = setInterval(() => { if (SNAP) refreshAll(); }, 60000);
