use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use clap::{Parser, Subcommand};

use crate::calc;
use crate::model::{Absence, GoalPeriod, TimeEntry};
use crate::parse;
use crate::storage;

#[derive(Parser)]
#[command(
    name = "clockout",
    version,
    about = "CLI and TUI to track worked hours against a weekly goal, and see what time you can leave for the day"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Log a worked time range, e.g. `clockout log 8:00-12:00`
    Log {
        /// Time range, e.g. "8:00-12:00"
        range: String,
        /// Relative day: "today" or "yesterday" (default: today)
        #[arg(long, conflicts_with = "date")]
        day: Option<String>,
        /// Explicit date, e.g. 2026-09-08
        #[arg(long, conflicts_with = "day")]
        date: Option<NaiveDate>,
    },
    /// Start an open-ended block, e.g. `clockout start 12:35` (default: now)
    Start {
        time: Option<String>,
        #[arg(long, conflicts_with = "date")]
        day: Option<String>,
        #[arg(long, conflicts_with = "day")]
        date: Option<NaiveDate>,
    },
    /// Close the open block started with `clockout start` (default: now)
    End { time: Option<String> },
    /// Close the open block (if any) and show where you stand for the day
    Done { time: Option<String> },
    /// Print a one-shot plain-text summary
    Status,
    /// Launch the interactive dashboard (also the default with no subcommand)
    Tui,
    /// List logged entries
    List {
        /// Show all entries instead of just this week's
        #[arg(long)]
        all: bool,
    },
    /// Show weekly totals, one week per row and one day per column
    History,
    /// Edit an existing entry by id
    Edit {
        id: u64,
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Delete an entry by id
    Delete { id: u64 },
    /// Manage the weekly-hours goal over time
    Goal {
        #[command(subcommand)]
        action: GoalCommand,
    },
    /// Record absences (sick days, vacation, etc.) that don't accrue time debt
    Absence {
        #[command(subcommand)]
        action: AbsenceCommand,
    },
}

#[derive(Subcommand)]
pub enum GoalCommand {
    /// Set a new weekly-hours goal, effective from a date (default: today)
    Set {
        hours: f64,
        #[arg(long)]
        from: Option<NaiveDate>,
    },
    /// List all goal periods
    List,
    /// Edit an existing goal period by id
    Edit {
        id: u64,
        #[arg(long)]
        hours: Option<f64>,
        #[arg(long)]
        from: Option<NaiveDate>,
    },
    /// Delete a goal period by id
    Delete { id: u64 },
}

