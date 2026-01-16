use crate::agent::Agent;
use crate::ai_sdk::{
    assistant_content_from_response, AssistantMessage, ContentBlock, MessageParam,
    ResponseContentBlock, UserMessage,
};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use std::io;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

const INPUT_HEIGHT: u16 = 6;

// Restores terminal settings even if the loop exits early.
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Self {
        Self
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout().flush();
    }
}

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
    ToolUse {
        name: String,
        input: String,
    },
    ToolResult {
        content: String,
        is_error: bool,
    },
    Info(String),
}

#[derive(Debug, Clone)]
struct LineSpec {
    text: String,
    style: Style,
}

impl LineSpec {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

impl ChatMessage {
    fn line_specs(&self) -> Vec<LineSpec> {
        match self {
            ChatMessage::User(msg) => {
                let header_style = Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default().fg(Color::Blue);
                let mut lines = vec![LineSpec::new("You:", header_style)];
                for line in msg.lines() {
                    lines.push(LineSpec::new(format!("  {}", line), body_style));
                }
                lines
            }
            ChatMessage::Assistant(msg) => {
                let header_style = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default().fg(Color::Yellow);
                let mut lines = vec![LineSpec::new("Claude:", header_style)];
                for line in msg.lines() {
                    lines.push(LineSpec::new(format!("  {}", line), body_style));
                }
                lines
            }
            ChatMessage::ToolUse { name, input } => {
                let header_style = Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD);
                let body_style = Style::default().fg(Color::Green);
                let input_str = Self::truncate(input, 200, "...\n[truncated]");
                let mut lines = vec![LineSpec::new(format!("tool: {}(", name), header_style)];
                for line in input_str.lines() {
                    lines.push(LineSpec::new(format!("  {}", line), body_style));
                }
                lines.push(LineSpec::new(")", header_style));
                lines
            }
            ChatMessage::ToolResult { content, is_error } => {
                let body_style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let header_style = body_style.add_modifier(Modifier::BOLD);
                let content_str = Self::truncate(content, 300, "...\n[output truncated]");
                let mut lines = vec![LineSpec::new("→ Result:", header_style)];
                for line in content_str.lines() {
                    lines.push(LineSpec::new(format!("  {}", line), body_style));
                }
                lines
            }
            ChatMessage::Info(msg) => vec![LineSpec::new(
                format!("ℹ {}", msg),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            )],
        }
    }

    fn to_text(&self) -> Text<'static> {
        let lines = self
            .line_specs()
            .into_iter()
            .map(|spec| Line::from(Span::styled(spec.text, spec.style)))
            .collect::<Vec<_>>();
        Text::from(lines)
    }

    fn plain_lines(&self) -> Vec<String> {
        self.line_specs()
            .into_iter()
            .map(|spec| spec.text)
            .collect()
    }

    fn rendered_height(&self, width: u16) -> u16 {
        let width = width.max(1) as usize;
        let mut total = 0usize;
        for line in self.plain_lines() {
            let len = line.len().max(1);
            total += (len + width - 1) / width;
        }
        total as u16
    }

    fn truncate(value: &str, max: usize, suffix: &str) -> String {
        if value.len() > max {
            let end = max.min(value.len());
            format!("{}{}", &value[..end], suffix)
        } else {
            value.to_string()
        }
    }
}

#[derive(Debug)]
pub enum UiEvent {
    ConversationAppend(MessageParam),
    ApiResponse(String),
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        content: String,
        is_error: bool,
    },
    Error(String),
    Info(String),
    Quit,
}

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

pub struct App {
    messages: Vec<ChatMessage>,
    input: InputBuffer,
    should_quit: bool,
    sender: mpsc::Sender<UiEvent>,
    receiver: mpsc::Receiver<UiEvent>,
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
            is_loading: false,
            agent: Arc::new(agent),
            conversation: Vec::new(),
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let title = if self.is_loading {
            " Input (Enter to send, Esc to quit) [Thinking...] "
        } else {
            " Input (Enter to send, Esc to quit) "
        };

