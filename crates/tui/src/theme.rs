use ratatui::style::Color;

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Theme {
    pub(crate) name: String,
    pub(crate) accent_color: Color,
    pub(crate) cell_line_color: Color,
    pub(crate) error_color: Color,
    pub(crate) reasoning_label_color: Color,
    pub(crate) reasoning_text_color: Color,
}

#[derive(Debug, Clone)]
pub(crate) struct ThemeSet {
    pub(crate) themes: Vec<Theme>,
}

impl Default for ThemeSet {
    fn default() -> Self {
        Self::builtin()
    }
}

impl ThemeSet {
    pub(crate) fn builtin() -> Self {
        Self {
            themes: vec![
                Theme {
                    name: "devo (default)".into(),
                    accent_color: Color::Rgb(0x58, 0xA6, 0xFF), // blue
                    cell_line_color: Color::Rgb(0x8B, 0x94, 0x9E), // gray
                    error_color: Color::Rgb(0xF8, 0x51, 0x49),  // red
                    reasoning_label_color: Color::Rgb(0xD2, 0xA8, 0xFF), // purple
                    reasoning_text_color: Color::Rgb(0x8B, 0x94, 0x9E), // gray
                },
                Theme {
                    name: "dark".into(),
                    accent_color: Color::Rgb(0x58, 0xA6, 0xFF), // blue
                    cell_line_color: Color::Rgb(0x48, 0x4F, 0x58), // muted gray
                    error_color: Color::Rgb(0xDA, 0x36, 0x3B),  // warm red
                    reasoning_label_color: Color::Rgb(0xD2, 0xA8, 0xFF), // purple
                    reasoning_text_color: Color::Rgb(0x6E, 0x76, 0x81), // muted gray
                },
                Theme {
                    name: "light".into(),
                    accent_color: Color::Rgb(0x09, 0x6D, 0xD9), // vivid blue
                    cell_line_color: Color::Rgb(0x8C, 0x95, 0x9E), // gray
                    error_color: Color::Rgb(0xCF, 0x22, 0x2E),  // red
                    reasoning_label_color: Color::Rgb(0x82, 0x50, 0xDF), // purple
                    reasoning_text_color: Color::Rgb(0x65, 0x6D, 0x76), // gray
                },
                Theme {
                    name: "aurora".into(),
                    accent_color: Color::Rgb(0x78, 0xD0, 0xA8), // teal green
                    cell_line_color: Color::Rgb(0x7B, 0x84, 0x92), // gray
                    error_color: Color::Rgb(0xF7, 0x76, 0x8E),  // rose
                    reasoning_label_color: Color::Rgb(0xE0, 0xAF, 0x68), // warm gold
                    reasoning_text_color: Color::Rgb(0xA9, 0xB1, 0xBF), // light gray
                },
            ],
        }
    }

    pub(crate) fn find(&self, name: &str) -> Option<&Theme> {
        self.themes.iter().find(|t| t.name == name)
    }

    pub(crate) fn default_theme() -> &'static str {
        "devo (default)"
    }

    pub(crate) fn tips() -> &'static [&'static str] {
        &[
            "Press Ctrl+T to toggle full-screen mode.",
            "Type / to see available slash commands.",
            "Use Ctrl+C to interrupt an active task.",
            "Press Up/Down in the composer to browse input history.",
            "Use /model to switch between configured models.",
            "Use /settings to change the UI color scheme.",
            "Use /status to see session configuration and token usage.",
            "Use /compact to reclaim context window space.",
            "Use /resume to pick up a previous chat.",
            "Use /new to start a fresh session.",
            "Press Ctrl+C twice while idle, or use /exit, to quit.",
            "Alt+Up/Alt+Down to select turns in the transcript.",
            "Use /diff to review changes.",
            "Reasoning effort can be configured via /model picker.",
        ]
    }
}
