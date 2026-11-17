// AI路由助手 前端逻辑
const _T = window.__TAURI__;
const invoke = _T?.core?.invoke || _T?.invoke;
const openUrl = _T?.opener?.openUrl || _T?.shell?.open || null;

const PLAT = {
  trae:      { name: 'Trae',      color: '#20b978', login: 'browser', desc: '将在本机 3379 端口起回调并打开 Trae 授权页，登录后自动回填。' },
  workbuddy: { name: 'WorkBuddy', color: '#00ba9c', login: 'browser', desc: '跳转腾讯 CodeBuddy 授权页，确认后自动取回凭据。' },
  minimax:   { name: 'MiniMax',   color: '#20b9ec', login: 'device',  desc: '打开 MiniMax 授权页并输入展示的确认码后，此处自动完成。' },
  qoder:     { name: 'Qoder',     color: '#606570', login: 'browser', desc: '打开 Qoder 设备授权页，选择账号授权后自动完成。' },
  codearts:  { name: '华为云码道', color: '#e93442', login: 'browser', desc: '打开码道授权页（IDE 通道），完成后自动换取 STS 凭据。' },
  'codearts-desktop': { name: '码道桌面', color: '#854cf5', login: 'browser', desc: '打开码道授权页（桌面通道），完成后自动换取 STS 凭据。' },
  zcode:     { name: 'ZCode',     color: '#2e7a6f', login: 'none',    desc: '仅支持扫描本机当前账号。' },
  doubao:    { name: '豆包',       color: '#c78f79', login: 'none',    desc: '走窗口驱动（后续版本接入）。' },
};
const NAV = [
  ['overview', '首页', icon('home')],
  ['accounts', '账号管理', icon('users')],
  ['checkin', '签到中心', icon('check')],
  ['statistics', '积分统计', icon('chart')],
  ['routing', '账号路由', icon('route')],
  ['settings', '设置', icon('gear')],
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

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const esc = (v) => { const d = document.createElement('div'); d.textContent = v == null ? '' : String(v); return d.innerHTML; };
function toast(msg, ms = 2800) { const t = $('#toast'); t.textContent = msg; t.style.display = 'block'; clearTimeout(t._h); t._h = setTimeout(() => t.style.display = 'none', ms); }
function platName(p) { return (PLAT[p] || {}).name || p; }
function platMeta(p) { return PLAT[p] || { color: '#456' }; }
function picon(p, cls) {
  const m = platMeta(p);
  const iconFile = { trae: 'trae.png', workbuddy: 'workbuddy.jpg', minimax: 'minimax.png', qoder: 'qoder.png', codearts: 'codearts.svg', 'codearts-desktop': 'codearts-desktop.png', doubao: 'doubao.ico', zcode: null }[p];
  if (iconFile) return '<img class="' + (cls || '') + '" src="platforms/' + iconFile + '" onerror="this.outerHTML=piconFallback(\'' + p + '\',\'' + (cls || '') + '\')">';
  return piconFallback(p, cls);
}
function piconFallback(p, cls) {
  const m = platMeta(p);
  return '<span class="picon ' + (cls || '') + '" style="background:' + m.color + '">' + esc(m.name ? m.name[0] : '?') + '</span>';
}
function stInfo(s) {
  const m = { success: ['st-ok','已领取'], already: ['st-ok','今日已领'], failed: ['st-bad','失败'],
    pending: ['st-warn','待确认'], 'needs-action': ['st-warn','需操作'], 'no-activity': ['st-idle','无活动'], idle: ['st-idle','待签到'], 'auth-expired': ['st-bad','授权过期'] };
  const [cls, txt] = m[s] || ['st-idle', s || '—'];
  return '<span class="status ' + cls + '"><span class="dot"></span>' + esc(txt) + '</span>';
}
function fmt(n) { return n == null ? '—' : (Math.round(n * 100) / 100).toLocaleString('zh-CN'); }
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
  $('#nav').innerHTML = NAV.map(([k, label, ic]) =>
    '<button data-page="' + k + '" class="' + (curPage === k ? 'selected' : '') + '">' + ic + '<span>' + label + '</span></button>').join('');
  $$('#nav button').forEach(b => b.addEventListener('click', () => showPage(b.dataset.page)));
}
function showPage(p) {
  curPage = p;
  $$('.page').forEach(s => s.classList.toggle('active', s.id === 'page-' + p));
  $$('#nav button').forEach(b => b.classList.toggle('selected', b.dataset.page === p));
  if (p === 'statistics') refreshStatistics();
  if (p === 'overview') drawOverview();
}

// ===== 数据 =====
async function refreshAll() {
  try {
    SNAP = await invoke('snapshot');
    SETTINGS = await invoke('settings_get');
    $('#cycle-label').textContent = '今日周期 ' + SNAP.cycle;
    $('#today-str').textContent = new Date().toLocaleDateString('zh-CN', { weekday: 'long', month: 'long', day: 'numeric' });
    $('#sched-time').textContent = SETTINGS.randomTime ? '随机' : SETTINGS.time;
    await refreshGateway();
  renderNav(); renderOverview(); renderAccounts(); renderCheckin(); refreshStatistics(); renderSettings(); renderRouting();
    if (curPage === 'overview') drawOverview();
    backfillQuotas();
    applyRefreshInterval();
  } catch (e) { toast('加载失败: ' + e, 4200); }
}

