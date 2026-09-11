use std::io;
use std::time::Duration;

use anyhow::Result;
use chrono::{Local, NaiveDate};
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
            KeyCode::Char('e') => app.open_edit(),
            KeyCode::Char('d') => app.request_delete(),
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
    let text = if target_today == 0 {
        let worked = calc::worked_minutes(&calc::entries_on(&app.entries, today));
        format!("No target today — logged {}", calc::format_hm(worked))
    } else {
        let now = Local::now().time();
        let leave = calc::leave_time(&app.entries, &app.config, today, now);
        format!("Leave at {}", leave.format("%H:%M"))
    };
    let block = Block::default().borders(Borders::ALL).title(format!(
        " {} ({}) ",
        today,
        today.format("%A")
    ));
    let p = Paragraph::new(Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )))
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

    let lines = vec![
        Line::from(format!(
            "Today:     {} / {} target ({} left)",
            calc::format_hm(worked_today),
            calc::format_hm(target_today),
            calc::format_hm(left_today)
        )),
        Line::from(format!(
            "This week: {} / {} goal ({} left)",
            calc::format_hm(worked_week),
            calc::format_hm(goal_week),
            calc::format_hm(left_week)
        )),
    ];
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    f.render_widget(p, area);
}

fn draw_entries(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.view {
        View::Today => " Today (Tab: week view) ",
        View::Week => " This week (Tab: today view) ",
    };
    let visible = app.visible();
    let gaps = calc::today_gaps(&app.entries, app.today());

    let mut items: Vec<ListItem> = Vec::new();
    for e in &visible {
        items.push(ListItem::new(format!(
            "#{:<4} {} {}-{}",
            e.id,
            e.date,
            e.start.format("%H:%M"),
            e.end.format("%H:%M")
        )));
    }
    if app.view == View::Today {
        for (start, end) in gaps {
            items.push(ListItem::new(Span::styled(
                format!(
                    "      {}-{} (gap/break)",
                    start.format("%H:%M"),
                    end.format("%H:%M")
                ),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
    }
    if items.is_empty() {
        items.push(ListItem::new("no entries"));
    }

    let mut state = ListState::default();
    if !visible.is_empty() {
        state.select(Some(app.selected.min(visible.len() - 1)));
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = match &app.status {
        Some(s) => s.clone(),
        None => "q quit | a add | e edit | d delete | j/k move | Tab view | r reload".to_string(),
    };
    let p = Paragraph::new(text);
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
    let block = Block::default().borders(Borders::ALL).title(title);
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
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::raw(format!("{label}: ")),
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
            Paragraph::new(Span::styled(err.as_str(), Style::default().fg(Color::Red))),
            chunks[3],
        );
    }
    f.render_widget(
        Paragraph::new("Tab: next field | Enter: save | Esc: cancel"),
        chunks[4],
    );
}

fn draw_confirm(f: &mut Frame, id: u64) {
    let area = centered_rect(40, 3, f.area());
    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(" Confirm ");
    let p = Paragraph::new(format!("Delete entry #{id}? (y/n)")).block(block);
    f.render_widget(p, area);
}
