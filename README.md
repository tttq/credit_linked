# AI路由助手

Rust + Tauri 2 实现的多平台 AI 编程工具积分管理桌面应用：多账号登录、自动签到、额度直显、积分统计、账号路由。

> 本仓库为 [AI Credit Manager (Electron)](https://github.com/) 的 Rust 复刻版本，协议逻辑对齐原版 vendor adapters。

## 功能

| 模块 | 说明 |
|---|---|
| 多账号登录 | Qoder / Trae / WorkBuddy / MiniMax / 华为云码道 五平台，应用内授权子窗口完成 OAuth/设备流登录，同平台可添加多个账号 |
| 签到中心 | 单账号 / 一键全部签到；结果分类 success / already / no-activity / needs-action |
| 自动签到 | 计划时间每日执行（北京时间），随机时间偏移、错过补跑（catchup）、失败重试提示 |
| 额度直显 | 查询后持久化，启动即显示余额，无需手动点击；支持 30/60/120 分钟自动刷新 |
| 积分统计 | 周 / 月 / 年 ECharts 趋势图 + 明细表格，按账号筛选 |
| 积分统计维度 | 每天 / 每周 / 每月 / 每年分桶汇总 |
| 账号管理 | 分平台分组展示、启用开关、删除、扫描本机导入（trae/workbuddy/qoder） |
| 账号路由 | 占位界面（规则引擎待开放） |
| 设置 | 自动签到、补跑、计划时间、刷新间隔、遥测开关，全部实时生效 |

## 平台

- Windows 10/11 x64（NSIS 安装包 / MSI）
- macOS 11+（Intel / Apple Silicon DMG）
- Linux x64（deb / AppImage / rpm）

## 从源码构建

依赖：Rust 1.77+（stable），Tauri 2 [系统依赖](https://tauri.app/start/prerequisites/)。

```bash
cargo install tauri-cli --version "^2" --locked
cargo tauri build
# 产物在 target/release/bundle/
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
    src/adapters/     # trae / workbuddy / minimax / qoder / codearts / doubao 协议实现
    src/login.rs      # OAuth 回调端口监听 / 设备流轮询 / 会话管理
    src/db.rs         # SQLite（accounts PK(provider,uid) 多账号 / history / settings）
    src/schedule.rs   # 计划任务原语
  credit-cli/         # 命令行入口（import / checkin / quota）
src-tauri/            # Tauri 2 壳
  src/main.rs         # RPC 命令面 + 自动签到调度器
  ui/                 # 前端（vanilla JS + 本地 echarts，无构建步骤，编译期内嵌）
  capabilities/       # Tauri 权限声明
.github/workflows/    # 多平台 CI 打包
```

## 数据与安全

- 所有凭据仅保存在本机：`%APPDATA%/credit-rs/credit.sqlite`（绝不与原版 Electron 应用目录共享）
- 统计/遥测仅记录随机安装标识、版本、日期；凭据永不上传
- 登录回调仅在 `127.0.0.1` 随机/固定端口监听，会话 5 分钟超时自动清理

## 协议说明

各平台登录/签到/额度协议通过对齐原版 `vendor/*.mjs` 逆向获得，仅供学习研究。请遵守目标平台服务条款，风险自担。

## License

MIT
