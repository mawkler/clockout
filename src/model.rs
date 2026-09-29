use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: u64,
    pub date: NaiveDate,
    #[serde(with = "hhmm_time")]
    pub start: NaiveTime,
    /// None means the block was started but not yet finished (`clockout
    /// start` without a matching `clockout end`).
    #[serde(
        with = "hhmm_time_opt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub end: Option<NaiveTime>,
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

fn default_estimated_lunch_break() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub weekly_goal_hours: f64,
    #[serde(default = "default_estimated_lunch_break")]
    pub estimated_lunch_break_minutes: u32,
    /// If true, Saturday and Sunday are left out of the week diagram, day
    /// navigation, and the history view entirely - not just zeroed targets
    /// (which they already have regardless of this flag).
    #[serde(default)]
    pub hide_weekends: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            weekly_goal_hours: 40.0,
            estimated_lunch_break_minutes: default_estimated_lunch_break(),
            hide_weekends: false,
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

mod hhmm_time_opt {
    use chrono::NaiveTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(time: &Option<NaiveTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match time {
            Some(t) => serializer.serialize_str(&t.format("%H:%M").to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(deserializer)?;
        opt.map(|s| NaiveTime::parse_from_str(&s, "%H:%M").map_err(serde::de::Error::custom))
            .transpose()
    }
}
