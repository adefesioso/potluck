//! Output sink, plus the two frontends: plain lines (pipes, scripts) and a full-screen TUI.

use std::{io::IsTerminal, sync::OnceLock, time::Duration};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::{io::AsyncBufReadExt, sync::mpsc};

static SINK: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

/// Prints a status or chat line: into the TUI when it runs, else to stdout.
pub fn say(line: String) {
    match SINK.get() {
        Some(tx) => {
            let _ = tx.send(line);
        }
        None => println!("{line}"),
    }
}

/// Lines for scripts (addresses, etc.); the TUI shows a summary instead.
pub fn script_only(line: String) {
    if SINK.get().is_none() {
        println!("{line}");
    }
}

macro_rules! say {
    ($($t:tt)*) => { $crate::ui::say(format!($($t)*)) };
}

pub enum Mode {
    Lines,
    Tui(mpsc::UnboundedReceiver<String>),
}

/// TUI on an interactive terminal, plain lines otherwise. Routes `say!` accordingly.
pub fn mode() -> Mode {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Mode::Lines;
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = SINK.set(tx);
    Mode::Tui(rx)
}

/// Leaves the TUI's alternate screen, if it was entered.
pub fn restore() {
    if SINK.get().is_some() {
        ratatui::restore();
    }
}

/// What the TUI shows beside the chat; refreshed every frame.
pub struct Sidebar {
    pub room: String,
    pub nick: String,
    /// (nick, owner, online or you)
    pub members: Vec<(String, bool, bool)>,
    /// "model: provider, provider"
    pub models: Vec<String>,
    /// Our listen addresses.
    pub addrs: usize,
}

/// Starts the frontend; yields typed lines and ends when the user quits.
pub fn input(
    mode: Mode,
    sidebar: impl Fn() -> Sidebar + Send + 'static,
) -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    match mode {
        Mode::Lines => tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }),
        Mode::Tui(out) => tokio::spawn(async move {
            let result = tui(out, tx, sidebar).await;
            ratatui::restore();
            if let Err(e) = result {
                eprintln!("tui: {e}");
            }
        }),
    };
    rx
}

async fn tui(
    mut out: mpsc::UnboundedReceiver<String>,
    input: mpsc::UnboundedSender<String>,
    sidebar: impl Fn() -> Sidebar,
) -> std::io::Result<()> {
    let mut term = ratatui::init();
    let mut app = App::default();
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1)); // sidebar refresh
    loop {
        let side = sidebar();
        term.draw(|f| app.draw(f, &side))?;
        tokio::select! {
            Some(line) = out.recv() => {
                app.lines.push(line);
                while let Ok(line) = out.try_recv() {
                    app.lines.push(line);
                }
            }
            Some(Ok(Event::Key(key))) = keys.next() => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Char('c' | 'd') if ctrl => return Ok(()),
                    KeyCode::Enter => {
                        let line: String = app.input.drain(..).collect();
                        app.cursor = 0;
                        app.scroll = 0;
                        if line.trim().is_empty() {
                            continue;
                        }
                        if !line.starts_with('/') {
                            app.lines.push(format!("<{}> {line}", side.nick));
                        }
                        app.history.push(line.clone());
                        app.recall = app.history.len();
                        if line.trim() == "/quit" || input.send(line).is_err() {
                            return Ok(());
                        }
                    }
                    code => app.edit(code),
                }
            }
            _ = tick.tick() => {}
        }
    }
}

#[derive(Default)]
struct App {
    lines: Vec<String>,
    input: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    /// Index into `history` while browsing with Up/Down.
    recall: usize,
    /// Messages scrolled up from the bottom.
    scroll: usize,
}

