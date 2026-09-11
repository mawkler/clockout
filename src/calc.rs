use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};

use crate::model::{Config, TimeEntry};

/// Minutes worked by this entry. An open entry (no end yet) contributes 0 -
/// its duration isn't known until `time end` closes it.
pub fn entry_minutes(e: &TimeEntry) -> i64 {
    match e.end {
        Some(end) => (end - e.start).num_minutes(),
        None => 0,
    }
}

/// The first entry with no end time (a block started via `time start` that
/// hasn't been closed with `time end` yet). At most one is expected to exist.
pub fn open_entry(entries: &[TimeEntry]) -> Option<&TimeEntry> {
    entries.iter().find(|e| e.end.is_none())
}

/// Formats an entry's end for display: "HH:MM", or "…" while still open.
pub fn format_end(end: Option<NaiveTime>) -> String {
    match end {
        Some(t) => t.format("%H:%M").to_string(),
        None => "…".to_string(),
    }
}

pub fn weekly_goal_minutes(cfg: &Config) -> i64 {
    (cfg.weekly_goal_hours * 60.0).round() as i64
}

pub fn daily_target_minutes(cfg: &Config, day: NaiveDate) -> i64 {
    match day.weekday() {
        Weekday::Sat | Weekday::Sun => 0,
        _ => (weekly_goal_minutes(cfg) as f64 / 5.0).round() as i64,
    }
}

/// Returns the (Monday, Sunday) pair bounding the week containing `day`.
pub fn week_bounds(day: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start = day - Duration::days(day.weekday().num_days_from_monday() as i64);
    (start, start + Duration::days(6))
}

pub fn entries_on(entries: &[TimeEntry], day: NaiveDate) -> Vec<&TimeEntry> {
    entries.iter().filter(|e| e.date == day).collect()
}

pub fn entries_in_week(entries: &[TimeEntry], week_start: NaiveDate) -> Vec<&TimeEntry> {
    let week_end = week_start + Duration::days(6);
    entries
        .iter()
        .filter(|e| e.date >= week_start && e.date <= week_end)
        .collect()
}

pub fn worked_minutes(entries: &[&TimeEntry]) -> i64 {
    entries.iter().map(|e| entry_minutes(e)).sum()
}

pub fn minutes_left_today(entries: &[TimeEntry], cfg: &Config, today: NaiveDate) -> i64 {
    let worked = worked_minutes(&entries_on(entries, today));
    (daily_target_minutes(cfg, today) - worked).max(0)
}

pub fn minutes_left_week(entries: &[TimeEntry], cfg: &Config, today: NaiveDate) -> i64 {
    let (week_start, _) = week_bounds(today);
    let worked = worked_minutes(&entries_in_week(entries, week_start));
    (weekly_goal_minutes(cfg) - worked).max(0)
}

/// Accumulated shortfall from days that have already fully elapsed this week
/// (i.e. before `today`), assuming each day was expected to hit its own
/// daily target. A day worked past its target offsets shortfall from another
/// day net; the total is clamped to 0 (never negative - being ahead isn't
/// "negative debt"). Today itself is excluded: its own remaining time is
/// already covered by `minutes_left_today` / `leave_time`.
pub fn time_debt_minutes(entries: &[TimeEntry], cfg: &Config, today: NaiveDate) -> i64 {
    let (week_start, _) = week_bounds(today);
    let mut debt = 0;
    let mut day = week_start;
    while day < today {
        let worked = worked_minutes(&entries_on(entries, day));
        debt += daily_target_minutes(cfg, day) - worked;
        day += Duration::days(1);
    }
    debt.max(0)
}

/// Gaps between consecutive logged entries for `day`, sorted by start time.
/// Purely descriptive - worked-time math never needs this, since minutes not
/// covered by any logged range are simply absent from the sum already.
pub fn today_gaps(entries: &[TimeEntry], day: NaiveDate) -> Vec<(NaiveTime, NaiveTime)> {
    let mut day_entries = entries_on(entries, day);
    day_entries.sort_by_key(|e| e.start);
    let mut gaps = Vec::new();
    for w in day_entries.windows(2) {
        // An open entry (w[0].end == None) has no end to gap from yet.
        if let Some(end0) = w[0].end.filter(|&end0| w[1].start > end0) {
            gaps.push((end0, w[1].start));
        }
    }
    gaps
}

/// Projected clock time the user can leave today, assuming no further breaks
/// beyond what's already implied by logged entries. If today has no logged
/// gap yet, `future_break_minutes` from config is added as an anticipated
/// break; once a real gap is logged, it supersedes the guess.
pub fn leave_time(
    entries: &[TimeEntry],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    leave_time_after(
        minutes_left_today(entries, cfg, today),
        entries,
        cfg,
        today,
        now,
    )
}

