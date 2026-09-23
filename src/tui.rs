use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveTime};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
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
use crate::model::{Config, TimeEntry};
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
    ConfirmDelete(u64),
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
}

struct ModalState {
    kind: ModalKind,
    editing_id: Option<u64>,
    date_buf: String,
    start_buf: String,
    end_buf: String,
    /// The single time field used by the End modal.
    time_buf: String,
    focus: usize, // 0=date, 1=start, 2=end for AddEdit; always 0 for End
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

struct App {
    entries: Vec<TimeEntry>,
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
    /// A transient footer message and when it was set, for `STATUS_DURATION`
    /// auto-dismissal - see `set_status`/`clear_expired_status`.
    status: Option<(StatusKind, String, Instant)>,
    quit: bool,
}

impl App {
    fn new() -> Result<Self> {
        let entries = storage::load_entries()?.entries;
        let config = storage::load_config()?;
        Ok(App {
            entries,
            config,
            focused_day: Local::now().date_naive(),
            focus_pane: Pane::Entries,
            selected: 0,
            mode: Mode::Normal,
            status: None,
            quit: false,
        })
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
        self.entries = storage::load_entries()?.entries;
        self.config = storage::load_config()?;
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
        Ok(())
    }

    fn today(&self) -> NaiveDate {
        Local::now().date_naive()
    }

    /// The focused day's entries, sorted by start time.
    fn visible(&self) -> Vec<&TimeEntry> {
        let mut v = calc::entries_on(&self.entries, self.focused_day);
        v.sort_by_key(|e| e.start);
        v
    }

    /// The 7 days (Monday-Sunday) of the current week, for the diagram.
    fn week_days(&self) -> Vec<NaiveDate> {
        let (week_start, _) = calc::week_bounds(self.today());
        (0..7)
            .map(|i| week_start + chrono::Duration::days(i))
            .collect()
    }

    fn selected_entry(&self) -> Option<TimeEntry> {
        self.visible().get(self.selected).map(|e| (*e).clone())
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

    /// Moves the focused day within the current week (clamped to
    /// Monday..Sunday) and resets entry selection, since the entry list
    /// underneath just changed. Called from j/k while the Days pane has
    /// focus.
    fn move_focused_day(&mut self, delta: i64) {
        let (week_start, week_end) = calc::week_bounds(self.today());
        let moved = self.focused_day + chrono::Duration::days(delta);
        self.focused_day = moved.clamp(week_start, week_end);
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
            focus: 1,
            field_selected: true,
            error: None,
        });
    }

