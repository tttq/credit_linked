//! 调度：UTC+8 日界 cycle + 随机窗口（randomSchedule.v1 语义对齐）。
//! 公式必须同式：minute = start + floor(seed / 2^32 * (end - start + 1))。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::adapters::day_cn;

/// 随机种子表：day → u32，当日固定保证计划可复现（只保留最近 4 天）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Seeds(pub BTreeMap<String, u32>);

/// 随机签到时间计划器（randomSchedule.v1 语义）。
pub struct RandomSchedule;

impl RandomSchedule {
    /// settings: time "HH:MM"、random_time、random_start、random_end
    /// 返回当日计划时间 "HH:MM"
    pub fn plan(seeds: &mut Seeds, time: &str, random_time: bool, start: &str, end: &str) -> anyhow::Result<String> {
        if !random_time {
            return Ok(time.to_string());
        }
        let (s_min, e_min) = (parse_minute(start)?, parse_minute(end)?);
        if s_min >= e_min {
            anyhow::bail!("随机签到时段无效");
        }
        let day = day_cn();
        let seed = *seeds.0.entry(day.clone()).or_insert_with(rand_seed);
        // 保留最近 4 天
        while seeds.0.len() > 4 {
            let first = seeds.0.keys().next().cloned().unwrap();
            seeds.0.remove(&first);
        }
        let span = e_min - s_min + 1;
        let selected = s_min + ((seed as u64) * span as u64 / (u32::MAX as u64 + 1)) as i64;
        Ok(format!("{:02}:{:02}", selected / 60, selected % 60))
    }
}

fn parse_minute(t: &str) -> anyhow::Result<i64> {
    let (h, m) = t.split_once(':').ok_or_else(|| anyhow::anyhow!("bad time {t}"))?;
    Ok(h.parse::<i64>()? * 60 + m.parse::<i64>()?)
}

/// 生成新的 32 位随机种子（OS 熵源）。
pub fn rand_seed() -> u32 {
    let mut b = [0u8; 4];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    u32::from_be_bytes(b)
}

/// 下次执行时间 ISO（当天计划点已过则顺延一天）
pub fn next_run(time: &str) -> String {
    let day = day_cn();
    let at = format!("{day}T{time}:00+08:00");
    let now = crate::adapters::now_iso();
    if at.as_str() > now.as_str() { at } else {
        // +1 天
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 + 8 * 3600 + 86400;
        let (y, mo, d) = crate::adapters::civil_from_days(secs / 86400);
        format!("{y:04}-{mo:02}-{d:02}T{time}:00+08:00")
    }
}