#[derive(Subcommand)]
pub enum AbsenceCommand {
    /// Mark a day absent, e.g. `clockout absence add --day yesterday --reason sick`
    Add {
        /// Relative day: "today" or "yesterday" (default: today)
        #[arg(long, conflicts_with = "date")]
        day: Option<String>,
        /// Explicit date, e.g. 2026-09-08
        #[arg(long, conflicts_with = "day")]
        date: Option<NaiveDate>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// List all absences
    List,
    /// Edit an existing absence by id
    Edit {
        id: u64,
        #[arg(long)]
        date: Option<NaiveDate>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Delete an absence by id
    Delete { id: u64 },
}

fn resolve_date(
    day: &Option<String>,
    date: &Option<NaiveDate>,
    today: NaiveDate,
) -> Result<NaiveDate, String> {
    if let Some(date) = date {
        return Ok(*date);
    }
    if let Some(day) = day {
        return parse::resolve_day_spec(day, today);
    }
    Ok(today)
}

pub fn run(command: Command) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    match command {
        Command::Log { range, day, date } => cmd_log(range, day, date, today),
        Command::Start { time, day, date } => cmd_start(time, day, date, today),
        Command::End { time } => cmd_end(time),
        Command::Done { time } => cmd_done(time, today),
        Command::Status => cmd_status(today),
        Command::Tui => crate::tui::run(),
        Command::List { all } => cmd_list(all, today),
        Command::History => cmd_history(today),
        Command::Edit { id, range, date } => cmd_edit(id, range, date),
        Command::Delete { id } => cmd_delete(id),
        Command::Goal { action } => cmd_goal(action, today),
        Command::Absence { action } => cmd_absence(action, today),
    }
}

fn cmd_log(
    range: String,
    day: Option<String>,
    date: Option<NaiveDate>,
    today: NaiveDate,
) -> anyhow::Result<()> {
    let entry_date = resolve_date(&day, &date, today).map_err(|e| anyhow::anyhow!(e))?;
    let (start, end) = parse::parse_range(&range).map_err(|e| anyhow::anyhow!(e))?;

    let candidate = storage::with_entries(|data| {
        if let Some(open) = calc::open_entry(&data.entries) {
            anyhow::bail!(
                "open block #{} ({} {}-…) needs `clockout end` first",
                open.id,
                open.date,
                open.start.format("%H:%M")
            );
        }
        if calc::is_absent(&data.absences, entry_date) {
            anyhow::bail!("{entry_date} is marked absent - delete the absence first");
        }
        let candidate = TimeEntry {
            id: data.next_id,
            date: entry_date,
            start,
            end: Some(end),
        };
        if let Some(conflict) = data
            .entries
            .iter()
            .find(|e| e.date == entry_date && calc::overlaps(e, &candidate))
        {
            anyhow::bail!(
                "range {}-{} overlaps existing entry #{} ({}-{})",
                start.format("%H:%M"),
                end.format("%H:%M"),
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end),
            );
        }

        data.next_id += 1;
        data.entries.push(candidate.clone());
        Ok(candidate)
    })?;
    println!(
        "logged #{}: {} {}-{}",
        candidate.id,
        entry_date,
        start.format("%H:%M"),
        end.format("%H:%M")
    );
    Ok(())
}

fn cmd_start(
    time: Option<String>,
    day: Option<String>,
    date: Option<NaiveDate>,
    today: NaiveDate,
) -> anyhow::Result<()> {
    let entry_date = resolve_date(&day, &date, today).map_err(|e| anyhow::anyhow!(e))?;
    let start = match time {
        Some(t) => parse::parse_time(&t).map_err(|e| anyhow::anyhow!(e))?,
        None => Local::now().time(),
    };
    let candidate = storage::with_entries(|data| {
        if let Some(open) = calc::open_entry(&data.entries) {
            anyhow::bail!(
                "already have an open block #{} ({} {}-…) - run `clockout end` first",
                open.id,
                open.date,
                open.start.format("%H:%M")
            );
        }
        if calc::is_absent(&data.absences, entry_date) {
            anyhow::bail!("{entry_date} is marked absent - delete the absence first");
        }
        let candidate = TimeEntry {
            id: data.next_id,
            date: entry_date,
            start,
            end: None,
        };
        if let Some(conflict) = data
            .entries
            .iter()
            .find(|e| e.date == entry_date && calc::overlaps(e, &candidate))
        {
            anyhow::bail!(
                "start {} overlaps existing entry #{} ({}-{})",
                start.format("%H:%M"),
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end),
            );
        }

        data.next_id += 1;
        data.entries.push(candidate.clone());
        Ok(candidate)
    })?;
    println!(
        "started #{}: {} {}-…",
        candidate.id,
        entry_date,
        start.format("%H:%M")
    );
    Ok(())
}

