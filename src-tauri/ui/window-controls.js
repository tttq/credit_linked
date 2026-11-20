/* ============================================================
 * window-controls.js — 面板顶栏窗口控制（自包含，独立于 app.js）
 *
 * 背景：此前的绑定写在 app.js 尾部且走 Tauri v1 的 window.getCurrent()
 * （Tauri 2 已移除该 API），加上 capabilities 缺 allow-hide/allow-show
 * 权限，导致「最小化」「退至后台」「退出程序」全部静默失效。
 *
 * 本脚本只做窗口控制这一件事：
 *   - 面板顶栏：panel-min（隐藏到托盘）、panel-close（弹关闭确认）
 *   - 关闭确认模态：quit-cancel / quit-to-tray / quit-exit
 *   - 后端 window-close-requested 事件（用户点系统关闭时也弹确认）
 *
 * 权限兜底：hide/show 走前端 window API；被拒（缺权限/旧 webview）时
 * 自动回落到后端 hide_main 命令（Rust 侧直调，无需前端权限）。
 * DOM 未就绪时等待 DOMContentLoaded，不丢绑定；重复加载幂等。
 * ============================================================ */
(function () {
  'use strict';

  function boot() {
    if (window.__windowControlsReady) return;
    window.__windowControlsReady = true;

    var _T = window.__TAURI__;
    var invoke = _T?.core?.invoke || _T?.invoke || window.invoke;

    // Tauri 2: getCurrentWindow()；兼容尝试 v1 的 getCurrent()（不存在则忽略）
    function curWin() {
      try { return _T?.window?.getCurrentWindow?.() || _T?.window?.getCurrent?.() || null; }
      catch (e) { return null; }
    }

    // 隐藏窗口：前端 API 失败时回落到后端 hide_main 命令
    function hideMain() {
      var w = curWin();
      try {
        var r = w && w.hide ? w.hide() : null;
        if (r && typeof r.catch === 'function') {
          r.catch(function () {
            try { invoke && invoke('hide_main'); } catch (e) {}
          });
        }
        if (!r) throw new Error('no window api');
      } catch (e) {
        try { invoke && invoke('hide_main'); } catch (e2) {}
      }
    }

    var mask = document.getElementById('quit-mask');
    var showQuit = function () { if (mask) mask.style.display = 'flex'; };
    var hideQuit = function () { if (mask) mask.style.display = 'none'; };

    // 顶栏按钮
    var min = document.getElementById('panel-min');
    if (min) min.addEventListener('click', function (e) { e.preventDefault(); hideMain(); });
    var close = document.getElementById('panel-close');
    if (close) close.addEventListener('click', function (e) { e.preventDefault(); showQuit(); });

    // 关闭确认模态三按钮
    if (mask) {
      var cancel = mask.querySelector('#quit-cancel');
      if (cancel) cancel.addEventListener('click', hideQuit);
      var toTray = mask.querySelector('#quit-to-tray');
      if (toTray) toTray.addEventListener('click', function () { hideQuit(); hideMain(); });
      var exit = mask.querySelector('#quit-exit');
      if (exit) exit.addEventListener('click', function () {
        hideQuit();
        try { invoke && invoke('quit_app'); } catch (e) {}
      });
    }

    // 后端拦截系统关闭后发来的事件 → 弹确认
    try {
      var listen = _T?.event?.listen;
      if (listen) {
        var p = listen('window-close-requested', showQuit);
        if (p && typeof p.catch === 'function') p.catch(function () {});
      }
    } catch (e) {}
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
