//! SQLite 存储（对齐原版 schema）：accounts/history/settings；WAL；BEGIN IMMEDIATE。

use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
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
        Ok(Self { conn: Mutex::new(conn) })
    }

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

    pub fn list_accounts(&self) -> anyhow::Result<Vec<(String, String, String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT provider, uid, name, secret, auth FROM accounts")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn record_history(&self, provider: &str, uid: &str, name: &str, cycle: &str, status: &str, amount: Option<f64>, message: Option<&str>, at: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO history (account_id, provider, name, cycle, status, amount, message, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![format!("{provider}:{uid}"), provider, name, cycle, status, amount, message, at],
        )?;
        Ok(())
    }

    pub fn history_today(&self, provider: &str, uid: &str, cycle: &str) -> anyhow::Result<Vec<(String, Option<f64>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT status, amount FROM history WHERE account_id=?1 AND cycle=?2 AND status IN ('success','already')")?;
        let rows = stmt.query_map(rusqlite::params![format!("{provider}:{uid}"), cycle], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_setting(&self, key: &str, default: &str) -> anyhow::Result<String> {
        let conn = self.conn.lock().unwrap();
        let v: Option<String> = conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).ok();
        Ok(v.unwrap_or_else(|| default.to_string()))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2", [key, value])?;
        Ok(())
    }

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

/// 开关自动签到。
/// 持久化最近一次额度查询结果（JSON 字符串）。
pub fn update_account_quota(&self, provider: &str, uid: &str, quota_json: &str) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "UPDATE accounts SET quota = ?1 WHERE provider = ?2 AND uid = ?3",
        rusqlite::params![quota_json, provider, uid],
    )?;
    Ok(())
}

pub fn set_account_auto(&self, provider: &str, uid: &str, auto: bool) -> anyhow::Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute("UPDATE accounts SET auto=?3 WHERE provider=?1 AND uid=?2", rusqlite::params![provider, uid, auto as i64])?;
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

    /// 记录一条网关请求日志（ok: 成功/失败）
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
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gateway_logs (at, platform, account, model, ok, status_code, message, tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                crate::adapters::now_iso(),
                platform,
                account,
                model,
                ok as i64,
                status_code,
                message,
                tokens
            ],
        )?;
        Ok(())
    }

    /// 最近网关请求日志（UI 展示用）
    pub fn list_gateway_logs(&self, limit: i64) -> anyhow::Result<Vec<GatewayLogRow>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, at, platform, account, model, ok, status_code, message, tokens FROM gateway_logs ORDER BY at DESC, id DESC LIMIT ?1",
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
}

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
