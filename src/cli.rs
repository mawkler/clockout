use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};

use crate::calc;
use crate::model::TimeEntry;
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
        Command::Status => cmd_status(today),
        Command::Tui => crate::tui::run(),
        Command::List { all } => cmd_list(all, today),
        Command::Edit { id, range, date } => cmd_edit(id, range, date),
        Command::Delete { id } => cmd_delete(id),
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

fn cmd_end(time: Option<String>) -> anyhow::Result<()> {
    let (id, entry_date, start, end) = storage::with_entries(|data| {
        let idx = data
            .entries
            .iter()
            .position(|e| e.end.is_none())
            .ok_or_else(|| anyhow::anyhow!("no open block - run `clockout start` first"))?;

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
        Ok((id, entry_date, start, end))
    })?;
    println!(
        "ended #{}: {} {}-{}",
        id,
        entry_date,
        start.format("%H:%M"),
        end.format("%H:%M")
    );
    Ok(())
}

fn cmd_status(today: NaiveDate) -> anyhow::Result<()> {
    let cfg = storage::load_config()?;
    let data = storage::load_entries()?;
    let now = Local::now().time();

    let worked_today = calc::worked_minutes(&calc::entries_on(&data.entries, today), today, now);
    let target_today = calc::daily_target_minutes(&cfg, today);
    let left_today = calc::minutes_left_today(&data.entries, &cfg, today, now);

    let (week_start, _) = calc::week_bounds(today);
    let worked_week = calc::worked_minutes(
        &calc::entries_in_week(&data.entries, week_start),
        today,
        now,
    );
    let goal_week = calc::weekly_goal_minutes(&cfg);
    let left_week = calc::minutes_left_week(&data.entries, &cfg, today, now);

    println!("{} ({})", today, today.format("%A"));
    if target_today == 0 {
        println!("No target today — logged {}", calc::format_hm(worked_today));
    } else {
        let leave = calc::leave_time(&data.entries, &cfg, today, now);
        println!(
            "Today:     {} / {} target ({} left)",
            calc::format_hm(worked_today),
            calc::format_hm(target_today),
            calc::format_hm(left_today)
        );
        println!("Leave at:  {}", leave.format("%H:%M"));
    }
    println!(
        "This week: {} / {} goal ({} left)",
        calc::format_hm(worked_week),
        calc::format_hm(goal_week),
        calc::format_hm(left_week)
    );
    let debt = calc::time_debt_minutes(&data.entries, &cfg, today, now);
    if debt > 0 {
        let clear_debt_by = calc::leave_time_debt(&data.entries, &cfg, today, now);
        println!(
            "Time debt: {} · clear by {}",
            calc::format_hm(debt),
            clear_debt_by.format("%H:%M")
        );
    } else {
        let surplus = calc::time_surplus_minutes(&data.entries, &cfg, today, now);
        if surplus > 0 {
            println!("Time debt: +{}", calc::format_hm(surplus));
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
