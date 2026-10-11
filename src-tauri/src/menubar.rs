//! Menu-bar popover command replies and point-space geometry; AppKit stays in `macos`.
use crate::control::{Done, Failure};
use serde::Serialize;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    create, hide, is_popover_window, recover_menu_only, set_height, show, update_menu_bar_title,
    WINDOW,
};
#[cfg(any(target_os = "macos", test))]
mod behavior;
#[cfg(target_os = "macos")]
mod render;
#[cfg(any(target_os = "macos", test))]
pub use behavior::*;

/// Same payload as a control-link reply, without transport-only `type` and `id` fields.
/// Close confirmation is a successful reply with `result: "needs-confirmation"` and `reason`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Reply {
    Success {
        ok: bool,
        #[serde(flatten)]
        fields: Done,
    },
    Failure {
        ok: bool,
        error: &'static str,
        message: String,
    },
}

impl From<Result<Done, Failure>> for Reply {
    fn from(result: Result<Done, Failure>) -> Self {
        match result {
            Ok(fields) => Self::Success { ok: true, fields },
            Err(failure) => Self::Failure {
                ok: false,
                error: failure.error,
                message: failure.message,
            },
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub const MIN_HEIGHT: f64 = 200.;
#[cfg(any(target_os = "macos", test))]
pub const MAX_HEIGHT: f64 = 600.;
#[cfg(any(target_os = "macos", test))]
pub const WIDTH: f64 = 320.;

/// AppKit screen coordinates in points (bottom-left origin), not Tauri physical pixels.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Left-align to the clicked status button, six points below both its actual lower edge
/// and the visible screen's menu-bar edge (auto-hidden/full-screen bars overlap visibleFrame).
#[cfg(any(target_os = "macos", test))]
pub fn anchored_frame(button: Rect, visible: Rect, requested_height: f64) -> Rect {
    let margin = 8_f64.min(visible.width.max(0.) / 2.);
    let anchor = (visible.y + visible.height.max(0.))
        .min(button.y)
        .max(visible.y);
    let top = anchor - 6_f64.min((anchor - visible.y).max(0.));
    let available_height = (top - visible.y - 8.).max(0.);
    let height = clamped_height(requested_height).min(available_height);
    let width = WIDTH.min((visible.width - margin * 2.).max(0.));
    let x = button.x.clamp(
        visible.x + margin,
        visible.x + visible.width.max(0.) - margin - width,
    );
    Rect {
        x,
        y: top - height,
        width,
        height,
    }
}

#[cfg(any(target_os = "macos", test))]
fn clamped_height(height: f64) -> f64 {
    if height.is_finite() {
        height.clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        MIN_HEIGHT
    }
}

/// Layout-driven height changes preserve the already shown panel's top/left edges, even
/// if revealing or auto-hiding the bar has since changed the status window/visible frame.
#[cfg(any(target_os = "macos", test))]
pub fn resized_frame(current: Rect, visible: Rect, requested_height: f64) -> Rect {
    let top = current.y + current.height;
    let height = clamped_height(requested_height).min((top - visible.y - 8.).max(0.));
    Rect {
        y: top - height,
        height,
        ..current
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Purple,
    Error,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Spacer(f64),
    Pill(String),
    Arc,
    Ring,
    Text {
        text: String,
        tone: Tone,
        kern_after: f64,
    },
}

#[cfg(any(target_os = "macos", test))]
pub fn cap(count: usize) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

/// Stable static segment order and exact attachment spacing from design board 07.
#[cfg(any(target_os = "macos", test))]
pub fn title_runs(counts: &crate::tray_model::BarCounts) -> Vec<Run> {
    let capacity = usize::from(counts.needs_you > 0) * 2
        + [counts.error, counts.working, counts.on_job]
            .iter()
            .filter(|count| **count > 0)
            .count()
            * 3;
    let mut runs = Vec::with_capacity(capacity);
    for (count, glyph, tone) in [
        (counts.needs_you, None, Tone::Purple),
        (counts.error, Some("×"), Tone::Error),
        (counts.working, Some("arc"), Tone::Purple),
        (counts.on_job, Some("ring"), Tone::Purple),
    ] {
        if count == 0 {
            continue;
        }
        runs.push(Run::Spacer(if runs.is_empty() { 6. } else { 8. }));
        let Some(glyph) = glyph else {
            runs.push(Run::Pill(cap(count)));
            continue;
        };
        runs.push(match glyph {
            "arc" => Run::Arc,
            "ring" => Run::Ring,
            _ => Run::Text {
                text: glyph.into(),
                tone,
                kern_after: 3.,
            },
        });
        runs.push(Run::Text {
            text: cap(count),
            tone,
            kern_after: 0.,
        });
    }
    runs
}

#[cfg(any(target_os = "macos", test))]
pub fn accessibility_label(counts: &crate::tray_model::BarCounts) -> String {
    use std::fmt::Write;
    let mut label = String::from("Ultra 9000");
    for (count, singular, plural) in [
        (counts.needs_you, "needs you", "need you"),
        (counts.error, "error", "errors"),
        (counts.working, "working", "working"),
        (counts.on_job, "on job", "on job"),
        (counts.unread, "unread", "unread"),
    ] {
        if count > 0 {
            let _ = write!(
                label,
                ", {count} {}",
                if count == 1 { singular } else { plural }
            );
        }
    }
    if label == "Ultra 9000" {
        label.push_str(", nothing waiting");
    }
    let _ = write!(label, ", {} running", counts.running);
    label
}

#[cfg(test)]
mod tests;