// ===== 首页 =====
function renderOverview() {
  const accs = SNAP.accounts || [];
  const grid = $('#tool-grid');
  if (!accs.length) { grid.innerHTML = '<div class="empty" style="grid-column:1/-1">还没有添加 AI 工具 —— 到「账号管理」添加账号</div>'; return; }
  grid.innerHTML = accs.map(a => {
    const m = platMeta(a.provider);
    const done = a.status === 'success' || a.status === 'already';
    const q = parseQuota(a);
    return '<section class="tool-card" data-provider="' + a.provider + '">' +
      '<div class="tool-identity">' + picon(a.provider) + '<div><h3>' + esc(platName(a.provider)) + '</h3><p class="subtle" title="' + esc(a.name) + '">' + esc(a.name || a.uid.slice(0, 10)) + '</p></div></div>' +
      '<div class="tool-balance"><span class="subtle">可用</span><strong>' + (q && q.remaining != null ? fmt(q.remaining) : '—') + '</strong></div>' +
      '<div class="tool-result">' + (done ? '<span class="checkin-done">✓ 已签到</span>' : stInfo(a.status)) +
      '<span class="subtle">' + esc(a.last_checkin ? '上次 ' + a.last_checkin.slice(5, 10) : '') + '</span></div></section>';
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
      legend: provs.length > 1 ? { data: provs.map(platName), bottom: 0, textStyle: { fontSize: 11, color: '#7f7d91' } } : undefined,
      grid: { left: 44, right: 14, top: 20, bottom: provs.length > 1 ? 42 : 26 },
      xAxis: { type: 'category', data: days.map(d => d.day.slice(overviewRange > 100 ? 2 : 5)), axisLabel: { fontSize: 10, color: '#7f7d91' } },
      yAxis: { type: 'value', axisLabel: { fontSize: 10, color: '#7f7d91' }, splitLine: { lineStyle: { color: '#eae9f1' } } },
      series: (provs.length ? provs : ['_']).map(p => ({
        name: p === '_' ? '签到积分' : platName(p), type: 'bar', stack: 'income',
        itemStyle: { color: p === '_' ? '#6862ed' : (colors[p] || '#6862ed'), borderRadius: p === '_' ? 4 : undefined, borderColor: '#fff', borderWidth: p === '_' ? 0 : 1 },
        data: days.map(d => p === '_' ? d.income : (d.by_provider?.[p] || 0)),
        barMaxWidth: 22,
      })),
    });
    const sum = days.reduce((s, d) => s + d.income, 0);
    $('#chart-sum').textContent = '期间签到积分 +' + fmt(sum);
  } catch (e) { toast('图表加载失败: ' + e); }
}
$$('#range-pills button').forEach(b => b.addEventListener('click', () => {
  overviewRange = Number(b.dataset.range);
  $$('#range-pills button').forEach(x => x.classList.toggle('active', x === b));
  drawOverview();
}));

