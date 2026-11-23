# AI Free Router

多平台 AI 编程工具**积分管理 + 账号路由**桌面应用。

Rust 原生 GUI（`gpui_kit` 0.10）,界面、业务、网关全部在同一进程内，启动无控制台黑框、exe 自带图标。

支持平台：**Trae / WorkBuddy / MiniMax / Qoder / ZCode / 华为云码道**。

---

## 环境要求

| 项 | 说明 |
|---|---|
| 系统 | Windows 10 / 11 x64 |
| Rust | stable ≥ **1.92**（MSVC 工具链：`rustup default stable-x86_64-pc-windows-msvc`） |
| Windows SDK | 构建时用 `rc.exe` 把图标编进 exe；找不到只告警、不影响编译 |

---

## 快速开始（启动命令）

```bash
# 1) 首次构建全部 crate（会编译 gpui，耗时较长，约 3-8 分钟）
cargo build

# 2) 启动桌面应用（开发模式，最常用）
cargo run -p credit-ui

# 3) 发布版（推荐日常使用）
cargo build --release -p credit-ui
.\target\release\credit-ui.exe
```

产物路径：

- 开发版：`target\debug\credit-ui.exe`
- 发布版：`target\release\credit-ui.exe`

> 直接双击 exe 也可以运行（工作目录建议设为仓库根目录，便于写日志）。
> 程序是纯 GUI（`#![windows_subsystem = "windows"]`），**不会弹控制台窗口**。

### 只编译检查（改代码后快速验证）

```bash
cargo check -p credit-ui          # 只检查 UI
cargo check                       # 检查整个工作区
```

---

## 其他组件

### 命令行工具 credit-cli

```bash
cargo run -p credit-cli -- accounts                  # 列出全部账号
cargo run -p credit-cli -- import trae               # 扫描本机导入 Trae 账号（加 --intl 导入国际版）
cargo run -p credit-cli -- state  trae <uid>         # 查询签到状态
cargo run -p credit-cli -- checkin trae <uid>        # 执行签到
cargo run -p credit-cli -- quota  trae <uid>         # 查询额度
cargo run -p credit-cli -- selftest                  # 加密协议自检
```

### 模型网关

网关由桌面应用内部启动（默认 `http://127.0.0.1:8756`），随 UI 一起运行，无需单独启动。
`credit-gateway` 同时也是可复用的 library crate。

---

## 数据与日志位置

| 内容 | 路径 |
|---|---|
| 数据库（账号/历史/设置） | `%APPDATA%\credit-rs\credit.sqlite` |
| 崩溃日志 | 程序工作目录 `credit-ui-panic.log`，同时写 `C:\temp\credit-ui-panic.log` |
| 安装包默认下载目录 | `%TEMP%\credit-rs-downloads`（可在「设置 → 工具安装」修改） |

---

## 项目结构

```
credit-rs/
├── Cargo.toml                 # workspace（4 个成员）
├── crates/
│   ├── credit-core/           # 平台适配器、签到/额度、加密、数据库（核心业务）
│   │   └── src/adapters/      # trae / workbuddy / minimax / qoder / zcode / codearts
│   ├── credit-gateway/        # OpenAI 兼容网关（axum）：/v1/models、/v1/chat/completions
│   ├── credit-cli/            # 命令行工具
│   └── credit-ui/             # gpui 桌面端
│       ├── build.rs           # 把 assets/app.ico 编进 exe
│       ├── assets/platforms/  # 平台图标（rust-embed 嵌入）
│       └── src/
│           ├── main.rs        # 入口：64MB 栈 UI 线程、主题恢复、窗口 560×680
│           ├── backend.rs     # 业务命令层（全部经全局 tokio runtime 派发）
│           ├── platform.rs    # 平台元数据 + 图标
│           ├── theme.rs       # 亮/暗双主题色板、跟随系统
│           ├── i18n.rs        # 中英双语文案
│           └── app/           # 视图层（按页面拆分）
│               ├── mod.rs         # 根视图、标题栏、导航、公共组件
│               ├── overview.rs    # 首页
│               ├── accounts.rs    # 账号管理
│               ├── checkin.rs     # 签到中心
│               ├── statistics.rs  # 积分统计
│               ├── routing.rs     # 账号路由
│               ├── settings.rs    # 设置（4 个 tab）
│               └── actions.rs     # 全部交互动作
```

---

## 功能

| 模块 | 说明 |
|---|---|
| 多账号登录 | 五平台支持应用内授权；同平台可添加多个账号，各自独立凭据 |
| 扫码/扫描导入 | 读取本机已登录客户端的凭据直接导入（Trae / WorkBuddy / Qoder / ZCode / MiniMax / 码道） |
| 签到中心 | 单账号 / 一键全部签到（**严格串行**，一个完成再处理下一个，单个失败不影响其余） |
| 自动签到 | 每日计划时间执行（北京时间）、随机时间偏移、错过补跑、失败记录 |
| 额度显示 | 查询后持久化，启动即显示余额；60 秒自动刷新 |
| 凭据续期 | 后台每 10 分钟巡检，**提前 30 分钟**自动续期临近过期凭据 |
| 积分统计 | 按天 / 周 / 月 / 年聚合，折线趋势图 + 峰值/合计摘要 |
| 账号路由 | 内置 OpenAI 兼容网关，平台下拉切换、账号一键切换、模型测试、请求日志 |
| 设置 | 自动任务、通用（语言/外观）、工具安装（下载进度条）、数据与版本 |

---

## 账号路由（模型网关）

把各平台账号聚合成一个 **OpenAI 兼容接口**，支持任意 OpenAI 客户端（Cherry Studio / ChatBox / LobeChat / Dify 等）。

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

### 特性

- **平台切换**：下拉切换请求平台，切换时自动检测并拉起对应客户端
- **模型测试**：下拉选择模型 → 输入测试消息 → 显示回复与 Token 速率
- **多账号容错**：同平台多账号自动轮换；请求失败自动 failover
- **速率限制**：RPM 滑动窗口限速（0 = 不限）
- **平台锁定**：开启「仅允许当前平台」后，跨平台模型请求返回 403

---

## 界面与窗口行为

- 窗口 560×680（最小 360×420，居中启动），**右侧仅有「最小化 / 关闭」，无最大化按钮**
- 关闭窗口时弹框二选一：**最小化到后台**（网关与自动签到继续运行）或 **退出程序**
- 外观支持 **跟随系统 / 浅色 / 深色**：跟随系统时读取 Windows 个性化开关（`AppsUseLightTheme`），全站配色自动切换
- 顶部导航当前页为主色实心胶囊；标题栏显示当前工作台名称
- 操作结果（签到/保存/报错）以 Toast 在窗口**中下方居中**提示
- 安装包下载显示**进度条**并实时轮询刷新

---

## 免责声明

- 本项目**仅供学习和技术研究使用**，严禁用于任何商业用途
- **禁止二次开发后销售**或以任何形式付费分发本项目及其衍生版本
- 使用本项目产生的一切后果由使用者自行承担，与项目作者无关