/// Closes the open block (if any) at `time` (parsed, default: now).
/// Returns `None` if there was nothing open to close - not an error,
/// since `cmd_done` treats "already clocked out" as fine. Shared by
/// `cmd_end` (which errors on `None`) and `cmd_done` (which doesn't).
fn close_open_block(
    time: Option<String>,
) -> anyhow::Result<Option<(u64, NaiveDate, NaiveTime, NaiveTime)>> {
    storage::with_entries(|data| {
        let Some(idx) = data.entries.iter().position(|e| e.end.is_none()) else {
            return Ok(None);
        };
        let end = match &time {
            Some(t) => parse::parse_time(t).map_err(|e| anyhow::anyhow!(e))?,
            None => Local::now().time(),
        };
        let (id, entry_date, start) = (
            data.entries[idx].id,
            data.entries[idx].date,
            data.entries[idx].start,
        );
        if start >= end {
            anyhow::bail!(
                "end {} must be after start {}",
                end.format("%H:%M"),
                start.format("%H:%M")
            );
        }
        let candidate = TimeEntry {
            id,
            date: entry_date,
            start,
            end: Some(end),
        };
        if let Some(conflict) = data
            .entries
            .iter()
            .find(|e| e.id != id && e.date == entry_date && calc::overlaps(e, &candidate))
        {
            anyhow::bail!(
                "end {} overlaps existing entry #{} ({}-{})",
                end.format("%H:%M"),
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end),
            );
        }

        data.entries[idx].end = Some(end);
        Ok(Some((id, entry_date, start, end)))
    })
}

fn cmd_end(time: Option<String>) -> anyhow::Result<()> {
    let (id, entry_date, start, end) = close_open_block(time)?
        .ok_or_else(|| anyhow::anyhow!("no open block - run `clockout start` first"))?;
    println!(
        "ended #{}: {} {}-{}",
        id,
        entry_date,
        start.format("%H:%M"),
        end.format("%H:%M")
    );
    Ok(())
}

/// Closes the open block at now (if any - a no-op otherwise, not an
/// error), then shows the same summary `cmd_status` does. Nothing is
/// "updated" beyond that write: the time debt/surplus shown is already
/// always live-derived from entries, never stored - this just surfaces
/// it at the moment you stop for the day.
fn cmd_done(time: Option<String>, today: NaiveDate) -> anyhow::Result<()> {
    if let Some((id, entry_date, start, end)) = close_open_block(time)? {
        println!(
            "ended #{}: {} {}-{}",
            id,
            entry_date,
            start.format("%H:%M"),
            end.format("%H:%M")
        );
    }
    cmd_status(today)
}

fn cmd_status(today: NaiveDate) -> anyhow::Result<()> {
    let cfg = storage::load_config()?;
    let data = storage::load_entries()?;
    let now = Local::now().time();

    let worked_today = calc::worked_minutes(&calc::entries_on(&data.entries, today), today, now);
    let target_today = calc::daily_target_minutes(&data.goal_periods, &data.absences, today);
    let left_today = calc::minutes_left_today(
        &data.entries,
        &data.goal_periods,
        &data.absences,
        today,
        now,
    );

    let (week_start, _) = calc::week_bounds(today);
    let worked_week = calc::worked_minutes(
        &calc::entries_in_week(&data.entries, week_start),
        today,
        now,
    );
    let goal_week =
        calc::weekly_goal_minutes_for_week(&data.goal_periods, &data.absences, week_start);
    let left_week = calc::minutes_left_week(
        &data.entries,
        &data.goal_periods,
        &data.absences,
        today,
        now,
    );

    println!("{} ({})", today, today.format("%A"));
    if target_today == 0 {
        if let Some(absence) = calc::absence_on(&data.absences, today) {
            let tail = absence
                .reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            println!("Absent today{tail}");
        } else {
            println!("No target today — logged {}", calc::format_hm(worked_today));
        }
    } else {
        println!(
            "Today:     {} / {} target ({} left)",
            calc::format_hm(worked_today),
            calc::format_hm(target_today),
            calc::format_hm(left_today)
        );
        if let Some(clocked_out) = calc::clocked_out_at(&data.entries, today) {
            println!("Clocked out: {}", clocked_out.format("%H:%M"));
        } else {
            let leave = calc::leave_time(
                &data.entries,
                &data.goal_periods,
                &data.absences,
                &cfg,
                today,
                now,
            );
            println!("Leave at:  {}", leave.format("%H:%M"));
        }
    }
    println!(
        "This week: {} / {} goal ({} left)",
        calc::format_hm(worked_week),
        calc::format_hm(goal_week),
        calc::format_hm(left_week)
    );
    let debt = calc::time_debt_minutes(
        &data.entries,
        &data.goal_periods,
        &data.absences,
        today,
        now,
    );
    if debt > 0 {
        let clear_debt_by = calc::leave_time_debt(
            &data.entries,
            &data.goal_periods,
            &data.absences,
            &cfg,
            today,
            now,
        );
        println!(
            "Time debt: {} · clear by {}",
            calc::format_hm(debt),
            clear_debt_by.format("%H:%M")
        );
    } else {
        let surplus = calc::time_surplus_minutes(
            &data.entries,
            &data.goal_periods,
            &data.absences,
            today,
            now,
        );
        if surplus > 0 {
            println!("Time debt: -{}", calc::format_hm(surplus));
        }
    }
    if let Some(open) = calc::open_entry(&data.entries) {
        println!(
            "Open block: {} started {} (`clockout end` to close)",
            open.date,
            open.start.format("%H:%M")
        );
    }
    Ok(())
}

