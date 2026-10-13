use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};

use crate::model::{Absence, Config, GoalPeriod, TimeEntry};

/// Minutes worked by this entry as of `now`. A closed entry (it has a
/// stored end) always counts in full, whatever that end is - even one
/// later than `now` - since closing a block is an explicit declaration
/// that the work happened, taken at face value. Only a still-open entry
/// (no end yet) accrues live and never gets credit for time that hasn't
/// happened: it's capped at `now` if it's today's entry, or through the
/// implicit end of day if it was left open on a past day (since `now`
/// can't meaningfully bound a day that's already over). Never negative -
/// a block that hasn't started yet as of its effective end contributes 0.
pub fn entry_minutes(e: &TimeEntry, today: NaiveDate, now: NaiveTime) -> i64 {
    let end_of_day = NaiveTime::from_hms_opt(23, 59, 59).unwrap();
    let effective_end = match e.end {
        Some(end) => end,
        None if e.date == today => now,
        None => end_of_day,
    };
    (effective_end - e.start).num_minutes().max(0)
}

/// The first entry with no end time (a block started via `clockout start` that
/// hasn't been closed with `clockout end` yet). At most one is expected to exist.
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

/// The `goal_periods` entry that currently applies to `day`: the one with
/// the latest `effective_from` at or before `day`. `None` only if
/// `goal_periods` is empty. Exposed (not just `goal_hours_for`) so
/// callers can also mark/highlight which period is "current" in a list,
/// not just read its number. If two periods share the same
/// `effective_from`, the one declared later in the file wins
/// (`Iterator::max_by_key`'s documented last-on-tie behavior) - the CLI
/// and TUI both reject creating that ambiguity going forward (see
/// `goal_period_date_conflicts`), so this is just a defensive fallback
/// for a hand-edited file.
pub fn current_goal_period(goal_periods: &[GoalPeriod], day: NaiveDate) -> Option<&GoalPeriod> {
    goal_periods
        .iter()
        .filter(|p| p.effective_from <= day)
        .max_by_key(|p| p.effective_from)
}

/// The weekly-goal-hours in effect for `day` - see `current_goal_period`.
/// Falls back to a hardcoded 40.0 only if `goal_periods` is empty (an
/// entries.json predating this feature, or a user hand-deleting every
/// period).
fn goal_hours_for(goal_periods: &[GoalPeriod], day: NaiveDate) -> f64 {
    current_goal_period(goal_periods, day)
        .map(|p| p.weekly_goal_hours)
        .unwrap_or(40.0)
}

/// True if `day` has a recorded absence.
pub fn is_absent(absences: &[Absence], day: NaiveDate) -> bool {
    absences.iter().any(|a| a.date == day)
}

/// The absence recorded for `day`, if any.
pub fn absence_on(absences: &[Absence], day: NaiveDate) -> Option<&Absence> {
    absences.iter().find(|a| a.date == day)
}

/// True if `candidate_date` already has a recorded absence (excluding
/// `excluding_id`, so editing one in place doesn't trip over itself).
/// Mirrors `goal_period_date_conflicts`.
pub fn absence_date_conflicts(
    absences: &[Absence],
    candidate_date: NaiveDate,
    excluding_id: Option<u64>,
) -> bool {
    absences
        .iter()
        .any(|a| Some(a.id) != excluding_id && a.date == candidate_date)
}

pub fn daily_target_minutes(
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    day: NaiveDate,
) -> i64 {
    if is_absent(absences, day) {
        return 0;
    }
    match day.weekday() {
        Weekday::Sat | Weekday::Sun => 0,
        _ => {
            let goal_minutes = (goal_hours_for(goal_periods, day) * 60.0).round() as i64;
            (goal_minutes as f64 / 5.0).round() as i64
        }
    }
}

/// Sum of `week_start`'s week's daily targets - correctly reflects a goal
/// change landing mid-week, not just one aligned to a Monday.
pub fn weekly_goal_minutes_for_week(
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    week_start: NaiveDate,
) -> i64 {
    (0..7)
        .map(|i| daily_target_minutes(goal_periods, absences, week_start + Duration::days(i)))
        .sum()
}

/// True if `candidate_date` collides with another period's
/// `effective_from` (excluding `excluding_id`, so editing a period in
/// place doesn't trip over itself). Shared by the CLI's `goal set`/`edit`
/// and the TUI's add/edit form, so both reject the same ambiguity.
pub fn goal_period_date_conflicts(
    goal_periods: &[GoalPeriod],
    candidate_date: NaiveDate,
    excluding_id: Option<u64>,
) -> bool {
    goal_periods
        .iter()
        .any(|p| Some(p.id) != excluding_id && p.effective_from == candidate_date)
}

/// Returns the (Monday, Sunday) pair bounding the week containing `day`.
pub fn week_bounds(day: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start = day - Duration::days(day.weekday().num_days_from_monday() as i64);
    (start, start + Duration::days(6))
}

pub fn entries_on(entries: &[TimeEntry], day: NaiveDate) -> Vec<&TimeEntry> {
    entries.iter().filter(|e| e.date == day).collect()
}

/// If you're done for the day - nothing currently open, and at least one
/// closed entry logged for `today` - the clock time the last one ended.
/// `None` while still actively working (an open block, on any day) or
/// before you've logged anything today at all, so callers can show this
/// instead of a "Leave at" projection only once there's actually
/// something to report as "clocked out at".
pub fn clocked_out_at(entries: &[TimeEntry], today: NaiveDate) -> Option<NaiveTime> {
    if open_entry(entries).is_some() {
        return None;
    }
    entries_on(entries, today)
        .into_iter()
        .filter_map(|e| e.end)
        .max()
}

