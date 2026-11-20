//! SQLite 存储（对齐原版 schema）：accounts/history/settings；WAL；BEGIN IMMEDIATE。

use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// SQLite 存储句柄：内部连接由互斥锁保护，公开方法均为短事务/单语句。
pub struct Store {
    conn: Mutex<Connection>,
}

/// 账号原始行：(provider, uid, name, secret, auth)
pub type RawAccountRow = (String, String, String, String, String);

/// 每日 token 统计行：(day, prompt_sum, completion_sum, total_sum, requests)
pub type DailyTokenStat = (String, i64, i64, i64, i64);

impl Store {
    /// 打开（必要时创建）数据库：启用 WAL/外键/3s busy 超时，并按序执行 v1→v3 迁移。
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=3000;",
        )?;
        Self::migrate(&conn)?;
        Self::migrate_v2(&conn)?;
        Self::migrate_v3(&conn)?;
        Self::migrate_v4(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 用默认目录（%APPDATA%/credit-rs/credit.sqlite）打开；独立于原版 Electron 应用目录。
    pub fn open_default() -> anyhow::Result<Self> {
        // 独立目录，绝不与原版 Electron 应用（%APPDATA%/credit-tauri(旧版)）共享
        let dir = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("AppData/Roaming"))
            .join("credit-rs");
        Self::open(&dir.join("credit.sqlite"))
    }

    fn migrate(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= 1 {
            return Ok(());
        }
        // 升级前备份（对齐 credit.sqlite.before-v1.bak）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts (
                provider TEXT NOT NULL,
                uid TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                secret TEXT NOT NULL DEFAULT '',
                auto INTEGER NOT NULL DEFAULT 1,
                auth TEXT NOT NULL DEFAULT '{}',
                expires TEXT,
                quota TEXT,
                status TEXT NOT NULL DEFAULT 'idle',
                message TEXT,
                last_checkin TEXT,
                PRIMARY KEY (provider, uid)
            );
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                cycle TEXT NOT NULL,
                status TEXT NOT NULL,
                amount REAL,
                message TEXT,
                at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS history_account ON history(account_id, at DESC);
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            PRAGMA user_version = 1;",
        )?;
        Ok(())
    }

    /// v2：网关请求日志（每请求一条：成功/失败）
    fn migrate_v2(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= 2 {
            return Ok(());
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS gateway_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at TEXT NOT NULL,
                platform TEXT NOT NULL,
                account TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                ok INTEGER NOT NULL,
                status_code INTEGER,
                message TEXT,
                tokens INTEGER
            );
            CREATE INDEX IF NOT EXISTS gateway_logs_at ON gateway_logs(at DESC);
            PRAGMA user_version = 2;",
        )?;
        Ok(())
    }

    /// v3：网关请求日志增加 duration_ms（请求耗时，毫秒）
    fn migrate_v3(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= 3 {
            return Ok(());
        }
        // 老库可能已有该列（重复执行容忍）：ALTER TABLE ADD COLUMN 失败直接忽略
        let _ = conn.execute("ALTER TABLE gateway_logs ADD COLUMN duration_ms INTEGER", []);
        let _ = conn.execute("PRAGMA user_version = 3", []);
        Ok(())
    }

    /// v4：网关请求日志增加 prompt/completion tokens 列（token 消耗统计用）
    fn migrate_v4(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= 4 {
            return Ok(());
        }
        let _ = conn.execute("ALTER TABLE gateway_logs ADD COLUMN prompt_tokens INTEGER", []);
        let _ = conn.execute("ALTER TABLE gateway_logs ADD COLUMN completion_tokens INTEGER", []);
        let _ = conn.execute("PRAGMA user_version = 4", []);
        Ok(())
    }

    /// 新增或更新账号凭据：auth 为序列化 Cred JSON，同 (provider, uid) 覆盖 name/secret/auth。
    pub fn upsert_account(&self, provider: &str, uid: &str, name: &str, secret: &str, meta: &str) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO accounts (provider, uid, name, secret, auth) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(provider, uid) DO UPDATE SET name=?3, secret=?4, auth=?5",
            rusqlite::params![provider, uid, name, secret, meta],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 全部账号的原始五元组（内部链路用；UI 展示用 list_accounts_full）。
    pub fn list_accounts(&self) -> anyhow::Result<Vec<RawAccountRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT provider, uid, name, secret, auth FROM accounts")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 记录一条签到历史（account_id = provider:uid）。
    // 列数与 history 表字段一一对应，不拆结构体
    #[allow(clippy::too_many_arguments)]
    pub fn record_history(&self, provider: &str, uid: &str, name: &str, cycle: &str, status: &str, amount: Option<f64>, message: Option<&str>, at: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO history (account_id, provider, name, cycle, status, amount, message, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![format!("{provider}:{uid}"), provider, name, cycle, status, amount, message, at],
        )?;
        Ok(())
    }

    /// 查询账号当日（cycle）已成功/已领过的历史，供签到幂等判重。
    pub fn history_today(&self, provider: &str, uid: &str, cycle: &str) -> anyhow::Result<Vec<(String, Option<f64>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT status, amount FROM history WHERE account_id=?1 AND cycle=?2 AND status IN ('success','already')")?;
        let rows = stmt.query_map(rusqlite::params![format!("{provider}:{uid}"), cycle], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 读 KV 设置；无值返回 default。
    pub fn get_setting(&self, key: &str, default: &str) -> anyhow::Result<String> {
        let conn = self.conn.lock().unwrap();
        let v: Option<String> = conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).ok();
        Ok(v.unwrap_or_else(|| default.to_string()))
    }

    /// 写 KV 设置（upsert）。
    pub fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2", [key, value])?;
        Ok(())
    }

    /// 更新账号状态列与提示消息（idle/success/already/failed/auth-expired 等，UI 徽标用）。
    pub fn update_account_status(&self, provider: &str, uid: &str, status: &str, message: Option<&str>) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET status=?3, message=?4 WHERE provider=?1 AND uid=?2",
            rusqlite::params![provider, uid, status, message],
        )?;
        Ok(())
    }