    fn open_edit(&mut self) {
        if let Some(e) = self.selected_entry() {
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
                focus: 1,
                field_selected: true,
                error: None,
            });
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
            focus: 0,
            field_selected: true,
            error: None,
        });
    }

    fn submit_modal(&mut self) -> Result<()> {
        let kind = match &self.mode {
            Mode::Modal(m) => m.kind,
            _ => return Ok(()),
        };
        match kind {
            ModalKind::AddEdit => self.submit_add_edit(),
            ModalKind::End => self.submit_end(),
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
        let result = storage::with_entries(|data| {
            let Some(open) = calc::open_entry(&data.entries) else {
                anyhow::bail!("no open block");
            };
            if time <= open.start {
                anyhow::bail!("end must be after start");
            }
            let (id, date, start) = (open.id, open.date, open.start);
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

            if let Some(existing) = data.entries.iter_mut().find(|e| e.id == id) {
                existing.end = Some(time);
            }
            Ok((data.entries.clone(), start))
        });

        let (entries, start) = match result {
            Ok(v) => v,
            Err(e) => {
                self.set_modal_error(e.to_string());
                return Ok(());
            }
        };
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
        Ok(())
    }

    fn set_modal_error(&mut self, err: String) {
        if let Mode::Modal(m) = &mut self.mode {
            m.error = Some(err);
        }
    }

    fn request_delete(&mut self) {
        if let Some(e) = self.selected_entry() {
            self.mode = Mode::ConfirmDelete(e.id);
        }
    }

    fn confirm_delete(&mut self, id: u64) -> Result<()> {
        let entries = storage::with_entries(|data| {
            data.entries.retain(|e| e.id != id);
            Ok(data.entries.clone())
        })?;
        self.entries = entries;
        self.mode = Mode::Normal;
        self.set_status(StatusKind::Info, format!("deleted #{id}"));
        Ok(())
    }
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
                handle_key(&mut app, key.code)?;
            }
        } else {
            app.reload()?;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) -> Result<()> {
    match &mut app.mode {
        Mode::Normal => match code {
            KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
            KeyCode::Char('a') => app.open_add(),
            KeyCode::Char('f') => app.open_finish(),
            KeyCode::Char('e') if app.focus_pane == Pane::Entries => app.open_edit(),
            KeyCode::Char('d') if app.focus_pane == Pane::Entries => app.request_delete(),
            KeyCode::Char('r') => app.reload()?,
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
        Mode::ConfirmDelete(id) => {
            let id = *id;
            match code {
                KeyCode::Char('y') => app.confirm_delete(id)?,
                KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
                _ => {}
            }
        }
        Mode::Modal(m) => {
            let field_count = match m.kind {
                ModalKind::AddEdit => 3,
                ModalKind::End => 1,
            };
            match code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => app.submit_modal()?,
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
    }
}

fn draw(f: &mut Frame, app: &App) {
    let now = Local::now().time();
    let debt = calc::time_debt_minutes(&app.entries, &app.config, app.today(), now);
    let surplus = calc::time_surplus_minutes(&app.entries, &app.config, app.today(), now);
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

    match &app.mode {
        Mode::Modal(m) => draw_modal(f, m),
        Mode::ConfirmDelete(id) => draw_confirm(f, *id),
        Mode::Normal => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let today = app.today();
    let now = Local::now().time();
    let target_today = calc::daily_target_minutes(&app.config, today);
    let spans = if target_today == 0 {
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
    } else {
        let leave = calc::leave_time(&app.entries, &app.config, today, now);
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
    let target_today = calc::daily_target_minutes(&app.config, today);
    let left_today = calc::minutes_left_today(&app.entries, &app.config, today, now);

    let (week_start, _) = calc::week_bounds(today);
    let worked_week =
        calc::worked_minutes(&calc::entries_in_week(&app.entries, week_start), today, now);
    let goal_week = calc::weekly_goal_minutes(&app.config);
    let left_week = calc::minutes_left_week(&app.entries, &app.config, today, now);
    let clear_debt_by = calc::leave_time_debt(&app.entries, &app.config, today, now);

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
    let lines = week_lines(app, inner_width);
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

    // app.selected indexes app.visible() (entries only, sorted the same
    // way); track which row of the combined, gap-interleaved list that
    // lands on so the highlight stays on the right entry.
    let mut items: Vec<ListItem> = Vec::new();
    let mut select_index = None;
    let mut entry_count = 0;
    for item in &combined {
        match item {
            DayItem::Entry(e) => {
                if entry_count == app.selected {
                    select_index = Some(items.len());
                }
                entry_count += 1;
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
    } else if entry_count > 0 {
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

/// One line per day of the current week: weekday name, total hours, and a
/// bar showing where in the day the logged time falls (all rows share the
/// same time-of-day scale so they're visually comparable).
fn week_lines(app: &App, inner_width: u16) -> Vec<Line<'static>> {
    let today = app.today();
    let now = Local::now().time();
    let (week_start, _) = calc::week_bounds(today);
    let (window_start, window_end) = day_window(&app.entries, week_start, now);

    app.week_days()
        .into_iter()
        .map(|day| {
            let day_entries = calc::entries_on(&app.entries, day);
            let total = calc::worked_minutes(&day_entries, today, now);
            let prefix = format!("{:<9} {:>7}  ", day.format("%A"), calc::format_hm(total));
            let bar_width = inner_width.saturating_sub(prefix.chars().count() as u16);
            let cells = day_bar(
                &day_entries,
                window_start,
                window_end,
                bar_width as usize,
                day,
                today,
                now,
            );

            // Mark the focused row on its prefix only - inverting the whole
            // line would make the bar's own colors hard to read. Dimmed
            // (bold only, no reverse) while the Entries pane has focus
            // instead of this one, so it's clear j/k won't move this row
            // right now. A future, unfocused day is grayed out - it hasn't
            // happened yet, so there's nothing to show there.
            let prefix_style = if day == app.focused_day {
                let base = Style::default().fg(LABEL).add_modifier(Modifier::BOLD);
                if app.focus_pane == Pane::Days {
                    base.add_modifier(Modifier::REVERSED)
                } else {
                    base
                }
            } else if day > today {
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
    window_start: NaiveTime,
    window_end: NaiveTime,
    width: usize,
    day: NaiveDate,
    today: NaiveDate,
    now: NaiveTime,
) -> Vec<Cell> {
    if width == 0 {
        return Vec::new();
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
        };
        spans.push(Span::styled(ch.to_string().repeat(j - i), style));
        i = j;
    }
    spans
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
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
        None => Paragraph::new(hint_line(&[
            ("q", "quit"),
            ("a", "add to this day"),
            ("f", "finish"),
            ("e", "edit"),
            ("d", "delete"),
            ("Tab", "switch pane"),
            ("j/k", "move"),
            ("r", "reload"),
        ])),
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
    }
}

fn draw_add_edit_modal(f: &mut Frame, m: &ModalState) {
    let area = centered_rect(50, 9, f.area());
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
    f.render_widget(
        Paragraph::new(Span::styled(
            "Tab: next field | Enter: save | Esc: cancel",
            Style::default().fg(MUTED),
        )),
        chunks[4],
    );
}

fn draw_time_modal(f: &mut Frame, m: &ModalState, title: &str, label: &str) {
    let area = centered_rect(46, 7, f.area());
    f.render_widget(Clear, area);

    let block = gray_block(title.to_string());
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
    f.render_widget(
        Paragraph::new(Span::styled(
            "Enter: save | Esc: cancel",
            Style::default().fg(MUTED),
        )),
        chunks[2],
    );
}

fn draw_confirm(f: &mut Frame, id: u64) {
    let area = centered_rect(40, 3, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DANGER))
        .title(" Confirm ");
    let p = Paragraph::new(Line::from(vec![
        Span::raw("Delete entry "),
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
