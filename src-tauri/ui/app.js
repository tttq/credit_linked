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
  if (iconFile) return '<img class="picon ' + (cls || '') + '" style="width:26px;height:26px;border-radius:7px;object-fit:cover;flex:0 0 auto" src="platforms/' + iconFile + '" onerror="this.outerHTML=piconFallback(\'' + p + '\',\'' + (cls || '') + '\')">';
  return piconFallback(p, cls);
}
function piconFallback(p, cls) {
  const m = platMeta(p);
  return '<span class="picon ' + (cls || '') + '" style="background:' + m.color + ';width:26px;height:26px;border-radius:7px;display:inline-flex;align-items:center;justify-content:center;color:#fff;font-weight:700;font-size:12px;flex:0 0 auto">' + esc(m.name ? m.name[0] : '?') + '</span>';
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
  // 固定平台清单：已添加平台在前，未添加平台也渲染（保证删除后添加入口不消失）
  const ALL_PLATS = Object.keys(PLAT).filter(p => p !== 'doubao');
  const added = [...new Set(accs.map(a => a.provider))];
  const plats = [...added, ...ALL_PLATS.filter(p => !added.includes(p))];
  $('#accounts-sub').textContent = '已连接 ' + added.length + ' 个平台 · ' + accs.length + ' 个账号';
  const el = $('#account-list');
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
      (list.length ? list.map(a => acctRow(a, p)).join('') : '<div class="empty" style="padding:14px">尚未添加账号 —— 点右上角「添加账号」开始</div>') +
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

function bindGwSave(btnId, inputId, field) {
  const btn = document.getElementById(btnId);
  const input = document.getElementById(inputId);
  if (!btn || !input) return;
  btn.addEventListener('click', async () => {
    btn.disabled = true;
    try {
      const val = field === 'port' ? Number(input.value) : Number(input.value);
      if (field === 'port' && !(val >= 1 && val <= 65535)) { toast('端口需在 1-65535 之间', 4200); return; }
      const r = await invoke('gateway_set_limits', { [field]: val });
      if (field === 'port') {
        toast(r.needsRestart ? '端口已保存 —— 重启应用后生效' : '端口已保存');
      } else {
        toast('速率限制已保存（实时生效）');
      }
      await refreshGateway();
    } catch (e) { toast('保存失败: ' + e, 4200); }
    btn.disabled = false;
  });
}

function renderRouting() {
  const el = $('#route-matrix');
  if (!GW_STATUS) { el.innerHTML = '<div class="empty">网关状态读取失败</div>'; return; }
  const st = GW_STATUS;
  const logStats = st.logStats || { total: 0, ok: 0, fail: 0 };
  const curMeta = GW_PLATFORMS[st.platform] || {};

  const logRows = (st.logs || []).slice(0, 30).map(l =>
    '<div class="route-row" style="font-size:12.5px">' +
    '<div style="flex:1;min-width:0;display:flex;align-items:center;gap:8px">' +
    '<span style="font-weight:700;flex:0 0 auto">' + (l.ok ? '<span style="color:var(--green,#16a34a)">✓</span>' : '<span style="color:var(--red,#dc2626)">✗</span>') + '</span>' +
    '<div style="min-width:0"><strong style="font-size:12.5px"><code style="font-size:12px">' + esc(l.model || '(无模型)') + '</code></strong>' +
    '<div class="subtle" style="margin-top:2px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + esc(l.at) + ' · ' + esc(l.platform) +
    (l.account ? ' · ' + esc(l.account) : '') + (l.message && !l.ok ? ' · <span style="color:var(--red,#dc2626)">' + esc(l.message) + '</span>' : '') + '</div></div></div>' +
    '</div>').join('') || '<div class="empty" style="padding:16px">暂无请求日志</div>';

  el.innerHTML =
    // ===== 网关卡 =====
    '<section class="surface" style="margin-bottom:14px;padding:18px 20px">' +
    '<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap">' +
    '<div style="flex:1;min-width:260px"><h3 style="font-size:14px">本地网关 · OpenAI 兼容接口</h3>' +
    '<div style="display:flex;align-items:center;gap:8px;margin-top:8px;flex-wrap:wrap">' +
    '<code style="background:var(--accent-soft);color:var(--accent);padding:8px 14px;border-radius:9px;font-size:13.5px;font-weight:600">' + esc(st.baseUrl) + '/v1</code>' +
    '<button class="small" id="gw-copy-url">复制</button>' +
    '<span class="subtle" style="font-size:11.5px">GET /v1/models · POST /v1/chat/completions</span></div>' +
    '<p class="subtle" style="font-size:11.5px;margin-top:8px">模型 id 用 <code>平台/模型</code>；裸模型 id 走当前平台。</p></div></div>' +
    '<div style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:12px;margin-top:14px;padding-top:14px;border-top:1px solid var(--line)">' +
    '<label class="subtle" style="font-size:12px">端口（改后需重启应用）<div style="display:flex;gap:6px;margin-top:5px">' +
    '<input id="gw-port" type="number" min="1" max="65535" value="' + st.port + '" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-port-save" class="small primary">保存</button></div></label>' +
    '<label class="subtle" style="font-size:12px">速率限制（RPM，0 = 不限）<div style="display:flex;gap:6px;margin-top:5px">' +
    '<input id="gw-rpm" type="number" min="0" value="' + (st.rpm || 0) + '" style="width:100%;padding:8px;border-radius:9px;border:1px solid var(--line);font:inherit">' +
    '<button id="gw-rpm-save" class="small primary">保存</button></div></label>' +
    '<label class="subtle" style="font-size:12px">仅允许当前平台<div style="margin-top:8px"><label class="switch"><input type="checkbox" id="gw-strict" ' + (st.strict !== false ? 'checked' : '') + '><span class="track"><span class="knob"></span></span></label></div>' +
    '<div class="subtle" style="margin-top:4px;font-size:11px">开启后跨平台请求将被 403 拒绝</div></label>' +
    '</div></section>' +

    // ===== 请求平台卡（全部整合进下拉框）=====
    '<section class="surface" style="margin-bottom:14px;padding:18px 20px">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">' +
    '<h3 style="font-size:14px">请求平台 <span class="subtle" style="font-size:11.5px;font-weight:400">裸模型 id 的默认路由</span></h3>' +
    '<select id="route-plat-select" style="padding:8px 12px;border-radius:9px;border:1px solid var(--line);font:inherit;min-width:220px">' +
    Object.entries(GW_PLATFORMS).map(([name, meta]) => '<option value="' + name + '"' + (st.platform === name ? ' selected' : '') + '>' + esc(meta.label) + ' · ' + ((st.accounts || {})[name] || 0) + ' 账号' + (meta.chat ? '' : '（仅签到）') + '</option>').join('') +
    '</select></div>' +
    '<div style="display:flex;align-items:center;gap:12px;margin-top:14px;padding:12px 14px;background:var(--accent-soft,#eef6ff);border-radius:11px">' +
    picon(st.platform) +
    '<div style="flex:1"><strong style="font-size:13.5px">' + esc(curMeta.label || st.platform) + '</strong>' +
    '<div class="subtle" style="margin-top:3px;font-size:12px">provider：' + esc(curMeta.provider || st.platform) + ' · 可用账号 <b>' + ((st.accounts || {})[st.platform] || 0) + '</b> 个' +
    (curMeta.chat ? '' : ' · 无公开对话协议，网关暂不代理') + '</div></div>' +
    '<span class="tag" style="background:var(--accent);color:#fff">当前</span></div>' +
    '</section>' +

    // ===== 模型测试卡 =====
    '<section class="surface" style="margin-bottom:14px;padding:18px 20px">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">' +
    '<h3 style="font-size:14px">模型测试 <span class="subtle" style="font-size:11.5px;font-weight:400">请求经本地网关转发到对应平台</span></h3>' +
    '<button id="gw-test-refresh" class="small">刷新模型</button></div>' +
    '<div style="display:flex;gap:8px;margin-top:12px;align-items:center;flex-wrap:wrap">' +
    '<select id="gw-test-model" style="flex:1;min-width:240px;padding:8px 12px;border-radius:9px;border:1px solid var(--line);font:inherit"><option value="">（点击「刷新模型」获取当前平台模型）</option></select>' +
    '</div>' +
    '<textarea id="gw-test-msg" rows="2" placeholder="输入测试消息，如：你好，请介绍一下你自己" style="width:100%;margin-top:10px;padding:10px 12px;border-radius:9px;border:1px solid var(--line);font:inherit;resize:vertical"></textarea>' +
    '<div style="display:flex;align-items:center;gap:10px;margin-top:10px">' +
    '<button id="gw-test-send" class="small primary">发送测试</button>' +
    '<span id="gw-test-status" class="subtle" style="font-size:12px"></span></div>' +
    '<div id="gw-test-result" style="margin-top:12px;padding:12px 14px;border-radius:11px;background:var(--surface-2,#f7f7fa);min-height:44px;font-size:13px;white-space:pre-wrap;word-break:break-word;display:none"></div>' +
    '</section>' +

    // ===== 请求日志卡 =====
    '<section class="surface" style="padding:18px 20px">' +
    '<div style="display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px"><h3 style="font-size:14px">请求日志' +
    '<span class="subtle" style="font-size:11.5px;font-weight:400"> · 最近 ' + Math.min(st.logs?.length || 0, 30) + ' 条 · 总计 ' + logStats.total + ' · <span style="color:var(--green,#16a34a)">成功 ' + logStats.ok + '</span> · <span style="color:var(--red,#dc2626)">失败 ' + logStats.fail + '</span></span></h3>' +
    ((logStats.total || 0) > 0 ? '<button id="gw-logs-clear" class="small danger-ghost">清空日志</button>' : '') + '</div>' +
    '<div style="margin-top:10px">' + logRows + '</div></section>';

  // ===== 绑定 =====
  el.querySelector('#gw-copy-url')?.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(st.baseUrl + '/v1'); toast('接口地址已复制'); } catch (e) { toast('复制失败: ' + e); }
  });
  const platSelect = el.querySelector('#route-plat-select');
  if (platSelect) platSelect.addEventListener('change', async () => {
    const p = platSelect.value;
    platSelect.disabled = true;
    try {
      await invoke('gateway_set_platform', { platform: p });
      toast('已切换请求平台：' + (GW_PLATFORMS[p]?.label || p));
      await refreshGateway(); renderRouting();
    } catch (e) { toast('切换失败: ' + e, 4200); renderRouting(); }
  });
  const clearBtn = el.querySelector('#gw-logs-clear');
  if (clearBtn) clearBtn.addEventListener('click', async () => {
    if (!confirm('确定清空全部请求日志？此操作不可恢复。')) return;
    clearBtn.disabled = true;
    try {
      const r = await invoke('gateway_clear_logs');
      toast('已清空 ' + (r.cleared || 0) + ' 条日志');
      await refreshGateway(); renderRouting();
    } catch (e) { toast('清空失败: ' + e, 4200); }
    clearBtn.disabled = false;
  });
  bindGwSave('gw-port-save', 'gw-port', 'port');
  bindGwSave('gw-rpm-save', 'gw-rpm', 'rpm');
  const strictChk = el.querySelector('#gw-strict');
  if (strictChk) strictChk.addEventListener('change', async () => {
    strictChk.disabled = true;
    try {
      await invoke('gateway_set_limits', { strict: strictChk.checked });
      toast('已' + (strictChk.checked ? '开启' : '关闭') + '「仅允许当前平台」');
      await refreshGateway();
    } catch (e) { strictChk.checked = !strictChk.checked; toast('设置失败: ' + e, 4200); }
    strictChk.disabled = false;
  });

  // ===== 模型测试 =====
  const modelSel = el.querySelector('#gw-test-model');
  const statusEl = el.querySelector('#gw-test-status');
  const resultEl = el.querySelector('#gw-test-result');
  const fetchModels = async () => {
    statusEl.textContent = '获取模型中…';
    try {
      const r = await fetch(st.baseUrl + '/v1/models').then(x => x.json());
      const models = (r.data || []).map(m => m.id);
      if (!models.length) {
        modelSel.innerHTML = '<option value="">（当前平台无可用模型 —— 检查账号）</option>';
        statusEl.textContent = '';
        return;
      }
      modelSel.innerHTML = models.map(m => '<option value="' + esc(m) + '">' + esc(m) + '</option>').join('');
      statusEl.textContent = models.length + ' 个模型';
    } catch (e) { statusEl.textContent = '获取失败: ' + e; }
  };
  el.querySelector('#gw-test-refresh')?.addEventListener('click', fetchModels);
  // 进页面自动获取一次
  fetchModels();
  el.querySelector('#gw-test-send')?.addEventListener('click', async () => {
    const model = modelSel.value;
    const msg = (el.querySelector('#gw-test-msg')?.value || '').trim();
    const sendBtn = el.querySelector('#gw-test-send');
    if (!model) { toast('请先获取并选择模型', 4200); return; }
    if (!msg) { toast('请输入测试消息', 4200); return; }
    sendBtn.disabled = true;
    statusEl.textContent = '请求中…';
    resultEl.style.display = 'block';
    resultEl.style.color = 'var(--subtle,#7f7d91)';
    resultEl.textContent = '等待平台响应…';
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
        resultEl.textContent = '✗ 失败（' + secs + 's）：' + (data.error.message || JSON.stringify(data.error));
        statusEl.textContent = '';
      } else {
        const content = data.choices?.[0]?.message?.content || JSON.stringify(data).slice(0, 500);
        const usage = data.usage ? ' · tokens ' + (data.usage.total_tokens ?? '?') : '';
        resultEl.style.color = 'var(--ink,#25252f)';
        resultEl.textContent = '✓ 成功（' + secs + 's' + usage + '）：\n\n' + content;
        statusEl.textContent = '';
      }
    } catch (e) {
      resultEl.style.color = 'var(--red,#dc2626)';
      resultEl.textContent = '✗ 请求异常：' + e;
      statusEl.textContent = '';
    }
    sendBtn.disabled = false;
  });
}