fn cmd_list(all: bool, today: NaiveDate) -> anyhow::Result<()> {
    let data = storage::load_entries()?;
    let mut entries: Vec<&TimeEntry> = if all {
        data.entries.iter().collect()
    } else {
        let (week_start, _) = calc::week_bounds(today);
        calc::entries_in_week(&data.entries, week_start)
    };
    entries.sort_by_key(|e| (e.date, e.start));
    if entries.is_empty() {
        println!("no entries");
        return Ok(());
    }
    for e in entries {
        println!(
            "#{:<4} {} {}-{}",
            e.id,
            e.date,
            e.start.format("%H:%M"),
            calc::format_end(e.end)
        );
    }
    Ok(())
}

const DAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn cmd_history(today: NaiveDate) -> anyhow::Result<()> {
    let cfg = storage::load_config()?;
    let data = storage::load_entries()?;
    let now = Local::now().time();

    let day_indices: Vec<usize> = (0..7).filter(|&i| !(cfg.hide_weekends && i >= 5)).collect();

    print!("{:<8}", "");
    for &i in &day_indices {
        print!("{:>7}", DAY_LABELS[i]);
    }
    println!("{:>12}", "Total");

    let (current_week_start, _) = calc::week_bounds(today);
    let rows = calc::history_weeks(
        &data.entries,
        &data.goal_periods,
        &data.absences,
        today,
        now,
    );
    for row in rows.iter().rev() {
        let marker = if row.week_start == current_week_start {
            "*"
        } else {
            " "
        };
        print!(
            "{:<8}",
            format!("W{:02}{marker}", row.week_start.iso_week().week())
        );
        for &i in &day_indices {
            let (day, worked) = row.day_totals[i];
            let cell = if day > today {
                "-".to_string()
            } else {
                calc::format_hm(worked)
            };
            print!("{cell:>7}");
        }
        println!(
            "{:>12}",
            format!(
                "{}/{}",
                calc::format_hm(row.worked_week),
                calc::format_hm(row.goal_week)
            )
        );
    }
    Ok(())
}

fn cmd_edit(id: u64, range: Option<String>, date: Option<NaiveDate>) -> anyhow::Result<()> {
    let updated = storage::with_entries(|data| {
        let idx = data
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("no entry with id {id}"))?;

        let mut updated = data.entries[idx].clone();
        if let Some(date) = date {
            updated.date = date;
        }
        if let Some(range) = &range {
            let (start, end) = parse::parse_range(range).map_err(|e| anyhow::anyhow!(e))?;
            updated.start = start;
            updated.end = Some(end);
        }

        if let Some(conflict) = data
            .entries
            .iter()
            .find(|e| e.id != id && e.date == updated.date && calc::overlaps(e, &updated))
        {
            anyhow::bail!(
                "range {}-{} overlaps existing entry #{} ({}-{})",
                updated.start.format("%H:%M"),
                calc::format_end(updated.end),
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end),
            );
        }

        data.entries[idx] = updated.clone();
        Ok(updated)
    })?;
    println!(
        "updated #{}: {} {}-{}",
        updated.id,
        updated.date,
        updated.start.format("%H:%M"),
        calc::format_end(updated.end)
    );
    Ok(())
}

