use crate::{
    Agent, AssistantMessage, ContentBlock, MessageParam, ResponseContentBlock, UserMessage,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::io;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// Terminal cleanup guard - ensures terminal is restored even on panic
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Self {
        Self
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = io::stdout().flush();
    }
}

// ============================================================================
// UI Event Types
// ============================================================================

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
    ToolUse {
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Info(String),
}

impl ChatMessage {
    fn to_text(&self) -> Text<'static> {
        match self {
            ChatMessage::User(msg) => {
                let mut lines = vec![Line::from(Span::styled(
                    "You:",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ))];
                for line in msg.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Blue),
                    )));
                }
                Text::from(lines)
            }
            ChatMessage::Assistant(msg) => {
                let mut lines = vec![Line::from(Span::styled(
                    "Claude:",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))];
                for line in msg.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Yellow),
                    )));
                }
                Text::from(lines)
            }
            ChatMessage::ToolUse { name, input } => {
                let input_str = if input.len() > 200 {
                    format!(
                        "{}...\n[truncated]",
                        &input[..input.len().min(200)]
                    )
                } else {
                    input.clone()
                };
                Text::from(vec![
                    Line::from(Span::styled(
                        format!("tool: {}(", name),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        format!("  {}", input_str),
                        Style::default().fg(Color::Green),
                    )),
                    Line::from(Span::styled(
                        ")",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )),
                ])
            }
            ChatMessage::ToolResult {
                content, is_error, ..
            } => {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let content_str = if content.len() > 300 {
                    format!(
                        "{}...\n[output truncated]",
                        &content[..content.len().min(300)]
                    )
                } else {
                    content.clone()
                };
                Text::from(vec![
                    Line::from(Span::styled(
                        "→ Result:",
                        style.add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(format!("  {}", content_str), style)),
                ])
            }
            ChatMessage::Info(msg) => Text::from(Line::from(Span::styled(
                format!("ℹ {}", msg),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            ))),
        }
    }

    fn line_count(&self) -> usize {
        match self {
            ChatMessage::User(msg) => msg.lines().count().max(1) + 1,
            ChatMessage::Assistant(msg) => msg.lines().count().max(1) + 1,
            ChatMessage::ToolUse { input, .. } => input.lines().count().max(1) + 2,
            ChatMessage::ToolResult { content, .. } => content.lines().count().max(1) + 1,
            ChatMessage::Info(_) => 1,
        }
    }
}

