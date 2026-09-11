use std::io;
use std::time::Duration;

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
/// A block that's been started but not finished yet.
const OPEN: Color = Color::Yellow;

fn gray_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(title.into())
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum View {
    Today,
    Week,
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
    /// Full 3-field add/edit form (date, start, end).
    AddEdit,
    /// Single time field: start an open-ended block.
    Start,
    /// Single time field: close the currently open block.
    End,
}

struct ModalState {
    kind: ModalKind,
    editing_id: Option<u64>,
    date_buf: String,
    start_buf: String,
    end_buf: String,
    /// The single time field used by Start/End modals.
    time_buf: String,
    focus: usize, // 0=date, 1=start, 2=end for AddEdit; always 0 for Start/End
    /// True while the focused field's existing text hasn't been touched yet:
    /// it renders as selected, and the next keystroke overwrites it instead
    /// of appending (like a browser text input). Reset on every focus
    /// change; cleared by the first edit to the field.
    field_selected: bool,
    error: Option<String>,
}

struct App {
    entries: Vec<TimeEntry>,
    config: Config,
    view: View,
    selected: usize,
    mode: Mode,
    status: Option<(StatusKind, String)>,
    quit: bool,
}

impl App {
    fn new() -> Result<Self> {
        let entries = storage::load_entries()?.entries;
        let config = storage::load_config()?;
        Ok(App {
            entries,
            config,
            view: View::Today,
            selected: 0,
            mode: Mode::Normal,
            status: None,
            quit: false,
        })
    }

