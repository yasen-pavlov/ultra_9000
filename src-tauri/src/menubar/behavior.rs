//! Platform-neutral menu-bar behavior and drawing metrics; no AppKit types or wiring.
use super::{Rect, Run, Tone};

pub fn panel_compatibility(
    window_size: usize,
    panel_size: usize,
    prevents_activation: bool,
) -> Result<(), &'static str> {
    if panel_size > window_size {
        Err("AppKit panel layout is larger than the Tauri window allocation")
    } else if !prevents_activation {
        Err("AppKit does not support nonactivating panel conversion")
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupPolicy {
    pub menu_on_left_click: bool,
    pub destroy_popover: bool,
}

/// A failed optional popover must leave a usable menu-only app, including partial creation.
pub fn startup_policy(popover_ready: bool, window_exists: bool) -> StartupPolicy {
    StartupPolicy {
        menu_on_left_click: !popover_ready,
        destroy_popover: !popover_ready && window_exists,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MousePhase {
    Down,
    Up,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickTarget {
    Status,
    Panel,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickRoute {
    Pass,
    Dismiss,
    Show,
    Hide,
    ContextDown,
    ContextUp,
    RestoreHighlight,
}

/// Actual native modifiers decide Ctrl-click and Cmd-drag before Tauri discards them.
pub fn click_route(
    target: ClickTarget,
    button: MouseButton,
    phase: MousePhase,
    control: bool,
    command: bool,
    open: bool,
) -> ClickRoute {
    if target != ClickTarget::Status {
        return if open && target == ClickTarget::Other && phase == MousePhase::Down {
            ClickRoute::Dismiss
        } else {
            ClickRoute::Pass
        };
    }
    if command {
        return ClickRoute::Pass;
    }
    match (button, phase, control) {
        (MouseButton::Right, MousePhase::Down, _) if open => ClickRoute::Dismiss,
        (MouseButton::Right | MouseButton::Other, _, _) => ClickRoute::Pass,
        (MouseButton::Left, MousePhase::Down, true) => ClickRoute::ContextDown,
        (MouseButton::Left, MousePhase::Up, true) => ClickRoute::ContextUp,
        (MouseButton::Left, MousePhase::Down, false) => {
            if open {
                ClickRoute::Hide
            } else {
                ClickRoute::Show
            }
        }
        (MouseButton::Left, MousePhase::Up, false) => ClickRoute::RestoreHighlight,
    }
}

/// Hidden, covered, or on another Space is not useful: h brings it to the current Space.
pub fn toggle_hides_main(visible: bool, unoccluded: bool, on_active_space: bool) -> bool {
    visible && unoccluded && on_active_space
}

pub fn button_help(menu_only: bool) -> &'static str {
    if menu_only {
        "Click to open the menu."
    } else {
        "Click for the sidebar. Control-click for the menu."
    }
}

impl Tone {
    pub fn color(self, dark: bool) -> u32 {
        match (self, dark) {
            (Self::Purple, true) => 0xc792ea,
            (Self::Purple, false) => 0x7443a8,
            (Self::Error, true) => 0xff8da1,
            (Self::Error, false) => 0xa8172f,
        }
    }
}

pub fn color_components(hex: u32, alpha: f64) -> [f64; 4] {
    [
        f64::from((hex >> 16) & 255) / 255.,
        f64::from((hex >> 8) & 255) / 255.,
        f64::from(hex & 255) / 255.,
        alpha,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillEdge {
    pub frame: Rect,
    pub radius: f64,
    pub thickness: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillGeometry {
    pub frame: Rect,
    pub radius: f64,
    pub edge: Option<PillEdge>,
    pub text_x: f64,
    pub baseline_y: f64,
}

/// Capped ASCII digits/99+ use fixed whole-point widths and a device-pixel light-bar edge.
pub fn pill_geometry(text: &str, dark: bool, scale: f64) -> PillGeometry {
    let width = match text.len() {
        1 => 22.,
        2 => 29.,
        _ => 36.,
    };
    let edge = 1. / scale.max(1.);
    PillGeometry {
        frame: Rect {
            x: 0.,
            y: 0.,
            width,
            height: 16.,
        },
        radius: 8.,
        edge: (!dark).then_some(PillEdge {
            frame: Rect {
                x: edge / 2.,
                y: edge / 2.,
                width: width - edge,
                height: 16. - edge,
            },
            radius: 8. - edge / 2.,
            thickness: edge,
        }),
        text_x: 9.5,
        baseline_y: 12.,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttachmentLayout {
    pub bounds: Rect,
    pub kern: f64,
}

/// Design baselines and spacing are independent of font rasterization and AppKit objects.
pub fn attachment_layout(run: &Run) -> Option<AttachmentLayout> {
    let (width, height, y, kern) = match run {
        Run::Spacer(width) => (*width, 0., 0., 0.),
        Run::Pill(text) => (pill_geometry(text, true, 1.).frame.width, 16., -3.5, 0.),
        Run::Arc | Run::Ring => (12., 12., -1.5, 3.),
        Run::Text { .. } => return None,
    };
    Some(AttachmentLayout {
        bounds: Rect {
            x: 0.,
            y,
            width,
            height,
        },
        kern,
    })
}