fn cmd_delete(id: u64) -> anyhow::Result<()> {
    storage::with_entries(|data| {
        let len_before = data.entries.len();
        data.entries.retain(|e| e.id != id);
        if data.entries.len() == len_before {
            anyhow::bail!("no entry with id {id}");
        }
        Ok(())
    })?;
    println!("deleted #{id}");
    Ok(())
}

fn cmd_goal(action: GoalCommand, today: NaiveDate) -> anyhow::Result<()> {
    match action {
        GoalCommand::Set { hours, from } => cmd_goal_set(hours, from, today),
        GoalCommand::List => cmd_goal_list(today),
        GoalCommand::Edit { id, hours, from } => cmd_goal_edit(id, hours, from),
        GoalCommand::Delete { id } => cmd_goal_delete(id),
    }
}

fn cmd_goal_set(hours: f64, from: Option<NaiveDate>, today: NaiveDate) -> anyhow::Result<()> {
    if hours <= 0.0 {
        anyhow::bail!("hours must be positive");
    }
    let effective_from = from.unwrap_or(today);
    let period = storage::with_entries(|data| {
        if calc::goal_period_date_conflicts(&data.goal_periods, effective_from, None) {
            anyhow::bail!(
                "a goal period already starts {effective_from} - use `clockout goal edit` instead"
            );
        }
        let period = GoalPeriod {
            id: data.next_id,
            effective_from,
            weekly_goal_hours: hours,
        };
        data.next_id += 1;
        data.goal_periods.push(period.clone());
        Ok(period)
    })?;
    println!(
        "set goal #{}: {}h/week from {}",
        period.id, period.weekly_goal_hours, period.effective_from
    );
    Ok(())
}

fn cmd_goal_list(today: NaiveDate) -> anyhow::Result<()> {
    let data = storage::load_entries()?;
    let mut periods: Vec<&GoalPeriod> = data.goal_periods.iter().collect();
    periods.sort_by_key(|p| p.effective_from);
    if periods.is_empty() {
        println!("no goal periods");
        return Ok(());
    }
    let current_id = calc::current_goal_period(&data.goal_periods, today).map(|p| p.id);
    for p in periods {
        let marker = if Some(p.id) == current_id { "*" } else { " " };
        println!(
            "#{:<4} {} {}h/week{marker}",
            p.id, p.effective_from, p.weekly_goal_hours
        );
    }
    Ok(())
}

fn cmd_goal_edit(id: u64, hours: Option<f64>, from: Option<NaiveDate>) -> anyhow::Result<()> {
    if let Some(hours) = hours
        && hours <= 0.0
    {
        anyhow::bail!("hours must be positive");
    }
    let updated = storage::with_entries(|data| {
        let idx = data
            .goal_periods
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("no goal period with id {id}"))?;

        let mut updated = data.goal_periods[idx].clone();
        if let Some(hours) = hours {
            updated.weekly_goal_hours = hours;
        }
        if let Some(from) = from {
            updated.effective_from = from;
        }

        if calc::goal_period_date_conflicts(&data.goal_periods, updated.effective_from, Some(id)) {
            anyhow::bail!("a goal period already starts {}", updated.effective_from);
        }

        data.goal_periods[idx] = updated.clone();
        Ok(updated)
    })?;
    println!(
        "updated goal #{}: {}h/week from {}",
        updated.id, updated.weekly_goal_hours, updated.effective_from
    );
    Ok(())
}