// ===== 账号管理 =====
function renderAccounts() {
  const accs = SNAP.accounts || [];
  const plats = [...new Set(accs.map(a => a.provider))];
  $('#accounts-sub').textContent = '已连接 ' + plats.length + ' 个平台 · ' + accs.length + ' 个账号';
  const el = $('#account-list');
  if (!accs.length) { el.innerHTML = '<div class="empty">还没有账号 —— 点右上角各平台「添加账号」</div>'; return; }
  el.innerHTML = plats.map(p => {
    const list = accs.filter(a => a.provider === p);
    const m = platMeta(p);
    const multi = ['trae', 'workbuddy', 'minimax', 'qoder', 'codearts', 'codearts-desktop'].includes(p);
    return '<section class="account-platform-group" data-provider="' + p + '">' +
      '<header class="account-platform-header">' +
      '<div class="account-platform-heading">' + picon(p) + '<h2>' + esc(platName(p)) + '</h2>' +
      '<span class="account-capability ' + (multi ? 'multi' : '') + '">' + (multi ? '支持多账号' : '仅支持单账号') + '</span>' +
      '<span class="account-platform-count subtle">' + list.length + ' 个账号</span></div>' +
      '<div class="actions">' +
      (m.login !== 'none' ? '<button data-add="' + p + '" class="small primary">＋ 添加账号</button>' : '') +
      (['trae','workbuddy','qoder','zcode','minimax'].includes(p) ? '<button data-import="' + p + '" class="small">扫描本机</button>' : '') +
      '</div></header>' +
      list.map(a => acctRow(a, p)).join('') +
      '</section>';
  }).join('');
  el.querySelectorAll('[data-add]').forEach(b => b.addEventListener('click', () => startLogin(b.dataset.add)));
  el.querySelectorAll('[data-import]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true; b.textContent = '扫描中…';
    try {
      const r = await invoke('import', { platform: b.dataset.import === 'workbuddy' ? 'wb' : b.dataset.import, intl: false });
      toast('导入成功：' + (r.name || r.uid));
      await refreshAll();
    } catch (e) { toast('导入失败: ' + e, 4200); }
    b.disabled = false; b.textContent = '扫描本机';
  }));
  el.querySelectorAll('.acct-actions button, .acct-actions input').forEach(wire);
}
function acctRow(a, p) {
  const q = parseQuota(a);
  return '<div class="account-group-row" data-p="' + esc(a.provider) + '" data-u="' + esc(a.uid) + '">' +
    '<div class="single-account-identity"><strong>' + esc(a.name || platName(p)) + '</strong><small>' + esc(a.uid) + (a.expires ? ' · 凭据到期 ' + a.expires.slice(0, 10) : '') + '</small></div>' +
    '<div class="single-account-balance"><strong>' + (q && q.remaining != null ? fmt(q.remaining) : '—') + '</strong><small>' + esc(a.message || '') + '</small></div>' +
    '<div class="single-account-status">' + stInfo(a.status) + '</div>' +
    '<div class="account-row-actions acct-actions">' +
    '<label class="switch" title="自动签到"><input type="checkbox" data-auto ' + (a.auto ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label>' +
    '<button class="small" data-act="checkin">签到</button>' +
    '<button class="small" data-act="quota">额度</button>' +
    '<button class="small danger-ghost" data-act="del">删除</button>' +
    '</div></div>';
}
function wire(el) {
  if (el.type === 'checkbox') {
    el.addEventListener('change', async () => {
      const row = el.closest('.account-group-row');
      try { await invoke('account_toggle_auto', { provider: row.dataset.p, uid: row.dataset.u, auto: el.checked }); toast('已' + (el.checked ? '开启' : '关闭') + '自动签到'); }
      catch (e) { toast('设置失败: ' + e); }
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
        toast((o.status === 'success' ? '领取成功' : o.status === 'already' ? '今日已领取' : o.status) + (o.amount != null ? ' +' + o.amount : '') + (o.message ? ' · ' + o.message : ''));
        await refreshAll();
      } else if (el.dataset.act === 'quota') {
        el.textContent = '…';
        const q = await invoke('quota_refresh', { provider: p, uid });
        const bal = row.querySelector('.single-account-balance strong');
        if (bal) bal.textContent = q.remaining != null ? fmt(q.remaining) : '—';
      } else if (el.dataset.act === 'del') {
        if (!confirm('删除账号 ' + uid.slice(0, 12) + '？历史记录保留。')) { el.disabled = false; return; }
        await invoke('account_delete', { provider: p, uid });
        toast('已删除');
        await refreshAll();
      }
    } catch (e) { toast('操作失败: ' + e, 4200); }
    el.disabled = false; el.textContent = old;
  });
}

// ===== 签到中心 =====
function renderCheckin() {
  const accs = SNAP.accounts || [];
  const done = accs.filter(a => a.status === 'success' || a.status === 'already').length;
  const pending = accs.filter(a => !a.status || a.status === 'idle' || a.status === 'no-activity').length;
  const confirmN = accs.filter(a => a.status === 'pending').length;
  const attention = accs.filter(a => a.status === 'failed' || a.status === 'needs-action' || a.status === 'auth-expired').length;
  $('#progress-hero').innerHTML =
    '<div><strong>' + done + ' <span style="font-size:20px;color:var(--subtle)">/ ' + accs.length + '</span></strong><p>当前周期已完成</p>' +
    '<div class="progress"><span style="width:' + (accs.length ? done / accs.length * 100 : 0) + '%"></span></div></div>' +
    '<div><b style="font-size:24px">' + pending + '</b><p>等待领取</p></div>' +
    '<div><b style="font-size:24px">' + confirmN + '</b><p>待确认</p></div>' +
    '<div><b style="font-size:24px;color:' + (attention ? 'var(--amber)' : 'inherit') + '">' + attention + '</b><p>需要处理</p></div>';
  const FILTERS = [['all','全部'],['done','已完成'],['pending','等待领取'],['confirm','待确认'],['attention','需要处理']];
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
  $('#checkin-list').innerHTML = list.length ? list.map(a =>
    '<div class="checkin-row" data-provider="' + a.provider + '">' +
    '<div class="checkin-identity">' + picon(a.provider) + '<div><strong>' + esc(platName(a.provider)) + '</strong><small>' + esc(a.name || a.uid.slice(0, 14)) + '</small></div></div>' +
    '<div><small class="subtle">状态</small><br>' + stInfo(a.status) + '</div>' +
    '<div><small class="subtle">上次</small><br><span style="font-size:12.5px">' + esc(a.last_checkin ? a.last_checkin.slice(0, 16).replace('T', ' ') : '—') + '</span></div>' +
    '<div style="margin-left:auto;display:flex;gap:6px">' +
    '<button class="small" data-ck-p="' + esc(a.provider) + '" data-ck-u="' + esc(a.uid) + '">立即签到</button>' +
    '<button class="small" data-qk-p="' + esc(a.provider) + '" data-qk-u="' + esc(a.uid) + '">刷新额度</button>' +
    '</div></div>').join('') : '<div class="empty">该筛选下暂无账号</div>';
  $$('#checkin-list [data-ck-p]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true; b.innerHTML = '<span class="spin"></span>';
    try {
      const r = await invoke('checkin', { provider: b.dataset.ckP, uid: b.dataset.ckU, all: false });
      const o = r.results[0];
      toast((o.status === 'success' ? '领取成功' : o.status === 'already' ? '今日已领取' : o.status) + (o.amount != null ? ' +' + o.amount : ''));
      await refreshAll();
    } catch (e) { toast('失败: ' + e, 4200); }
    b.disabled = false; b.textContent = '立即签到';
  }));
  $$('#checkin-list [data-qk-p]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true;
    try { await invoke('quota_refresh', { provider: b.dataset.qkP, uid: b.dataset.qkU }); toast('额度已刷新'); await refreshAll(); }
    catch (e) { toast('失败: ' + e, 4200); }
    b.disabled = false;
  }));
}