/// Leave time if today should also pay off the rest of the week's shortfall
/// (any prior days under target), not just today's own even split.
pub fn leave_time_week(
    entries: &[TimeEntry],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    leave_time_after(
        minutes_left_week(entries, cfg, today),
        entries,
        cfg,
        today,
        now,
    )
}

fn leave_time_after(
    minutes_owed: i64,
    entries: &[TimeEntry],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    let projected_break = if today_gaps(entries, today).is_empty() {
        cfg.future_break_minutes as i64
    } else {
        0
    };
    now + Duration::minutes(minutes_owed + projected_break)
}

/// True if two entries on the same day overlap. Adjacency (a.end == b.start)
/// does not count as overlap. An open entry (no end yet) is treated as
/// running through the end of the day for this check, since anything is
/// still in conflict with it until it's closed.
pub fn overlaps(a: &TimeEntry, b: &TimeEntry) -> bool {
    let end_of_day = NaiveTime::from_hms_opt(23, 59, 59).unwrap();
    let a_end = a.end.unwrap_or(end_of_day);
    let b_end = b.end.unwrap_or(end_of_day);
    a.start < b_end && b.start < a_end
}

/// Formats a minute count as "Xh" when there are no leftover minutes, or
/// "XhYYm" otherwise, e.g. 240 -> "4h", 270 -> "4h30m", 30 -> "30m".
pub fn format_hm(minutes: i64) -> String {
    let sign = if minutes < 0 { "-" } else { "" };
    let abs = minutes.abs();
    let h = abs / 60;
    let m = abs % 60;
    match (h, m) {
        (0, 0) => format!("{sign}0h"),
        (_, 0) => format!("{sign}{h}h"),
        (0, _) => format!("{sign}{m}m"),
        _ => format!("{sign}{h}h{m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn entry(id: u64, date: NaiveDate, start: &str, end: &str) -> TimeEntry {
        TimeEntry {
            id,
            date,
            start: NaiveTime::parse_from_str(start, "%H:%M").unwrap(),
            end: Some(NaiveTime::parse_from_str(end, "%H:%M").unwrap()),
        }
    }

    fn open(id: u64, date: NaiveDate, start: &str) -> TimeEntry {
        TimeEntry {
            id,
            date,
            start: NaiveTime::parse_from_str(start, "%H:%M").unwrap(),
            end: None,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn cfg(goal: f64, future_break: u32) -> Config {
        Config {
            weekly_goal_hours: goal,
            future_break_minutes: future_break,
        }
    }

    #[test]
    fn worked_minutes_sums_entries() {
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00"), entry(2, d, "13:00", "15:15")];
        let refs: Vec<&TimeEntry> = entries.iter().collect();
        assert_eq!(worked_minutes(&refs), 4 * 60 + 2 * 60 + 15);
    }

    #[test]
    fn daily_target_weekday_vs_weekend() {
        let c = cfg(40.0, 30);
        // 2026-09-11 is a Friday
        assert_eq!(daily_target_minutes(&c, date(2026, 9, 11)), 480);
        // 2026-09-12 is a Saturday
        assert_eq!(daily_target_minutes(&c, date(2026, 9, 12)), 0);
        // 2026-09-13 is a Sunday
        assert_eq!(daily_target_minutes(&c, date(2026, 9, 13)), 0);
    }

    #[test]
    fn daily_target_rounds_uneven_goal() {
        let c = cfg(37.5, 30);
        // 37.5h * 60 = 2250 minutes / 5 = 450 minutes/day
        assert_eq!(daily_target_minutes(&c, date(2026, 9, 11)), 450);
    }

    #[test]
    fn minutes_left_today_clamps_to_zero() {
        let c = cfg(40.0, 30);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "18:00")]; // 10h worked, target 8h
        assert_eq!(minutes_left_today(&entries, &c, d), 0);
    }

    #[test]
    fn minutes_left_week_sums_and_clamps() {
        let c = cfg(40.0, 30);
        let monday = date(2026, 9, 7);
        let entries = vec![
            entry(1, monday, "08:00", "16:00"),            // 8h
            entry(2, date(2026, 9, 8), "08:00", "16:00"),  // 8h, Tue
            entry(3, date(2026, 9, 14), "08:00", "16:00"), // next week, excluded
        ];
        assert_eq!(minutes_left_week(&entries, &c, monday), 40 * 60 - 16 * 60);
    }

    #[test]
    fn minutes_left_week_clamps_to_zero() {
        let c = cfg(10.0, 30);
        let monday = date(2026, 9, 7);
        let entries = vec![entry(1, monday, "08:00", "18:00")]; // 10h worked, goal 10h
        assert_eq!(minutes_left_week(&entries, &c, monday), 0);
    }

    #[test]
    fn week_bounds_various_days() {
        let monday = date(2026, 9, 7);
        let sunday = date(2026, 9, 13);
        let midweek = date(2026, 9, 10);
        assert_eq!(week_bounds(monday), (monday, sunday));
        assert_eq!(week_bounds(sunday), (monday, sunday));
        assert_eq!(week_bounds(midweek), (monday, sunday));
    }

    #[test]
    fn time_debt_zero_on_first_day_of_week() {
        let c = cfg(40.0, 30);
        let monday = date(2026, 9, 7);
        assert_eq!(time_debt_minutes(&[], &c, monday), 0);
    }

    #[test]
    fn time_debt_sums_elapsed_shortfalls() {
        let c = cfg(40.0, 30); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:30"), // Mon 5h30m, deficit 2h30m
            entry(2, date(2026, 9, 8), "08:00", "14:20"), // Tue 6h20m, deficit 1h40m
            entry(3, date(2026, 9, 9), "08:00", "14:45"), // Wed 6h45m, deficit 1h15m
            entry(4, date(2026, 9, 10), "08:00", "13:10"), // Thu 5h10m, deficit 2h50m
        ];
        let friday = date(2026, 9, 11);
        // Only Mon-Thu have elapsed; Friday itself is excluded.
        assert_eq!(
            time_debt_minutes(&entries, &c, friday),
            150 + 100 + 75 + 170
        );
    }

    #[test]
    fn time_debt_nets_surplus_against_shortfall() {
        let c = cfg(40.0, 30);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "16:00"), // Mon 8h, surplus 0
            entry(2, date(2026, 9, 8), "08:00", "13:00"), // Tue 5h, deficit 3h
        ];
        let wednesday = date(2026, 9, 9);
        assert_eq!(time_debt_minutes(&entries, &c, wednesday), 180);
    }

    #[test]
    fn time_debt_clamps_to_zero_when_net_ahead() {
        let c = cfg(40.0, 30);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "19:40"), // Mon 11h40m, surplus 3h40m
            entry(2, date(2026, 9, 8), "08:00", "14:40"), // Tue 6h40m, deficit 1h20m
        ];
        let wednesday = date(2026, 9, 9);
        assert_eq!(time_debt_minutes(&entries, &c, wednesday), 0);
    }

    #[test]
    fn time_debt_weekend_work_offsets_weekday_shortfall() {
        let c = cfg(40.0, 30);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "14:20"), // Mon 6h20m, deficit 1h40m
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
            entry(3, date(2026, 9, 9), "08:00", "16:00"), // Wed 8h, on target
            entry(4, date(2026, 9, 10), "08:00", "16:00"), // Thu 8h, on target
            entry(5, date(2026, 9, 11), "08:00", "16:00"), // Fri 8h, on target
            entry(6, date(2026, 9, 12), "09:00", "10:00"), // Sat 1h (target 0), offsets debt
        ];
        // Sunday: the whole Mon-Sat week has elapsed.
        let sunday = date(2026, 9, 13);
        assert_eq!(time_debt_minutes(&entries, &c, sunday), 100 - 60);
    }

    #[test]
    fn today_gaps_various_cases() {
        let d = date(2026, 9, 11);
        assert_eq!(today_gaps(&[], d), vec![]);

        let one = vec![entry(1, d, "08:00", "12:00")];
        assert_eq!(today_gaps(&one, d), vec![]);

        let contiguous = vec![entry(1, d, "08:00", "12:00"), entry(2, d, "12:00", "16:00")];
        assert_eq!(today_gaps(&contiguous, d), vec![]);

        let with_gap = vec![entry(1, d, "08:00", "12:00"), entry(2, d, "13:00", "15:15")];
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        assert_eq!(today_gaps(&with_gap, d), vec![(t("12:00"), t("13:00"))]);

        // unsorted input still produces a correctly ordered gap
        let unsorted = vec![entry(2, d, "13:00", "15:15"), entry(1, d, "08:00", "12:00")];
        assert_eq!(today_gaps(&unsorted, d), vec![(t("12:00"), t("13:00"))]);
    }

    #[test]
    fn format_hm_omits_zero_minutes() {
        assert_eq!(format_hm(240), "4h");
        assert_eq!(format_hm(270), "4h30m");
        assert_eq!(format_hm(30), "30m");
        assert_eq!(format_hm(0), "0h");
        assert_eq!(format_hm(-90), "-1h30m");
    }

    #[test]
    fn leave_time_no_gap_applies_default_padding() {
        let c = cfg(40.0, 30);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00")]; // 4h worked, 4h left
        let now = NaiveTime::parse_from_str("12:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("16:30", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_logged_gap_skips_padding() {
        let c = cfg(40.0, 30);
        let d = date(2026, 9, 11);
        let entries = vec![
            entry(1, d, "08:00", "12:00"),
            entry(2, d, "13:00", "15:00"), // 6h worked total, 2h left, real gap logged
        ];
        let now = NaiveTime::parse_from_str("15:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("17:00", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_zero_future_break_no_padding() {
        let c = cfg(40.0, 0);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00")]; // 4h worked, 4h left
        let now = NaiveTime::parse_from_str("12:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("16:00", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_week_pays_off_prior_shortfall() {
        let c = cfg(40.0, 0);
        // Monday-Thursday only 4h/day logged (target 8h/day => 16h shortfall).
        let monday = date(2026, 9, 7);
        let mut entries = vec![
            entry(1, monday, "08:00", "12:00"),
            entry(2, date(2026, 9, 8), "08:00", "12:00"),
            entry(3, date(2026, 9, 9), "08:00", "12:00"),
            entry(4, date(2026, 9, 10), "08:00", "12:00"),
        ];
        let friday = date(2026, 9, 11);
        entries.push(entry(5, friday, "08:00", "12:00")); // 4h today too
        let now = NaiveTime::parse_from_str("12:00", "%H:%M").unwrap();

        // Today-only target (8h/day): 4h left.
        assert_eq!(
            leave_time(&entries, &c, friday, now),
            NaiveTime::parse_from_str("16:00", "%H:%M").unwrap()
        );
        // Full week (40h goal, 20h worked so far): 20h left, wraps past midnight.
        assert_eq!(
            leave_time_week(&entries, &c, friday, now),
            NaiveTime::parse_from_str("08:00", "%H:%M").unwrap()
        );
    }

    #[test]
    fn leave_time_week_matches_today_when_week_goal_already_met() {
        let c = cfg(10.0, 0);
        let monday = date(2026, 9, 7);
        // Week goal (10h) already met on Monday alone; nothing owed today.
        let entries = vec![entry(1, monday, "08:00", "18:00")];
        let friday = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("09:00", "%H:%M").unwrap();
        assert_eq!(leave_time_week(&entries, &c, friday, now), now);
    }

    #[test]
    fn overlaps_cases() {
        let d = date(2026, 9, 11);
        let a = entry(1, d, "08:00", "12:00");
        let b = entry(2, d, "11:00", "13:00");
        assert!(overlaps(&a, &b));

        let c = entry(3, d, "12:00", "16:00");
        assert!(!overlaps(&a, &c)); // adjacency is not overlap

        let e = entry(4, d, "13:00", "15:00");
        assert!(!overlaps(&a, &e));
    }

    #[test]
    fn overlaps_with_open_entry() {
        let d = date(2026, 9, 11);
        let o = open(1, d, "12:35");
        // Anything starting at or after the open entry's start conflicts...
        let later = entry(2, d, "13:00", "14:00");
        assert!(overlaps(&o, &later));
        // ...but something fully before it doesn't.
        let earlier = entry(3, d, "08:00", "12:00");
        assert!(!overlaps(&o, &earlier));
    }

    #[test]
    fn entry_minutes_open_entry_is_zero() {
        let d = date(2026, 9, 11);
        assert_eq!(entry_minutes(&open(1, d, "12:35")), 0);
    }

    #[test]
    fn open_entry_finds_the_unfinished_block() {
        let d = date(2026, 9, 11);
        assert_eq!(open_entry(&[]), None);
        let entries = vec![entry(1, d, "08:00", "12:00"), open(2, d, "12:35")];
        assert_eq!(open_entry(&entries).map(|e| e.id), Some(2));
    }

    #[test]
    fn worked_minutes_ignores_open_entry() {
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00"), open(2, d, "12:35")];
        assert_eq!(worked_minutes(&entries.iter().collect::<Vec<_>>()), 4 * 60);
    }

    #[test]
    fn today_gaps_open_entry_as_right_side_still_gaps() {
        let d = date(2026, 9, 11);
        // Closed entry, then an open one - the gap between them (the break
        // taken before resuming) is still real, reportable info.
        let entries = vec![entry(1, d, "08:00", "12:00"), open(2, d, "13:00")];
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        assert_eq!(today_gaps(&entries, d), vec![(t("12:00"), t("13:00"))]);
    }

    #[test]
    fn today_gaps_skips_open_entry_as_left_side() {
        let d = date(2026, 9, 11);
        // An open entry has no end yet, so nothing after it can be reported
        // as a gap from it.
        let entries = vec![open(1, d, "08:00"), entry(2, d, "13:00", "14:00")];
        assert_eq!(today_gaps(&entries, d), vec![]);
    }

    #[test]
    fn format_end_some_and_none() {
        let t = NaiveTime::parse_from_str("12:35", "%H:%M").unwrap();
        assert_eq!(format_end(Some(t)), "12:35");
        assert_eq!(format_end(None), "…");
    }
}