fn cmd_goal_delete(id: u64) -> anyhow::Result<()> {
    storage::with_entries(|data| {
        if data.goal_periods.len() <= 1 {
            anyhow::bail!("can't delete the only remaining goal period");
        }
        let len_before = data.goal_periods.len();
        data.goal_periods.retain(|p| p.id != id);
        if data.goal_periods.len() == len_before {
            anyhow::bail!("no goal period with id {id}");
        }
        Ok(())
    })?;
    println!("deleted goal #{id}");
    Ok(())
}

fn cmd_absence(action: AbsenceCommand, today: NaiveDate) -> anyhow::Result<()> {
    match action {
        AbsenceCommand::Add { day, date, reason } => cmd_absence_add(day, date, reason, today),
        AbsenceCommand::List => cmd_absence_list(),
        AbsenceCommand::Edit { id, date, reason } => cmd_absence_edit(id, date, reason),
        AbsenceCommand::Delete { id } => cmd_absence_delete(id),
    }
}

fn cmd_absence_add(
    day: Option<String>,
    date: Option<NaiveDate>,
    reason: Option<String>,
    today: NaiveDate,
) -> anyhow::Result<()> {
    let absence_date = resolve_date(&day, &date, today).map_err(|e| anyhow::anyhow!(e))?;
    let absence = storage::with_entries(|data| {
        if calc::absence_date_conflicts(&data.absences, absence_date, None) {
            anyhow::bail!("{absence_date} is already marked absent");
        }
        if !calc::entries_on(&data.entries, absence_date).is_empty() {
            anyhow::bail!("{absence_date} already has logged entries");
        }
        let absence = Absence {
            id: data.next_id,
            date: absence_date,
            reason,
        };
        data.next_id += 1;
        data.absences.push(absence.clone());
        Ok(absence)
    })?;
    let tail = absence
        .reason
        .as_deref()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    println!("marked absent #{}: {}{tail}", absence.id, absence.date);
    Ok(())
}

fn cmd_absence_list() -> anyhow::Result<()> {
    let data = storage::load_entries()?;
    let mut absences: Vec<&Absence> = data.absences.iter().collect();
    absences.sort_by_key(|a| a.date);
    if absences.is_empty() {
        println!("no absences");
        return Ok(());
    }
    for a in absences {
        let tail = a
            .reason
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        println!("#{:<4} {}{tail}", a.id, a.date);
    }
    Ok(())
}

fn cmd_absence_edit(
    id: u64,
    date: Option<NaiveDate>,
    reason: Option<String>,
) -> anyhow::Result<()> {
    let updated = storage::with_entries(|data| {
        let idx = data
            .absences
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow::anyhow!("no absence with id {id}"))?;

        let mut updated = data.absences[idx].clone();
        if let Some(date) = date {
            updated.date = date;
        }
        if let Some(reason) = reason {
            updated.reason = Some(reason);
        }

        if calc::absence_date_conflicts(&data.absences, updated.date, Some(id)) {
            anyhow::bail!("{} is already marked absent", updated.date);
        }
        if !calc::entries_on(&data.entries, updated.date).is_empty() {
            anyhow::bail!("{} already has logged entries", updated.date);
        }

        data.absences[idx] = updated.clone();
        Ok(updated)
    })?;
    let tail = updated
        .reason
        .as_deref()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    println!("updated absence #{}: {}{tail}", updated.id, updated.date);
    Ok(())
}

fn cmd_absence_delete(id: u64) -> anyhow::Result<()> {
    storage::with_entries(|data| {
        let len_before = data.absences.len();
        data.absences.retain(|a| a.id != id);
        if data.absences.len() == len_before {
            anyhow::bail!("no absence with id {id}");
        }
        Ok(())
    })?;
    println!("deleted absence #{id}");
    Ok(())
}