// ===== 积分统计 =====
async function refreshStatistics() {
  const sel = $('#stat-account');
  const accs = SNAP?.accounts || [];
  const cur = sel.value;
  sel.innerHTML = '<option value="all">全部账号</option>' + accs.map(a =>
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
    const totalSum = items.reduce((s, i) => s + i.sum, 0);
    const totalCount = items.reduce((s, i) => s + i.count, 0);
    const successCount = items.reduce((s, i) => s + i.count, 0);
    $('#stat-metrics').innerHTML =
      '<div><p>期间签到积分</p><strong>' + fmt(totalSum) + '</strong></div>' +
      '<div><p>记录次数</p><strong>' + totalCount + '</strong></div>' +
      '<div><p>统计桶数</p><strong>' + items.length + '</strong></div>';
    const el = $('#stat-chart');
    charts.st ||= echarts.init(el);
    charts.st.resize();
    charts.st.setOption({
      tooltip: { trigger: 'axis' },
      grid: { left: 46, right: 14, top: 22, bottom: 28 },
      xAxis: { type: 'category', data: items.map(i => i.bucket), axisLabel: { fontSize: 10, color: '#7f7d91', rotate: items.length > 12 ? 30 : 0 } },
      yAxis: { type: 'value', axisLabel: { fontSize: 10, color: '#7f7d91' }, splitLine: { lineStyle: { color: '#eae9f1' } } },
      series: [{ name: '签到积分', type: 'bar', itemStyle: { color: '#6862ed', borderRadius: 4 }, barMaxWidth: 26, data: items.map(i => Math.round(i.sum * 100) / 100) }],
    });
    // ledger：该账号（或全部）最近记录
    let rows = SNAP.history || [];
    if (provider) rows = rows.filter(h => h.provider === provider && (!uid || h.uid === uid));
    $('#ledger-body').innerHTML = rows.length ? rows.map(h =>
      '<tr><td style="color:var(--subtle);white-space:nowrap">' + esc((h.at || '').slice(0, 16).replace('T', ' ')) + '</td>' +
      '<td>' + stInfo(h.status) + '</td>' +
      '<td style="color:var(--subtle)">' + esc(h.message || '') + '</td>' +
      '<td style="text-align:right;color:var(--green);font-weight:600">' + (h.amount != null ? '+' + h.amount : '') + '</td></tr>').join('')
      : '<tr><td colspan="4" style="text-align:center;color:var(--subtle);padding:24px">暂无记录</td></tr>';
  } catch (e) { toast('统计读取失败: ' + e, 4200); }
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
  'workbuddy':        { label: 'WorkBuddy 国内版', provider: 'workbuddy', chat: true },
  'workbuddy-intl':   { label: 'WorkBuddy 国际版', provider: 'workbuddy-intl', chat: true },
  'trae':             { label: 'Trae 国内版 (SOLO)', provider: 'trae', chat: true },
  'trae-enterprise':  { label: 'Trae 企业版', provider: 'trae', chat: true },
  'trae-global':      { label: 'Trae 国际版', provider: 'trae', chat: true },
  'zcode':            { label: 'ZCode (智谱)', provider: 'zcode', chat: true },
  'minimax':          { label: 'MiniMax', provider: 'minimax', chat: true },
  'qoder':            { label: 'Qoder', provider: 'qoder', chat: true },
  'codearts':         { label: '华为码道', provider: 'codearts', chat: false },
};
let GW_STATUS = null;

async function refreshGateway() {
  try {
    GW_STATUS = await invoke('gateway_status');
  } catch (e) { GW_STATUS = null; }
}

