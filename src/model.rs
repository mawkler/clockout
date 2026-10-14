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

/// One era of the weekly-hours goal: `weekly_goal_hours` applies from
/// `effective_from` (inclusive) until the next period's `effective_from`,
/// or indefinitely if it's the latest one. See `calc::goal_hours_for`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalPeriod {
    pub id: u64,
    pub effective_from: NaiveDate,
    pub weekly_goal_hours: f64,
}

/// A day (or part of one) you were absent (sick, vacation, etc.) and so
/// don't owe a target for. If `start`/`end` are set, only that time range
/// on `date` is absent (e.g. a doctor's appointment) and the day's target
/// is reduced by that duration instead of zeroed - see
/// `calc::daily_target_minutes`/`calc::absence_blocks`. Both `None` means
/// the whole day is absent, which is also the default when adding one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Absence {
    pub id: u64,
    pub date: NaiveDate,
    #[serde(
        with = "hhmm_time_opt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub start: Option<NaiveTime>,
    #[serde(
        with = "hhmm_time_opt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub end: Option<NaiveTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntriesFile {
    pub version: u32,
    pub next_id: u64,
    pub entries: Vec<TimeEntry>,
    #[serde(default)]
    pub goal_periods: Vec<GoalPeriod>,
    #[serde(default)]
    pub absences: Vec<Absence>,
}

impl Default for EntriesFile {
    fn default() -> Self {
        EntriesFile {
            version: 1,
            next_id: 2, // id 1 is used by the seeded period below
            entries: Vec::new(),
            goal_periods: vec![GoalPeriod {
                id: 1,
                // Placeholder - irrelevant which date, since the earliest
                // period always applies retroactively (see
                // calc::goal_hours_for).
                effective_from: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
                weekly_goal_hours: 40.0,
            }],
            absences: Vec::new(),
        }
    }
}

fn default_estimated_lunch_break() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
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