impl App {
    fn edit(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.input.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up | KeyCode::Down => {
                self.recall = match code {
                    KeyCode::Up => self.recall.saturating_sub(1),
                    _ => (self.recall + 1).min(self.history.len()),
                };
                self.input = self
                    .history
                    .get(self.recall)
                    .map_or(vec![], |l| l.chars().collect());
                self.cursor = self.input.len();
            }
            KeyCode::PageUp => {
                self.scroll = (self.scroll + 10).min(self.lines.len().saturating_sub(1))
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
    }

    fn draw(&self, f: &mut Frame, side: &Sidebar) {
        let [title, body, input] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(f.area());
        let [chat, list] =
            Layout::horizontal([Constraint::Min(20), Constraint::Length(28)]).areas(body);

        let online = side.members.iter().filter(|m| m.2).count();
        let scrolled = if self.scroll > 0 {
            format!(" · scrolled up {}", self.scroll)
        } else {
            String::new()
        };
        let bar = format!(
            " potluck · {} · {} · {online}/{} online · {} addresses{scrolled}",
            side.room,
            side.nick,
            side.members.len(),
            side.addrs
        );
        f.render_widget(Paragraph::new(bar).reversed(), title);

        self.draw_chat(f, chat);

        let mut items = vec![Line::from("Members".bold())];
        for (nick, owner, here) in &side.members {
            let style = if *here {
                Style::new().fg(Color::Green)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            items.push(Line::styled(
                format!("{}{nick}", if *owner { "~" } else { " " }),
                style,
            ));
        }
        items.push(Line::from(""));
        items.push(Line::from("Models".bold()));
        if side.models.is_empty() {
            items.push(Line::styled(
                "none online",
                Style::new().fg(Color::DarkGray),
            ));
        }
        items.extend(side.models.iter().map(|m| Line::from(m.as_str())));
        let block = Block::new()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(Color::DarkGray));
        f.render_widget(
            Paragraph::new(items)
                .block(block)
                .wrap(Wrap { trim: false }),
            list,
        );

        let text: String = self.input.iter().collect();
        f.render_widget(Paragraph::new(format!("> {text}")), input);
        f.set_cursor_position((input.x + 2 + self.cursor as u16, input.y));
    }

    /// Newest messages at the bottom; wraps long lines (tickets break mid-word).
    fn draw_chat(&self, f: &mut Frame, area: Rect) {
        let end = self.lines.len().saturating_sub(self.scroll);
        // ponytail: wraps only the last 200 messages per frame; enough to fill any screen.
        let shown: Vec<Line> = self.lines[end.saturating_sub(200)..end]
            .iter()
            .map(|l| styled(l))
            .collect();
        let chat = Paragraph::new(shown).wrap(Wrap { trim: false });
        let overflow = chat
            .line_count(area.width)
            .saturating_sub(area.height as usize) as u16;
        f.render_widget(chat.scroll((overflow, 0)), area);
    }
}

/// Status lines dim, nicks colored by name.
fn styled(line: &str) -> Line<'_> {
    if line.starts_with("* ") {
        return Line::styled(line, Style::new().fg(Color::DarkGray));
    }
    if line.starts_with('<')
        && let Some(end) = line.find('>')
    {
        const COLORS: [Color; 6] = [
            Color::Cyan,
            Color::Magenta,
            Color::Yellow,
            Color::Blue,
            Color::Green,
            Color::Red,
        ];
        let nick = &line[1..end];
        let color = COLORS[nick.bytes().map(usize::from).sum::<usize>() % COLORS.len()];
        return Line::from(vec![
            Span::styled(
                &line[..=end],
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(&line[end + 1..]),
        ]);
    }
    Line::from(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_history() {
        let mut app = App::default();
        for c in "hxi".chars() {
            app.edit(KeyCode::Char(c));
        }
        app.edit(KeyCode::Left);
        app.edit(KeyCode::Backspace);
        assert_eq!(app.input.iter().collect::<String>(), "hi");
        app.history = vec!["one".into(), "two".into()];
        app.recall = 2;
        app.edit(KeyCode::Up);
        app.edit(KeyCode::Up);
        assert_eq!(app.input.iter().collect::<String>(), "one");
        app.edit(KeyCode::Down);
        app.edit(KeyCode::Down);
        assert!(app.input.is_empty());
    }

    #[test]
    fn renders_chat_and_sidebar() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let app = App {
            lines: vec![
                "* in friends as alice".into(),
                "<bob> hello".into(),
                "x".repeat(70),
            ],
            ..App::default()
        };
        let side = Sidebar {
            room: "friends".into(),
            nick: "alice".into(),
            members: vec![("alice".into(), true, true), ("bob".into(), false, false)],
            models: vec!["m: bob".into()],
            addrs: 2,
        };
        term.draw(|f| app.draw(f, &side)).unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<String> = buf
            .content
            .chunks(60)
            .map(|r| r.iter().map(|c| c.symbol()).collect())
            .collect();
        let screen = rows.join("\n");
        println!("{screen}");
        assert!(rows[0].contains("friends · alice · 1/2 online · 2 addresses"));
        for s in ["<bob> hello", "~alice", "│ bob", "m: bob", "> "] {
            assert!(screen.contains(s), "missing {s:?}");
        }
        assert_eq!(screen.matches('x').count(), 70, "long line wraps fully");
    }
}