#[derive(Debug)]
pub enum UiEvent {
    UserMessage(String),
    ApiResponse(String),
    ToolCall {
        name: String,
        id: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Error(String),
    Info(String),
    Quit,
}

// Simple multi-line input buffer
struct InputBuffer {
    lines: Vec<String>,
    cursor_x: usize,
    cursor_y: usize,
}

impl InputBuffer {
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_y];
        if self.cursor_x >= line.len() {
            line.push(c);
        } else {
            line.insert(self.cursor_x, c);
        }
        self.cursor_x += 1;
    }

    fn delete_char(&mut self) {
        let line = &mut self.lines[self.cursor_y];
        if self.cursor_x > 0 {
            line.remove(self.cursor_x - 1);
            self.cursor_x -= 1;
        } else if self.cursor_y > 0 {
            // Merge with previous line
            let prev_line = self.lines.remove(self.cursor_y);
            self.cursor_y -= 1;
            self.cursor_x = self.lines[self.cursor_y].len();
            self.lines[self.cursor_y].push_str(&prev_line);
        }
    }

    fn new_line(&mut self) {
        let line = &self.lines[self.cursor_y];
        let remaining: String = line.chars().skip(self.cursor_x).collect();
        self.lines[self.cursor_y] = line.chars().take(self.cursor_x).collect();
        self.lines.insert(self.cursor_y + 1, remaining);
        self.cursor_y += 1;
        self.cursor_x = 0;
    }

    fn move_left(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
        } else if self.cursor_y > 0 {
            self.cursor_y -= 1;
            self.cursor_x = self.lines[self.cursor_y].len();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_y].len();
        if self.cursor_x < line_len {
            self.cursor_x += 1;
        } else if self.cursor_y < self.lines.len() - 1 {
            self.cursor_y += 1;
            self.cursor_x = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_y > 0 {
            self.cursor_y -= 1;
            self.cursor_x = self.cursor_x.min(self.lines[self.cursor_y].len());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_y < self.lines.len() - 1 {
            self.cursor_y += 1;
            self.cursor_x = self.cursor_x.min(self.lines[self.cursor_y].len());
        }
    }

    fn to_string(&self) -> String {
        self.lines.join("\n")
    }

    fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    fn render(&self) -> Text<'static> {
        if self.is_empty() {
            return Text::from(Span::styled(
                "Type your message here...",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Text::from(
            self.lines
                .iter()
                .map(|l| Line::from(l.clone()))
                .collect::<Vec<_>>(),
        )
    }
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Application State
// ============================================================================

pub struct App {
    messages: Vec<ChatMessage>,
    input: InputBuffer,
    should_quit: bool,
    sender: mpsc::Sender<UiEvent>,
    receiver: mpsc::Receiver<UiEvent>,
    scroll_offset: usize,
    is_loading: bool,
    agent: Arc<Agent>,
    conversation: Vec<MessageParam>,
}

impl App {
    pub fn new(agent: Agent) -> Self {
        let (sender, receiver) = mpsc::channel(100);

        Self {
            messages: Vec::new(),
            input: InputBuffer::new(),
            should_quit: false,
            sender,
            receiver,
            scroll_offset: 0,
            is_loading: false,
            agent: Arc::new(agent),
            conversation: Vec::new(),
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let size = f.area();

        // Calculate heights
        let input_height = 6;
        let _messages_height = size.height.saturating_sub(input_height);

        // Split layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Min(0),
                    Constraint::Length(input_height.saturating_sub(1)),
                ]
                .as_ref(),
            )
            .split(size);

        // Messages area
        let messages_chunk = chunks[0];

        // Build message text
        let mut full_text = Text::default();
        for msg in &self.messages {
            full_text.extend(msg.to_text());
            full_text.extend(Text::raw("\n\n"));
        }

        if self.is_loading {
            full_text.extend(Text::from(Line::from(Span::styled(
                "Thinking...",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ))));
        }

        // Render messages with scroll
        let messages_paragraph = Paragraph::new(full_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Messages ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset as u16, 0));

        f.render_widget(messages_paragraph, messages_chunk);

        // Input area
        let input_chunk = chunks[1];
        let input_paragraph = Paragraph::new(self.input.render())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Input (Enter to send, Esc to quit) "),
            )
            .wrap(Wrap { trim: false });

        f.render_widget(input_paragraph, input_chunk);

        // Set cursor position
        let cursor_x = (self.input.cursor_x + 1) as u16;
        let cursor_y = self.input.cursor_y as u16;
        let x = (input_chunk.x + cursor_x).min(input_chunk.x + input_chunk.width - 2);
        let y = (input_chunk.y + 1 + cursor_y).min(input_chunk.y + input_chunk.height - 2);
        f.set_cursor_position((x, y));
    }

    fn handle_events(&mut self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Check for UI events from channel (non-blocking)
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                UiEvent::ApiResponse(msg) => {
                    self.messages.push(ChatMessage::Assistant(msg));
                    self.is_loading = false;
                    self.scroll_to_bottom();
                }
                UiEvent::ToolCall { name, input, .. } => {
                    self.messages.push(ChatMessage::ToolUse {
                        name,
                        input: serde_json::to_string(&input).unwrap_or_default(),
                    });
                    self.scroll_to_bottom();
                }
                UiEvent::ToolResult {
                    tool_use_id: _,
                    content,
                    is_error,
                } => {
                    self.messages.push(ChatMessage::ToolResult {
                        tool_use_id: String::new(),
                        content,
                        is_error,
                    });
                    self.scroll_to_bottom();
                }
                UiEvent::Error(err) => {
                    self.messages
                        .push(ChatMessage::Info(format!("Error: {}", err)));
                    self.is_loading = false;
                    self.scroll_to_bottom();
                }
                UiEvent::Info(msg) => {
                    if msg != "Done" {
                        self.messages.push(ChatMessage::Info(msg));
                        self.scroll_to_bottom();
                    }
                    self.is_loading = false;
                }
                UiEvent::Quit => {
                    self.should_quit = true;
                    return Ok(false);
                }
                UiEvent::UserMessage(_) => {}
            }
        }

        // Poll for terminal events with timeout
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    self.should_quit = true;
                    let _ = self.sender.blocking_send(UiEvent::Quit);
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Esc => {
                        self.should_quit = true;
                        let _ = self.sender.blocking_send(UiEvent::Quit);
                        return Ok(false);
                    }
                    KeyCode::Enter => {
                        if !self.input.is_empty() {
                            let msg = self.input.to_string();
                            if !msg.trim().is_empty() {
                                self.messages.push(ChatMessage::User(msg.clone()));
                                self.conversation
                                    .push(MessageParam::User(UserMessage::from_text(msg.clone())));
                                self.input.clear();
                                self.is_loading = true;
                                self.scroll_to_bottom();

                                // Spawn the async task without blocking
                                let agent = Arc::clone(&self.agent);
                                let sender = self.sender.clone();
                                tokio::spawn(async move {
                                    let mut current_conversation = vec![MessageParam::User(UserMessage::from_text(msg.clone()))];

                                    loop {
                                        match agent.run_inference(&current_conversation).await {
                                            Ok(response) => {
                                                let mut tool_results: Vec<ContentBlock> = Vec::new();

                                                for content in &response.content {
                                                    match content {
                                                        ResponseContentBlock::Text { text } => {
                                                            let _ = sender.send(UiEvent::ApiResponse(text.clone())).await;
                                                        }
                                                        ResponseContentBlock::ToolUse { id, name, input } => {
                                                            let _ = sender.send(UiEvent::ToolCall {
                                                                name: name.clone(),
                                                                id: id.clone(),
                                                                input: input.clone(),
                                                            }).await;

                                                            let result = agent.execute_tool(id.clone(), name.clone(), input.clone()).await;

                                                            let (content, is_error) = match &result {
                                                                ContentBlock::ToolResult { content, is_error, .. } => {
                                                                    (content.clone(), is_error.unwrap_or(false))
                                                                }
                                                                _ => (String::new(), false),
                                                            };

                                                            let _ = sender.send(UiEvent::ToolResult {
                                                                tool_use_id: id.clone(),
                                                                content,
                                                                is_error,
                                                            }).await;

                                                            tool_results.push(result);
                                                        }
                                                    }
                                                }

                                                let assistant_content: Vec<ContentBlock> = response
                                                    .content
                                                    .into_iter()
                                                    .map(|c| match c {
                                                        ResponseContentBlock::Text { text } => ContentBlock::Text { text },
                                                        ResponseContentBlock::ToolUse { id, name, input } => {
                                                            ContentBlock::ToolUse { id, name, input }
                                                        }
                                                    })
                                                    .collect();

                                                current_conversation.push(MessageParam::Assistant(AssistantMessage::new(assistant_content)));

                                                if tool_results.is_empty() {
                                                    break;
                                                }

                                                current_conversation.push(MessageParam::User(UserMessage::new(tool_results)));
                                            }
                                            Err(e) => {
                                                let _ = sender.send(UiEvent::Error(e.to_string())).await;
                                                break;
                                            }
                                        }
                                    }
                                    let _ = sender.send(UiEvent::Info("Done".to_string())).await;
                                });
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        self.input.insert_char(c);
                    }
                    KeyCode::Backspace => {
                        self.input.delete_char();
                    }
                    KeyCode::Left => {
                        self.input.move_left();
                    }
                    KeyCode::Right => {
                        self.input.move_right();
                    }
                    KeyCode::Up => {
                        self.input.move_up();
                    }
                    KeyCode::Down => {
                        self.input.move_down();
                    }
                    KeyCode::Home => {
                        self.input.cursor_x = 0;
                    }
                    KeyCode::End => {
                        self.input.cursor_x = self.input.lines[self.input.cursor_y].len();
                    }
                    _ => {}
                }
            }
        }

        Ok(true)
    }

    fn scroll_to_bottom(&mut self) {
        let total_lines: usize = self.messages.iter().map(|m| m.line_count()).sum();
        let gaps = self.messages.len().saturating_sub(1) * 2;
        self.scroll_offset = (total_lines + gaps + 2).saturating_sub(20);
    }
}

// ============================================================================
// TUI Entry Point
// ============================================================================

pub fn run_tui(agent: Agent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = App::new(agent);

    // Set up cleanup guard - will clean up even if we panic
    let _guard = TerminalGuard::new();

    // Initial draw
    terminal.draw(|f| app.draw(f))?;

    while !app.should_quit {
        terminal.draw(|f| app.draw(f))?;

        if !app.handle_events()? {
            break;
        }

        // Small sleep to prevent busy-waiting
        std::thread::sleep(Duration::from_millis(10));
    }

    // Normal cleanup - the guard will also handle this on drop, but we do it explicitly
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    let mut stdout = io::stdout();
    stdout.flush()?;

    Ok(())
}
