use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, NaiveTime, Weekday};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::calc;
use crate::model::{Absence, Config, GoalPeriod, TimeEntry};
use crate::parse;
use crate::storage;

const BORDER: Color = Color::DarkGray;
const MUTED: Color = Color::Gray;
const LABEL: Color = Color::White;
const ACCENT: Color = Color::Cyan;
const DANGER: Color = Color::Red;
const SUCCESS: Color = Color::Green;

/// How long a footer status message (e.g. "saved") stays up before it's
/// cleared and the key-hint line reappears underneath.
const STATUS_DURATION: Duration = Duration::from_secs(2);

fn gray_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(title.into())
}

enum Mode {
    Normal,
    Modal(ModalState),
    ConfirmDelete(DeleteTarget),
}

/// What a confirmed delete removes - entries, goal periods, and absences
/// are all deleted through the same `y`/`n` confirm prompt, so this says
/// which list (and which id in it) to act on.
#[derive(Clone, Copy)]
enum DeleteTarget {
    Entry(u64),
    GoalPeriod(u64),
    Absence(u64),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum StatusKind {
    Info,
    Error,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum ModalKind {
    /// Full 3-field add/edit form (date, start, end). Leaving end blank
    /// starts an open-ended block instead of a closed one.
    AddEdit,
    /// Single time field: close the currently open block.
    End,
    /// Two-field form (date, hours) for adding/editing a goal period.
    GoalPeriod,
    /// Two-field form (date, reason) for adding/editing an absence - reached
    /// from the Add popup with Alt+a (see `App::toggle_add_mode`), reusing
    /// `date_buf`/`start_buf` (the latter repurposed as the reason field)
    /// rather than growing `ModalState` with absence-specific buffers.
    Absence,
}

struct ModalState {
    kind: ModalKind,
    editing_id: Option<u64>,
    date_buf: String,
    start_buf: String,
    end_buf: String,
    /// The single time field used by the End modal.
    time_buf: String,
    /// The hours field used by the GoalPeriod modal.
    hours_buf: String,
    focus: usize, // 0=date, 1=start, 2=end for AddEdit; always 0 for End; 0=date, 1=hours for GoalPeriod; 0=date, 1=reason for Absence
    /// True while the focused field's existing text hasn't been touched yet:
    /// it renders as selected, and the next keystroke overwrites it instead
    /// of appending (like a browser text input). Reset on every focus
    /// change; cleared by the first edit to the field.
    field_selected: bool,
    error: Option<String>,
}

/// Which of the two panes j/k currently navigates. Switched with Tab.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Pane {
    Days,
    Entries,
}

/// One selectable row in the focused day's entries panel: either a logged
/// time entry or the day's absence (at most one of the latter, shown first
/// - see `App::visible`). e/d act on whichever is selected.
#[derive(Clone)]
enum DaySelectable {
    Absence(Absence),
    Entry(TimeEntry),
}

/// Which top-level view is showing. Dashboard/History are toggled with
/// 'h'; Goals is reached via the Dashboard's "other" submenu ('o', then
/// 'g' - see `App::other_menu_open`) rather than its own top-level key.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Screen {
    Dashboard,
    History,
    Goals,
}

struct App {
    entries: Vec<TimeEntry>,
    goal_periods: Vec<GoalPeriod>,
    absences: Vec<Absence>,
    config: Config,
    /// The day whose entries show in the entries panel and whose row is
    /// marked in the week diagram. Defaults to today; moved with j/k while
    /// the Days pane has focus.
    focused_day: NaiveDate,
    /// Which pane Tab has focus on; determines what j/k moves.
    focus_pane: Pane,
    /// Index into the focused day's entries, for e/d and highlighting.
    selected: usize,
    mode: Mode,
    screen: Screen,
    /// Index (newest-first) of the selected row in the history view, for
    /// j/k and the week-diagram preview below it.
    history_selected: usize,
    /// Index (chronological) of the selected row in the Goals screen.
    goal_selected: usize,
    /// True while the Dashboard's "other" submenu is open - swaps the
    /// footer to that submenu's own keys (currently just 'g' for Goals)
    /// instead of advertising them in the main hint line.
    other_menu_open: bool,
    /// A transient footer message and when it was set, for `STATUS_DURATION`
    /// auto-dismissal - see `set_status`/`clear_expired_status`.
    status: Option<(StatusKind, String, Instant)>,
    quit: bool,
}

impl App {
    fn new() -> Result<Self> {
        let data = storage::load_entries()?;
        let config = storage::load_config()?;
        let mut app = App {
            entries: data.entries,
            goal_periods: data.goal_periods,
            absences: data.absences,
            config,
            focused_day: Local::now().date_naive(),
            focus_pane: Pane::Entries,
            selected: 0,
            mode: Mode::Normal,
            screen: Screen::Dashboard,
            history_selected: 0,
            goal_selected: 0,
            other_menu_open: false,
            status: None,
            quit: false,
        };
        app.snap_focused_day();
        Ok(app)
    }

    fn set_status(&mut self, kind: StatusKind, message: impl Into<String>) {
        self.status = Some((kind, message.into(), Instant::now()));
    }

    /// Clears the footer status once it's been up for `STATUS_DURATION`, so
    /// the key hints reappear on their own instead of staying hidden.
    fn clear_expired_status(&mut self) {
        if let Some((_, _, set_at)) = &self.status
            && set_at.elapsed() >= STATUS_DURATION
        {
            self.status = None;
        }
    }

    fn reload(&mut self) -> Result<()> {
        let data = storage::load_entries()?;
        self.entries = data.entries;
        self.goal_periods = data.goal_periods;
        self.absences = data.absences;
        self.config = storage::load_config()?;
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
        self.snap_focused_day();
        self.clamp_history_selection();
        self.clamp_goal_selection();
        Ok(())
    }

