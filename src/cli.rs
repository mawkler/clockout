use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};

use crate::calc;
use crate::model::TimeEntry;
use crate::parse;
use crate::storage;

#[derive(Parser)]
#[command(
    name = "time",
    version,
    about = "Track weekly work hours against a goal"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Log a worked time range, e.g. `time log 8:00-12:00`
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

    let mut data = storage::load_entries()?;
    let candidate = TimeEntry {
        id: data.next_id,
        date: entry_date,
        start,
        end,
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
            conflict.end.format("%H:%M"),
        );
    }

    data.next_id += 1;
    data.entries.push(candidate.clone());
    storage::save_entries(&data)?;
    println!(
        "logged #{}: {} {}-{}",
        candidate.id,
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

    let worked_today = calc::worked_minutes(&calc::entries_on(&data.entries, today));
    let target_today = calc::daily_target_minutes(&cfg, today);
    let left_today = calc::minutes_left_today(&data.entries, &cfg, today);

    let (week_start, _) = calc::week_bounds(today);
    let worked_week = calc::worked_minutes(&calc::entries_in_week(&data.entries, week_start));
    let goal_week = calc::weekly_goal_minutes(&cfg);
    let left_week = calc::minutes_left_week(&data.entries, &cfg, today);

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
    let clear_debt_by = calc::leave_time_week(&data.entries, &cfg, today, now);
    println!(
        "Clear time debt by: {} (pay off the whole week today)",
        clear_debt_by.format("%H:%M")
    );
    let debt = calc::time_debt_minutes(&data.entries, &cfg, today);
    if debt > 0 {
        println!(
            "Time debt: {} (shortfall from earlier this week)",
            calc::format_hm(debt)
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
            e.end.format("%H:%M")
        );
    }
    Ok(())
}

fn cmd_edit(id: u64, range: Option<String>, date: Option<NaiveDate>) -> anyhow::Result<()> {
    let mut data = storage::load_entries()?;
    let idx = data
        .entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("no entry with id {id}"))?;

    let mut updated = data.entries[idx].clone();
    if let Some(date) = date {
        updated.date = date;
    }
    if let Some(range) = range {
        let (start, end) = parse::parse_range(&range).map_err(|e| anyhow::anyhow!(e))?;
        updated.start = start;
        updated.end = end;
    }

    if let Some(conflict) = data
        .entries
        .iter()
        .find(|e| e.id != id && e.date == updated.date && calc::overlaps(e, &updated))
    {
        anyhow::bail!(
            "range {}-{} overlaps existing entry #{} ({}-{})",
            updated.start.format("%H:%M"),
            updated.end.format("%H:%M"),
            conflict.id,
            conflict.start.format("%H:%M"),
            conflict.end.format("%H:%M"),
        );
    }

    data.entries[idx] = updated.clone();
    storage::save_entries(&data)?;
    println!(
        "updated #{}: {} {}-{}",
        updated.id,
        updated.date,
        updated.start.format("%H:%M"),
        updated.end.format("%H:%M")
    );
    Ok(())
}

fn cmd_delete(id: u64) -> anyhow::Result<()> {
    let mut data = storage::load_entries()?;
    let len_before = data.entries.len();
    data.entries.retain(|e| e.id != id);
    if data.entries.len() == len_before {
        anyhow::bail!("no entry with id {id}");
    }
    storage::save_entries(&data)?;
    println!("deleted #{id}");
    Ok(())
}
