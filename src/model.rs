use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: u64,
    pub date: NaiveDate,
    #[serde(with = "hhmm_time")]
    pub start: NaiveTime,
    #[serde(with = "hhmm_time")]
    pub end: NaiveTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntriesFile {
    pub version: u32,
    pub next_id: u64,
    pub entries: Vec<TimeEntry>,
}

impl Default for EntriesFile {
    fn default() -> Self {
        EntriesFile {
            version: 1,
            next_id: 1,
            entries: Vec::new(),
        }
    }
}

fn default_future_break() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub weekly_goal_hours: f64,
    #[serde(default = "default_future_break")]
    pub future_break_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            weekly_goal_hours: 40.0,
            future_break_minutes: default_future_break(),
        }
    }
}

mod hhmm_time {
    use chrono::NaiveTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(time: &NaiveTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&time.format("%H:%M").to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        NaiveTime::parse_from_str(&s, "%H:%M").map_err(serde::de::Error::custom)
    }
}