    /// True if `day` should be left out of the week diagram, day navigation,
    /// and the history view because `hide_weekends` is on and it's a
    /// Saturday or Sunday.
    fn hides(&self, day: NaiveDate) -> bool {
        self.config.hide_weekends && matches!(day.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Same as `hides`, but for a Mon=0..Sun=6 column index (the history
    /// view's day columns) rather than an actual date.
    fn hides_index(&self, i: usize) -> bool {
        self.config.hide_weekends && i >= 5
    }

    /// If `focused_day` is currently a hidden day (e.g. `hide_weekends` was
    /// just turned on, or the app started on a hidden weekend day), move it
    /// to the nearest visible day in the current week instead.
    fn snap_focused_day(&mut self) {
        if !self.hides(self.focused_day) {
            return;
        }
        if let Some(&nearest) = self
            .week_days()
            .iter()
            .min_by_key(|&&d| (d - self.focused_day).num_days().abs())
        {
            self.focused_day = nearest;
        }
    }

    fn today(&self) -> NaiveDate {
        Local::now().date_naive()
    }

    /// The focused day's selectable rows: its absence (if any, always
    /// first - a day has at most one) followed by its entries sorted by
    /// start time.
    fn visible(&self) -> Vec<DaySelectable> {
        let mut v = Vec::new();
        if let Some(a) = calc::absence_on(&self.absences, self.focused_day) {
            v.push(DaySelectable::Absence(a.clone()));
        }
        let mut entries = calc::entries_on(&self.entries, self.focused_day);
        entries.sort_by_key(|e| e.start);
        v.extend(entries.into_iter().cloned().map(DaySelectable::Entry));
        v
    }

    /// The days of `week_start`'s week (Monday-Sunday, or Monday-Friday if
    /// `hide_weekends` is on).
    fn days_in_week(&self, week_start: NaiveDate) -> Vec<NaiveDate> {
        (0..7)
            .map(|i| week_start + chrono::Duration::days(i))
            .filter(|&d| !self.hides(d))
            .collect()
    }

    /// The days of the current week, shown in the diagram and navigable
    /// with j/k.
    fn week_days(&self) -> Vec<NaiveDate> {
        let (week_start, _) = calc::week_bounds(self.today());
        self.days_in_week(week_start)
    }

    fn selected_item(&self) -> Option<DaySelectable> {
        self.visible().into_iter().nth(self.selected)
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected as i32 + delta;
        self.selected = cur.clamp(0, len as i32 - 1) as usize;
    }

    /// Moves the focused day within the current week's visible days
    /// (see `week_days`, clamped to its first/last) and resets entry
    /// selection, since the entry list underneath just changed. Called
    /// from j/k while the Days pane has focus.
    fn move_focused_day(&mut self, delta: i64) {
        let days = self.week_days();
        let idx = days
            .iter()
            .position(|&d| d == self.focused_day)
            .unwrap_or(0);
        let new_idx = (idx as i64 + delta).clamp(0, days.len() as i64 - 1) as usize;
        self.focused_day = days[new_idx];
        self.selected = 0;
    }

    /// Open the add-entry modal, prefilled with the focused day and now.
    /// Leaving the end field blank on submit starts an open-ended block
    /// instead of a closed entry.
    fn open_add(&mut self) {
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::AddEdit,
            editing_id: None,
            date_buf: self.focused_day.to_string(),
            start_buf: Local::now().time().format("%H:%M").to_string(),
            end_buf: String::new(),
            time_buf: String::new(),
            hours_buf: String::new(),
            focus: 1,
            field_selected: true,
            error: None,
        });
    }

    fn open_edit(&mut self) {
        match self.selected_item() {
            Some(DaySelectable::Entry(e)) => {
                // An in-progress entry already has its start filled in -
                // jump straight to the blank end field instead of the
                // start field.
                let focus = if e.end.is_none() { 2 } else { 1 };
                self.mode = Mode::Modal(ModalState {
                    kind: ModalKind::AddEdit,
                    editing_id: Some(e.id),
                    date_buf: e.date.to_string(),
                    start_buf: e.start.format("%H:%M").to_string(),
                    end_buf: e
                        .end
                        .map(|t| t.format("%H:%M").to_string())
                        .unwrap_or_default(),
                    time_buf: String::new(),
                    hours_buf: String::new(),
                    focus,
                    field_selected: true,
                    error: None,
                });
            }
            Some(DaySelectable::Absence(a)) => self.open_absence_edit(&a),
            None => {}
        }
    }

    /// Open the add-absence modal, prefilled with the focused day and a
    /// blank reason.
    fn open_absence_add(&mut self) {
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::Absence,
            editing_id: None,
            date_buf: self.focused_day.to_string(),
            start_buf: String::new(),
            end_buf: String::new(),
            time_buf: String::new(),
            hours_buf: String::new(),
            focus: 0,
            field_selected: true,
            error: None,
        });
    }

    fn open_absence_edit(&mut self, a: &Absence) {
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::Absence,
            editing_id: Some(a.id),
            date_buf: a.date.to_string(),
            start_buf: a.reason.clone().unwrap_or_default(),
            end_buf: String::new(),
            time_buf: String::new(),
            hours_buf: String::new(),
            focus: 0,
            field_selected: true,
            error: None,
        });
    }

    /// While adding (not editing - converting an existing record between
    /// kinds isn't supported), flips the Add popup between a normal time
    /// entry and an absence (Alt+a). Resets the modal fresh for the other
    /// kind rather than merging field text across kinds, since `start_buf`
    /// means different things in each (start time vs. reason).
    fn toggle_add_mode(&mut self) {
        let Mode::Modal(m) = &self.mode else {
            return;
        };
        if m.editing_id.is_some() {
            return;
        }
        match m.kind {
            ModalKind::AddEdit => self.open_absence_add(),
            ModalKind::Absence => self.open_add(),
            _ => {}
        }
    }

    /// Open the "finish the open block" modal, prefilled with now. Refuses
    /// if there is no open block.
    fn open_finish(&mut self) {
        if calc::open_entry(&self.entries).is_none() {
            self.set_status(
                StatusKind::Error,
                "no open block to finish - press a to start one",
            );
            return;
        }
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::End,
            editing_id: None,
            date_buf: String::new(),
            start_buf: String::new(),
            end_buf: String::new(),
            time_buf: Local::now().time().format("%H:%M").to_string(),
            hours_buf: String::new(),
            focus: 0,
            field_selected: true,
            error: None,
        });
    }

    /// The Goals screen's periods, sorted chronologically (oldest first) -
    /// unlike History's newest-first, this reads better as "how my goal
    /// evolved over time."
    fn visible_goal_periods(&self) -> Vec<&GoalPeriod> {
        let mut v: Vec<&GoalPeriod> = self.goal_periods.iter().collect();
        v.sort_by_key(|p| p.effective_from);
        v
    }

    fn selected_goal_period(&self) -> Option<GoalPeriod> {
        self.visible_goal_periods()
            .get(self.goal_selected)
            .map(|p| (*p).clone())
    }

    fn move_goal_selection(&mut self, delta: i32) {
        let len = self.visible_goal_periods().len();
        if len == 0 {
            self.goal_selected = 0;
            return;
        }
        let cur = self.goal_selected as i32 + delta;
        self.goal_selected = cur.clamp(0, len as i32 - 1) as usize;
    }

    fn clamp_goal_selection(&mut self) {
        let len = self.goal_periods.len();
        if len == 0 {
            self.goal_selected = 0;
        } else if self.goal_selected >= len {
            self.goal_selected = len - 1;
        }
    }

    /// Open the add-goal-period modal, prefilled with today.
    fn open_goal_add(&mut self) {
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::GoalPeriod,
            editing_id: None,
            date_buf: self.today().to_string(),
            start_buf: String::new(),
            end_buf: String::new(),
            time_buf: String::new(),
            hours_buf: String::new(),
            focus: 0,
            field_selected: true,
            error: None,
        });
    }

    fn open_goal_edit(&mut self) {
        if let Some(p) = self.selected_goal_period() {
            self.mode = Mode::Modal(ModalState {
                kind: ModalKind::GoalPeriod,
                editing_id: Some(p.id),
                date_buf: p.effective_from.to_string(),
                start_buf: String::new(),
                end_buf: String::new(),
                time_buf: String::new(),
                hours_buf: p.weekly_goal_hours.to_string(),
                focus: 0,
                field_selected: true,
                error: None,
            });
        }
    }

    fn submit_modal(&mut self) -> Result<()> {
        let kind = match &self.mode {
            Mode::Modal(m) => m.kind,
            _ => return Ok(()),
        };
        match kind {
            ModalKind::AddEdit => self.submit_add_edit(),
            ModalKind::End => self.submit_end(),
            ModalKind::GoalPeriod => self.submit_goal_period(),
            ModalKind::Absence => self.submit_absence(),
        }
    }

    /// Submits the add/edit modal. An empty end field means "still going" -
    /// it starts (or, when editing, re-opens) a block instead of closing
    /// one. An empty start field is always an error: a block needs a start
    /// before it can have an end.
    fn submit_add_edit(&mut self) -> Result<()> {
        let Mode::Modal(m) = &self.mode else {
            return Ok(());
        };

        if m.editing_id.is_none() && calc::open_entry(&self.entries).is_some() {
            self.set_modal_error("finish the open block first (f)".to_string());
            return Ok(());
        }

        let date = parse::resolve_day_spec(&m.date_buf, self.today()).or_else(|_| {
            NaiveDate::parse_from_str(&m.date_buf, "%Y-%m-%d")
                .map_err(|_| format!("invalid date '{}', expected YYYY-MM-DD", m.date_buf))
        });
        let date = match date {
            Ok(d) => d,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };

        if m.start_buf.trim().is_empty() {
            self.set_modal_error("start time is required".to_string());
            return Ok(());
        }
        let start = match parse::parse_time(&m.start_buf) {
            Ok(s) => s,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };
        let end = if m.end_buf.trim().is_empty() {
            None
        } else {
            match parse::parse_time(&m.end_buf) {
                Ok(e) => Some(e),
                Err(e) => {
                    self.set_modal_error(e);
                    return Ok(());
                }
            }
        };
        if let Some(end) = end
            && start >= end
        {
            self.set_modal_error("start must be before end".to_string());
            return Ok(());
        }

        let editing_id = m.editing_id;
        let candidate = TimeEntry {
            id: editing_id.unwrap_or(0),
            date,
            start,
            end,
        };

        let result = storage::with_entries(|data| {
            if calc::is_absent(&data.absences, candidate.date) {
                anyhow::bail!(
                    "{} is marked absent - delete the absence first",
                    candidate.date
                );
            }
            if let Some(conflict) = data.entries.iter().find(|e| {
                e.date == candidate.date
                    && Some(e.id) != editing_id
                    && calc::overlaps(e, &candidate)
            }) {
                anyhow::bail!(
                    "overlaps existing entry #{} ({}-{})",
                    conflict.id,
                    conflict.start.format("%H:%M"),
                    calc::format_end(conflict.end)
                );
            }

            if let Some(id) = editing_id {
                if let Some(existing) = data.entries.iter_mut().find(|e| e.id == id) {
                    existing.date = date;
                    existing.start = start;
                    existing.end = end;
                }
            } else {
                let id = data.next_id;
                data.next_id += 1;
                data.entries.push(TimeEntry {
                    id,
                    date,
                    start,
                    end,
                });
            }
            Ok(data.entries.clone())
        });

        match result {
            Ok(entries) => {
                self.entries = entries;
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "saved");
            }
            Err(e) => self.set_modal_error(e.to_string()),
        }
        Ok(())
    }

    fn submit_end(&mut self) -> Result<()> {
        let Mode::Modal(m) = &self.mode else {
            return Ok(());
        };
        let time = match parse::parse_time(&m.time_buf) {
            Ok(t) => t,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };
        match close_open_block(time) {
            Ok(Some((entries, start))) => {
                self.entries = entries;
                self.mode = Mode::Normal;
                self.set_status(
                    StatusKind::Info,
                    format!(
                        "finished {}-{}",
                        start.format("%H:%M"),
                        time.format("%H:%M")
                    ),
                );
            }
            Ok(None) => self.set_modal_error("no open block".to_string()),
            Err(e) => self.set_modal_error(e.to_string()),
        }
        Ok(())
    }

    /// "I'm done for the day": closes the open block at `now` (if any -
    /// a no-op otherwise, not an error), then reports a one-line
    /// end-of-day summary - hours worked vs target, and the resulting
    /// time debt/surplus, which `done` doesn't compute or store anything
    /// new for, it's already always live-derived from entries.
    fn clock_out(&mut self) -> Result<()> {
        let now = Local::now().time();
        match close_open_block(now) {
            Ok(Some((entries, _start))) => self.entries = entries,
            Ok(None) => {}
            Err(e) => {
                self.set_status(StatusKind::Error, e.to_string());
                return Ok(());
            }
        }

        let today = self.today();
        let worked_today =
            calc::worked_minutes(&calc::entries_on(&self.entries, today), today, now);
        let target_today = calc::daily_target_minutes(&self.goal_periods, &self.absences, today);
        let debt = calc::time_debt_minutes(
            &self.entries,
            &self.goal_periods,
            &self.absences,
            today,
            now,
        );
        let tail = if debt > 0 {
            format!(", time debt {}", calc::format_hm(debt))
        } else {
            let surplus = calc::time_surplus_minutes(
                &self.entries,
                &self.goal_periods,
                &self.absences,
                today,
                now,
            );
            if surplus > 0 {
                format!(", -{} debt", calc::format_hm(surplus))
            } else {
                String::new()
            }
        };
        let summary = if target_today == 0 {
            if calc::is_absent(&self.absences, today) {
                "clocked out - absent today".to_string()
            } else {
                format!(
                    "clocked out - logged {} (no target today)",
                    calc::format_hm(worked_today)
                )
            }
        } else {
            format!(
                "clocked out - {} / {} today{tail}",
                calc::format_hm(worked_today),
                calc::format_hm(target_today),
            )
        };
        self.set_status(StatusKind::Info, summary);
        Ok(())
    }

    /// Submits the goal-period modal: parses the date (same relative/ISO
    /// fallback chain as `submit_add_edit`) and hours, rejects a duplicate
    /// `effective_from` (`calc::goal_period_date_conflicts`), then either
    /// updates the period being edited or appends a new one.
    fn submit_goal_period(&mut self) -> Result<()> {
        let Mode::Modal(m) = &self.mode else {
            return Ok(());
        };

        let date = parse::resolve_day_spec(&m.date_buf, self.today()).or_else(|_| {
            NaiveDate::parse_from_str(&m.date_buf, "%Y-%m-%d")
                .map_err(|_| format!("invalid date '{}', expected YYYY-MM-DD", m.date_buf))
        });
        let date = match date {
            Ok(d) => d,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };

        let hours: f64 = match m.hours_buf.trim().parse() {
            Ok(h) if h > 0.0 => h,
            _ => {
                self.set_modal_error("hours must be a positive number".to_string());
                return Ok(());
            }
        };

        let editing_id = m.editing_id;
        let result = storage::with_entries(|data| {
            if calc::goal_period_date_conflicts(&data.goal_periods, date, editing_id) {
                anyhow::bail!("a goal period already starts {date}");
            }
            if let Some(id) = editing_id {
                if let Some(existing) = data.goal_periods.iter_mut().find(|p| p.id == id) {
                    existing.effective_from = date;
                    existing.weekly_goal_hours = hours;
                }
            } else {
                let id = data.next_id;
                data.next_id += 1;
                data.goal_periods.push(GoalPeriod {
                    id,
                    effective_from: date,
                    weekly_goal_hours: hours,
                });
            }
            Ok(data.goal_periods.clone())
        });

        match result {
            Ok(goal_periods) => {
                self.goal_periods = goal_periods;
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "saved");
            }
            Err(e) => self.set_modal_error(e.to_string()),
        }
        Ok(())
    }

    /// Submits the absence modal: parses the date (same relative/ISO
    /// fallback chain as `submit_add_edit`), takes the reason field as-is
    /// (blank -> `None`), rejects a duplicate date
    /// (`calc::absence_date_conflicts`) or one that already has logged
    /// entries, then either updates the absence being edited or appends a
    /// new one.
    fn submit_absence(&mut self) -> Result<()> {
        let Mode::Modal(m) = &self.mode else {
            return Ok(());
        };

        let date = parse::resolve_day_spec(&m.date_buf, self.today()).or_else(|_| {
            NaiveDate::parse_from_str(&m.date_buf, "%Y-%m-%d")
                .map_err(|_| format!("invalid date '{}', expected YYYY-MM-DD", m.date_buf))
        });
        let date = match date {
            Ok(d) => d,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };

        let reason = m.start_buf.trim();
        let reason = if reason.is_empty() {
            None
        } else {
            Some(reason.to_string())
        };

        let editing_id = m.editing_id;
        let result = storage::with_entries(|data| {
            if calc::absence_date_conflicts(&data.absences, date, editing_id) {
                anyhow::bail!("{date} is already marked absent");
            }
            if !calc::entries_on(&data.entries, date).is_empty() {
                anyhow::bail!("{date} already has logged entries");
            }
            if let Some(id) = editing_id {
                if let Some(existing) = data.absences.iter_mut().find(|a| a.id == id) {
                    existing.date = date;
                    existing.reason = reason.clone();
                }
            } else {
                let id = data.next_id;
                data.next_id += 1;
                data.absences.push(Absence {
                    id,
                    date,
                    reason: reason.clone(),
                });
            }
            Ok(data.absences.clone())
        });

        match result {
            Ok(absences) => {
                self.absences = absences;
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "saved");
            }
            Err(e) => self.set_modal_error(e.to_string()),
        }
        Ok(())
    }

    fn set_modal_error(&mut self, err: String) {
        if let Mode::Modal(m) = &mut self.mode {
            m.error = Some(err);
        }
    }

    fn request_delete(&mut self) {
        match self.selected_item() {
            Some(DaySelectable::Entry(e)) => {
                self.mode = Mode::ConfirmDelete(DeleteTarget::Entry(e.id));
            }
            Some(DaySelectable::Absence(a)) => {
                self.mode = Mode::ConfirmDelete(DeleteTarget::Absence(a.id));
            }
            None => {}
        }
    }

    fn request_goal_delete(&mut self) {
        if let Some(p) = self.selected_goal_period() {
            self.mode = Mode::ConfirmDelete(DeleteTarget::GoalPeriod(p.id));
        }
    }

    fn confirm_delete(&mut self, target: DeleteTarget) -> Result<()> {
        match target {
            DeleteTarget::Entry(id) => {
                let entries = storage::with_entries(|data| {
                    data.entries.retain(|e| e.id != id);
                    Ok(data.entries.clone())
                })?;
                self.entries = entries;
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, format!("deleted #{id}"));
            }
            DeleteTarget::GoalPeriod(id) => {
                let result = storage::with_entries(|data| {
                    if data.goal_periods.len() <= 1 {
                        anyhow::bail!("can't delete the only remaining goal period");
                    }
                    data.goal_periods.retain(|p| p.id != id);
                    Ok(data.goal_periods.clone())
                });
                self.mode = Mode::Normal;
                match result {
                    Ok(goal_periods) => {
                        self.goal_periods = goal_periods;
                        self.set_status(StatusKind::Info, format!("deleted goal #{id}"));
                    }
                    Err(e) => self.set_status(StatusKind::Error, e.to_string()),
                }
            }
            DeleteTarget::Absence(id) => {
                let absences = storage::with_entries(|data| {
                    data.absences.retain(|a| a.id != id);
                    Ok(data.absences.clone())
                })?;
                self.absences = absences;
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, format!("deleted absence #{id}"));
            }
        }
        Ok(())
    }

    /// Number of rows the history view currently has to show.
    fn history_row_count(&self) -> usize {
        calc::history_weeks(
            &self.entries,
            &self.goal_periods,
            &self.absences,
            self.today(),
            Local::now().time(),
        )
        .len()
    }

    fn clamp_history_selection(&mut self) {
        let max = self.history_row_count().saturating_sub(1);
        self.history_selected = self.history_selected.min(max);
    }

    /// Moves the history view's row selection by `delta` (positive =
    /// later/down), clamped to the weeks there are to show. `draw_history`
    /// scrolls the list to keep the selection visible.
    fn move_history_selection(&mut self, delta: i64) {
        let max = self.history_row_count().saturating_sub(1) as i64;
        self.history_selected = (self.history_selected as i64 + delta).clamp(0, max) as usize;
    }
}

