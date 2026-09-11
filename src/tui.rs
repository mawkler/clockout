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

struct ModalState {
    editing_id: Option<u64>,
    date_buf: String,
    start_buf: String,
    end_buf: String,
    focus: usize, // 0=date, 1=start, 2=end
    error: Option<String>,
}

struct App {
    entries: Vec<TimeEntry>,
    config: Config,
    view: View,
    selected: usize,
    mode: Mode,
    status: Option<String>,
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
            editing_id: None,
            date_buf: self.today().to_string(),
            start_buf: String::new(),
            end_buf: String::new(),
            focus: 1,
            error: None,
        });
    }

    fn open_edit(&mut self) {
        if let Some(e) = self.selected_entry() {
            self.mode = Mode::Modal(ModalState {
                editing_id: Some(e.id),
                date_buf: e.date.to_string(),
                start_buf: e.start.format("%H:%M").to_string(),
                end_buf: e.end.format("%H:%M").to_string(),
                focus: 1,
                error: None,
            });
        }
    }

    fn submit_modal(&mut self) -> Result<()> {
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
            end,
        };

        if let Some(conflict) = self.entries.iter().find(|e| {
            e.date == candidate.date && Some(e.id) != editing_id && calc::overlaps(e, &candidate)
        }) {
            self.set_modal_error(format!(
                "overlaps existing entry #{} ({}-{})",
                conflict.id,
                conflict.start.format("%H:%M"),
                conflict.end.format("%H:%M")
            ));
            return Ok(());
        }

        let mut data = storage::load_entries()?;
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
        storage::save_entries(&data)?;
        self.entries = data.entries;
        self.mode = Mode::Normal;
        self.status = Some("saved".to_string());
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
        self.status = Some(format!("deleted #{id}"));
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
        Mode::Modal(m) => match code {
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Enter => app.submit_modal()?,
            KeyCode::Tab | KeyCode::Down => m.focus = (m.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => m.focus = (m.focus + 2) % 3,
            KeyCode::Backspace => {
                m.error = None;
                current_buf(m).pop();
            }
            KeyCode::Char(c) => {
                m.error = None;
                current_buf(m).push(c);
            }
            _ => {}
        },
    }
    Ok(())
}

