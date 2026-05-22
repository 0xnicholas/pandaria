use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub text: Color,
    pub accent: Color,
    pub muted: Color,
    pub dim: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub border: Color,
    pub heading: Color,
    pub code_bg: Color,
    pub user_message_bg: Color,
    pub tool_pending_bg: Color,
    pub tool_success_bg: Color,
    pub tool_error_bg: Color,
    pub thinking_text: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Color::Rgb(220, 220, 220),
            accent: Color::Rgb(100, 180, 255),
            muted: Color::Rgb(128, 128, 128),
            dim: Color::Rgb(80, 80, 80),
            success: Color::Rgb(80, 200, 120),
            error: Color::Rgb(255, 80, 80),
            warning: Color::Rgb(255, 200, 70),
            border: Color::Rgb(60, 60, 60),
            heading: Color::Rgb(255, 220, 100),
            code_bg: Color::Rgb(30, 30, 40),
            user_message_bg: Color::Rgb(25, 40, 60),
            tool_pending_bg: Color::Rgb(50, 50, 20),
            tool_success_bg: Color::Rgb(20, 50, 20),
            tool_error_bg: Color::Rgb(50, 20, 20),
            thinking_text: Color::Rgb(150, 150, 200),
        }
    }
}

impl Theme {
    pub fn body(&self) -> Style {
        Style::default().fg(self.text)
    }
    pub fn bold(&self) -> Style {
        Style::default().fg(self.text).add_modifier(Modifier::BOLD)
    }
    pub fn heading(&self, _level: u8) -> Style {
        Style::default()
            .fg(self.heading)
            .add_modifier(Modifier::BOLD)
    }
    pub fn code(&self) -> Style {
        Style::default().fg(self.accent).bg(self.code_bg)
    }
    pub fn link(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::UNDERLINED)
    }
    pub fn italic(&self) -> Style {
        Style::default()
            .fg(self.text)
            .add_modifier(Modifier::ITALIC)
    }
    pub fn muted(&self) -> Style {
        Style::default().fg(self.muted)
    }
}
