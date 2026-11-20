/* ============================================================
 * i18n.js — 零构建国际化层（zh-CN / en-US）
 * 以普通 <script> 在 app.js 之前加载；提供全局 t() / I18n。
 * ============================================================ */
(function () {
  'use strict';

  var DICT = {
    'zh-CN': {
      // 品牌 / 通用
      app_name: 'AI Free Router',
      app_name_short: 'AI Free Router',
      brand_symbol: 'R',
      ok: '确定', cancel: '取消', save: '保存', close: '关闭', refresh: '刷新',
      confirm: '提示', loading: '加载中…', saving: '保存中…', copy: '复制', copied: '已复制',
      current: '当前', all: '全部', daily: '每天', weekly: '每周', monthly: '每月', yearly: '每年',
      failed: '失败', minutes: '分钟',
      // 顶栏
      minimize_to_tray: '最小化到托盘', close_app: '关闭', one_click_checkin: '一键签到', refresh_balance: '刷新余额',
      // 导航
      nav_home: '首页', nav_accounts: '账号管理', nav_checkin: '签到中心', nav_stats: '统计', nav_route: '账号路由', nav_settings: '设置',
      // 首页
      today_cycle: '今日周期', points_trend: '积分趋势', my_ai_tools: '我的 AI 工具', manage_accounts: '管理账号 →',
      checkin_points: '签到积分', period_points: '期间签到积分 +', chart_load_failed: '图表加载失败: ',
      empty_no_tools: '还没有添加 AI 工具 —— 到「账号管理」添加账号',
      available: '可用', checked_in: '✓ 已签到', last: '上次 ',
      period_points_short: '期间签到积分', records: '记录次数', buckets: '统计桶数', all_accounts: '全部账号',
      token_usage: 'Token 消耗', token_period: '期间 Token 消耗', token_stats_all: 'Token 消耗为本地网关全部账号的聚合统计（网关日志），单账号视图暂不支持 token 统计。', token_stats_single: '单账号视图仅统计签到积分；Token 消耗请切回「全部账号」。',
      stat_read_failed: '统计读取失败: ',
      // 统计图例（折线图：签到积分 + token 消耗）
      stat_token_used: 'Token 消耗', stat_metric_points: '签到积分（左轴）', stat_metric_tokens: 'Token 消耗（右轴）',
      stat_tokens_short: 'Token 消耗',
      // 平台描述（PLAT desc）
      desc_trae: '字节跳动的 AI IDE，支持 SOLO 模式',
      desc_workbuddy: '腾讯的 AI 编码工作台，国内/国际双版本',
      desc_minimax: 'MiniMax 的 Agentic 编码工具',
      desc_qoder: '阿里的 Agentic 编码 IDE',
      desc_codearts: '华为云 CodeArts 盘古助手，IDE 插件',
      desc_codearts_desktop: '华为码道桌面客户端',
      desc_zcode: '智谱的编码智能体',
      desc_doubao: '豆包大模型桌面端',
      // 状态
      st_success: '已领取', st_already: '今日已领', st_failed: '失败', st_pending: '待确认',
      st_needs_action: '需操作', st_no_activity: '无活动', st_idle: '待签到', st_auth_expired: '授权过期',
      today_claimed: '今日已领取', checkin_error: '签到异常', today_not_claimed: '今日未领取',
      cred_expires: ' · 凭据到期 ', available_balance: '可用余额',
      pname_trae: 'Trae', pname_workbuddy: 'WorkBuddy', pname_minimax: 'MiniMax', pname_qoder: 'Qoder',
      pname_codearts: '华为云码道', pname_codearts_desktop: '码道桌面', pname_zcode: 'ZCode', pname_doubao: '豆包',
      // 账号管理
      accounts_connected: '已连接 {n} 个平台 · {m} 个账号', multi_account: '支持多账号', single_account: '仅支持单账号',
      n_accounts: '{n} 个账号', add_account: '＋ 添加账号', scan_local: '扫描本机', scanning: '扫描中…',
      empty_no_accounts: '尚未添加账号 —— 点右上角「添加账号」开始',
      import_ok: '导入成功：', import_failed: '导入失败: ', data_load_failed: '数据加载失败: ',
      checkin_now: '立即签到', quota_refreshed: '额度已刷新', status: '状态', last_time: '上次',
      no_accounts_in_filter: '该筛选下暂无账号', claim_ok: '领取成功', already_claimed: '今日已领取', op_failed: '失败: ',
      filter_all: '全部', filter_done: '已完成', filter_pending: '等待领取', filter_confirm: '待确认', filter_attention: '需要处理',
      cycle_done: '当前周期已完成', waiting_claim: '等待领取',
      checkin: '签到', quota_btn: '额度', device_btn: '设备号', device_btn_tip: 'Trae 一个设备号一天只能签一个账号：为该账号设置独立设备号',
      delete: '删除', deleted: '已删除', auto_on: '已开启自动签到', auto_off: '已关闭自动签到',
      delete_account: '删除账号', delete_confirm_msg: '删除账号 {u}？历史记录保留。',
      dev_unavailable: '设备号设置不可用',
      dev_desc: 'Trae 一个设备号一天只能签一个账号，且必须是本机已注册的设备号（否则签到报 9074）。多账号请各用各的号；留空保存 = 自动从本机空闲设备号中挑选。',
      dev_loading: ' 正在读取设备号…', dev_local_found: '本机检测到 {n} 个已注册设备号：', dev_local_n: '本机设备 {n}',
      dev_auto_option: '自动（签到时从空闲设备号中挑选）',
      dev_none_found: '未检测到本机 Trae 客户端设备号。可在装过 Trae 并登录过的电脑上，从 %APPDATA%\\Trae CN\\User\\globalStorage\\storage.json 里找 iCubeAuthInfo://icube-dc: 开头的键，冒号后的数字即设备号。',
      dev_manual_ph: '手动输入设备号（8~20 位纯数字，可留空）',
      dev_set_ok: '设备号已设置：', dev_cleared: '已清除，签到时自动挑选空闲设备号',
      checkin_running: ' 签到中…', checkin_done_stats: '签到完成：成功 {ok} · 已领 {al} · 失败 {bad}', checkin_failed: '签到失败: ',
      refreshing: '刷新中…', balance_refreshed: '余额已刷新', fetching_quota: '正在获取额度…', login_ok_toast: '登录成功：{n}，正在获取额度',
      // 设置设备号（开发模态）
      dev_title: '设置设备号', dev_saved: '设备号已保存',
      // 网关
      gw_title: '本地网关 · OpenAI 兼容接口', port: '端口', port_restart_hint: '改后需重启应用',
      rate_limit: '速率限制', rate_limit_hint: 'RPM · 0 = 不限', strict_current_only: '仅允许当前平台',
      strict_hint: '开启后跨平台请求被 403 拒绝',
      request_platform: '请求平台', bare_model_hint: '裸模型 id 的默认路由',
      models_follow_platform: '/v1/models 只返回「请求平台」所选平台的模型；切换平台后模型清单跟随变化。',
      model_id_hint: '模型 id 用 <code>平台/模型</code>；裸模型 id 走当前平台。',
      n_accounts_suffix: '{n} 账号', signin_only: '（仅签到）', provider_label: 'provider：', available_accounts: '可用账号 <b>{n}</b> 个',
      no_chat_protocol: ' · 无公开对话协议，网关暂不代理',
      account_switch: '账号切换', account_switch_hint: '当前平台 · 点击即切换本地账号', switch_next: '↻ 一键切换下一个',
      model_test: '模型测试', model_test_hint: '请求经本地网关转发到对应平台', refresh_models: '刷新模型',
      model_placeholder: '（点击「刷新模型」获取当前平台模型）', no_models: '（无可用模型）',
      test_msg_placeholder: '输入测试消息，如：你好，请介绍一下你自己', send_test: '发送测试',
      fetching_models: '获取模型中…', n_models: '{n} 个模型', fetch_models_failed: '获取失败: ',
      codearts_expired: '码道凭据可能已过期（客户端 refresh 会轮换 STS） ',
      sync_from_client: '从客户端同步最新会话', syncing: '同步中…',
      sync_ok: '已同步 {names} 的最新凭据',
      sync_no_login: '客户端里没有登录态 —— 请先在码道客户端登录，或在账号管理「扫描本机」导入',
      sync_failed: '同步失败: ', no_accounts_or_expired: '该平台没有可用账号或凭据失效 —— 到账号管理检查',
      select_model_first: '请先获取并选择模型', input_test_msg: '请输入测试消息',
      requesting: '请求中…', waiting_response: '等待平台响应…（均衡/极致档为深度推理模型，响应可能需要 1-2 分钟）',
      test_failed: '✗ 失败（{s}s）：', test_ok: '✓ {model}（{s}s{usage}）：', request_error: '✗ 请求异常：',
      usage_tokens: ' · tokens 输入 {in} + 输出 {out} = {total} · 输出速度 ',
      request_log: '请求日志', clear_log: '清空日志', no_logs: '暂无请求日志',
      log_stats: ' · 最近 {n} 条 · 总计 {t} · <span style="color:var(--green,#16a34a)">成功 {ok}</span> · <span style="color:var(--red,#dc2626)">失败 {fail}</span>',
      log_time: '请求时间 ', log_duration: ' · 耗时 ', no_model: '(无模型)',
      clear_logs_title: '清空请求日志', clear_logs_msg: '确定清空全部请求日志？此操作不可恢复。', clear_btn: '清空',
      cleared_n: '已清空 {n} 条日志', clear_failed: '清空失败: ',
      gw_read_failed: '网关状态读取失败', copied_url: '接口地址已复制', copy_failed: '复制失败: ',
      no_accounts_current: '当前平台没有可用账号', switched_account: '已切换账号：', cred_check_failed: ' · 凭据校验失败: ',
      switch_failed: '切换失败: ', client_not_running: '{p} 客户端未运行 —— 已自动启动', switched_platform: '已切换请求平台：',
      refreshing_logs: '刷新中…', logs_refreshed: '已刷新请求日志', refresh_failed: '刷新失败: ',
      no_accounts_platform: '当前平台（{p}）没有可用账号 —— 请先在账号管理添加', balance: ' · 余额 ', quota_exhausted: '额度耗尽',
      rule_modal_title: '新建路由规则',
      rule_modal_desc: '模型前缀命中的请求固定路由到指定平台/账号；留空表示跟随当前平台。',
      rule_model_prefix: '模型前缀（* 匹配全部）', rule_model_ph: '如 workbuddy/glm-5.3-flash 或 *',
      rule_target_platform: '目标平台', rule_follow_platform: '跟随当前平台', rule_account: '指定账号（可选）',
      rule_default_account: '平台内默认选号', rule_saved: '规则已保存',
      port_invalid: '端口需在 1-65535 之间', port_saved_restart: '端口已保存 —— 重启应用后生效', port_saved: '端口已保存',
      rate_saved: '速率限制已保存（实时生效）', strict_on: '已开启「仅允许当前平台」', strict_off: '已关闭「仅允许当前平台」',
      save_failed: '保存失败: ', set_failed: '设置失败: ',
      // GW 平台标签
      gw_workbuddy: 'WorkBuddy 国内版', gw_workbuddy_intl: 'WorkBuddy 国际版', gw_trae: 'Trae 国内版 (SOLO)',
      gw_trae_enterprise: 'Trae 企业版', gw_trae_global: 'Trae 国际版', gw_zcode: 'ZCode (智谱)',
      gw_minimax: 'MiniMax', gw_qoder: 'Qoder', gw_codearts: '华为码道 (IDE)', gw_codearts_desktop: '码道桌面',
      // 设置
      settings_sub: '按你的节奏，安排这个工作台。', tab_tasks: '自动任务', tab_general: '通用', tab_tools: '工具安装', tab_data: '数据与版本',
      language: '界面语言', language_desc: '切换后立即生效；托盘菜单需重启应用后更新。',
      appearance: '外观', appearance_desc: '界面配色：跟随系统、浅色或深色；选「跟随系统」时随操作系统深浅色实时切换。',
      theme_system: '跟随系统', theme_light: '浅色', theme_dark: '深色',
      auto_checkin: '自动签到', auto_checkin_desc: '每个平台仅为已启用自动签到的账号执行任务。',
      catchup: '错过时间后补跑', catchup_desc: '启动、唤醒或修改时间后检查当前周期。',
      refresh_interval: '余额刷新间隔', refresh_interval_desc: '仅查询额度，不消耗模型调用。',
      m30: '30分钟', m60: '60分钟', m120: '120分钟',
      sched_time: '计划时间', sched_time_desc: '每天执行的北京时间（随机模式忽略）。', random_time: '随机时间', random: '随机',
      exec_preview: '执行预览 · 保存后生效', saved_time_is: '当前已保存时间：{v}。', every_day: '每天 {v}',
      notice_beijing: '所有平台均按你保存的北京时间执行。开启补跑后，会检查已错过计划时间的账号。已领取账号会跳过。',
      base_usage_stats: '基础使用统计', base_usage_desc: '仅上报随机安装标识、版本、日期与运行状态，不上传平台凭据。',
      privacy_notice: '平台账号、凭据及积分历史保存在本机，不上传任何凭据。',
      app_version: 'Rust + Tauri 实现 · 数据库：credit.sqlite（本机）', local_edition: '本地版',
      software_update: '软件更新', checking_release: '检查 GitHub Releases 最新版本…', recheck: '重新检查',
      download_update: '下载更新', restart_install: '重启并安装',
      data_dir: '数据目录', data_dir_desc: '%APPDATA%/credit-rs/credit.sqlite · 平台凭据经系统密钥加密', open_dir: '打开目录',
      db_notice: '数据库升级前会备份；导出记录不包含凭据。更新包下载后需手动点击「重启并安装」（静默安装）。',
      checking_update: '检查更新中…', new_version_found: '发现新版本 <b>v{latest}</b>（当前 v{current}）',
      goto_release: '前往 Release 页下载', already_latest: '已是最新版本 v{v}', check_failed: '检查失败: ',
      downloading: '下载中…', download_done_toast: '下载完成（{mb} MB）— 点击「重启并安装」', download_failed: '下载失败: ', install_failed: '安装失败: ',
      accounts_records: '账号数据记录 {n} 条',
      tools_one_click: '平台工具一键安装', tools_one_click_desc: '下载官方安装包并静默安装。支持直链的平台自动下载；其余平台将打开官方下载页。',
      tools_dl_path: '安装包下载路径', tools_dl_path_desc: '留空使用系统临时目录。安装程序本身默认装到系统默认位置（安装器参数可控部分有限）。',
      tools_dl_path_ph: 'D:\\Tools\\Installers',
      detect_state: '检测安装状态…', tools_load_failed: '加载失败: ',
      installed_yes: '✓ 已安装 ', not_detected: '未检测到', reinstall: '重新安装', one_click_install: '一键下载安装', open_dl_page: '打开下载页',
      specify_dir: '指定目录', specify_dir_tip: '客户端装在非常规位置时，手动指定安装目录',
      dl_state_downloading: '下载中 {n}%', dl_state_downloaded: '下载完成', dl_state_installing: '安装中…', dl_state_installed: '✓ 已安装', dl_state_timeout: '安装器已退出（请手动确认）',
      installed_accounts: ' · 已装 {n} 个账号', remaining_quota: ' · 剩余额度 ', no_account_added: ' · 未添加账号',
      direct_link: '支持应用内直链下载 · ', no_direct_link: '无稳定直链 → 打开官方下载页手动下载',
      tools_notice: '已安装检测：自动扫描注册表卸载信息与常见安装路径；若客户端装在自定义位置，点「指定目录」手动导入安装目录（含主程序的文件夹）。',
      open_source_repo: '开源地址',
      disclaimer: '免责声明：本项目仅供学习与技术交流使用，请勿用于任何商业用途。使用本项目产生的全部风险与后果由使用者自行承担；请遵守各平台的服务条款，因违规使用导致的账号限制、封禁等后果与本项目及其作者无关。',
      input_install_dir: '请输入 {name} 的安装目录（含主 exe 的文件夹路径）：',
      install_dir_cleared: '已清除手动指定目录', imported: '已导入：', op_error: '操作失败: ',
      processing: '处理中…', silent_install_started: '{name} 静默安装已启动，完成后状态会更新为「已安装」',
      no_direct_open_page: '该平台暂无稳定直链，已打开官网下载页', dl_install_failed: '下载/安装失败: ',
      dl_page_opened: '已打开官方下载页', open_failed: '打开失败: ', install_flow_done: ' 安装流程结束',
      tool_trae: 'Trae（字节跳动）', tool_workbuddy: 'WorkBuddy / CodeBuddy（腾讯）', tool_minimax: 'MiniMax Code',
      tool_qoder: 'Qoder', tool_zcode: 'ZCode（智谱）', tool_codearts: '华为云码道',
      settings_saved: '设置已保存', settings_reverted: '已还原未保存修改', settings_saved_sched: '设置已保存，任务按新计划安排',
      // 登录
      login: '登录', open_auth_page: '打开授权页面', refresh_auth_window: '刷新授权窗口',
      login_title: '登录 {p}', starting_auth: ' 正在发起授权…', auth_link_ready: '授权链接已生成（5 分钟内有效）：',
      waiting_auth: '等待授权完成…', login_ok: '✔ 登录成功：', auth_timeout: '授权已超时，请重新发起',
      // 静态页面文案
      checkin_sub: '按账号查看领取进度；支持多个独立授权账号，逐个执行。',
      sched_checkin_label: '计划签到时间（北京时间）', beijing_note: '所有平台统一按北京时间执行',
      stats_sub: '按登录账号切换，查看每天 / 每周 / 每月 / 每年的积分记录汇总。',
      select_account: '选择账号', trend: '趋势', credit_in: '签到到账',
      route_title: '账号路由 · 模型网关',
      route_sub: '切换请求平台、配置路由规则；网关对本地提供 OpenAI 兼容接口。',
      cancel_changes: '取消修改', save_settings: '保存设置',
      quit_desc: '选择窗口关闭行为：最小化到托盘可保持网关与自动签到继续运行。',
      quit_title: '关闭 AI Free Router', quit_bg: '退至后台', quit_app: '退出程序',
      // locale 名称
      lang_zh: '中文（简体）', lang_en: 'English'
    },
    'en-US': {
      app_name: 'AI Free Router',
      app_name_short: 'AI Free Router',
      brand_symbol: 'R',
      ok: 'OK', cancel: 'Cancel', save: 'Save', close: 'Close', refresh: 'Refresh',
      confirm: 'Confirm', loading: 'Loading…', saving: 'Saving…', copy: 'Copy', copied: 'Copied',
      current: 'Current', all: 'All', daily: 'Daily', weekly: 'Weekly', monthly: 'Monthly', yearly: 'Yearly',
      failed: 'Failed', minutes: ' min',
      minimize_to_tray: 'Minimize to tray', close_app: 'Close', one_click_checkin: 'Check in all', refresh_balance: 'Refresh balance',
      nav_home: 'Home', nav_accounts: 'Accounts', nav_checkin: 'Check-in', nav_stats: 'Statistics', nav_route: 'Routing', nav_settings: 'Settings',
      today_cycle: 'Today\u2019s cycle', points_trend: 'Points trend', my_ai_tools: 'My AI tools', manage_accounts: 'Manage accounts →',
      checkin_points: 'Check-in points', period_points: 'Check-in points this period +', chart_load_failed: 'Failed to load chart: ',
      empty_no_tools: 'No AI tools added yet — go to \u201cAccounts\u201d to add an account',
      available: 'Available', checked_in: '✓ Checked in', last: 'Last ',
      period_points_short: 'Points this period', records: 'Records', buckets: 'Buckets', all_accounts: 'All accounts',
      token_usage: 'Token usage', token_period: 'Tokens this period', token_stats_all: 'Token usage aggregates all accounts via the local gateway log; per-account token stats are not available.', token_stats_single: 'Single-account view shows check-in points only; switch to \u201cAll accounts\u201d for token usage.',
      stat_read_failed: 'Failed to load statistics: ',
      desc_trae: 'ByteDance\u2019s AI IDE with SOLO mode',
      desc_workbuddy: 'Tencent\u2019s AI coding workbench (China / global)',
      desc_minimax: 'MiniMax agentic coding tool',
      desc_qoder: 'Alibaba\u2019s agentic coding IDE',
      desc_codearts: 'Huawei Cloud CodeArts Pangu assistant, IDE plugin',
      desc_codearts_desktop: 'Huawei CodeArts desktop client',
      desc_zcode: 'Zhipu coding agent',
      desc_doubao: 'Doubao desktop client',
      st_success: 'Claimed', st_already: 'Claimed today', st_failed: 'Failed', st_pending: 'Awaiting confirm',
      st_needs_action: 'Action needed', st_no_activity: 'No activity', st_idle: 'Pending', st_auth_expired: 'Auth expired',
      today_claimed: 'claimed today', checkin_error: 'check-in error', today_not_claimed: 'not claimed today',
      cred_expires: ' · credentials expire ', available_balance: 'Available balance',
      pname_trae: 'Trae', pname_workbuddy: 'WorkBuddy', pname_minimax: 'MiniMax', pname_qoder: 'Qoder',
      pname_codearts: 'Huawei CodeArts', pname_codearts_desktop: 'CodeArts Desktop', pname_zcode: 'ZCode', pname_doubao: 'Doubao',
      accounts_connected: '{n} platforms connected · {m} accounts', multi_account: 'Multi-account', single_account: 'Single account',
      n_accounts: '{n} accounts', add_account: '＋ Add account', scan_local: 'Scan this PC', scanning: 'Scanning…',
      empty_no_accounts: 'No accounts yet — click \u201cAdd account\u201d in the top right to start',
      import_ok: 'Imported: ', import_failed: 'Import failed: ', data_load_failed: 'Failed to load data: ',
      checkin_now: 'Check in now', quota_refreshed: 'Quota refreshed', status: 'Status', last_time: 'Last',
      no_accounts_in_filter: 'No accounts under this filter', claim_ok: 'Claimed', already_claimed: 'Already claimed today', op_failed: 'Failed: ',
      filter_all: 'All', filter_done: 'Done', filter_pending: 'Waiting', filter_confirm: 'To confirm', filter_attention: 'Needs attention',
      cycle_done: 'completed this cycle', waiting_claim: 'waiting to claim',
      checkin: 'Check in', quota_btn: 'Quota', device_btn: 'Device ID', device_btn_tip: 'A Trae device ID can check in only one account per day — set a dedicated device ID for this account',
      delete: 'Delete', deleted: 'Deleted', auto_on: 'Auto check-in enabled', auto_off: 'Auto check-in disabled',
      delete_account: 'Delete account', delete_confirm_msg: 'Delete account {u}? History is kept.',
      dev_unavailable: 'Device ID settings unavailable',
      dev_desc: 'A Trae device ID can check in only one account per day and must be registered on this machine (otherwise check-in fails with 9074). Use separate IDs for multiple accounts; saving empty = auto-pick a free device ID.',
      dev_loading: ' Reading device IDs…', dev_local_found: 'Found {n} registered device IDs on this machine:', dev_local_n: 'Local device {n}',
      dev_auto_option: 'Auto (pick a free device ID at check-in)',
      dev_none_found: 'No Trae client device IDs found on this machine. On a computer with Trae installed and signed in, open %APPDATA%\\Trae CN\\User\\globalStorage\\storage.json and look for keys starting with iCubeAuthInfo://icube-dc: — the digits after the colon are the device ID.',
      dev_manual_ph: 'Enter device ID manually (8-20 digits, optional)',
      dev_set_ok: 'Device ID set: ', dev_cleared: 'Cleared — a free device ID will be picked at check-in',
      checkin_running: ' Checking in…', checkin_done_stats: 'Check-in finished: {ok} claimed · {al} already · {bad} failed', checkin_failed: 'Check-in failed: ',
      refreshing: 'Refreshing…', balance_refreshed: 'Balance refreshed', fetching_quota: 'fetching quota…', login_ok_toast: 'Signed in: {n}, fetching quota',
      dev_title: 'Set device ID', dev_saved: 'Device ID saved',
      gw_title: 'Local gateway · OpenAI-compatible API', port: 'Port', port_restart_hint: 'Restart required after change',
      rate_limit: 'Rate limit', rate_limit_hint: 'RPM · 0 = unlimited', strict_current_only: 'Allow current platform only',
      strict_hint: 'Cross-platform requests are rejected with 403 when on',
      request_platform: 'Request platform', bare_model_hint: 'Default routing for bare model ids',
      models_follow_platform: '/v1/models returns only the models of the platform selected under \u201cRequest platform\u201d; the list follows platform switches.',
      model_id_hint: 'Use <code>platform/model</code> as the model id; bare ids go to the current platform.',
      n_accounts_suffix: '{n} accounts', signin_only: ' (check-in only)', provider_label: 'provider: ', available_accounts: '<b>{n}</b> accounts available',
      no_chat_protocol: ' · No public chat protocol, not proxied by the gateway',
      account_switch: 'Account switching', account_switch_hint: 'Current platform · click to switch local account', switch_next: '↻ Switch to next',
      model_test: 'Model test', model_test_hint: 'Requests are forwarded to the platform via the local gateway', refresh_models: 'Refresh models',
      model_placeholder: '(Click \u201cRefresh models\u201d to list models of the current platform)', no_models: '(No models available)',
      test_msg_placeholder: 'Enter a test message, e.g. "Hello, introduce yourself"', send_test: 'Send test',
      fetching_models: 'Fetching models…', n_models: '{n} models', fetch_models_failed: 'Fetch failed: ',
      codearts_expired: 'CodeArts credentials may have expired (client refresh rotates STS) ',
      sync_from_client: 'Sync latest session from client', syncing: 'Syncing…',
      sync_ok: 'Synced latest credentials for {names}',
      sync_no_login: 'No login session in the client — sign in via the CodeArts client first, or import via \u201cScan this PC\u201d in Accounts',
      sync_failed: 'Sync failed: ', no_accounts_or_expired: 'No available accounts or credentials expired on this platform — check Accounts',
      select_model_first: 'Fetch and select a model first', input_test_msg: 'Enter a test message',
      requesting: 'Requesting…', waiting_response: 'Waiting for platform response… (balanced/max modes are deep-reasoning models, may take 1-2 minutes)',
      test_failed: '✗ Failed ({s}s): ', test_ok: '✓ {model} ({s}s{usage}):', request_error: '✗ Request error: ',
      usage_tokens: ' · tokens input {in} + output {out} = {total} · output speed ',
      request_log: 'Request log', clear_log: 'Clear log', no_logs: 'No request logs',
      log_stats: ' · Last {n} · Total {t} · <span style="color:var(--green,#16a34a)">OK {ok}</span> · <span style="color:var(--red,#dc2626)">Failed {fail}</span>',
      log_time: 'Requested at ', log_duration: ' · took ', no_model: '(no model)',
      clear_logs_title: 'Clear request log', clear_logs_msg: 'Clear the entire request log? This cannot be undone.', clear_btn: 'Clear',
      cleared_n: 'Cleared {n} log entries', clear_failed: 'Clear failed: ',
      gw_read_failed: 'Failed to read gateway status', copied_url: 'Endpoint copied', copy_failed: 'Copy failed: ',
      no_accounts_current: 'No available accounts on the current platform', switched_account: 'Switched account: ', cred_check_failed: ' · credential check failed: ',
      switch_failed: 'Switch failed: ', client_not_running: '{p} client was not running — launched automatically', switched_platform: 'Switched request platform: ',
      refreshing_logs: 'Refreshing…', logs_refreshed: 'Request logs refreshed', refresh_failed: 'Refresh failed: ',
      no_accounts_platform: 'No available accounts for platform ({p}) — add one in Accounts first', balance: ' · balance ', quota_exhausted: 'Quota exhausted',
      rule_modal_title: 'New routing rule',
      rule_modal_desc: 'Requests whose model prefix matches are routed to the specified platform/account; leave empty to follow the current platform.',
      rule_model_prefix: 'Model prefix (* matches all)', rule_model_ph: 'e.g. workbuddy/glm-5.3-flash or *',
      rule_target_platform: 'Target platform', rule_follow_platform: 'Follow current platform', rule_account: 'Account (optional)',
      rule_default_account: 'Default account within platform', rule_saved: 'Rule saved',
      port_invalid: 'Port must be between 1 and 65535', port_saved_restart: 'Port saved — takes effect after app restart', port_saved: 'Port saved',
      rate_saved: 'Rate limit saved (effective immediately)', strict_on: '\u201cAllow current platform only\u201d enabled', strict_off: '\u201cAllow current platform only\u201d disabled',
      save_failed: 'Save failed: ', set_failed: 'Failed to save: ',
      gw_workbuddy: 'WorkBuddy (CN)', gw_workbuddy_intl: 'WorkBuddy (Global)', gw_trae: 'Trae (CN, SOLO)',
      gw_trae_enterprise: 'Trae Enterprise', gw_trae_global: 'Trae (Global)', gw_zcode: 'ZCode (Zhipu)',
      gw_minimax: 'MiniMax', gw_qoder: 'Qoder', gw_codearts: 'Huawei CodeArts (IDE)', gw_codearts_desktop: 'CodeArts Desktop',
      settings_sub: 'Set up this workspace at your own pace.', tab_tasks: 'Automated tasks', tab_general: 'General', tab_tools: 'Tool install', tab_data: 'Data & version',
      language: 'Language', language_desc: 'Applies immediately; tray menu updates after restart.',
      appearance: 'Appearance', appearance_desc: 'UI color scheme: follow system, light or dark. "Follow system" switches in real time with the OS color mode.',
      theme_system: 'System', theme_light: 'Light', theme_dark: 'Dark',
      auto_checkin: 'Automatic check-in', auto_checkin_desc: 'Runs only for accounts with auto check-in enabled.',
      catchup: 'Catch up missed runs', catchup_desc: 'Checks the current cycle on startup, wake-up or schedule change.',
      refresh_interval: 'Balance refresh interval', refresh_interval_desc: 'Queries quota only, no model calls consumed.',
      m30: '30 min', m60: '60 min', m120: '120 min',
      sched_time: 'Scheduled time', sched_time_desc: 'Beijing time to run daily (ignored in random mode).', random_time: 'Random time', random: 'Random',
      exec_preview: 'Run preview · effective after save', saved_time_is: 'Saved schedule: {v}.', every_day: 'Daily {v}',
      notice_beijing: 'All platforms run at your saved Beijing time. With catch-up on, accounts that missed their scheduled time are checked; claimed accounts are skipped.',
      base_usage_stats: 'Basic usage statistics', base_usage_desc: 'Reports only a random install id, version, date and run status — never platform credentials.',
      privacy_notice: 'Platform accounts, credentials and points history are stored locally; no credentials are uploaded.',
      app_version: 'Rust + Tauri · database: credit.sqlite (local)', local_edition: 'Local edition',
      software_update: 'Software update', checking_release: 'Checking GitHub Releases…', recheck: 'Check again',
      download_update: 'Download update', restart_install: 'Restart & install',
      data_dir: 'Data directory', data_dir_desc: '%APPDATA%/credit-rs/credit.sqlite · credentials encrypted via system keystore', open_dir: 'Open folder',
      db_notice: 'The database is backed up before upgrade; exports exclude credentials. After the update package downloads, click \u201cRestart & install\u201d (silent install).',
      checking_update: 'Checking for updates…', new_version_found: 'New version found: <b>v{latest}</b> (current v{current})',
      goto_release: 'Open the Release page to download', already_latest: 'Already the latest version v{v}', check_failed: 'Check failed: ',
      downloading: 'Downloading…', download_done_toast: 'Download complete ({mb} MB) — click \u201cRestart & install\u201d', download_failed: 'Download failed: ', install_failed: 'Install failed: ',
      accounts_records: '{n} account records',
      tools_one_click: 'One-click platform tool install', tools_one_click_desc: 'Downloads official installers and installs silently. Platforms with direct links download automatically; others open the official download page.',
      tools_dl_path: 'Installer download path', tools_dl_path_desc: 'Leave empty to use the system temp directory. Installers themselves default to the system default location (limited installer options).',
      tools_dl_path_ph: 'D:\\Tools\\Installers',
      detect_state: 'Detecting install status…', tools_load_failed: 'Load failed: ',
      installed_yes: '✓ Installed ', not_detected: 'Not detected', reinstall: 'Reinstall', one_click_install: 'Download & install', open_dl_page: 'Open download page',
      specify_dir: 'Set folder', specify_dir_tip: 'Point to the install folder manually when the client is installed in a non-default location',
      dl_state_downloading: 'Downloading {n}%', dl_state_downloaded: 'Downloaded', dl_state_installing: 'Installing…', dl_state_installed: '✓ Installed', dl_state_timeout: 'Installer exited (please verify manually)',
      installed_accounts: ' · {n} accounts imported', remaining_quota: ' · remaining quota ', no_account_added: ' · no account added',
      direct_link: 'In-app direct download supported · ', no_direct_link: 'No stable direct link → open the official download page',
      tools_notice: 'Install detection scans registry uninstall entries and common install paths; if the client is installed elsewhere, click \u201cSet folder\u201d to import the install folder (the one containing the main exe).',
      open_source_repo: 'Open source',
      disclaimer: 'Disclaimer: this project is for learning and technical exchange only and must not be used for commercial purposes. You assume all risks and consequences arising from its use; comply with each platform\u2019s terms of service \u2014 the project and its authors are not liable for account restrictions, bans or similar outcomes caused by misuse.',
      input_install_dir: 'Enter the install folder of {name} (the folder containing the main exe):',
      install_dir_cleared: 'Manual folder cleared', imported: 'Imported: ', op_error: 'Operation failed: ',
      processing: 'Processing…', silent_install_started: '{name} silent install started; status becomes \u201cInstalled\u201d when finished',
      no_direct_open_page: 'No stable direct link; opened the official download page', dl_install_failed: 'Download/install failed: ',
      dl_page_opened: 'Official download page opened', open_failed: 'Failed to open: ', install_flow_done: ' install finished',
      tool_trae: 'Trae (ByteDance)', tool_workbuddy: 'WorkBuddy / CodeBuddy (Tencent)', tool_minimax: 'MiniMax Code',
      tool_qoder: 'Qoder', tool_zcode: 'ZCode (Zhipu)', tool_codearts: 'Huawei Cloud CodeArts',
      settings_saved: 'Settings saved', settings_reverted: 'Unsaved changes reverted', settings_saved_sched: 'Settings saved; tasks rescheduled',
      login: 'Sign in', open_auth_page: 'Open authorization page', refresh_auth_window: 'Refresh authorization window',
      login_title: 'Sign in to {p}', starting_auth: ' Starting authorization…', auth_link_ready: 'Authorization link generated (valid for 5 minutes):',
      waiting_auth: 'Waiting for authorization…', login_ok: '✔ Signed in: ', auth_timeout: 'Authorization timed out — please try again',
      checkin_sub: 'View claim progress per account; supports multiple independently authorized accounts, run one by one.',
      sched_checkin_label: 'Scheduled check-in time (Beijing)', beijing_note: 'All platforms run on Beijing time',
      stats_sub: 'Switch by signed-in account to view daily / weekly / monthly / yearly points summaries.',
      select_account: 'Select account', trend: 'Trend', credit_in: 'Check-in credited',
      route_title: 'Routing · Model gateway',
      route_sub: 'Switch request platform and configure routing rules; the gateway serves an OpenAI-compatible API locally.',
      cancel_changes: 'Discard changes', save_settings: 'Save settings',
      quit_desc: 'Choose the window close behavior: minimize to tray keeps the gateway and auto check-in running.',
      quit_title: 'Close AI Free Router', quit_bg: 'To background', quit_app: 'Quit',
      lang_zh: '中文（简体）', lang_en: 'English'
    }
  };

  var SAVED_KEY = 'locale';
  var SUPPORTED = ['zh-CN', 'en-US'];
  var curLocale = 'zh-CN';

  function normalize(l) {
    if (!l) return null;
    l = String(l).trim();
    if (!l) return null;
    var low = l.toLowerCase();
    if (low.indexOf('zh') === 0) return 'zh-CN';
    if (low.indexOf('en') === 0) return 'en-US';
    return DICT[l] ? l : null;
  }

  function detectNavigator() {
    try {
      var langs = navigator.languages || (navigator.language ? [navigator.language] : []);
      for (var i = 0; i < langs.length; i++) {
        var n = normalize(langs[i]);
        if (n) return n;
      }
    } catch (e) {}
    return 'zh-CN';
  }

  function resolveInitial() {
    var stored = null;
    try { stored = window.localStorage ? window.localStorage.getItem('ui-locale') : null; } catch (e) {}
    return normalize(stored) || detectNavigator();
  }

  function applyVars(str, vars) {
    if (vars) {
      for (var k in vars) {
        if (Object.prototype.hasOwnProperty.call(vars, k)) {
          str = str.split('{' + k + '}').join(String(vars[k]));
        }
      }
    }
    return str;
  }

  function t(key, vars) {
    var d = DICT[curLocale] || {};
    var s = d[key];
    if (s == null) s = DICT['zh-CN'][key];
    if (s == null) s = key;
    return applyVars(String(s), vars);
  }

  function localeTag() { return curLocale === 'en-US' ? 'en-US' : 'zh-CN'; }

  function fmtNumber(n) {
    if (n == null) return '—';
    return (Math.round(n * 100) / 100).toLocaleString(localeTag());
  }

  function fmtDate(d, opts) {
    try { return new Date(d).toLocaleDateString(localeTag(), opts || {}); }
    catch (e) { return String(d); }
  }

  function applyI18n(root) {
    var scope = root || document;
    var nodes = scope.querySelectorAll('[data-i18n]');
    for (var i = 0; i < nodes.length; i++) nodes[i].textContent = t(nodes[i].getAttribute('data-i18n'));
    var phs = scope.querySelectorAll('[data-i18n-placeholder]');
    for (var j = 0; j < phs.length; j++) phs[j].setAttribute('placeholder', t(phs[j].getAttribute('data-i18n-placeholder')));
    var tls = scope.querySelectorAll('[data-i18n-title]');
    for (var k = 0; k < tls.length; k++) tls[k].setAttribute('title', t(tls[k].getAttribute('data-i18n-title')));
    var als = scope.querySelectorAll('[data-i18n-aria]');
    for (var m = 0; m < als.length; m++) als[m].setAttribute('aria-label', t(als[m].getAttribute('data-i18n-aria')));
    if (!root) {
      document.documentElement.lang = localeTag();
      if (window.__TAURI__ && window.__TAURI__.window && window.__TAURI__.window.getCurrentWindow) {
        try { window.__TAURI__.window.getCurrentWindow().setTitle(t('app_name')); } catch (e) {}
      } else if (document.title !== undefined) {
        document.title = t('app_name');
      }
    }
  }

  function persist(locale) {
    try { if (window.localStorage) window.localStorage.setItem('ui-locale', locale); } catch (e) {}
    try {
      if (typeof window.invoke === 'function') {
        var r = window.invoke('settings_set', { key: SAVED_KEY, value: locale });
        if (r && typeof r.catch === 'function') r.catch(function () {});
      }
    } catch (e) {}
  }

  var I18n = {
    getLocale: function () { return curLocale; },
    locales: SUPPORTED,
    localeTag: localeTag,
    isZh: function () { return curLocale === 'zh-CN'; },
    fmtNumber: fmtNumber,
    fmtDate: fmtDate,
    // 本机 Profile 的 localStorage 被清过（重装/清缓存）但 DB 还留着上次选择时，
    // 用 DB 里的 locale 恢复；用户从未选择过（DB 也为空）则保持「跟随系统」。
    restoreFromDb: function (dbLocale) {
      var n = normalize(dbLocale);
      if (!n) return curLocale;
      try { if (window.localStorage && window.localStorage.getItem('ui-locale')) return curLocale; } catch (e) {}
      if (n === curLocale) return curLocale;
      curLocale = n;
      applyI18n();
      if (typeof window.renderNav === 'function') { try { window.renderNav(); } catch (e) {} }
      return curLocale;
    },
    setLocale: function (locale) {
      var n = normalize(locale);
      if (!n || n === curLocale) return curLocale;
      curLocale = n;
      persist(n);
      applyI18n();
      if (typeof window.renderNav === 'function') { try { window.renderNav(); } catch (e) {} }
      if (typeof window.refreshAll === 'function') { try { window.refreshAll(); } catch (e) {} }
      return curLocale;
    }
  };

  curLocale = resolveInitial();

  window.I18n = I18n;
  window.t = t;
})();