    fn reload(&mut self) -> Result<()> {
        self.entries = storage::load_entries()?.entries;
        self.config = storage::load_config()?;
        let len = self.visible_len();
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

    fn visible(&self) -> Vec<&TimeEntry> {
        let today = self.today();
        let mut v = match self.view {
            View::Today => calc::entries_on(&self.entries, today),
            View::Week => {
                let (week_start, _) = calc::week_bounds(today);
                calc::entries_in_week(&self.entries, week_start)
            }
        };
        v.sort_by_key(|e| (e.date, e.start));
        v
    }

    /// Distinct dates with logged entries in the current week, ascending.
    /// Used for the week view, which shows one summary row per day.
    fn visible_days(&self) -> Vec<NaiveDate> {
        let (week_start, _) = calc::week_bounds(self.today());
        let mut dates: Vec<NaiveDate> = calc::entries_in_week(&self.entries, week_start)
            .iter()
            .map(|e| e.date)
            .collect();
        dates.sort();
        dates.dedup();
        dates
    }

    /// Number of selectable rows for the current view: entries in Today
    /// view, day-summary rows in Week view.
    fn visible_len(&self) -> usize {
        match self.view {
            View::Today => self.visible().len(),
            View::Week => self.visible_days().len(),
        }
    }

    fn selected_entry(&self) -> Option<TimeEntry> {
        self.visible().get(self.selected).map(|e| (*e).clone())
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible_len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected as i32 + delta;
        self.selected = cur.clamp(0, len as i32 - 1) as usize;
    }

    fn open_add(&mut self) {
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::AddEdit,
            editing_id: None,
            date_buf: self.today().to_string(),
            start_buf: String::new(),
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

    /// Open the "start an open-ended block" modal, prefilled with now.
    /// Refuses if a block is already open.
    fn open_start(&mut self) {
        if let Some(open) = calc::open_entry(&self.entries) {
            self.status = Some((
                StatusKind::Error,
                format!(
                    "already have an open block from {} - press f to finish it",
                    open.start.format("%H:%M")
                ),
            ));
            return;
        }
        self.mode = Mode::Modal(ModalState {
            kind: ModalKind::Start,
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

    /// Open the "finish the open block" modal, prefilled with now. Refuses
    /// if there is no open block.
    fn open_finish(&mut self) {
        if calc::open_entry(&self.entries).is_none() {
            self.status = Some((
                StatusKind::Error,
                "no open block to finish - press s to start one".to_string(),
            ));
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
            ModalKind::Start => self.submit_start(),
            ModalKind::End => self.submit_end(),
        }
    }

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

        let start = parse::parse_time(&m.start_buf);
        let start = match start {
            Ok(s) => s,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };
        let end = parse::parse_time(&m.end_buf);
        let end = match end {
            Ok(e) => e,
            Err(e) => {
                self.set_modal_error(e);
                return Ok(());
            }
        };
        if start >= end {
            self.set_modal_error("start must be before end".to_string());
            return Ok(());
        }

        let editing_id = m.editing_id;
        let candidate = TimeEntry {
            id: editing_id.unwrap_or(0),
            date,
            start,
            end: Some(end),
        };

        if let Some(conflict) = self.entries.iter().find(|e| {
            e.date == candidate.date && Some(e.id) != editing_id && calc::overlaps(e, &candidate)
        }) {
            self.set_modal_error(format!(
                "overlaps existing entry #{} ({}-{})",
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end)
            ));
            return Ok(());
        }

        let mut data = storage::load_entries()?;
        if let Some(id) = editing_id {
            if let Some(existing) = data.entries.iter_mut().find(|e| e.id == id) {
                existing.date = date;
                existing.start = start;
                existing.end = Some(end);
            }
        } else {
            let id = data.next_id;
            data.next_id += 1;
            data.entries.push(TimeEntry {
                id,
                date,
                start,
                end: Some(end),
            });
        }
        storage::save_entries(&data)?;
        self.entries = data.entries;
        self.mode = Mode::Normal;
        self.status = Some((StatusKind::Info, "saved".to_string()));
        Ok(())
    }

    fn submit_start(&mut self) -> Result<()> {
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
        if calc::open_entry(&self.entries).is_some() {
            self.set_modal_error("already have an open block".to_string());
            return Ok(());
        }
        let today = self.today();
        let candidate = TimeEntry {
            id: 0,
            date: today,
            start: time,
            end: None,
        };
        if let Some(conflict) = self
            .entries
            .iter()
            .find(|e| e.date == today && calc::overlaps(e, &candidate))
        {
            self.set_modal_error(format!(
                "overlaps existing entry #{} ({}-{})",
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end)
            ));
            return Ok(());
        }

        let mut data = storage::load_entries()?;
        let id = data.next_id;
        data.next_id += 1;
        data.entries.push(TimeEntry {
            id,
            date: today,
            start: time,
            end: None,
        });
        storage::save_entries(&data)?;
        self.entries = data.entries;
        self.mode = Mode::Normal;
        self.status = Some((
            StatusKind::Info,
            format!("started at {}", time.format("%H:%M")),
        ));
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
        let Some(open) = calc::open_entry(&self.entries) else {
            self.set_modal_error("no open block".to_string());
            return Ok(());
        };
        if time <= open.start {
            self.set_modal_error("end must be after start".to_string());
            return Ok(());
        }
        let (id, date, start) = (open.id, open.date, open.start);
        let candidate = TimeEntry {
            id,
            date,
            start,
            end: Some(time),
        };
        if let Some(conflict) = self
            .entries
            .iter()
            .find(|e| e.id != id && e.date == date && calc::overlaps(e, &candidate))
        {
            self.set_modal_error(format!(
                "overlaps existing entry #{} ({}-{})",
                conflict.id,
                conflict.start.format("%H:%M"),
                calc::format_end(conflict.end)
            ));
            return Ok(());
        }

        let mut data = storage::load_entries()?;
        if let Some(existing) = data.entries.iter_mut().find(|e| e.id == id) {
            existing.end = Some(time);
        }
        storage::save_entries(&data)?;
        self.entries = data.entries;
        self.mode = Mode::Normal;
        self.status = Some((
            StatusKind::Info,
            format!(
                "finished {}-{}",
                start.format("%H:%M"),
                time.format("%H:%M")
            ),
        ));
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
        let mut data = storage::load_entries()?;
        data.entries.retain(|e| e.id != id);
        storage::save_entries(&data)?;
        self.entries = data.entries;
        self.mode = Mode::Normal;
        self.status = Some((StatusKind::Info, format!("deleted #{id}")));
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
            KeyCode::Char('s') => app.open_start(),
            KeyCode::Char('f') => app.open_finish(),
            KeyCode::Char('e') if app.view == View::Today => app.open_edit(),
            KeyCode::Char('d') if app.view == View::Today => app.request_delete(),
            KeyCode::Char('r') => app.reload()?,
            KeyCode::Tab => {
                app.view = if app.view == View::Today {
                    View::Week
                } else {
                    View::Today
                };
                app.selected = 0;
            }
            KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
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
                ModalKind::Start | ModalKind::End => 1,
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
        ModalKind::Start | ModalKind::End => &mut m.time_buf,
    }
}

fn draw(f: &mut Frame, app: &App) {
    let now = Local::now().time();
    let debt = calc::time_debt_minutes(&app.entries, &app.config, app.today(), now);
    let stats_height = if debt > 0 { 5 } else { 4 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(stats_height),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1], debt);
    draw_entries(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);

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
    let mut spans = if target_today == 0 {
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
    if let Some(open) = calc::open_entry(&app.entries)
        && open.date == today
    {
        spans.push(Span::styled("  ·  open since ", Style::default().fg(LABEL)));
        spans.push(Span::styled(
            open.start.format("%H:%M").to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    }
    let block = gray_block(format!(" {} ({}) ", today, today.format("%A")));
    let p = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(p, area);
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect, debt: i64) {
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
    let clear_debt_by = calc::leave_time_week(&app.entries, &app.config, today, now);

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
    }
    let p = Paragraph::new(lines).block(gray_block(""));
    f.render_widget(p, area);
}

fn draw_entries(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.view {
        View::Today => " Today (Tab: week view) ",
        View::Week => " This week (Tab: today view) ",
    };
    // Matches Block::inner() for a plain Borders::ALL block: 1 column each side.
    let inner_width = area.width.saturating_sub(2);

    let items: Vec<ListItem> = match app.view {
        View::Today => today_items(app),
        View::Week => week_items(app, inner_width, app.selected),
    };
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
    let len = app.visible_len();
    if len > 0 {
        state.select(Some(app.selected.min(len - 1)));
    }

    // Week view marks the selected row itself (bolding its weekday/hours
    // prefix in week_items) rather than inverting the whole line, since
    // inverting would make the bar's own colors hard to read.
    let highlight_style = match app.view {
        View::Today => Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
        View::Week => Style::default(),
    };
    let list = List::new(items)
        .block(gray_block(title))
        .highlight_style(highlight_style);
    f.render_stateful_widget(list, area, &mut state);
}

fn today_items(app: &App) -> Vec<ListItem<'static>> {
    let today = app.today();
    let entries = calc::entries_on(&app.entries, today);
    let gaps = calc::today_gaps(&app.entries, today);

    let mut items: Vec<ListItem> = Vec::new();
    for e in &entries {
        let range_style = if e.end.is_none() {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("{} ", e.date), Style::default().fg(LABEL)),
            Span::styled(
                format!("{}-{}", e.start.format("%H:%M"), calc::format_end(e.end)),
                range_style,
            ),
        ])));
    }
    for (start, end) in gaps {
        items.push(ListItem::new(Span::styled(
            format!(
                "           {}-{} (gap/break)",
                start.format("%H:%M"),
                end.format("%H:%M")
            ),
            Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
        )));
    }
    items
}