fn current_buf(m: &mut ModalState) -> &mut String {
    match m.focus {
        0 => &mut m.date_buf,
        1 => &mut m.start_buf,
        _ => &mut m.end_buf,
    }
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1]);
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
    let target_today = calc::daily_target_minutes(&app.config, today);
    let line = if target_today == 0 {
        let worked = calc::worked_minutes(&calc::entries_on(&app.entries, today));
        Line::from(vec![
            Span::styled(
                "No target today — logged ",
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                calc::format_hm(worked),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        let now = Local::now().time();
        let leave = calc::leave_time(&app.entries, &app.config, today, now);
        Line::from(vec![
            Span::styled(
                "Leave at ",
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                leave.format("%H:%M").to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    let block = gray_block(format!(" {} ({}) ", today, today.format("%A")));
    let p = Paragraph::new(line)
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(p, area);
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
    let today = app.today();
    let worked_today = calc::worked_minutes(&calc::entries_on(&app.entries, today));
    let target_today = calc::daily_target_minutes(&app.config, today);
    let left_today = calc::minutes_left_today(&app.entries, &app.config, today);

    let (week_start, _) = calc::week_bounds(today);
    let worked_week = calc::worked_minutes(&calc::entries_in_week(&app.entries, week_start));
    let goal_week = calc::weekly_goal_minutes(&app.config);
    let left_week = calc::minutes_left_week(&app.entries, &app.config, today);
    let now = Local::now().time();
    let clear_debt_by = calc::leave_time_week(&app.entries, &app.config, today, now);

    let label = Style::default().fg(LABEL);
    let value = Style::default().add_modifier(Modifier::BOLD);

    let stat_line = |prefix: &str,
                     worked: i64,
                     of: &str,
                     total: i64,
                     remaining: i64,
                     clear_by: Option<NaiveTime>| {
        let left_color = if remaining == 0 { SUCCESS } else { ACCENT };
        let mut spans = vec![
            Span::styled(prefix.to_string(), label),
            Span::styled(calc::format_hm(worked), value),
            Span::styled(format!(" / {} {of} ", calc::format_hm(total)), label),
            Span::styled(
                format!("({} left)", calc::format_hm(remaining)),
                Style::default().fg(left_color).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(t) = clear_by {
            spans.push(Span::styled(" · clear time debt by ", label));
            spans.push(Span::styled(
                t.format("%H:%M").to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
        }
        Line::from(spans)
    };

    let lines = vec![
        stat_line(
            "Today:     ",
            worked_today,
            "target",
            target_today,
            left_today,
            None,
        ),
        stat_line(
            "This week: ",
            worked_week,
            "goal",
            goal_week,
            left_week,
            Some(clear_debt_by),
        ),
    ];
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
        View::Week => week_items(app, inner_width),
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

    // Reversed (not a hardcoded fg/bg) so each span's own color still comes
    // through on the selected row - important for the week view's bar, whose
    // filled/empty cells would otherwise become indistinguishable.
    let list = List::new(items).block(gray_block(title)).highlight_style(
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, area, &mut state);
}

fn today_items(app: &App) -> Vec<ListItem<'static>> {
    let today = app.today();
    let entries = calc::entries_on(&app.entries, today);
    let gaps = calc::today_gaps(&app.entries, today);

    let mut items: Vec<ListItem> = Vec::new();
    for e in &entries {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("#{:<4} ", e.id), Style::default().fg(MUTED)),
            Span::styled(format!("{} ", e.date), Style::default().fg(LABEL)),
            Span::styled(
                format!("{}-{}", e.start.format("%H:%M"), e.end.format("%H:%M")),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])));
    }
    for (start, end) in gaps {
        items.push(ListItem::new(Span::styled(
            format!(
                "      {}-{} (gap/break)",
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
fn week_items(app: &App, inner_width: u16) -> Vec<ListItem<'static>> {
    let (week_start, _) = calc::week_bounds(app.today());
    let days = app.visible_days();
    let (window_start, window_end) = day_window(&app.entries, week_start);

    days.into_iter()
        .map(|day| {
            let day_entries = calc::entries_on(&app.entries, day);
            let total = calc::worked_minutes(&day_entries);
            let prefix = format!("{:<9} {:>7}  ", day.format("%A"), calc::format_hm(total));
            let bar_width = inner_width.saturating_sub(prefix.chars().count() as u16);
            let cells = day_bar(&day_entries, window_start, window_end, bar_width as usize);

            let mut spans = vec![Span::styled(prefix, Style::default().fg(LABEL))];
            spans.extend(bar_spans(&cells));
            ListItem::new(Line::from(spans))
        })
        .collect()
}

/// The shared time-of-day window used to draw week-view bars: a sensible
/// default workday, widened to cover any entry that falls outside it.
fn day_window(entries: &[TimeEntry], week_start: NaiveDate) -> (NaiveTime, NaiveTime) {
    let default_start = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let default_end = NaiveTime::from_hms_opt(20, 0, 0).unwrap();
    let week_entries = calc::entries_in_week(entries, week_start);
    let start = week_entries
        .iter()
        .map(|e| e.start)
        .min()
        .unwrap_or(default_start)
        .min(default_start);
    let end = week_entries
        .iter()
        .map(|e| e.end)
        .max()
        .unwrap_or(default_end)
        .max(default_end);
    (start, end)
}

/// Rasterizes a day's entries across `width` columns spanning
/// [window_start, window_end): true where a column's time falls inside a
/// logged entry.
fn day_bar(
    entries: &[&TimeEntry],
    window_start: NaiveTime,
    window_end: NaiveTime,
    width: usize,
) -> Vec<bool> {
    if width == 0 {
        return Vec::new();
    }
    let total_minutes = (window_end - window_start).num_minutes().max(1);
    (0..width)
        .map(|c| {
            let offset = (c as i64 * total_minutes) / width as i64;
            let t = window_start + chrono::Duration::minutes(offset);
            entries.iter().any(|e| e.start <= t && t < e.end)
        })
        .collect()
}

fn bar_spans(cells: &[bool]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let value = cells[i];
        let mut j = i + 1;
        while j < cells.len() && cells[j] == value {
            j += 1;
        }
        let ch = if value { '█' } else { '·' };
        let style = if value {
            Style::default().fg(ACCENT)
        } else {
            Style::default().fg(MUTED)
        };
        spans.push(Span::styled(ch.to_string().repeat(j - i), style));
        i = j;
    }
    spans
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let p = match &app.status {
        Some(s) => Paragraph::new(Span::styled(
            s.clone(),
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        )),
        None => {
            let hint = match app.view {
                View::Today => {
                    "q quit | a add | e edit | d delete | j/k move | Tab view | r reload"
                }
                View::Week => "q quit | a add | j/k move | Tab view | r reload",
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

fn draw_modal(f: &mut Frame, m: &ModalState) {
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

    let field = |label: &str, value: &str, focused: bool| {
        let style = if focused {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(LABEL)),
            Span::styled(value.to_string(), style),
        ])
    };

    f.render_widget(
        Paragraph::new(field(
            "Date (YYYY-MM-DD/today/yesterday)",
            &m.date_buf,
            m.focus == 0,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(field("Start (HH:MM)", &m.start_buf, m.focus == 1)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(field("End   (HH:MM)", &m.end_buf, m.focus == 2)),
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