function renderRouting() {
  const el = $('#route-matrix');
  if (!GW_STATUS) { el.innerHTML = '<div class="empty">网关状态读取失败</div>'; return; }
  const st = GW_STATUS;
  const platRows = Object.entries(GW_PLATFORMS).map(([name, meta]) => {
    const count = (st.accounts || {})[name] || 0;
    const isCurrent = st.platform === name;
    return '<div class="route-row" data-platform="' + name + '">' +
      '<div style="flex:1"><strong style="font-size:13.5px">' + esc(meta.label) +
      (isCurrent ? ' <span class="tag" style="background:var(--accent-soft);color:var(--accent)">当前平台</span>' : '') +
      (meta.chat ? '' : ' <span class="tag" style="background:#f7ecd9;color:var(--amber)">仅签到/额度</span>') +
      '</strong><div class="subtle" style="margin-top:3px">账号池 provider：' + esc(meta.provider) + ' · 可用账号 ' + count + ' 个' +
      (meta.chat ? '' : ' · 无公开对话协议，网关暂不代理') + '</div></div>' +
      (isCurrent ? '' : '<button class="small" data-switch="' + name + '">切换到此平台</button>') +
      '</div>';
  }).join('');
  const ruleRows = (st.rules || []).map(r =>
    '<div class="route-row" data-rule="' + esc(r.id) + '">' +
    '<div style="flex:1"><strong style="font-size:13.5px">模型前缀 ' + esc(r.model_pattern) + '</strong>' +
    '<div class="subtle" style="margin-top:3px">' +
    (r.platform ? '平台 → ' + esc(GW_PLATFORMS[r.platform]?.label || r.platform) : '平台 → 跟随当前') +
    (r.account ? ' · 账号 → ' + esc(r.account) : ' · 账号 → 平台默认') +
    (r.enabled ? '' : ' · 已停用') + '</div></div>' +
    '<button class="small" data-rule-toggle="' + esc(r.id) + '" data-enabled="' + (r.enabled ? '1' : '0') + '">' + (r.enabled ? '停用' : '启用') + '</button>' +
    '<button class="small" data-rule-del="' + esc(r.id) + '">删除</button>' +
    '</div>').join('') || '<div class="empty">暂无规则 —— 全部请求按当前平台 + 平台内默认选号</div>';

  const logRows = (st.logs || []).slice(0, 30).map(l =>
    '<div class="route-row" style="font-size:12.5px">' +
    '<div style="flex:1;min-width:0"><strong style="font-size:12.5px">' + (l.ok ? '<span style="color:var(--green)">✓ 成功</span>' : '<span style="color:var(--red)">✗ 失败</span>') +
    ' <code style="font-size:12px">' + esc(l.model || '(无模型)') + '</code></strong>' +
    '<div class="subtle" style="margin-top:2px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + esc(l.at) + ' · ' + esc(l.platform) +
    (l.account ? ' · ' + esc(l.account) : '') + (l.message && !l.ok ? ' · ' + esc(l.message) : '') + '</div></div>' +
    '</div>').join('') || '<div class="empty">暂无请求日志</div>';

  el.innerHTML =
    // 网关地址卡
    '<section class="surface" style="margin-bottom:14px"><div class="setting-row">' +
    '<div><h3>本地网关（OpenAI 兼容接口）</h3><p class="subtle">像本地 Ollama 一样，把下面的接口地址填进任意支持 OpenAI 的客户端。</p></div>' +
    '<div><strong style="font-family:monospace;font-size:13px">' + esc(st.baseUrl) + '/v1</strong>' +
    '<div class="subtle" style="margin-top:4px">GET /v1/models · POST /v1/chat/completions · 端口 <b id="gw-port-now">' + st.port + '</b></div></div></div>' +
    '<p class="notice">模型 id 用 <code>平台/模型</code>（如 <code>workbuddy/glm-5.3-flash</code>）；裸模型 id 走下方当前平台。</p>' +
    // 端口 / 速率 / 仅当前平台 —— 自定义设置
    '<div style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:10px;margin-top:10px;padding-top:10px;border-top:1px solid var(--line)">' +
    '<label class="subtle">端口（改后需重启应用）<div style="display:flex;gap:6px;margin-top:4px">' +
    '<input id="gw-port" type="number" min="1" max="65535" value="' + st.port + '" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-port-save" class="small primary">保存</button></div></label>' +
    '<label class="subtle">速率限制（RPM，0 = 不限）<div style="display:flex;gap:6px;margin-top:4px">' +
    '<input id="gw-rpm" type="number" min="0" value="' + (st.rpm || 0) + '" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-rpm-save" class="small primary">保存</button></div></label>' +
    '<label class="subtle">仅允许当前平台<div style="margin-top:10px"><label class="switch"><input type="checkbox" id="gw-strict" ' + (st.strict !== false ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label></div>' +
    '<div class="subtle" style="margin-top:4px">开启后显式 <code>平台/模型</code> 与规则改平台都会被 403 拒绝</div></label>' +
    '</div></section>' +
    // 请求日志
    '<h3 class="spaced">请求日志（最近 ' + Math.min(st.logs?.length || 0, 30) + ' 条 · 总计 ' + (st.logStats?.total || 0) + ' · 成功 ' + (st.logStats?.ok || 0) + ' · 失败 ' + (st.logStats?.fail || 0) + '）</h3>' + logRows +
    // 当前平台
    '<h3 class="spaced">请求平台（裸模型 id 的默认路由）</h3>' + platRows +
    // 规则
    '<h3 class="spaced">路由规则（按模型前缀固定平台/账号）</h3>' + ruleRows +
    '<div class="actions" style="margin-top:10px"><button id="rule-add" class="small primary">＋ 新建规则</button></div>';

  const bindGwSave = (btnId, inputId, field) => {
    const btn = el.querySelector('#' + btnId);
    if (!btn) return;
    btn.addEventListener('click', async () => {
      const raw = el.querySelector('#' + inputId)?.value;
      const val = parseInt(raw, 10);
      if (isNaN(val)) { toast('请输入数字', 3600); return; }
      btn.disabled = true;
      try {
        const r = await invoke('gateway_set_limits', { [field]: val });
        toast(field === 'port'
          ? (r.needsRestart ? '端口已保存，重启应用后生效' : '端口已保存（与当前一致）')
          : '速率限制已保存（实时生效）');
        await refreshGateway(); renderRouting();
      } catch (e) { toast('保存失败: ' + e, 4200); }
      btn.disabled = false;
    });
  };
  bindGwSave('gw-port-save', 'gw-port', 'port');
  bindGwSave('gw-rpm-save', 'gw-rpm', 'rpm');
  const strictSw = el.querySelector('#gw-strict');
  if (strictSw) strictSw.addEventListener('change', async () => {
    try {
      await invoke('gateway_set_limits', { strict: strictSw.checked });
      toast(strictSw.checked ? '已锁定当前平台' : '已允许任意平台（规则可改平台）');
      await refreshGateway();
    } catch (e) { toast('操作失败: ' + e, 4200); strictSw.checked = !strictSw.checked; }
  });

  el.querySelectorAll('[data-switch]').forEach(b => b.addEventListener('click', async () => {
    b.disabled = true;
    try {
      await invoke('gateway_set_platform', { platform: b.dataset.switch });
      toast('已切换请求平台');
      await refreshGateway(); renderRouting();
    } catch (e) { toast('切换失败: ' + e, 4200); }
    b.disabled = false;
  }));
  el.querySelectorAll('[data-rule-del]').forEach(b => b.addEventListener('click', async () => {
    try {
      await invoke('gateway_delete_rule', { id: b.dataset.ruleDel });
      toast('规则已删除');
      await refreshGateway(); renderRouting();
    } catch (e) { toast('删除失败: ' + e, 4200); }
  }));
  el.querySelectorAll('[data-rule-toggle]').forEach(b => b.addEventListener('click', async () => {
    const rule = (GW_STATUS.rules || []).find(r => r.id === b.dataset.ruleToggle);
    if (!rule) return;
    try {
      await invoke('gateway_update_rule', { rule: { ...rule, enabled: rule.enabled ? false : true } });
      await refreshGateway(); renderRouting();
    } catch (e) { toast('操作失败: ' + e, 4200); }
  }));
  $('#rule-add').addEventListener('click', () => showRuleModal());
}

function showRuleModal() {
  const accs = SNAP?.accounts || [];
  const accOptions = accs.map(a => '<option value="' + esc(a.provider + ':' + a.uid) + '">' + esc(platName(a.provider) + ' · ' + (a.name || a.uid.slice(0, 10))) + '</option>').join('');
  const platOptions = Object.entries(GW_PLATFORMS).map(([k, v]) => '<option value="' + k + '">' + esc(v.label) + '</option>').join('');
  const mask = document.createElement('div');
  mask.className = 'modal-mask'; mask.style.display = 'flex';
  mask.innerHTML = '<div class="modal"><h3>新建路由规则</h3>' +
    '<div class="desc">模型前缀命中的请求固定路由到指定平台/账号；留空表示跟随当前平台。</div>' +
    '<div style="display:grid;gap:10px;margin:12px 0">' +
    '<label class="subtle">模型前缀（* 匹配全部）<input id="rule-model" placeholder="如 workbuddy/glm-5.3-flash 或 *" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"></label>' +
    '<label class="subtle">目标平台<select id="rule-platform" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">跟随当前平台</option>' + platOptions + '</select></label>' +
    '<label class="subtle">指定账号（可选）<select id="rule-account" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">平台内默认选号</option>' + accOptions + '</select></label>' +
    '</div><div class="foot"><button id="rule-cancel">取消</button><button id="rule-save" class="primary">保存</button></div></div>';
  document.body.appendChild(mask);
  mask.querySelector('#rule-cancel').addEventListener('click', () => mask.remove());
  mask.querySelector('#rule-save').addEventListener('click', async () => {
    const modelPattern = mask.querySelector('#rule-model').value.trim() || '*';
    const platform = mask.querySelector('#rule-platform').value;
    const account = mask.querySelector('#rule-account').value;
    try {
      await invoke('gateway_add_rule', { modelPattern, platform, account, weight: 1, enabled: true });
      toast('规则已保存');
      mask.remove();
      await refreshGateway(); renderRouting();
    } catch (e) { toast('保存失败: ' + e, 4200); }
  });
}

// ===== 设置 =====
let settingsDraft = null;
function renderSettings() {
  settingsDraft = { ...SETTINGS };
  const body = $('#settings-body');
  const tab = document.querySelector('#settings-tabs button.active')?.dataset.tab || 'tasks';
  if (tab === 'tasks') {
    body.innerHTML = '<div class="task-settings"><div>' +
      '<div class="setting-row"><div><h3>自动签到</h3><p>每个平台仅为已启用自动签到的账号执行任务。</p></div>' + sw('auto', settingsDraft.auto) + '</div>' +
      '<div class="setting-row"><div><h3>错过时间后补跑</h3><p>启动、唤醒或修改时间后检查当前周期。</p></div>' + sw('catchup', settingsDraft.catchup) + '</div>' +
      '<div class="setting-row"><div><h3>余额刷新间隔</h3><p>仅查询额度，不消耗模型调用。</p></div>' +
      '<select data-set="refreshMinutes"><option value="30"' + (settingsDraft.refreshMinutes === 30 ? ' selected' : '') + '>30分钟</option>' +
      '<option value="60"' + (settingsDraft.refreshMinutes === 60 ? ' selected' : '') + '>60分钟</option>' +
      '<option value="120"' + (settingsDraft.refreshMinutes === 120 ? ' selected' : '') + '>120分钟</option></select></div>' +
      '<div class="setting-row"><div><h3>计划时间</h3><p>每天执行的北京时间（随机模式忽略）。</p></div>' +
      '<input type="time" data-set="time" value="' + esc(settingsDraft.time) + '" style="padding:7px 10px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
      '<label class="switch" title="随机时间"><input type="checkbox" data-setb="randomTime" ' + (settingsDraft.randomTime ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label></div>' +
      '</div><div class="settings-preview"><h3 class="spaced" style="margin-bottom:6px">执行预览 · 保存后生效</h3>' +
      '<p class="subtle">当前已保存时间：' + esc(SETTINGS.randomTime ? '随机' : SETTINGS.time) + '。</p>' +
      (SNAP?.accounts || []).slice(0, 8).map(a => '<div class="preview-row">' + picon(a.provider) + '<strong>' + esc(platName(a.provider)) + '</strong><span class="subtle" style="margin-left:auto">每天 ' + esc(settingsDraft.randomTime ? '随机' : settingsDraft.time) + '</span></div>').join('') +
      '<p class="notice">所有平台均按你保存的北京时间执行。开启补跑后，会检查已错过计划时间的账号。已领取账号会跳过。</p>' +
      '</div></div>';
  } else if (tab === 'general') {
    body.innerHTML =
      '<div class="setting-row"><div><h3>基础使用统计</h3><p>仅上报随机安装标识、版本、日期与运行状态，不上传平台凭据。</p></div>' + sw('telemetry', settingsDraft.telemetry) + '</div>' +
      '<p class="notice">平台账号、凭据及积分历史保存在本机，不上传任何凭据。</p>';
  } else {
    body.innerHTML =
      '<div class="setting-row"><div><h3>AI路由助手 v0.2.0</h3><p>Rust + Tauri 实现 · 数据库：credit.sqlite（本机）</p></div><span class="tag">本地版</span></div>' +
      '<div class="setting-row"><div><h3>数据目录</h3><p>%APPDATA%/credit-rs/credit.sqlite · 平台凭据经系统密钥加密</p></div>' +
      '<button data-open-db class="small">打开目录</button></div>' +
      '<p class="notice">数据库升级前会备份；导出记录不包含凭据。</p>';
    body.querySelector('[data-open-db]').addEventListener('click', async () => {
      try { await invoke('account_list_all'); toast('账号数据记录 ' + (SNAP?.accounts?.length || 0) + ' 条'); } catch {}
    });
  }
  body.querySelectorAll('input[data-setb], input[data-sw]').forEach(wireSetting);
  body.querySelectorAll('[data-set]').forEach(el => el.addEventListener('change', () => {
    settingsDraft[el.dataset.set] = el.dataset.set === 'refreshMinutes' ? Number(el.value) : el.value;
  }));
}
function sw(key, checked) {
  return '<label class="switch"><input type="checkbox" data-setb="' + key + '" ' + (checked ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label>';
}
function wireSetting(el) {
  el.addEventListener('change', () => { settingsDraft[el.dataset.setb] = el.checked; });
}
$$('#settings-tabs button').forEach(b => b.addEventListener('click', () => {
  $$('#settings-tabs button').forEach(x => x.classList.toggle('active', x === b));
  renderSettings();
}));
$('#settings-cancel').addEventListener('click', () => { renderSettings(); toast('已还原未保存修改'); });
$('#settings-save').addEventListener('click', async () => {
  try {
    for (const [k, v] of Object.entries(settingsDraft)) {
      if (SETTINGS[k] !== v) await invoke('settings_set', { key: k, value: String(v) });
    }
    toast('设置已保存，任务按新计划安排');
    applyRefreshInterval();
    await refreshAll();
    // 补跑：开启 catchup 且今日计划时间已过 → 后台调度器 60s 内接管（Rust auto_tick）
  } catch (e) { toast('保存失败: ' + e, 4200); }
});

// ===== 登录模态 =====
async function startLogin(p) {
  const meta = PLAT[p] || {};
  $('#login-title').textContent = '登录 ' + platName(p);
  $('#login-desc').textContent = meta.desc || '';
  $('#login-body').innerHTML = '<div style="padding:18px 0"><span class="spin"></span> 正在发起授权…</div>';
  $('#login-mask').classList.add('show');
  clearInterval(loginTimer);
  try {
    const r = await invoke('login_start', { provider: p });
    $('#login-body').innerHTML =
      '<div style="font-size:12.5px;color:var(--subtle);margin-bottom:10px">授权链接已生成（5 分钟内有效）：</div>' +
      '<div style="word-break:break-all;font-size:12px;background:var(--bg);border-radius:8px;padding:10px;max-height:120px;overflow:auto">' + esc(r.url) + '</div>' +
      '<div id="login-status" style="margin-top:12px;font-size:13px"><span class="spin"></span> 等待授权完成…</div>';
    // 应用内授权子窗口（不是系统浏览器）：iframe 会被平台 X-Frame-Options 拒绝，Tauri 子窗口不受限
    await invoke('login_open_window', { sessionId: r.sessionId, url: r.url, provider: p });
    curLoginSid = r.sessionId;
    $('#login-open').textContent = '刷新授权窗口';
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
          $('#login-body').innerHTML = '<div style="color:var(--green);font-weight:600;padding:16px 0">✔ 登录成功：' + esc(s.account?.name || '') + ' · 正在获取额度…</div>';
          toast('登录成功：' + (s.account?.name || '') + '，正在获取额度');
          // 登录后立即获取：余额 + 状态
          try { await invoke('quota_refresh', { provider: s.account.provider, uid: s.account.uid }); } catch {}
          await refreshAll();
          $('#login-mask').classList.remove('show');
          showPage('accounts');
        } else if (s.status === 'expired') {
          clearInterval(loginTimer);
          $('#login-status').innerHTML = '<span style="color:var(--amber)">授权已超时，请重新发起</span>';
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
    $('#login-open').textContent = '打开授权页面';
    $('#login-mask').classList.remove('show');
  });

// ===== 顶部 =====
async function checkinAll(btn) {
  btn.disabled = true; const old = btn.textContent; btn.innerHTML = '<span class="spin"></span> 签到中…';
  try {
    const r = await invoke('checkin', { provider: '', uid: '', all: true });
    const ok = r.results.filter(x => x.status === 'success').length;
    const al = r.results.filter(x => x.status === 'already').length;
    const bad = r.results.filter(x => x.status === 'failed').length;
    toast('签到完成：成功 ' + ok + ' · 已领 ' + al + ' · 失败 ' + bad);
    await refreshAll();
  } catch (e) { toast('签到失败: ' + e); }
  btn.disabled = false; btn.textContent = old;
}
$('#btn-checkin-all').addEventListener('click', (e) => checkinAll(e.currentTarget));
$('#btn-checkin-all2').addEventListener('click', (e) => checkinAll(e.currentTarget));
async function quotaAll(btn) {
  btn.disabled = true; const old = btn.textContent; btn.textContent = '刷新中…';
  try {
    const accs = SNAP?.accounts || [];
    for (const a of accs) { try { await invoke('quota_refresh', { provider: a.provider, uid: a.uid }); } catch {} }
    toast('余额已刷新');
    await refreshAll();
  } catch (e) { toast('刷新失败: ' + e); }
  btn.disabled = false; btn.textContent = old;
}
$('#btn-quota-all').addEventListener('click', (e) => quotaAll(e.currentTarget));
$('#btn-quota-all2').addEventListener('click', (e) => quotaAll(e.currentTarget));
$('#btn-refresh').addEventListener('click', refreshAll);
$$('[data-goto]').forEach(b => b.addEventListener('click', () => showPage(b.dataset.goto)));
window.addEventListener('resize', () => { charts.ov?.resize(); charts.st?.resize(); });
window.piconFallback = piconFallback;

refreshAll();
function applyRefreshInterval() {
  if (window.__refreshTimer) clearInterval(window.__refreshTimer);
  const mins = (SETTINGS && SETTINGS.refreshMinutes) || 60;
  window.__refreshTimer = setInterval(() => { if (SNAP) refreshAll(); }, Math.max(1, mins) * 60 * 1000);
}
window.__refreshTimer = setInterval(() => { if (SNAP) refreshAll(); }, 60000);