/// One summary row per worked day: weekday name, total hours, and a bar
/// showing where in the day the logged time falls (all rows share the same
/// time-of-day scale so they're visually comparable).
fn week_items(app: &App, inner_width: u16, selected: usize) -> Vec<ListItem<'static>> {
    let today = app.today();
    let now = Local::now().time();
    let (week_start, _) = calc::week_bounds(today);
    let days = app.visible_days();
    let (window_start, window_end) = day_window(&app.entries, week_start, now);

    days.into_iter()
        .enumerate()
        .map(|(i, day)| {
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

            // Mark the selected row on its prefix only - inverting the
            // whole line would make the bar's own colors hard to read.
            let prefix_style = if i == selected {
                Style::default()
                    .fg(LABEL)
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(LABEL)
            };
            let mut spans = vec![Span::styled(prefix, prefix_style)];
            spans.extend(bar_spans(&cells));
            ListItem::new(Line::from(spans))
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

#[derive(PartialEq, Eq, Clone, Copy)]
enum Cell {
    Empty,
    Worked,
    /// Part of a block that's been started but not finished yet.
    Open,
}

/// Rasterizes a day's entries across `width` columns spanning
/// [window_start, window_end). An open block (no end yet) gets a single
/// marker column at its current leading edge - `now` if `day` is `today`, or
/// the window's end otherwise (no better bound for a block left open on a
/// past day) - rather than filling its whole (still-growing) span.
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
    let mut cells: Vec<Cell> = (0..width)
        .map(|c| {
            let offset = (c as i64 * total_minutes) / width as i64;
            let t = window_start + chrono::Duration::minutes(offset);
            let worked = entries
                .iter()
                .any(|e| e.end.is_some_and(|end| e.start <= t && t < end));
            if worked { Cell::Worked } else { Cell::Empty }
        })
        .collect();

    if let Some(open) = entries.iter().find(|e| e.end.is_none()) {
        let open_end = if day == today { now } else { window_end };
        if open.start < open_end {
            let offset = (open_end - window_start)
                .num_minutes()
                .clamp(0, total_minutes);
            let col = ((offset * width as i64) / total_minutes) as usize;
            let col = col.min(width - 1);
            if cells[col] != Cell::Worked {
                cells[col] = Cell::Open;
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
            Cell::Open => ('┃', Style::default().fg(OPEN)),
            Cell::Empty => ('·', Style::default().fg(MUTED)),
        };
        spans.push(Span::styled(ch.to_string().repeat(j - i), style));
        i = j;
    }
    spans
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let p = match &app.status {
        Some((kind, s)) => {
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
        None => {
            let hint = match app.view {
                View::Today => {
                    "q quit | a add | s start | f finish | e edit | d delete | j/k move | Tab view | r reload"
                }
                View::Week => {
                    "q quit | a add | s start | f finish | j/k move | Tab view | r reload"
                }
            };
            Paragraph::new(Span::styled(hint, Style::default().fg(MUTED)))
        }
    };
    f.render_widget(p, area);
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
        ModalKind::Start => draw_time_modal(f, m, " Start block ", "Start time (HH:MM)"),
        ModalKind::End => draw_time_modal(f, m, " Finish block ", "End time (HH:MM)"),
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
            "Date (YYYY-MM-DD/today/yesterday)",
            &m.date_buf,
            m.focus == 0,
            m.field_selected,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "Start (HH:MM)",
            &m.start_buf,
            m.focus == 1,
            m.field_selected,
        )),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(field_line(
            "End   (HH:MM)",
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