pub fn entries_in_week(entries: &[TimeEntry], week_start: NaiveDate) -> Vec<&TimeEntry> {
    let week_end = week_start + Duration::days(6);
    entries
        .iter()
        .filter(|e| e.date >= week_start && e.date <= week_end)
        .collect()
}

pub fn worked_minutes(entries: &[&TimeEntry], today: NaiveDate, now: NaiveTime) -> i64 {
    entries.iter().map(|e| entry_minutes(e, today, now)).sum()
}

/// One row of the history view: a week's Monday, its Mon-Sun daily totals
/// (minutes worked that day, live-capped same as everywhere else), and the
/// week's total against the weekly goal.
pub struct WeekRow {
    pub week_start: NaiveDate,
    pub day_totals: Vec<(NaiveDate, i64)>,
    pub worked_week: i64,
    pub goal_week: i64,
}

/// One row per week from the earliest logged entry's week through the
/// current week, oldest first. Which days to actually display (e.g.
/// dropping weekends) is a rendering concern left to the caller - this
/// always computes all 7 days, so nothing here needs to know about
/// `Config::hide_weekends`.
pub fn history_weeks(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> Vec<WeekRow> {
    let earliest = entries.iter().map(|e| e.date).min().unwrap_or(today);
    let (first_week_start, _) = week_bounds(earliest);
    let (current_week_start, _) = week_bounds(today);

    let mut rows = Vec::new();
    let mut week_start = first_week_start;
    while week_start <= current_week_start {
        let day_totals: Vec<(NaiveDate, i64)> = (0..7)
            .map(|i| {
                let day = week_start + Duration::days(i);
                let worked = worked_minutes(&entries_on(entries, day), today, now);
                (day, worked)
            })
            .collect();
        let worked_week = day_totals.iter().map(|(_, m)| m).sum();
        let goal_week = weekly_goal_minutes_for_week(goal_periods, absences, week_start);
        rows.push(WeekRow {
            week_start,
            day_totals,
            worked_week,
            goal_week,
        });
        week_start += Duration::weeks(1);
    }
    rows
}

pub fn minutes_left_today(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> i64 {
    let worked = worked_minutes(&entries_on(entries, today), today, now);
    (daily_target_minutes(goal_periods, absences, today) - worked).max(0)
}

pub fn minutes_left_week(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> i64 {
    let (week_start, _) = week_bounds(today);
    let worked = worked_minutes(&entries_in_week(entries, week_start), today, now);
    (weekly_goal_minutes_for_week(goal_periods, absences, week_start) - worked).max(0)
}

/// Signed version of the time debt: positive means behind (that many
/// minutes owing), negative means ahead (that many minutes of surplus left
/// over after every historical shortfall is paid off). Net shortfall from
/// every day that has already fully elapsed before `today`, going all the
/// way back to the earliest logged entry (not just the current week) - each
/// day expected to hit its own daily target, with a day worked past target
/// offsetting shortfall from another day, in any week, net. Unlike
/// `minutes_left_week`, this does not reset at the start of a new week:
/// unpaid shortfall from a prior week carries forward until later surplus
/// days pay it off. Reduced live by any surplus already worked today - so
/// it can clear (and go negative) before the day is over, as soon as
/// today's extra hours are enough to pay it off. Not yet having worked
/// today's own target isn't held against you here (that's what
/// `minutes_left_today` / `leave_time` already show); only hours worked
/// *past* it count.
fn time_balance_minutes(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> i64 {
    let start = entries.iter().map(|e| e.date).min().unwrap_or(today);
    let mut historical = 0;
    let mut day = start;
    while day < today {
        let worked = worked_minutes(&entries_on(entries, day), today, now);
        historical += daily_target_minutes(goal_periods, absences, day) - worked;
        day += Duration::days(1);
    }
    let worked_today = worked_minutes(&entries_on(entries, today), today, now);
    let surplus_today = (worked_today - daily_target_minutes(goal_periods, absences, today)).max(0);
    historical - surplus_today
}

/// Minutes currently owed toward the time debt - see `time_balance_minutes`.
/// Clamped to 0 (never negative); use `time_surplus_minutes` to see how far
/// ahead you are once the debt is fully paid off.
pub fn time_debt_minutes(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> i64 {
    time_balance_minutes(entries, goal_periods, absences, today, now).max(0)
}

/// Minutes worked beyond what's needed to clear all outstanding time debt -
/// 0 whenever there's still debt owing (or the balance is exactly even).
/// This is what surfaces "negative debt": once today's surplus fully pays
/// off every prior shortfall, further surplus shows up here instead of
/// silently vanishing into the debt's floor at 0.
pub fn time_surplus_minutes(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    today: NaiveDate,
    now: NaiveTime,
) -> i64 {
    (-time_balance_minutes(entries, goal_periods, absences, today, now)).max(0)
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
/// gap yet, `estimated_lunch_break_minutes` from config is added as an
/// anticipated break; once a real gap is logged, it supersedes the guess.
///
/// Any banked surplus (negative time debt - see `time_surplus_minutes`)
/// offsets what's left of today's own target, so being ahead overall lets
/// you leave earlier today - not just show up as a bigger number on the
/// "clear by" projection, which only ever applies once you're behind.
/// Floors at `now`: a surplus big enough to have already covered today's
/// whole target (or more) means "you could leave right now", never a time
/// in the past.
pub fn leave_time(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    let left_today = minutes_left_today(entries, goal_periods, absences, today, now);
    let surplus = time_surplus_minutes(entries, goal_periods, absences, today, now);
    let minutes_owed = (left_today - surplus).max(0);
    leave_time_after(minutes_owed, entries, cfg, today, now)
}

/// Clock time you'd need to work until today to pay off the current time
/// debt (see `time_debt_minutes`) on top of today's own target - not the
/// whole week's remaining goal, just today's target plus the
/// carried-forward shortfall. `time_debt_minutes` only nets *surplus*
/// (hours worked past today's own target) against the debt, so paying it
/// off is never faster than `leave_time` alone: you hit today's target
/// first, and only time worked beyond that starts counting toward the
/// debt. Same break-projection rule as `leave_time`.
pub fn leave_time_debt(
    entries: &[TimeEntry],
    goal_periods: &[GoalPeriod],
    absences: &[Absence],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    let minutes_owed = minutes_left_today(entries, goal_periods, absences, today, now)
        + time_debt_minutes(entries, goal_periods, absences, today, now);
    leave_time_after(minutes_owed, entries, cfg, today, now)
}

fn leave_time_after(
    minutes_owed: i64,
    entries: &[TimeEntry],
    cfg: &Config,
    today: NaiveDate,
    now: NaiveTime,
) -> NaiveTime {
    let projected_break = if today_gaps(entries, today).is_empty() {
        cfg.estimated_lunch_break_minutes as i64
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

    /// A `now` value for tests where only closed entries are involved, so
    /// the exact time truly doesn't matter.
    fn end_of_day() -> NaiveTime {
        NaiveTime::from_hms_opt(23, 59, 59).unwrap()
    }

    fn cfg(lunch_break: u32) -> Config {
        Config {
            estimated_lunch_break_minutes: lunch_break,
            hide_weekends: false,
        }
    }

    fn goal_periods(hours: f64) -> Vec<GoalPeriod> {
        vec![GoalPeriod {
            id: 1,
            effective_from: date(1970, 1, 1),
            weekly_goal_hours: hours,
        }]
    }

    fn absence(id: u64, date: NaiveDate, reason: Option<&str>) -> Absence {
        Absence {
            id,
            date,
            reason: reason.map(String::from),
        }
    }

    #[test]
    fn worked_minutes_sums_entries() {
        let d = date(2026, 9, 11);
        let entries = [entry(1, d, "08:00", "12:00"), entry(2, d, "13:00", "15:15")];
        let refs: Vec<&TimeEntry> = entries.iter().collect();
        assert_eq!(worked_minutes(&refs, d, end_of_day()), 4 * 60 + 2 * 60 + 15);
    }

    #[test]
    fn daily_target_weekday_vs_weekend() {
        let g = goal_periods(40.0);
        // 2026-09-11 is a Friday
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 11)), 480);
        // 2026-09-12 is a Saturday
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 12)), 0);
        // 2026-09-13 is a Sunday
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 13)), 0);
    }

    #[test]
    fn daily_target_rounds_uneven_goal() {
        let g = goal_periods(37.5);
        // 37.5h * 60 = 2250 minutes / 5 = 450 minutes/day
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 11)), 450);
    }

    #[test]
    fn minutes_left_today_clamps_to_zero() {
        let g = goal_periods(40.0);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "18:00")]; // 10h worked, target 8h
        assert_eq!(minutes_left_today(&entries, &g, &[], d, end_of_day()), 0);
    }

    #[test]
    fn minutes_left_week_sums_and_clamps() {
        let g = goal_periods(40.0);
        let monday = date(2026, 9, 7);
        let entries = vec![
            entry(1, monday, "08:00", "16:00"),            // 8h
            entry(2, date(2026, 9, 8), "08:00", "16:00"),  // 8h, Tue
            entry(3, date(2026, 9, 14), "08:00", "16:00"), // next week, excluded
        ];
        assert_eq!(
            minutes_left_week(&entries, &g, &[], monday, end_of_day()),
            40 * 60 - 16 * 60
        );
    }

    #[test]
    fn minutes_left_week_clamps_to_zero() {
        let g = goal_periods(10.0);
        let monday = date(2026, 9, 7);
        let entries = vec![entry(1, monday, "08:00", "18:00")]; // 10h worked, goal 10h
        assert_eq!(
            minutes_left_week(&entries, &g, &[], monday, end_of_day()),
            0
        );
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
    fn goal_hours_for_single_period_applies_to_every_day() {
        let g = goal_periods(30.0);
        assert_eq!(goal_hours_for(&g, date(1970, 1, 2)), 30.0);
        assert_eq!(goal_hours_for(&g, date(2026, 9, 11)), 30.0);
        assert_eq!(goal_hours_for(&g, date(2099, 1, 1)), 30.0);
    }

    #[test]
    fn goal_hours_for_returns_latest_period_on_and_after_its_effective_from() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 32.0,
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0,
            },
        ];
        assert_eq!(goal_hours_for(&g, date(2026, 9, 27)), 32.0);
        // Inclusive: the change date itself already uses the new goal.
        assert_eq!(goal_hours_for(&g, date(2026, 9, 28)), 30.0);
        assert_eq!(goal_hours_for(&g, date(2026, 10, 5)), 30.0);
    }

    #[test]
    fn goal_hours_for_out_of_order_periods_still_resolve_by_date_not_declaration_order() {
        // Declared newest-first - resolution must still go by date, not
        // position in the list.
        let g = vec![
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0,
            },
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 32.0,
            },
        ];
        assert_eq!(goal_hours_for(&g, date(2026, 9, 27)), 32.0);
        assert_eq!(goal_hours_for(&g, date(2026, 9, 28)), 30.0);
    }

    #[test]
    fn goal_hours_for_duplicate_effective_from_picks_last_declared() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0,
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 35.0,
            },
        ];
        assert_eq!(goal_hours_for(&g, date(2026, 9, 28)), 35.0);
    }

    #[test]
    fn goal_hours_for_empty_falls_back_to_hardcoded_default() {
        assert_eq!(goal_hours_for(&[], date(2026, 9, 28)), 40.0);
    }

    #[test]
    fn goal_period_date_conflicts_detects_collision_and_excludes_given_id() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 32.0,
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0,
            },
        ];
        assert!(goal_period_date_conflicts(&g, date(2026, 9, 28), None));
        assert!(!goal_period_date_conflicts(&g, date(2026, 9, 28), Some(2)));
        assert!(!goal_period_date_conflicts(&g, date(2026, 10, 1), None));
    }

    #[test]
    fn is_absent_true_on_absent_day_false_otherwise() {
        let a = vec![absence(1, date(2026, 10, 12), None)];
        assert!(is_absent(&a, date(2026, 10, 12)));
        assert!(!is_absent(&a, date(2026, 10, 13)));
    }

    #[test]
    fn absence_on_finds_the_right_record() {
        let a = vec![
            absence(1, date(2026, 10, 12), Some("sick")),
            absence(2, date(2026, 10, 20), None),
        ];
        assert_eq!(absence_on(&a, date(2026, 10, 12)).map(|r| r.id), Some(1));
        assert_eq!(
            absence_on(&a, date(2026, 10, 12)).and_then(|r| r.reason.as_deref()),
            Some("sick")
        );
        assert!(absence_on(&a, date(2026, 10, 13)).is_none());
    }

    #[test]
    fn absence_date_conflicts_detects_collision_and_excludes_given_id() {
        let a = vec![
            absence(1, date(2026, 10, 12), None),
            absence(2, date(2026, 10, 20), None),
        ];
        assert!(absence_date_conflicts(&a, date(2026, 10, 12), None));
        assert!(!absence_date_conflicts(&a, date(2026, 10, 12), Some(1)));
        assert!(!absence_date_conflicts(&a, date(2026, 10, 13), None));
    }

    #[test]
    fn daily_target_minutes_zero_on_an_absent_weekday() {
        let g = goal_periods(40.0);
        // 2026-10-12 is a Monday - would normally have a target.
        let a = vec![absence(1, date(2026, 10, 12), None)];
        assert_eq!(daily_target_minutes(&g, &a, date(2026, 10, 12)), 0);
    }

    #[test]
    fn time_debt_minutes_does_not_accrue_deficit_for_an_absent_day() {
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![entry(1, date(2026, 9, 8), "08:00", "16:00")]; // Tue, on target
        // Monday was absent - without the absence, it would accrue a full
        // 8h (480min) deficit; with it, this day contributes nothing.
        let a = vec![absence(1, date(2026, 9, 7), Some("sick"))];
        let wednesday = date(2026, 9, 9);
        assert_eq!(
            time_debt_minutes(&entries, &g, &a, wednesday, end_of_day()),
            0
        );
    }

    #[test]
    fn weekly_goal_minutes_for_week_excludes_an_absent_day() {
        let g = goal_periods(40.0); // 8h/day
        let monday = date(2026, 9, 7);
        let a = vec![absence(1, monday, None)];
        // Mon absent (0) + Tue-Fri at 8h each = 32h = 1920min.
        assert_eq!(weekly_goal_minutes_for_week(&g, &a, monday), 1920);
    }

    #[test]
    fn history_weeks_colors_an_absent_day_like_a_zero_target_day() {
        // Confirms the "free win": once daily_target_minutes returns 0 for
        // an absent day, history_weeks's goal_week already reflects it with
        // no further changes needed - the day's coloring (a UI concern)
        // then naturally treats it the same as a zero-target weekend.
        let g = goal_periods(40.0);
        let monday = date(2026, 9, 7);
        let a = vec![absence(1, monday, None)];
        let today = date(2026, 9, 9);
        let rows = history_weeks(&[], &g, &a, today, end_of_day());
        // Mon absent (0) + Tue (0, no entries logged) + ... but only Mon is
        // relevant here: confirm its slot's target contribution is excluded
        // from the week total by checking the week total against a
        // no-absence baseline.
        let rows_no_absence = history_weeks(&[], &g, &[], today, end_of_day());
        assert_eq!(rows[0].goal_week, rows_no_absence[0].goal_week - 480);
    }

    #[test]
    fn daily_target_minutes_uses_goal_in_effect_for_that_day() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 40.0, // 8h/day
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0, // 6h/day
            },
        ];
        // 2026-09-25 is a Friday, before the change.
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 25)), 480);
        // 2026-09-28 is a Monday, the change date itself.
        assert_eq!(daily_target_minutes(&g, &[], date(2026, 9, 28)), 360);
    }

    #[test]
    fn time_debt_minutes_uses_old_goal_for_past_days_and_new_goal_going_forward() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 40.0, // 8h/day
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28),
                weekly_goal_hours: 30.0, // 6h/day
            },
        ];
        let entries = vec![
            entry(1, date(2026, 9, 25), "08:00", "14:00"), // Fri, old goal: 6h worked, 8h target, deficit 2h
            entry(2, date(2026, 9, 28), "08:00", "13:00"), // Mon, new goal: 5h worked, 6h target, deficit 1h
        ];
        let today = date(2026, 9, 29); // Tuesday, both days fully elapsed
        // 2h (old goal shortfall) + 1h (new goal shortfall) = 3h = 180min.
        // If the new goal were wrongly applied retroactively to Friday too
        // (6h target instead of 8h), Friday's deficit would only be 0 -
        // this pins that it isn't.
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], today, end_of_day()),
            180
        );
    }

    #[test]
    fn weekly_goal_minutes_for_week_reflects_mid_week_goal_change() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 40.0, // 8h/day
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 10), // Thursday
                weekly_goal_hours: 30.0,           // 6h/day
            },
        ];
        let monday = date(2026, 9, 7);
        // Mon, Tue, Wed at 8h + Thu, Fri at 6h = 24h + 12h = 36h = 2160min.
        assert_eq!(weekly_goal_minutes_for_week(&g, &[], monday), 2160);
    }

    #[test]
    fn history_weeks_goal_week_reflects_goal_in_effect_for_that_specific_week() {
        let g = vec![
            GoalPeriod {
                id: 1,
                effective_from: date(1970, 1, 1),
                weekly_goal_hours: 40.0,
            },
            GoalPeriod {
                id: 2,
                effective_from: date(2026, 9, 28), // Monday
                weekly_goal_hours: 30.0,
            },
        ];
        let entries = vec![
            entry(1, date(2026, 9, 21), "08:00", "16:00"), // week before the change
            entry(2, date(2026, 9, 28), "08:00", "16:00"), // week of the change
        ];
        let today = date(2026, 9, 29);
        let rows = history_weeks(&entries, &g, &[], today, end_of_day());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].week_start, date(2026, 9, 21));
        assert_eq!(rows[0].goal_week, 40 * 60);
        assert_eq!(rows[1].week_start, date(2026, 9, 28));
        assert_eq!(rows[1].goal_week, 30 * 60);
    }

    #[test]
    fn history_weeks_empty_with_no_entries_is_just_the_current_week() {
        let g = goal_periods(40.0);
        let today = date(2026, 9, 11); // Friday
        let rows = history_weeks(&[], &g, &[], today, end_of_day());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].week_start, date(2026, 9, 7));
        assert_eq!(rows[0].worked_week, 0);
        assert_eq!(rows[0].day_totals.len(), 7);
        assert_eq!(rows[0].day_totals[0], (date(2026, 9, 7), 0));
    }

    #[test]
    fn history_weeks_spans_from_earliest_entry_to_current_week() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 8, 31), "08:00", "16:00"), // Mon, 3 weeks back
            entry(2, date(2026, 9, 9), "08:00", "12:00"),  // Wed, 1 week back
        ];
        let today = date(2026, 9, 11); // Friday, current week starts 9/7
        let rows = history_weeks(&entries, &g, &[], today, end_of_day());
        assert_eq!(
            rows.iter().map(|r| r.week_start).collect::<Vec<_>>(),
            vec![date(2026, 8, 31), date(2026, 9, 7),]
        );
        assert_eq!(rows[0].worked_week, 8 * 60);
        assert_eq!(rows[1].worked_week, 4 * 60);
        // Wednesday's total lands in the right day slot of the right row.
        assert_eq!(rows[1].day_totals[2], (date(2026, 9, 9), 4 * 60));
    }

    #[test]
    fn time_debt_zero_on_first_day_of_week() {
        let g = goal_periods(40.0);
        let monday = date(2026, 9, 7);
        assert_eq!(time_debt_minutes(&[], &g, &[], monday, end_of_day()), 0);
    }

    #[test]
    fn time_debt_sums_elapsed_shortfalls() {
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:30"), // Mon 5h30m, deficit 2h30m
            entry(2, date(2026, 9, 8), "08:00", "14:20"), // Tue 6h20m, deficit 1h40m
            entry(3, date(2026, 9, 9), "08:00", "14:45"), // Wed 6h45m, deficit 1h15m
            entry(4, date(2026, 9, 10), "08:00", "13:10"), // Thu 5h10m, deficit 2h50m
        ];
        let friday = date(2026, 9, 11);
        // Nothing worked yet today, so no surplus to offset the debt with.
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], friday, end_of_day()),
            150 + 100 + 75 + 170
        );
    }

    #[test]
    fn time_debt_nets_surplus_against_shortfall() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "16:00"), // Mon 8h, surplus 0
            entry(2, date(2026, 9, 8), "08:00", "13:00"), // Tue 5h, deficit 3h
        ];
        let wednesday = date(2026, 9, 9);
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], wednesday, end_of_day()),
            180
        );
    }

    #[test]
    fn time_debt_clamps_to_zero_when_net_ahead() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "19:40"), // Mon 11h40m, surplus 3h40m
            entry(2, date(2026, 9, 8), "08:00", "14:40"), // Tue 6h40m, deficit 1h20m
        ];
        let wednesday = date(2026, 9, 9);
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], wednesday, end_of_day()),
            0
        );
    }

    #[test]
    fn time_surplus_minutes_shows_net_ahead_amount() {
        let g = goal_periods(40.0);
        // Same fixture as time_debt_clamps_to_zero_when_net_ahead: net
        // ahead by 3h40m - 1h20m = 2h20m (140min), which the debt figure
        // itself just floors to 0 - this is where that surplus shows up.
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "19:40"), // Mon 11h40m, surplus 3h40m
            entry(2, date(2026, 9, 8), "08:00", "14:40"), // Tue 6h40m, deficit 1h20m
        ];
        let wednesday = date(2026, 9, 9);
        assert_eq!(
            time_surplus_minutes(&entries, &g, &[], wednesday, end_of_day()),
            140
        );
    }

    #[test]
    fn time_surplus_minutes_zero_while_debt_still_owed() {
        let g = goal_periods(40.0);
        let entries = vec![entry(1, date(2026, 9, 7), "08:00", "13:00")]; // Mon 5h, deficit 3h
        let wednesday = date(2026, 9, 9);
        let debt = time_debt_minutes(&entries, &g, &[], wednesday, end_of_day());
        assert!(debt > 0, "test needs outstanding debt to be meaningful");
        assert_eq!(
            time_surplus_minutes(&entries, &g, &[], wednesday, end_of_day()),
            0
        );
    }

    #[test]
    fn time_debt_weekend_work_offsets_weekday_shortfall() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "14:20"), // Mon 6h20m, deficit 1h40m
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
            entry(3, date(2026, 9, 9), "08:00", "16:00"), // Wed 8h, on target
            entry(4, date(2026, 9, 10), "08:00", "16:00"), // Thu 8h, on target
            entry(5, date(2026, 9, 11), "08:00", "16:00"), // Fri 8h, on target
            entry(6, date(2026, 9, 12), "09:00", "10:00"), // Sat 1h (target 0), offsets debt
        ];
        // Sunday: the whole Mon-Sat week has elapsed; nothing logged Sunday itself.
        let sunday = date(2026, 9, 13);
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], sunday, end_of_day()),
            100 - 60
        );
    }

    #[test]
    fn time_debt_cleared_live_by_todays_surplus_before_day_ends() {
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:30"), // Mon deficit 2h30m (150min)
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
            entry(3, date(2026, 9, 9), "08:00", "16:00"), // Wed 8h, on target
            entry(4, date(2026, 9, 10), "08:00", "16:00"), // Thu 8h, on target
            entry(5, date(2026, 9, 11), "08:00", "18:30"), // Fri: 10h30m so far, surplus 150min
        ];
        let friday = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("18:30", "%H:%M").unwrap();
        // Paid off mid-day, well before Friday is actually over.
        assert_eq!(time_debt_minutes(&entries, &g, &[], friday, now), 0);
    }

    #[test]
    fn time_debt_reduced_but_not_cleared_by_partial_surplus() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:30"), // Mon deficit 150min
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
            entry(3, date(2026, 9, 9), "08:00", "16:00"), // Wed 8h, on target
            entry(4, date(2026, 9, 10), "08:00", "16:00"), // Thu 8h, on target
            entry(5, date(2026, 9, 11), "08:00", "16:30"), // Fri 8h30m, surplus 30min
        ];
        let friday = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("16:30", "%H:%M").unwrap();
        assert_eq!(time_debt_minutes(&entries, &g, &[], friday, now), 150 - 30);
    }

    #[test]
    fn time_debt_reflects_live_open_entry_progress() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:30"), // Mon deficit 150min
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
            entry(3, date(2026, 9, 9), "08:00", "16:00"), // Wed 8h, on target
            entry(4, date(2026, 9, 10), "08:00", "16:00"), // Thu 8h, on target
            open(5, date(2026, 9, 11), "08:00"),          // Fri: open block since 08:00
        ];
        let friday = date(2026, 9, 11);
        // Only 6h worked so far - below today's own 8h target, no surplus yet.
        let now_early = NaiveTime::parse_from_str("14:00", "%H:%M").unwrap();
        assert_eq!(time_debt_minutes(&entries, &g, &[], friday, now_early), 150);
        // By 18:30, 10h30m elapsed - 2h30m surplus, exactly enough to clear
        // the debt, without ever calling `clockout end`.
        let now_later = NaiveTime::parse_from_str("18:30", "%H:%M").unwrap();
        assert_eq!(time_debt_minutes(&entries, &g, &[], friday, now_later), 0);
    }

    #[test]
    fn time_debt_carries_shortfall_into_new_week() {
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 8, 31), "08:00", "16:00"), // Mon wk1, 8h, on target
            entry(2, date(2026, 9, 1), "08:00", "13:00"),  // Tue wk1, 5h, deficit 3h (180min)
            entry(3, date(2026, 9, 2), "08:00", "16:00"),  // Wed wk1, 8h, on target
            entry(4, date(2026, 9, 3), "08:00", "16:00"),  // Thu wk1, 8h, on target
            entry(5, date(2026, 9, 4), "08:00", "16:00"),  // Fri wk1, 8h, on target
        ];
        // New week (Monday), nothing logged yet today. The old week-anchored
        // logic would read 0 here; Tuesday's shortfall from last week
        // should still show.
        let monday_week2 = date(2026, 9, 7);
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], monday_week2, end_of_day()),
            180
        );
    }

    #[test]
    fn time_debt_paid_off_by_surplus_in_a_later_week() {
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 8, 31), "08:00", "16:00"), // Mon wk1, 8h, on target
            entry(2, date(2026, 9, 1), "08:00", "13:00"),  // Tue wk1, 5h, deficit 3h (180min)
            entry(3, date(2026, 9, 2), "08:00", "16:00"),  // Wed wk1, 8h, on target
            entry(4, date(2026, 9, 3), "08:00", "16:00"),  // Thu wk1, 8h, on target
            entry(5, date(2026, 9, 4), "08:00", "16:00"),  // Fri wk1, 8h, on target
            entry(6, date(2026, 9, 7), "08:00", "20:00"),  // Mon wk2, 12h, surplus 4h (240min)
        ];
        // A day into week 2, Monday's big surplus more than pays off the
        // 180min carried over from week 1: debt is fully cleared.
        let tuesday_week2 = date(2026, 9, 8);
        assert_eq!(
            time_debt_minutes(&entries, &g, &[], tuesday_week2, end_of_day()),
            0
        );
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
        let c = cfg(30);
        let g = goal_periods(40.0);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00")]; // 4h worked, 4h left
        let now = NaiveTime::parse_from_str("12:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &g, &[], &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("16:30", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_logged_gap_skips_padding() {
        let c = cfg(30);
        let g = goal_periods(40.0);
        let d = date(2026, 9, 11);
        let entries = vec![
            entry(1, d, "08:00", "12:00"),
            entry(2, d, "13:00", "15:00"), // 6h worked total, 2h left, real gap logged
        ];
        let now = NaiveTime::parse_from_str("15:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &g, &[], &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("17:00", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_zero_lunch_break_no_padding() {
        let c = cfg(0);
        let g = goal_periods(40.0);
        let d = date(2026, 9, 11);
        let entries = vec![entry(1, d, "08:00", "12:00")]; // 4h worked, 4h left
        let now = NaiveTime::parse_from_str("12:00", "%H:%M").unwrap();
        let leave = leave_time(&entries, &g, &[], &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("16:00", "%H:%M").unwrap());
    }

    #[test]
    fn leave_time_debt_adds_debt_on_top_of_todays_own_target() {
        let c = cfg(0);
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "13:00"), // Mon 5h, deficit 3h (180min)
            entry(2, date(2026, 9, 8), "08:00", "16:00"), // Tue 8h, on target
        ];
        // Wednesday, midday: 3h owed from Monday, nothing worked yet today -
        // so today's own 8h target is still fully outstanding too. Debt only
        // starts paying down once today's target is met (time_debt_minutes
        // nets *surplus* against it), so this is now + 8h target + 3h debt,
        // not just now + the 180min debt in isolation.
        let wednesday = date(2026, 9, 9);
        let now = NaiveTime::parse_from_str("14:00", "%H:%M").unwrap();
        assert_eq!(
            leave_time_debt(&entries, &g, &[], &c, wednesday, now),
            NaiveTime::parse_from_str("01:00", "%H:%M").unwrap() // wraps past midnight
        );
    }

    #[test]
    fn leave_time_debt_equals_leave_time_plus_debt() {
        // Directly encodes the intended relationship: clearing the debt
        // never happens before you'd otherwise leave for the day - it's
        // that same leave time, plus the debt on top.
        let c = cfg(30);
        let g = goal_periods(40.0);
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "14:20"), // Mon 6h20m, deficit 1h40m
            open(2, date(2026, 9, 8), "09:05"),           // Tue: open block since 09:05
        ];
        let tuesday = date(2026, 9, 8);
        let now = NaiveTime::parse_from_str("10:23", "%H:%M").unwrap();
        let debt = time_debt_minutes(&entries, &g, &[], tuesday, now);
        assert!(debt > 0, "test needs a nonzero debt to be meaningful");
        assert_eq!(
            leave_time_debt(&entries, &g, &[], &c, tuesday, now),
            leave_time(&entries, &g, &[], &c, tuesday, now) + Duration::minutes(debt)
        );
    }

    #[test]
    fn leave_time_debt_matches_now_when_no_debt_and_todays_target_met() {
        let c = cfg(0);
        let g = goal_periods(40.0);
        let d = date(2026, 9, 7);
        // Entry's end already reached by `now`, so it's fully credited:
        // today's target is exactly met (0 left) and there's no debt.
        let entries = vec![entry(1, d, "08:00", "16:00")];
        let now = NaiveTime::parse_from_str("16:00", "%H:%M").unwrap();
        assert_eq!(leave_time_debt(&entries, &g, &[], &c, d, now), now);
    }

    #[test]
    fn leave_time_subtracts_banked_surplus_from_todays_target() {
        let c = cfg(0); // no break padding
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "08:00", "20:00"), // Mon, 12h, surplus 4h (240min)
            entry(2, date(2026, 9, 8), "08:00", "10:00"), // Tue, 2h so far
        ];
        let tuesday = date(2026, 9, 8);
        let now = NaiveTime::parse_from_str("10:00", "%H:%M").unwrap();
        // left_today would be 6h (8h target - 2h worked), but the 4h
        // banked from Monday's surplus knocks that down to 2h.
        let surplus = time_surplus_minutes(&entries, &g, &[], tuesday, now);
        assert_eq!(
            surplus, 240,
            "test needs a nonzero surplus to be meaningful"
        );
        assert_eq!(
            leave_time(&entries, &g, &[], &c, tuesday, now),
            NaiveTime::parse_from_str("12:00", "%H:%M").unwrap()
        );
    }

    #[test]
    fn leave_time_floors_at_now_when_surplus_exceeds_todays_target() {
        let c = cfg(0);
        let g = goal_periods(40.0); // 8h/day target
        let entries = vec![
            entry(1, date(2026, 9, 7), "06:00", "22:00"), // Mon, 16h, surplus 8h
        ];
        let tuesday = date(2026, 9, 8);
        let now = NaiveTime::parse_from_str("09:00", "%H:%M").unwrap();
        // Nothing worked yet today (left_today = 8h), but Monday's 8h
        // surplus fully covers it - "leave at" floors at `now` rather than
        // going negative/into the past.
        let surplus = time_surplus_minutes(&entries, &g, &[], tuesday, now);
        assert!(
            surplus >= 8 * 60,
            "test needs a surplus covering the whole 8h target"
        );
        assert_eq!(leave_time(&entries, &g, &[], &c, tuesday, now), now);
    }

    #[test]
    fn leave_time_holds_steady_while_an_open_block_is_running() {
        let c = cfg(0);
        let g = goal_periods(40.0); // 8h/day target, no break padding
        let d = date(2026, 9, 11);
        let entries = vec![open(1, d, "08:00")];
        // At 08:00, 0 worked, full 8h owed: leave at 16:00.
        let now1 = NaiveTime::parse_from_str("08:00", "%H:%M").unwrap();
        assert_eq!(
            leave_time(&entries, &g, &[], &c, d, now1),
            NaiveTime::parse_from_str("16:00", "%H:%M").unwrap()
        );
        // An hour later, still working (block still open): that hour's now
        // been earned back, so the projection holds at 16:00 rather than
        // receding to 17:00.
        let now2 = NaiveTime::parse_from_str("09:00", "%H:%M").unwrap();
        assert_eq!(
            leave_time(&entries, &g, &[], &c, d, now2),
            NaiveTime::parse_from_str("16:00", "%H:%M").unwrap()
        );
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
    fn entry_minutes_open_entry_at_start_is_zero() {
        let d = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("12:35", "%H:%M").unwrap();
        assert_eq!(entry_minutes(&open(1, d, "12:35"), d, now), 0);
    }

    #[test]
    fn entry_minutes_closed_future_end_counts_in_full() {
        let d = date(2026, 9, 11);
        // Closed 12:35-17:00 even though it's currently only 16:00: closing
        // an entry is a declaration the work happened, trusted at face
        // value - the whole 4h25m counts, not just up to now.
        let now = NaiveTime::parse_from_str("16:00", "%H:%M").unwrap();
        assert_eq!(entry_minutes(&entry(1, d, "12:35", "17:00"), d, now), 265); // 4h25m
    }

    #[test]
    fn entry_minutes_closed_entirely_future_counts_in_full() {
        let d = date(2026, 9, 11);
        // A closed block for later today that hasn't started yet still
        // counts in full - it's closed, so it's trusted regardless of now.
        let now = NaiveTime::parse_from_str("10:00", "%H:%M").unwrap();
        assert_eq!(entry_minutes(&entry(1, d, "12:35", "17:00"), d, now), 265); // 4h25m
    }

    #[test]
    fn entry_minutes_past_day_future_looking_end_is_unaffected() {
        // A closed entry on a day that's already over always counts in full,
        // regardless of what `now` (today's clock) happens to be.
        let yesterday = date(2026, 9, 10);
        let today = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("08:00", "%H:%M").unwrap();
        assert_eq!(
            entry_minutes(&entry(1, yesterday, "12:35", "17:00"), today, now),
            265 // 4h25m, the full stored duration
        );
    }

    #[test]
    fn leave_time_credits_a_future_logged_end_once_closed() {
        let c = cfg(0);
        let g = goal_periods(40.0); // 8h/day target
        let d = date(2026, 9, 11);
        // A morning block plus an afternoon block logged in advance, end
        // still in the future relative to `now` - but closed, so trusted.
        let entries = vec![
            entry(1, d, "08:40", "12:00"), // 3h20m
            entry(2, d, "12:35", "17:00"), // 4h25m, closed through 17:00
        ];
        // It's only 16:00 right now, but both are closed entries: the full
        // 4h25m of the second block counts, not just up to now.
        let now = NaiveTime::parse_from_str("16:00", "%H:%M").unwrap();
        // worked: 3h20m + 4h25m = 7h45m; target 8h -> 15m left.
        let leave = leave_time(&entries, &g, &[], &c, d, now);
        assert_eq!(leave, NaiveTime::parse_from_str("16:15", "%H:%M").unwrap());
    }

    #[test]
    fn entry_minutes_open_today_counts_elapsed() {
        let d = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("10:15", "%H:%M").unwrap();
        assert_eq!(entry_minutes(&open(1, d, "08:00"), d, now), 135); // 2h15m
    }

    #[test]
    fn entry_minutes_open_from_past_day_counts_through_end_of_day() {
        let yesterday = date(2026, 9, 10);
        let today = date(2026, 9, 11);
        let now = NaiveTime::parse_from_str("09:00", "%H:%M").unwrap(); // today; irrelevant here
        // Started 22:00 yesterday, never closed: counted through 23:59:59
        // that day, not against today's wall-clock time.
        assert_eq!(
            entry_minutes(&open(1, yesterday, "22:00"), today, now),
            119 // 22:00 -> 23:59:59 is 1h59m59s; num_minutes() truncates
        );
    }

    #[test]
    fn open_entry_finds_the_unfinished_block() {
        let d = date(2026, 9, 11);
        assert_eq!(open_entry(&[]), None);
        let entries = vec![entry(1, d, "08:00", "12:00"), open(2, d, "12:35")];
        assert_eq!(open_entry(&entries).map(|e| e.id), Some(2));
    }

    #[test]
    fn clocked_out_at_none_before_anything_logged_today() {
        let today = date(2026, 9, 11);
        assert_eq!(clocked_out_at(&[], today), None);
    }

    #[test]
    fn clocked_out_at_none_while_a_block_is_open() {
        let today = date(2026, 9, 11);
        let entries = vec![open(1, today, "08:00")];
        assert_eq!(clocked_out_at(&entries, today), None);
        // Even an open block from a past day still counts as "still going".
        let entries = vec![open(1, date(2026, 9, 10), "22:00")];
        assert_eq!(clocked_out_at(&entries, today), None);
    }

    #[test]
    fn clocked_out_at_latest_end_once_closed_for_the_day() {
        let today = date(2026, 9, 11);
        let entries = vec![
            entry(1, today, "08:00", "12:00"),
            entry(2, today, "13:00", "17:00"),
        ];
        assert_eq!(
            clocked_out_at(&entries, today),
            Some(NaiveTime::parse_from_str("17:00", "%H:%M").unwrap())
        );
    }

    #[test]
    fn worked_minutes_includes_live_open_entry() {
        let d = date(2026, 9, 11);
        let entries = [entry(1, d, "08:00", "12:00"), open(2, d, "12:35")];
        let now = NaiveTime::parse_from_str("14:00", "%H:%M").unwrap();
        // Closed 4h + open-so-far 1h25m (12:35 -> 14:00).
        assert_eq!(
            worked_minutes(&entries.iter().collect::<Vec<_>>(), d, now),
            4 * 60 + 85
        );
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