/// Closes the open block (if any) at `time`, persisting via
/// `storage::with_entries`. Returns the updated entries and the closed
/// block's start, or `Ok(None)` if there was nothing open to close - not
/// an error, since `clock_out` treats "already clocked out" as fine.
/// Shared by `submit_end` (via the End modal) and `App::clock_out` (no
/// modal, always `now`), so both apply the same validation.
fn close_open_block(time: NaiveTime) -> Result<Option<(Vec<TimeEntry>, NaiveTime)>> {
    storage::with_entries(|data| {
        let Some(idx) = data.entries.iter().position(|e| e.end.is_none()) else {
            return Ok(None);
        };
        let (id, date, start) = (
            data.entries[idx].id,
            data.entries[idx].date,
            data.entries[idx].start,
        );
        if time <= start {
            anyhow::bail!("end must be after start");
        }
        let candidate = TimeEntry {
            id,
            date,
            start,
            end: Some(time),
        };
        if let Some(conflict) = data
            .entries
            .iter()
            .find(|e| e.id != id && e.date == date && calc::overlaps(e, &candidate))
        {
            anyhow::bail!(
                "overlaps existing entry #{} ({}-{})",
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end)
            );
        }

        data.entries[idx].end = Some(time);
        Ok(Some((data.entries.clone(), start)))
    })
}