        let input_paragraph = Paragraph::new(self.input.render())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: false });

        f.render_widget(input_paragraph, area);

        let cursor_x = (self.input.cursor_x + 1) as u16;
        let cursor_y = self.input.cursor_y as u16;
        let x = (area.x + cursor_x).min(area.x + area.width - 2);
        let y = (area.y + 1 + cursor_y).min(area.y + area.height - 2);
        f.set_cursor_position((x, y));
    }

    fn append_message(
        &mut self,
        terminal: &mut TuiTerminal,
        message: ChatMessage,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let width = terminal.size()?.width;
        let height = message.rendered_height(width).saturating_add(1);
        let mut text = message.to_text();
        text.extend(Text::raw("\n"));
        // Insert above the inline viewport so the log stays in scrollback.
        terminal.insert_before(height, |buf| {
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            paragraph.render(buf.area, buf);
        })?;
        self.messages.push(message);
        Ok(())
    }

    fn handle_events(
        &mut self,
        terminal: &mut TuiTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                UiEvent::ApiResponse(msg) => {
                    self.append_message(terminal, ChatMessage::Assistant(msg))?;
                    self.is_loading = false;
                }
                UiEvent::ConversationAppend(message) => {
                    self.conversation.push(message);
                }
                UiEvent::ToolCall { name, input } => {
                    self.append_message(
                        terminal,
                        ChatMessage::ToolUse {
                            name,
                            input: serde_json::to_string(&input).unwrap_or_default(),
                        },
                    )?;
                }
                UiEvent::ToolResult { content, is_error } => {
                    self.append_message(
                        terminal,
                        ChatMessage::ToolResult {
                            content,
                            is_error,
                        },
                    )?;
                }
                UiEvent::Error(err) => {
                    self.append_message(terminal, ChatMessage::Info(format!("Error: {}", err)))?;
                    self.is_loading = false;
                }
                UiEvent::Info(msg) => {
                    if msg != "Done" {
                        self.append_message(terminal, ChatMessage::Info(msg))?;
                    }
                    self.is_loading = false;
                }
                UiEvent::Quit => {
                    self.should_quit = true;
                    return Ok(false);
                }
            }
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    self.should_quit = true;
                    let _ = self.sender.try_send(UiEvent::Quit);
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Esc => {
                        self.should_quit = true;
                        let _ = self.sender.try_send(UiEvent::Quit);
                        return Ok(false);
                    }
                    KeyCode::Enter => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            self.input.new_line();
                        } else if !self.input.is_empty() {
                            let msg = self.input.to_string();
                            if !msg.trim().is_empty() {
                                self.append_message(terminal, ChatMessage::User(msg.clone()))?;
                                self.conversation
                                    .push(MessageParam::User(UserMessage::from_text(msg.clone())));
                                self.input.clear();
                                self.is_loading = true;

                                let mut current_conversation = self.conversation.clone();

                                let agent = Arc::clone(&self.agent);
                                let sender = self.sender.clone();
                                tokio::spawn(async move {
                                    loop {
                                        match agent
                                            .run_inference(current_conversation.as_slice())
                                            .await
                                        {
                                            Ok(response) => {
                                                let mut tool_results: Vec<ContentBlock> =
                                                    Vec::new();

                                                for content in &response.content {
                                                    match content {
                                                        ResponseContentBlock::Text { text } => {
                                                            let _ = sender
                                                                .send(UiEvent::ApiResponse(
                                                                    text.clone(),
                                                                ))
                                                                .await;
                                                        }
                                                        ResponseContentBlock::ToolUse {
                                                            id,
                                                            name,
                                                            input,
                                                        } => {
                                                            let _ = sender
                                                                .send(UiEvent::ToolCall {
                                                                    name: name.clone(),
                                                                    input: input.clone(),
                                                                })
                                                                .await;

                                                            let result = agent
                                                                .execute_tool(
                                                                    id.clone(),
                                                                    name.clone(),
                                                                    input.clone(),
                                                                )
                                                                .await;

                                                            let (content, is_error) =
                                                                match &result {
                                                                    ContentBlock::ToolResult {
                                                                        content,
                                                                        is_error,
                                                                        ..
                                                                    } => (
                                                                        content.clone(),
                                                                        is_error.unwrap_or(false),
                                                                    ),
                                                                    _ => (String::new(), false),
                                                                };

                                                            let _ = sender
                                                                .send(UiEvent::ToolResult {
                                                                    content,
                                                                    is_error,
                                                                })
                                                                .await;

                                                            tool_results.push(result);
                                                        }
                                                    }
                                                }

                                                let assistant_content =
                                                    assistant_content_from_response(&response);

                                                let assistant_message = MessageParam::Assistant(
                                                    AssistantMessage::new(assistant_content),
                                                );
                                                current_conversation
                                                    .push(assistant_message.clone());
                                                let _ = sender
                                                    .send(UiEvent::ConversationAppend(
                                                        assistant_message,
                                                    ))
                                                    .await;

                                                if tool_results.is_empty() {
                                                    break;
                                                }

                                                let tool_message = MessageParam::User(
                                                    UserMessage::new(tool_results),
                                                );
                                                current_conversation.push(tool_message.clone());
                                                let _ = sender
                                                    .send(UiEvent::ConversationAppend(
                                                        tool_message,
                                                    ))
                                                    .await;
                                            }
                                            Err(e) => {
                                                let _ =
                                                    sender.send(UiEvent::Error(e.to_string())).await;
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
}

pub fn run_tui(agent: Agent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let (_, rows) = size()?;
    if rows > 0 {
        // Push existing screen content into scrollback without clearing it.
        for _ in 0..rows {
            writeln!(stdout)?;
        }
        stdout.flush()?;
    }
    execute!(stdout, MoveTo(0, 0))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(INPUT_HEIGHT),
        },
    )?;

    let mut app = App::new(agent);

    let _guard = TerminalGuard::new();

    terminal.draw(|f| app.draw(f))?;

    while !app.should_quit {
        if !app.handle_events(&mut terminal)? {
            break;
        }

        terminal.draw(|f| app.draw(f))?;

        std::thread::sleep(Duration::from_millis(10));
    }

    disable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::InputBuffer;

    #[test]
    fn input_buffer_shift_enter_inserts_new_line() {
        let mut buffer = InputBuffer::new();
        for ch in "hello".chars() {
            buffer.insert_char(ch);
        }
        buffer.new_line();
        for ch in "world".chars() {
            buffer.insert_char(ch);
        }

        assert_eq!(buffer.to_string(), "hello\nworld");
        assert_eq!(buffer.lines.len(), 2);
        assert_eq!(buffer.cursor_y, 1);
    }
}
