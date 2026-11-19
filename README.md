# AI路由助手

Rust + Tauri 2 实现的多平台 AI 编程工具积分管理桌面应用：多账号登录、自动签到、额度直显、积分统计、账号路由。

## 功能

| 模块 | 说明 |
|---|---|
| 多账号登录 | Qoder / Trae / WorkBuddy / MiniMax / 华为云码道 五平台，应用内授权子窗口完成 OAuth/设备流登录，同平台可添加多个账号 |
| 签到中心 | 单账号 / 一键全部签到；结果分类 success / already / no-activity / needs-action |
| 自动签到 | 计划时间每日执行（北京时间），随机时间偏移、错过补跑（catchup）、失败重试提示 |
| 额度直显 | 查询后持久化，启动即显示余额，无需手动点击；支持 30/60/120 分钟自动刷新 |
| 积分统计 | 周 / 月 / 年 ECharts 趋势图 + 明细表格，按账号筛选 |
| 积分统计维度 | 每天 / 每周 / 每月 / 每年分桶汇总 |
| 账号管理 | 分平台分组展示、启用开关、删除、扫描本机导入（trae/workbuddy/qoder/zcode/minimax） |
| 账号路由 | 内置 OpenAI 兼容网关（详见下方「账号路由」） |
| 设置 | 自动签到、补跑、计划时间、刷新间隔、遥测开关、工具安装、自动更新，全部实时生效 |
| 托盘常驻 | 关闭窗口最小化到系统托盘，网关与自动签到持续运行；托盘菜单可恢复/退出 |

## 账号路由

内置本地模型网关：把各平台账号聚合成一个 **OpenAI 兼容接口**，任意支持
OpenAI 的客户端（Cherry Studio / ChatBox / LobeChat / Dify 等）都能直接使用。

### 接口

```
GET  /v1/models              模型清单（跟随当前请求平台）
POST /v1/chat/completions    Chat Completions（流式 SSE / 非流式）
GET  /v1/platforms           平台清单与当前平台
```

### 接入配置

| 配置项 | 值 |
|---|---|
| Base URL | `http://127.0.0.1:8756/v1`（端口可在「账号路由」页修改） |
| API Key | 默认留空；可在设置中启用 Bearer 鉴权 |

### 核心特性

- **平台切换**：账号路由页下拉切换请求平台，切换时自动检测并拉起对应平台客户端
- **模型测试**：内置测试面板，获取当前平台模型 → 发送测试消息 → 显示回复与
  Token 消耗速度（tok/s）
- **多账号容错**：同平台多账号自动轮换；请求失败自动 failover 到下一个账号
- **速率限制**：RPM 滑动窗口限速（0 = 不限），实时生效
- **平台锁定**：「仅允许当前平台」开启后，跨平台模型请求被 403 拒绝
- **设备指纹**：各平台请求逐字对齐官方客户端（WorkBuddy Turing 设备签名、
  Qoder sidecar 官方签名、MiniMax mavis 会话头等）
- **请求日志**：每次调用记录成功/失败、状态码、耗时与原因，可一键清空
- **模型清单**：96 个模型（workbuddy 33 / trae 33 / qoder 20 / zcode 6 / minimax 4），
  全部来自各平台官方客户端目录并去重
- **模型展示差异说明**：网关 `/v1/models` 返回的模型来自各平台**官方客户端的本地目录/配置文件**，
  与平台客户端 UI 当前展示的模型列表**可能不完全一致**——Qoder 云端为 auto 自动路由、无公开
  models 端点，清单是其客户端配置的静态快照；且网关会剔除内部/实验通道变体（`-ctrl`/`-exp`/`preview-x` 等）
  并对同名模型去重。平台侧随时可能上下架或更名模型，网关清单可能滞后于平台更新，请以实际调用结果为准。

### 使用流程

1. 账号管理页添加各平台账号（至少一个）
2. 账号路由页选择「请求平台」（下拉框；客户端未启动会自动拉起）
3. 模型测试卡可先验证通路
4. 外部客户端填入上方 Base URL，直接对话

## 平台

- Windows 10/11 x64（NSIS 安装包）
- macOS 11+（Intel / Apple Silicon DMG）
- Linux x64（deb / AppImage / rpm）

## 从源码构建

依赖：Rust 1.77+（stable），Tauri 2 [系统依赖](https://tauri.app/start/prerequisites/)。

```bash
cargo install tauri-cli --version "^2" --locked
cargo tauri build
# 产物在 target/release/bundle/（Windows: nsis/*.exe; macOS: dmg/*.dmg; Linux: deb/*.deb, appimage/*.AppImage, rpm/*.rpm）
```

仅构建调试版：

```bash
cargo build --release -p credit-tauri
# 运行 target/release/credit-tauri.exe（Windows；其他平台为对应二进制名）
```

## 下载

见 [Releases](../../releases)：每个 tag（`v*`）自动构建四组产物（Windows / Linux / macOS x64 / macOS arm64）。

## 项目结构

```
crates/
  credit-core/        # 核心库：平台适配器、登录编排、签到协议、DB、调度
    src/adapters/     # trae / workbuddy / minimax / qoder / codearts 协议实现
    src/login.rs      # OAuth 回调端口监听 / 设备流轮询 / 会话管理
    src/db.rs         # SQLite（accounts PK(provider,uid) 多账号 / history / settings / gateway_logs）
    src/schedule.rs   # 计划任务原语
  credit-gateway/     # 本地模型网关（axum）：OpenAI 兼容接口 + 账号路由 + 限速 + 日志
  credit-cli/         # 命令行入口（import / checkin / quota）
src-tauri/            # Tauri 2 壳
  src/main.rs         # RPC 命令面 + 自动签到调度器 + 托盘 + 更新
  ui/                 # 前端（vanilla JS + 本地 echarts，无构建步骤，编译期内嵌）
.github/workflows/    # 多平台 CI 打包
```

## 数据与安全

- 所有凭据仅保存在本机：`%APPDATA%/credit-rs/credit.sqlite`，凭据永不上传
- 登录回调仅在 `127.0.0.1` 监听，会话 5 分钟超时自动清理
- 统计/遥测仅记录随机安装标识、版本、日期（可在设置中关闭）

## License

本项目基于 [MIT License](LICENSE) 并附加非商业限制条款（附加条款优先），详见 [LICENSE](LICENSE)：

- 本项目**仅供学习和技术研究使用**，严禁用于任何商业用途
- **禁止二次开发后销售**或以任何形式付费分发本项目及其衍生版本
- 使用本项目产生的一切后果由使用者自行承担，与项目作者无关