pub fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new()?;

    while !app.quit {
        app.clear_expired_status();
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                handle_key(&mut app, key.code, key.modifiers)?;
            }
        } else {
            app.reload()?;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    match &mut app.mode {
        Mode::Normal => match app.screen {
            Screen::Dashboard if app.other_menu_open => match code {
                KeyCode::Char('g') => {
                    app.other_menu_open = false;
                    app.screen = Screen::Goals;
                }
                KeyCode::Esc => app.other_menu_open = false,
                _ => {}
            },
            Screen::Dashboard => match code {
                KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
                KeyCode::Char('a') => app.open_add(),
                KeyCode::Char('f') => app.open_finish(),
                KeyCode::Char('c') => app.clock_out()?,
                KeyCode::Char('e') if app.focus_pane == Pane::Entries => app.open_edit(),
                KeyCode::Char('d') if app.focus_pane == Pane::Entries => app.request_delete(),
                KeyCode::Char('r') => app.reload()?,
                KeyCode::Char('h') => app.screen = Screen::History,
                // The submenu's own keys (currently just 'g' for Goals)
                // aren't in draw_footer's hints - only "other" is, until
                // this is pressed.
                KeyCode::Char('o') => app.other_menu_open = true,
                KeyCode::Tab => {
                    app.focus_pane = match app.focus_pane {
                        Pane::Days => Pane::Entries,
                        Pane::Entries => Pane::Days,
                    };
                }
                KeyCode::Char('j') | KeyCode::Down => match app.focus_pane {
                    Pane::Days => app.move_focused_day(1),
                    Pane::Entries => app.move_selection(1),
                },
                KeyCode::Char('k') | KeyCode::Up => match app.focus_pane {
                    Pane::Days => app.move_focused_day(-1),
                    Pane::Entries => app.move_selection(-1),
                },
                _ => {}
            },
            Screen::History => match code {
                KeyCode::Char('q') => app.quit = true,
                KeyCode::Char('h') | KeyCode::Esc => app.screen = Screen::Dashboard,
                KeyCode::Char('r') => app.reload()?,
                KeyCode::Char('j') | KeyCode::Down => app.move_history_selection(1),
                KeyCode::Char('k') | KeyCode::Up => app.move_history_selection(-1),
                _ => {}
            },
            Screen::Goals => match code {
                KeyCode::Char('q') => app.quit = true,
                KeyCode::Char('g') | KeyCode::Esc => app.screen = Screen::Dashboard,
                KeyCode::Char('a') => app.open_goal_add(),
                KeyCode::Char('e') => app.open_goal_edit(),
                KeyCode::Char('d') => app.request_goal_delete(),
                KeyCode::Char('r') => app.reload()?,
                KeyCode::Char('j') | KeyCode::Down => app.move_goal_selection(1),
                KeyCode::Char('k') | KeyCode::Up => app.move_goal_selection(-1),
                _ => {}
            },
        },
        Mode::ConfirmDelete(target) => {
            let target = *target;
            match code {
                KeyCode::Char('y') => app.confirm_delete(target)?,
                KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
                _ => {}
            }
        }
        Mode::Modal(m) => {
            let field_count = match m.kind {
                ModalKind::AddEdit => 3,
                ModalKind::End => 1,
                ModalKind::GoalPeriod => 2,
                ModalKind::Absence => 2,
            };
            match code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => app.submit_modal()?,
                KeyCode::Char('a') if modifiers.contains(KeyModifiers::ALT) => {
                    app.toggle_add_mode();
                }
                KeyCode::Tab | KeyCode::Down => {
                    m.focus = (m.focus + 1) % field_count;
                    m.field_selected = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    m.focus = (m.focus + field_count - 1) % field_count;
                    m.field_selected = true;
                }
                KeyCode::Backspace => {
                    m.error = None;
                    if m.field_selected {
                        current_buf(m).clear();
                        m.field_selected = false;
                    } else {
                        current_buf(m).pop();
                    }
                }
                KeyCode::Char(c) => {
                    m.error = None;
                    if m.field_selected {
                        current_buf(m).clear();
                        m.field_selected = false;
                    }
                    current_buf(m).push(c);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn current_buf(m: &mut ModalState) -> &mut String {
    match m.kind {
        ModalKind::AddEdit => match m.focus {
            0 => &mut m.date_buf,
            1 => &mut m.start_buf,
            _ => &mut m.end_buf,
        },
        ModalKind::End => &mut m.time_buf,
        ModalKind::GoalPeriod => match m.focus {
            0 => &mut m.date_buf,
            _ => &mut m.hours_buf,
        },
        ModalKind::Absence => match m.focus {
            0 => &mut m.date_buf,
            _ => &mut m.start_buf,
        },
    }
}

fn draw(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::Dashboard => draw_dashboard(f, app),
        Screen::History => draw_history_screen(f, app),
        Screen::Goals => draw_goals_screen(f, app),
    }

    match &app.mode {
        Mode::Modal(m) => draw_modal(f, m),
        Mode::ConfirmDelete(target) => draw_confirm(f, *target),
        Mode::Normal => {}
    }
}

fn draw_dashboard(f: &mut Frame, app: &App) {
    let now = Local::now().time();
    let debt = calc::time_debt_minutes(
        &app.entries,
        &app.goal_periods,
        &app.absences,
        app.today(),
        now,
    );
    let surplus = calc::time_surplus_minutes(
        &app.entries,
        &app.goal_periods,
        &app.absences,
        app.today(),
        now,
    );
    let stats_height = if debt > 0 || surplus > 0 { 5 } else { 4 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(stats_height),
            Constraint::Length(9),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1], debt, surplus);
    draw_week_diagram(f, app, chunks[2]);
    draw_day_entries(f, app, chunks[3]);
    draw_footer(f, app, chunks[4]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let today = app.today();
    let now = Local::now().time();
    let target_today = calc::daily_target_minutes(&app.goal_periods, &app.absences, today);
    let spans = if target_today == 0 {
        if let Some(absence) = calc::absence_on(&app.absences, today) {
            let tail = absence
                .reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            vec![Span::styled(
                format!("Absent today{tail}"),
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            )]
        } else {
            let worked = calc::worked_minutes(&calc::entries_on(&app.entries, today), today, now);
            vec![
                Span::styled(
                    "No target today — logged ",
                    Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    calc::format_hm(worked),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
            ]
        }
    } else if let Some(clocked_out) = calc::clocked_out_at(&app.entries, today) {
        vec![
            Span::styled(
                "Clocked out at ",
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                clocked_out.format("%H:%M").to_string(),
                Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        let leave = calc::leave_time(
            &app.entries,
            &app.goal_periods,
            &app.absences,
            &app.config,
            today,
            now,
        );
        vec![
            Span::styled(
                "Leave at ",
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                leave.format("%H:%M").to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]
    };
    let block = gray_block(format!(" {} ({}) ", today, today.format("%A")));
    let p = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(p, area);
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect, debt: i64, surplus: i64) {
    let today = app.today();
    let now = Local::now().time();
    let worked_today = calc::worked_minutes(&calc::entries_on(&app.entries, today), today, now);
    let target_today = calc::daily_target_minutes(&app.goal_periods, &app.absences, today);
    let left_today =
        calc::minutes_left_today(&app.entries, &app.goal_periods, &app.absences, today, now);

    let (week_start, _) = calc::week_bounds(today);
    let worked_week =
        calc::worked_minutes(&calc::entries_in_week(&app.entries, week_start), today, now);
    let goal_week =
        calc::weekly_goal_minutes_for_week(&app.goal_periods, &app.absences, week_start);
    let left_week =
        calc::minutes_left_week(&app.entries, &app.goal_periods, &app.absences, today, now);
    let clear_debt_by = calc::leave_time_debt(
        &app.entries,
        &app.goal_periods,
        &app.absences,
        &app.config,
        today,
        now,
    );

    let label = Style::default().fg(LABEL);
    let value = Style::default().add_modifier(Modifier::BOLD);

    let stat_line = |prefix: &str, worked: i64, of: &str, total: i64, remaining: i64| {
        let left_color = if remaining == 0 { SUCCESS } else { ACCENT };
        Line::from(vec![
            Span::styled(prefix.to_string(), label),
            Span::styled(calc::format_hm(worked), value),
            Span::styled(format!(" / {} {of} ", calc::format_hm(total)), label),
            Span::styled(
                format!("({} left)", calc::format_hm(remaining)),
                Style::default().fg(left_color).add_modifier(Modifier::BOLD),
            ),
        ])
    };

    let mut lines = vec![
        stat_line(
            "Today:     ",
            worked_today,
            "target",
            target_today,
            left_today,
        ),
        stat_line("This week: ", worked_week, "goal", goal_week, left_week),
    ];
    if debt > 0 {
        lines.push(Line::from(vec![
            Span::styled("Time debt: ", label),
            Span::styled(
                calc::format_hm(debt),
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · clear by ", label),
            Span::styled(
                clear_debt_by.format("%H:%M").to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]));
    } else if surplus > 0 {
        lines.push(Line::from(vec![
            Span::styled("Time debt: ", label),
            Span::styled(
                format!("-{}", calc::format_hm(surplus)),
                Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    let p = Paragraph::new(lines).block(gray_block(""));
    f.render_widget(p, area);
}

/// The week diagram: one row per day of the current week, always shown -
/// h/l moves which day is focused (marked on its prefix).
fn draw_week_diagram(f: &mut Frame, app: &App, area: Rect) {
    // Matches Block::inner() for a plain Borders::ALL block: 1 column each side.
    let inner_width = area.width.saturating_sub(2);
    let (week_start, _) = calc::week_bounds(app.today());
    let lines = week_lines(app, week_start, inner_width);
    let p = Paragraph::new(lines).block(gray_block(" This week "));
    f.render_widget(p, area);
}

/// One row in the entries panel: either a real, selectable entry or a
/// purely descriptive gap/break, ordered chronologically by start time.
enum DayItem<'a> {
    Entry(&'a TimeEntry),
    Gap(NaiveTime, NaiveTime),
}

/// The entries panel: the focused day's entries and gaps. j/k moves
/// selection within this list, for e/d.
fn draw_day_entries(f: &mut Frame, app: &App, area: Rect) {
    let day = app.focused_day;
    let entries = calc::entries_on(&app.entries, day);
    let gaps = calc::today_gaps(&app.entries, day);

    let mut combined: Vec<DayItem> = entries.into_iter().map(DayItem::Entry).collect();
    combined.extend(
        gaps.into_iter()
            .map(|(start, end)| DayItem::Gap(start, end)),
    );
    combined.sort_by_key(|item| match item {
        DayItem::Entry(e) => e.start,
        DayItem::Gap(start, _) => *start,
    });

    // app.selected indexes app.visible() (the day's absence, if any, then
    // its entries - see DaySelectable); track which row of this combined,
    // gap-interleaved list that lands on so the highlight stays on the
    // right one.
    let mut items: Vec<ListItem> = Vec::new();
    let mut select_index = None;
    let mut selectable_count = 0;
    if let Some(a) = calc::absence_on(&app.absences, day) {
        if selectable_count == app.selected {
            select_index = Some(items.len());
        }
        selectable_count += 1;
        let tail = a
            .reason
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        items.push(ListItem::new(Span::styled(
            format!("Absent{tail}"),
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    for item in &combined {
        match item {
            DayItem::Entry(e) => {
                if selectable_count == app.selected {
                    select_index = Some(items.len());
                }
                selectable_count += 1;
                let range_style = if e.end.is_none() {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                items.push(ListItem::new(Span::styled(
                    format!("{}-{}", e.start.format("%H:%M"), calc::format_end(e.end)),
                    range_style,
                )));
            }
            DayItem::Gap(start, end) => {
                items.push(ListItem::new(Span::styled(
                    format!("{}-{} (break)", start.format("%H:%M"), end.format("%H:%M")),
                    Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }
    let no_entries = items.is_empty();
    let items = if no_entries {
        vec![ListItem::new(Span::styled(
            "no entries",
            Style::default().fg(MUTED),
        ))]
    } else {
        items
    };

    let mut state = ListState::default();
    if no_entries {
        // Highlight the placeholder itself, so it's clear this pane (not
        // the week diagram) has focus even when there's nothing to select.
        state.select(Some(0));
    } else if selectable_count > 0 {
        state.select(select_index);
    }

    // Dimmed (bold only, no reverse) while the Days pane has focus instead
    // of this one, so it's clear j/k won't move this selection right now.
    let highlight_style = if app.focus_pane == Pane::Entries {
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let title = format!(" {} ({}) ", day, day.format("%A"));
    let list = List::new(items)
        .block(gray_block(title))
        .highlight_style(highlight_style);
    f.render_stateful_widget(list, area, &mut state);
}

/// One line per day of `week_start`'s week: weekday name, total hours, and
/// a bar showing where in the day the logged time falls (all rows share
/// the same time-of-day scale so they're visually comparable). Used both
/// for the dashboard's always-current-week diagram and the history view's
/// preview of whichever week is selected there.
fn week_lines(app: &App, week_start: NaiveDate, inner_width: u16) -> Vec<Line<'static>> {
    let today = app.today();
    let now = Local::now().time();
    let (window_start, window_end) = day_window(&app.entries, week_start, now);

    app.days_in_week(week_start)
        .into_iter()
        .map(|day| {
            let day_entries = calc::entries_on(&app.entries, day);
            let total = calc::worked_minutes(&day_entries, today, now);
            let absent = calc::is_absent(&app.absences, day);
            let prefix = format!("{:<9} {:>7}  ", day.format("%A"), calc::format_hm(total));
            let bar_width = inner_width.saturating_sub(prefix.chars().count() as u16);
            let cells = day_bar(
                &day_entries,
                (window_start, window_end),
                bar_width as usize,
                day,
                today,
                now,
                absent,
            );

            // Mark the focused row on its prefix only - inverting the whole
            // line would make the bar's own colors hard to read. Dimmed
            // (bold only, no reverse) while the Entries pane has focus
            // instead of this one, so it's clear j/k won't move this row
            // right now. A future, unfocused day (or one marked absent) is
            // grayed out - there's nothing worked to show there.
            let prefix_style = if day == app.focused_day {
                let base = Style::default().fg(LABEL).add_modifier(Modifier::BOLD);
                if app.focus_pane == Pane::Days {
                    base.add_modifier(Modifier::REVERSED)
                } else {
                    base
                }
            } else if day > today || absent {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(LABEL)
            };
            let mut spans = vec![Span::styled(prefix, prefix_style)];
            spans.extend(bar_spans(&cells));
            Line::from(spans)
        })
        .collect()
}

/// The shared time-of-day window used to draw week-view bars: a sensible
/// default workday, widened to cover any entry that falls outside it (and,
/// if a block is still open, up to now so its in-progress span is visible).
fn day_window(
    entries: &[TimeEntry],
    week_start: NaiveDate,
    now: NaiveTime,
) -> (NaiveTime, NaiveTime) {
    let default_start = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let default_end = NaiveTime::from_hms_opt(20, 0, 0).unwrap();
    let week_entries = calc::entries_in_week(entries, week_start);
    let start = week_entries
        .iter()
        .map(|e| e.start)
        .min()
        .unwrap_or(default_start)
        .min(default_start);
    let mut end = week_entries
        .iter()
        .filter_map(|e| e.end)
        .max()
        .unwrap_or(default_end)
        .max(default_end);
    if week_entries.iter().any(|e| e.end.is_none()) {
        end = end.max(now);
    }
    (start, end)
}

/// Glyphs for the trailing edge of an open block's elapsed span, ending
/// exactly at its current edge (the freshest, still-provisional minutes);
/// everything earlier in that span renders as plain solid `Cell::Worked`,
/// same as a closed entry.
const FADE: [char; 3] = ['▓', '▒', '░'];

#[derive(PartialEq, Eq, Clone, Copy)]
enum Cell {
    Empty,
    Worked,
    /// One of the last few columns of an open block's elapsed span. The
    /// index selects a glyph from `FADE` (higher = closer to now/fainter).
    Open(usize),
    /// The whole day is marked absent - fills the bar instead of showing
    /// empty dots, so it reads as "blocked off" rather than "nothing
    /// logged yet".
    Absent,
}

/// Rasterizes a day's entries across `width` columns spanning
/// [window_start, window_end). An open block (no end yet) is assumed
/// worked from its start through its current edge - `now` if `day` is
/// `today`, or the window's end otherwise (no better bound for a block
/// left open on a past day) - filled solid like a closed entry, except the
/// last few columns fade out (see `FADE`) so it still reads as ongoing
/// rather than definitively closed.
fn day_bar(
    entries: &[&TimeEntry],
    window: (NaiveTime, NaiveTime),
    width: usize,
    day: NaiveDate,
    today: NaiveDate,
    now: NaiveTime,
    absent: bool,
) -> Vec<Cell> {
    let (window_start, window_end) = window;
    if width == 0 {
        return Vec::new();
    }
    if absent {
        return vec![Cell::Absent; width];
    }
    let total_minutes = (window_end - window_start).num_minutes().max(1);
    let col_time = |c: usize| {
        let offset = (c as i64 * total_minutes) / width as i64;
        window_start + chrono::Duration::minutes(offset)
    };
    // Half-open [start, end) time span covered by column `c`. Used instead
    // of a single sampled point so a span narrower than one column's time
    // resolution (e.g. a block started a minute ago on a wide window) still
    // overlaps - and lights up - the column it falls in, rather than only
    // appearing once elapsed time crosses the next column boundary.
    let col_range = |c: usize| (col_time(c), col_time(c + 1));
    let is_closed_worked = |c: usize| {
        let (col_start, col_end) = col_range(c);
        entries.iter().any(|e| {
            e.end
                .is_some_and(|end| e.start < col_end && col_start < end)
        })
    };

    let open = entries.iter().find(|e| e.end.is_none());
    let open_end = open.map(|_| if day == today { now } else { window_end });
    let is_open_worked = |c: usize| {
        let (col_start, col_end) = col_range(c);
        open.zip(open_end)
            .is_some_and(|(o, oe)| o.start < col_end && col_start < oe)
    };

    let mut cells: Vec<Cell> = (0..width)
        .map(|c| {
            if is_closed_worked(c) || is_open_worked(c) {
                Cell::Worked
            } else {
                Cell::Empty
            }
        })
        .collect();

    // The fade's last glyph must land on the same column the loop above
    // already marked as the open block's final worked column - so find it
    // by the identical predicate, rather than re-deriving a column from a
    // time offset (that inverse isn't exact under integer truncation and
    // could land one column short, leaving a stray solid cell past the fade).
    if let Some(boundary) = (0..width).rev().find(|&c| is_open_worked(c)) {
        for stage in 0..FADE.len() {
            let back = FADE.len() - 1 - stage;
            if let Some(c) = boundary.checked_sub(back)
                && !is_closed_worked(c)
            {
                cells[c] = Cell::Open(stage);
            }
        }
    }
    cells
}

fn bar_spans(cells: &[Cell]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let value = cells[i];
        let mut j = i + 1;
        while j < cells.len() && cells[j] == value {
            j += 1;
        }
        let (ch, style) = match value {
            Cell::Worked => ('█', Style::default().fg(ACCENT)),
            Cell::Open(stage) => (FADE[stage], Style::default().fg(ACCENT)),
            Cell::Empty => ('·', Style::default().fg(MUTED)),
            Cell::Absent => ('▒', Style::default().fg(MUTED)),
        };
        spans.push(Span::styled(ch.to_string().repeat(j - i), style));
        i = j;
    }
    spans
}

/// The footer's hint line while a modal is open - replaces the screen's
/// normal hints with ones relevant to the popup, since the popup itself no
/// longer renders its own (too narrow to fit them all legibly).
fn modal_footer_hints(m: &ModalState) -> Vec<(&'static str, &'static str)> {
    let mut hints = Vec::new();
    if m.kind != ModalKind::End {
        hints.push(("Tab", "next field"));
    }
    hints.push(("Enter", "save"));
    hints.push(("Esc", "cancel"));
    // Converting an existing record between kinds isn't supported - see
    // `App::toggle_add_mode` - so the toggle is only offered while adding.
    match m.kind {
        ModalKind::AddEdit if m.editing_id.is_none() => hints.push(("Alt+a", "add absence")),
        ModalKind::Absence if m.editing_id.is_none() => hints.push(("Alt+a", "back to entry")),
        _ => {}
    }
    hints
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Mode::Modal(m) = &app.mode {
        f.render_widget(Paragraph::new(hint_line(&modal_footer_hints(m))), area);
        return;
    }
    let p = match &app.status {
        Some((kind, s, _)) => {
            let color = if *kind == StatusKind::Error {
                DANGER
            } else {
                SUCCESS
            };
            Paragraph::new(Span::styled(
                s.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
        }
        None if app.other_menu_open => Paragraph::new(hint_line(&[("g", "goal"), ("Esc", "back")])),
        None => {
            let mut hints: Vec<(&str, &str)> = vec![("q", "quit"), ("a", "add")];
            // Only worth advertising once there's actually a block to
            // finish - pressing it otherwise already shows a clear error.
            if calc::open_entry(&app.entries).is_some() {
                hints.push(("f", "finish block"));
            }
            hints.extend([
                ("c", "clock out"),
                ("e", "edit"),
                ("d", "delete"),
                ("Tab", "switch pane"),
                ("j/k", "move"),
                ("h", "history"),
                ("o", "other"),
                ("r", "reload"),
            ]);
            Paragraph::new(hint_line(&hints))
        }
    };
    f.render_widget(p, area);
}

/// Renders "key description | key description | ...", the key in LABEL
/// (brighter) and the rest in MUTED, so the keys themselves stand out.
fn hint_line(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(MUTED)));
        }
        spans.push(Span::styled(key.to_string(), Style::default().fg(LABEL)));
        spans.push(Span::styled(format!(" {desc}"), Style::default().fg(MUTED)));
    }
    Line::from(spans)
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Renders a modal input field. When focused, shows either the existing
/// text as "selected" (about to be overwritten by the next keystroke) or,
/// once edited, plain text with a block cursor trailing it - mirroring how
/// a browser text input behaves.
fn field_line(label: &str, value: &str, focused: bool, field_selected: bool) -> Line<'static> {
    let label_span = Span::styled(format!("{label}: "), Style::default().fg(LABEL));
    if !focused {
        return Line::from(vec![label_span, Span::raw(value.to_string())]);
    }
    let cursor_style = Style::default().fg(Color::Black).bg(ACCENT);
    if field_selected {
        let shown = if value.is_empty() { " " } else { value };
        Line::from(vec![
            label_span,
            Span::styled(shown.to_string(), cursor_style.add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(vec![
            label_span,
            Span::raw(value.to_string()),
            Span::styled(" ", cursor_style),
        ])
    }
}

fn draw_modal(f: &mut Frame, m: &ModalState) {
    match m.kind {
        ModalKind::AddEdit => draw_add_edit_modal(f, m),
        ModalKind::End => draw_time_modal(f, m, " Finish block ", "End time"),
        ModalKind::GoalPeriod => draw_goal_period_modal(f, m),
        ModalKind::Absence => draw_absence_modal(f, m),
    }
}

fn draw_add_edit_modal(f: &mut Frame, m: &ModalState) {
    let area = centered_rect(50, 6, f.area());
    f.render_widget(Clear, area);

    let title = if m.editing_id.is_some() {
        " Edit entry "
    } else {
        " Add entry "
    };
    let block = gray_block(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(field_line(
            "Date",
            &m.date_buf,
            m.focus == 0,
            m.field_selected,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "Start",
            &m.start_buf,
            m.focus == 1,
            m.field_selected,
        )),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "End",
            &m.end_buf,
            m.focus == 2,
            m.field_selected,
        )),
        chunks[2],
    );
    if let Some(err) = &m.error {
        f.render_widget(
            Paragraph::new(Span::styled(
                err.as_str(),
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            )),
            chunks[3],
        );
    }
}

fn draw_time_modal(f: &mut Frame, m: &ModalState, title: &str, label: &str) {
    let area = centered_rect(46, 4, f.area());
    f.render_widget(Clear, area);

    let block = gray_block(title.to_string());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    f.render_widget(
        Paragraph::new(field_line(label, &m.time_buf, true, m.field_selected)),
        chunks[0],
    );
    if let Some(err) = &m.error {
        f.render_widget(
            Paragraph::new(Span::styled(
                err.as_str(),
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            )),
            chunks[1],
        );
    }
}

fn draw_goal_period_modal(f: &mut Frame, m: &ModalState) {
    let area = centered_rect(46, 5, f.area());
    f.render_widget(Clear, area);

    let title = if m.editing_id.is_some() {
        " Edit goal period "
    } else {
        " Add goal period "
    };
    let block = gray_block(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(field_line(
            "From",
            &m.date_buf,
            m.focus == 0,
            m.field_selected,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "Hours/week",
            &m.hours_buf,
            m.focus == 1,
            m.field_selected,
        )),
        chunks[1],
    );
    if let Some(err) = &m.error {
        f.render_widget(
            Paragraph::new(Span::styled(
                err.as_str(),
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            )),
            chunks[2],
        );
    }
}

fn draw_absence_modal(f: &mut Frame, m: &ModalState) {
    let area = centered_rect(46, 5, f.area());
    f.render_widget(Clear, area);

    let title = if m.editing_id.is_some() {
        " Edit absence "
    } else {
        " Add absence "
    };
    let block = gray_block(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(field_line(
            "Date",
            &m.date_buf,
            m.focus == 0,
            m.field_selected,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "Reason",
            &m.start_buf,
            m.focus == 1,
            m.field_selected,
        )),
        chunks[1],
    );
    if let Some(err) = &m.error {
        f.render_widget(
            Paragraph::new(Span::styled(
                err.as_str(),
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            )),
            chunks[2],
        );
    }
}

fn draw_confirm(f: &mut Frame, target: DeleteTarget) {
    let area = centered_rect(40, 3, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DANGER))
        .title(" Confirm ");
    let (label, id) = match target {
        DeleteTarget::Entry(id) => ("Delete entry ", id),
        DeleteTarget::GoalPeriod(id) => ("Delete goal period ", id),
        DeleteTarget::Absence(id) => ("Delete absence ", id),
    };
    let p = Paragraph::new(Line::from(vec![
        Span::raw(label),
        Span::styled(
            format!("#{id}"),
            Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
        ),
        Span::raw("? "),
        Span::styled("(y/n)", Style::default().fg(MUTED)),
    ]))
    .block(block);
    f.render_widget(p, area);
}

const DAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn draw_history_screen(f: &mut Frame, app: &App) {
    let today = app.today();
    let now = Local::now().time();
    let mut rows = calc::history_weeks(&app.entries, &app.goal_periods, &app.absences, today, now);
    rows.reverse(); // newest first

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(9),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_history(f, app, chunks[0], &rows);
    draw_history_preview(f, app, chunks[1], &rows);
    draw_history_footer(f, chunks[2]);
}

/// One selectable row per week, newest first: a week-number label, each
/// visible day's worked total (colored - see `history_cell_color`), and
/// the week's total against the goal. The header row itself is fixed above
/// the list and never scrolls; j/k moves `app.history_selected`, which also
/// drives `draw_history_preview` below.
fn draw_history(f: &mut Frame, app: &App, area: Rect, rows: &[calc::WeekRow]) {
    let today = app.today();
    let day_indices: Vec<usize> = (0..7).filter(|&i| !app.hides_index(i)).collect();

    let label_width = 8usize;
    let day_width = 7usize;
    let total_width = 12usize;

    let mut header_spans = vec![Span::styled(
        " ".repeat(label_width),
        Style::default().fg(LABEL),
    )];
    for &i in &day_indices {
        header_spans.push(Span::styled(
            format!("{:>day_width$}", DAY_LABELS[i]),
            Style::default().fg(LABEL),
        ));
    }
    header_spans.push(Span::styled(
        format!("{:>total_width$}", "Total"),
        Style::default().fg(LABEL),
    ));

    let block = gray_block(" History ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    f.render_widget(Paragraph::new(Line::from(header_spans)), inner_chunks[0]);

    let (current_week_start, _) = calc::week_bounds(today);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let label = format!(
                "{:<label_width$}",
                format!("W{:02}", row.week_start.iso_week().week())
            );
            let label_style = if row.week_start == current_week_start {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(LABEL)
            };
            let mut spans = vec![Span::styled(label, label_style)];
            for &i in &day_indices {
                let (day, worked) = row.day_totals[i];
                let target = calc::daily_target_minutes(&app.goal_periods, &app.absences, day);
                let color = history_cell_color(day, today, worked, target);
                let text = if day > today {
                    "-".to_string()
                } else {
                    calc::format_hm(worked)
                };
                spans.push(Span::styled(
                    format!("{text:>day_width$}"),
                    Style::default().fg(color),
                ));
            }
            let total_color = if row.worked_week >= row.goal_week {
                SUCCESS
            } else {
                ACCENT
            };
            spans.push(Span::styled(
                format!(
                    "{:>total_width$}",
                    format!(
                        "{}/{}",
                        calc::format_hm(row.worked_week),
                        calc::format_hm(row.goal_week)
                    )
                ),
                Style::default()
                    .fg(total_color)
                    .add_modifier(Modifier::BOLD),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut state = ListState::default();
    if !rows.is_empty() {
        state.select(Some(app.history_selected.min(rows.len() - 1)));
    }
    let list = List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, inner_chunks[1], &mut state);
}

/// The selected week's diagram, reusing the exact same per-day bar
/// rendering (`week_lines`) as the dashboard's "This week" panel.
fn draw_history_preview(f: &mut Frame, app: &App, area: Rect, rows: &[calc::WeekRow]) {
    let Some(row) = rows.get(app.history_selected.min(rows.len().saturating_sub(1))) else {
        return;
    };
    let inner_width = area.width.saturating_sub(2);
    let lines = week_lines(app, row.week_start, inner_width);
    let (_, week_end) = calc::week_bounds(row.week_start);
    let title = format!(
        " W{:02} ({} - {}) ",
        row.week_start.iso_week().week(),
        row.week_start.format("%b %d"),
        week_end.format("%b %d")
    );
    let p = Paragraph::new(lines).block(gray_block(title));
    f.render_widget(p, area);
}

/// Color for one history cell: muted for a day that hasn't happened yet or
/// has no target and nothing logged, green once that day's target is met,
/// cyan for today (still in progress) or any other logged time, red for a
/// weekday that's already over with nothing logged at all.
fn history_cell_color(day: NaiveDate, today: NaiveDate, worked: i64, target: i64) -> Color {
    if day > today {
        MUTED
    } else if target > 0 && worked >= target {
        SUCCESS
    } else if day == today || worked > 0 {
        ACCENT
    } else if target == 0 {
        MUTED
    } else {
        DANGER
    }
}

fn draw_history_footer(f: &mut Frame, area: Rect) {
    let p = Paragraph::new(hint_line(&[
        ("q", "quit"),
        ("j/k", "select week"),
        ("h/Esc", "back"),
        ("r", "reload"),
    ]));
    f.render_widget(p, area);
}

fn draw_goals_screen(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    draw_goals(f, app, chunks[0]);
    draw_goals_footer(f, app, chunks[1]);
}

/// A selectable list of every goal period, oldest first (reads as a
/// timeline of how the goal evolved), the one currently in effect
/// highlighted with the same black-on-accent background History uses for
/// the current week. a/e/d add, edit, and delete the selected period.
fn draw_goals(f: &mut Frame, app: &App, area: Rect) {
    let today = app.today();
    let periods = app.visible_goal_periods();
    let current_id = calc::current_goal_period(&app.goal_periods, today).map(|p| p.id);

    let label_width = 17usize;
    let hours_width = 10usize;

    let mut header_spans = vec![Span::styled(
        format!("{:<label_width$}", "Effective from"),
        Style::default().fg(LABEL),
    )];
    header_spans.push(Span::styled(
        format!("{:>hours_width$}", "Hours/week"),
        Style::default().fg(LABEL),
    ));

    let block = gray_block(" Goals ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    f.render_widget(Paragraph::new(Line::from(header_spans)), inner_chunks[0]);

    let items: Vec<ListItem> = periods
        .iter()
        .map(|p| {
            let label_style = if Some(p.id) == current_id {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(LABEL)
            };
            let spans = vec![
                Span::styled(
                    format!("{:<label_width$}", p.effective_from.to_string()),
                    label_style,
                ),
                Span::styled(
                    format!("{:>hours_width$}", format!("{}h", p.weekly_goal_hours)),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
            ];
            ListItem::new(Line::from(spans))
        })
        .collect();

    let no_periods = items.is_empty();
    let items = if no_periods {
        vec![ListItem::new(Span::styled(
            "no goal periods",
            Style::default().fg(MUTED),
        ))]
    } else {
        items
    };

    let mut state = ListState::default();
    if no_periods {
        state.select(Some(0));
    } else {
        state.select(Some(app.goal_selected.min(periods.len() - 1)));
    }
    let list = List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, inner_chunks[1], &mut state);
}

fn draw_goals_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Mode::Modal(m) = &app.mode {
        f.render_widget(Paragraph::new(hint_line(&modal_footer_hints(m))), area);
        return;
    }
    let p = Paragraph::new(hint_line(&[
        ("q", "quit"),
        ("a", "add"),
        ("e", "edit"),
        ("d", "delete"),
        ("j/k", "select"),
        ("g/Esc", "back"),
        ("r", "reload"),
    ]));
    f.render_widget(p, area);
}