/// 删除账号（及其历史保留）。
pub fn delete_account(&self, provider: &str, uid: &str) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute("DELETE FROM accounts WHERE provider=?1 AND uid=?2", rusqlite::params![provider, uid])?;
    Ok(())
}

/// 持久化最近一次额度查询结果（JSON 字符串），启动后直接展示无需重拉。
pub fn update_account_quota(&self, provider: &str, uid: &str, quota_json: &str) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "UPDATE accounts SET quota = ?1 WHERE provider = ?2 AND uid = ?3",
        rusqlite::params![quota_json, provider, uid],
    )?;
    Ok(())
}

/// 开关自动签到（auto 列）。
pub fn set_account_auto(&self, provider: &str, uid: &str, auto: bool) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute("UPDATE accounts SET auto=?3 WHERE provider=?1 AND uid=?2", rusqlite::params![provider, uid, auto as i64])?;
    Ok(())
}

/// 更新凭据到期时间（CodeArts STS 等）
pub fn update_account_expires(&self, provider: &str, uid: &str, expires: Option<&str>) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute("UPDATE accounts SET expires=?3 WHERE provider=?1 AND uid=?2", rusqlite::params![provider, uid, expires])?;
    Ok(())
}

/// 完整账号行（UI 展示用）。
pub fn list_accounts_full(&self) -> anyhow::Result<Vec<AccountRow>> {
    let conn = self.conn.lock().unwrap();
    let mut st = conn.prepare(
        "SELECT provider, uid, name, status, message, last_checkin, expires, auto, quota FROM accounts ORDER BY provider, uid",
    )?;
    let rows = st.query_map([], |r| {
        Ok(AccountRow {
            provider: r.get(0)?,
            uid: r.get(1)?,
            name: r.get(2)?,
            status: r.get(3)?,
            message: r.get(4)?,
            last_checkin: r.get(5)?,
            expires: r.get(6)?,
            auto: r.get::<_, i64>(7)? != 0,
            quota: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 最近历史（UI 展示用）。
pub fn list_history(&self, limit: i64) -> anyhow::Result<Vec<HistoryRow>> {
    let conn = self.conn.lock().unwrap();
    let mut st = conn.prepare(
        "SELECT provider, account_id, name, cycle, status, amount, message, at FROM history ORDER BY at DESC, rowid DESC LIMIT ?",
    )?;
    let rows = st.query_map([limit], |r| {
        Ok(HistoryRow {
            provider: r.get(0)?,
            uid: r.get(1)?,
            name: r.get(2)?,
            cycle: r.get(3)?,
            status: r.get(4)?,
            amount: r.get(5)?,
            message: r.get(6)?,
            at: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

    /// 记录一条网关请求日志（ok: 成功/失败；tokens 可拆分 prompt/completion 记录）
    #[allow(clippy::too_many_arguments)]
    pub fn log_gateway_request(
        &self,
        platform: &str,
        account: &str,
        model: &str,
        ok: bool,
        status_code: Option<i64>,
        message: Option<&str>,
        tokens: Option<i64>,
    ) -> anyhow::Result<i64> {
        self.log_gateway_request_full(platform, account, model, ok, status_code, message, tokens, None, None)
    }

    /// 带输入/输出 token 拆分的完整请求日志记录（token 统计数据源）。
    /// tokens 为 None 时自动取 prompt + completion。
    #[allow(clippy::too_many_arguments)]
    pub fn log_gateway_request_full(
        &self,
        platform: &str,
        account: &str,
        model: &str,
        ok: bool,
        status_code: Option<i64>,
        message: Option<&str>,
        tokens: Option<i64>,
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
    ) -> anyhow::Result<i64> {
        let tokens = tokens.or_else(|| match (prompt_tokens, completion_tokens) {
            (Some(p), Some(c)) => Some(p + c),
            _ => None,
        });
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gateway_logs (at, platform, account, model, ok, status_code, message, tokens, prompt_tokens, completion_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                crate::adapters::now_iso(),
                platform,
                account,
                model,
                ok as i64,
                status_code,
                message,
                tokens,
                prompt_tokens,
                completion_tokens
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 按日期聚合 token 消耗（签到积分统计同源展示）：只统计成功请求。
    /// 返回 Vec<(day, prompt_sum, completion_sum, total_sum, requests)>。
    pub fn token_stats_daily(&self, limit: i64) -> anyhow::Result<Vec<DailyTokenStat>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT substr(at, 1, 10) AS day,
                    COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(COALESCE(tokens, prompt_tokens + completion_tokens)), 0),
                    COUNT(*)
             FROM gateway_logs
             WHERE ok = 1 AND (tokens IS NOT NULL OR prompt_tokens IS NOT NULL OR completion_tokens IS NOT NULL)
             GROUP BY day ORDER BY day DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 流式请求结束回填 token 用量（Usage 事件在流中到达，Drop 时统一写入）。
    pub fn update_gateway_log_tokens(&self, id: i64, prompt_tokens: Option<i64>, completion_tokens: Option<i64>) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateway_logs SET prompt_tokens = ?2, completion_tokens = ?3, tokens = COALESCE(?2, 0) + COALESCE(?3, 0) WHERE id = ?1",
            rusqlite::params![id, prompt_tokens, completion_tokens],
        )?;
        Ok(())
    }

    /// 请求结束时回填耗时（毫秒）/结果（流式请求先记开始行，结束或中断时更新）。
    /// ok=None 表示只更新耗时。
    pub fn finish_gateway_log(
        &self,
        id: i64,
        duration_ms: i64,
        ok: Option<bool>,
        status_code: Option<i64>,
        message: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        match ok {
            Some(okv) => conn.execute(
                "UPDATE gateway_logs SET duration_ms = ?2, ok = ?3, status_code = ?4, message = ?5 WHERE id = ?1",
                rusqlite::params![id, duration_ms, okv as i64, status_code, message],
            )?,
            None => conn.execute(
                "UPDATE gateway_logs SET duration_ms = ?2 WHERE id = ?1",
                rusqlite::params![id, duration_ms],
            )?,
        };
        Ok(())
    }

    /// 最近网关请求日志（UI 展示用）
    pub fn list_gateway_logs(&self, limit: i64) -> anyhow::Result<Vec<GatewayLogRow>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, at, platform, account, model, ok, status_code, message, tokens, duration_ms FROM gateway_logs ORDER BY at DESC, id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit], |r| {
            Ok(GatewayLogRow {
                id: r.get(0)?,
                at: r.get(1)?,
                platform: r.get(2)?,
                account: r.get(3)?,
                model: r.get(4)?,
                ok: r.get::<_, i64>(5)? != 0,
                status_code: r.get(6)?,
                message: r.get(7)?,
                tokens: r.get(8)?,
                duration_ms: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 清空全部网关请求日志
    pub fn clear_gateway_logs(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM gateway_logs", [])?;
        Ok(n as i64)
    }

    /// 日志统计：总请求数 / 成功 / 失败
    pub fn gateway_log_stats(&self) -> anyhow::Result<(i64, i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let (total, ok): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(ok), 0) FROM gateway_logs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((total, ok, total - ok))
    }
}

/// 网关请求日志行（UI 展示用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayLogRow {
    pub id: i64,
    pub at: String,
    pub platform: String,
    pub account: String,
    pub model: String,
    pub ok: bool,
    pub status_code: Option<i64>,
    pub message: Option<String>,
    pub tokens: Option<i64>,
    /// 请求耗时（毫秒）；NULL = 尚未结束（流式中）
    pub duration_ms: Option<i64>,
}

/// 账号全行（UI 展示用：状态/消息/到期/自动开关/额度快照）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountRow {
    pub provider: String,
    pub uid: String,
    pub name: String,
    pub status: String,
    pub message: Option<String>,
    pub last_checkin: Option<String>,
    pub expires: Option<String>,
    pub auto: bool,
    pub quota: Option<String>,
}

/// 签到历史行（UI 展示用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryRow {
    pub provider: String,
    pub uid: String,
    pub name: String,
    pub cycle: String,
    pub status: String,
    pub amount: Option<f64>,
    pub message: Option<String>,
    pub at: String,
}
