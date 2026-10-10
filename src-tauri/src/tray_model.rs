//! Platform-independent, app-owned tray presentation (approved board 16).
use crate::{
    config::ToolKind,
    shortcuts::Action,
    status::Activity,
    workspace::{ItemState, Process, ProjectState, WorkspaceState},
};

pub const NOTIFY_LABEL: &str = "Notify when an agent needs me";
pub const QUIT_LABEL: &str = "Quit…";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    /// `{glyph} {marker}  {title}`: `!` needs you, `●` unseen your turn, `…` working, no marker
    /// otherwise. ` — {project}` is appended only when another running item has the same
    /// title. Kept short and marker-first because Noctalia's tray menu is a fixed 220 px,
    /// elides each row, and ignores dbusmenu row images.
    pub label: String,
    /// `{project} · {state word}` for menus with room for it (macOS menu bar).
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayModel {
    pub header: String,
    pub entries: Vec<Entry>,
    pub more_running: usize,
    pub visible: bool,
    pub notify_checked: bool,
    pub needs_you: usize,
    pub tooltip: String,
    pub hide_shortcut: Option<String>,
    pub quit_shortcut: Option<String>,
    pub dark_icon: bool,
}

impl TrayModel {
    pub fn build(state: &WorkspaceState, visible: bool, notify_checked: bool) -> Self {
        let mut model = Self::from_items(
            &state.items,
            &state.projects,
            |tool| {
                state
                    .tools
                    .iter()
                    .find(|t| t.id == tool)
                    .map(|t| t.glyph.as_str())
                    .filter(|g| !g.is_empty())
                    .unwrap_or("ag")
            },
            visible,
            notify_checked,
        );
        model.hide_shortcut = state
            .shortcuts
            .iter()
            .find(|s| s.action == Action::Hide)
            .and_then(|s| s.label.clone());
        model.quit_shortcut = state
            .shortcuts
            .iter()
            .find(|s| s.action == Action::Quit)
            .and_then(|s| s.label.clone());
        model
    }

    fn from_items<'a>(
        items: &[ItemState],
        projects: &[ProjectState],
        glyph: impl Fn(&str) -> &'a str,
        visible: bool,
        notify_checked: bool,
    ) -> Self {
        let mut running: Vec<_> = items
            .iter()
            .filter(|i| i.process == Process::Running)
            .collect();
        let count = running.len();
        let needs_you = running
            .iter()
            .filter(|i| matches!(i.activity, Activity::NeedsYou(_)))
            .count();
        running.sort_by_key(|i| {
            let group = if matches!(i.activity, Activity::NeedsYou(_)) {
                0
            } else if i.activity == Activity::YourTurn && i.unread {
                1
            } else if i.kind == ToolKind::Agent {
                2
            } else {
                3
            };
            // Waiting items are oldest first; other agents/items use newest activity first.
            let time = if group < 2 {
                i.state_since
            } else {
                u64::MAX - i.state_since
            };
            (group, time, i.started_at, i.id.as_str())
        });
        let entries = running
            .iter()
            .take(3)
            .map(|i| {
                let project = projects
                    .iter()
                    .find(|p| p.id == i.project_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                let duplicate = running.iter().any(|o| o.id != i.id && o.title == i.title);
                entry(i, glyph(&i.tool), project, duplicate)
            })
            .collect();
        let hidden = if visible { "" } else { " · hidden" };
        Self {
            header: format!("Ultra 9000{hidden} · {count} running"),
            entries,
            more_running: count.saturating_sub(3),
            visible,
            notify_checked,
            needs_you,
            tooltip: if needs_you > 0 {
                format!("Ultra 9000 · {needs_you} need you")
            } else {
                format!("Ultra 9000 · {count} running")
            },
            hide_shortcut: None,
            quit_shortcut: None,
            dark_icon: false,
        }
    }

    pub fn visibility_label(&self) -> &'static str {
        if self.visible {
            "Hide Ultra 9000"
        } else {
            "Show Ultra 9000"
        }
    }

    pub fn attention(&self) -> bool {
        self.needs_you > 0
    }
}

fn entry(item: &ItemState, glyph: &str, project: &str, duplicate_title: bool) -> Entry {
    let (marker, word) = match item.activity {
        Activity::NeedsYou(_) => (Some('!'), "needs you"),
        Activity::YourTurn => (item.unread.then_some('●'), "your turn"),
        Activity::Working => (Some('…'), "working"),
        Activity::OnJob => (Some('…'), "on job"),
        _ => (None, "running"),
    };
    let lead = marker.map_or_else(|| format!("{glyph}  "), |m| format!("{glyph} {m}  "));
    let label = if duplicate_title {
        format!("{lead}{} — {project}", item.title)
    } else {
        format!("{lead}{}", item.title)
    };
    Entry {
        id: item.id.clone(),
        label,
        detail: format!("{project} · {word}"),
    }
}

#[cfg(test)]
mod tests;
