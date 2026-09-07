//! Nonmodal recovery controls above the composer, leaving draft text intact.

use devo_protocol::native::rpc_turn::TurnRecovery;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget, Wrap},
};

use crate::render::renderable::Renderable;

#[derive(Default)]
pub(super) struct TurnRecoveryPanel {
    pub recovery: Option<TurnRecovery>,
    pub pending: bool,
    pub error: Option<String>,
}

impl Renderable for TurnRecoveryPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let Some(recovery) = &self.recovery else {
            return;
        };
        let controls = if self.pending {
            "Waiting for server…"
        } else {
            "Ctrl+R Continue   Ctrl+X Cancel"
        };
        let text = format!(
            "This turn stopped unexpectedly. Continue from the saved context?\n{}\n{controls}{}",
            recovery.reason,
            self.error
                .as_ref()
                .map_or(String::new(), |error| format!("\n{error}"))
        );
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let Some(recovery) = &self.recovery else {
            return 0;
        };
        let width = usize::from(width.max(1));
        ("This turn stopped unexpectedly. Continue from the saved context?"
            .len()
            .div_ceil(width)
            + recovery.reason.chars().count().max(1).div_ceil(width)
            + 1
            + self
                .error
                .as_ref()
                .map_or(0, |error| error.chars().count().max(1).div_ceil(width))) as u16
    }
}
