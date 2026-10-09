//! Pure signal decoding: parse_omp_title, parse_osc777 and AgentTracker.
//! π: labels have no state, represented by the inner Option in parse_omp_title.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleState {
    Working,
    Idle,
    Attention,
}
pub fn parse_omp_title(raw: &str) -> Option<(Option<TitleState>, String)> {
    title_parts(raw).map(|(state, label)| (state, label.to_owned()))
}
fn title_parts(raw: &str) -> Option<(Option<TitleState>, &str)> {
    if let Some(label) = raw.strip_prefix("π:") {
        return Some((None, label.trim()));
    }
    let rest = raw.strip_prefix("π ")?;
    let (token, label) = rest.split_once(' ').unwrap_or((rest, ""));
    let state = match token {
        ">" => Some(TitleState::Idle),
        "!" => Some(TitleState::Attention),
        ":" => Some(TitleState::Working),
        s if s.chars().count() == 1
            && s.chars().all(|c| {
                matches!(
                    c,
                    '\u{2800}'..='\u{28ff}' | '○' | '◔' | '◑' | '◕' | '●' | '-' | '\\' | '|' | '/'
                )
            }) =>
        {
            Some(TitleState::Working)
        }
        _ => return None,
    };
    Some((state, label.trim()))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OmpEvent {
    SessionStart {
        session_id: String,
        cwd: Option<String>,
    },
    PromptSubmit,
    PermissionRequest,
    PermissionReplied,
    QuestionAsked,
    ToolComplete {
        tool_name: String,
    },
    Stop,
    StopFailure {
        aborted: bool,
    },
}
// Deserialize only metadata, never retaining query, response or summary text.
#[derive(Deserialize)]
struct Envelope {
    event: String,
    agent: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    tool_name: Option<String>,
    error_type: Option<String>,
}
fn partial_string(json: &str, key: &str) -> Option<String> {
    // Capture complete top-level fields before an EOF in a later query/response.
    // Side effects survive the enclosing object's missing closing delimiter.
    let mut found = None;
    struct Field<'a> {
        key: &'a str,
        found: &'a mut Option<String>,
    }
    impl<'de> serde::de::Visitor<'de> for Field<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("metadata object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
            while let Some(k) = map.next_key::<String>()? {
                if k == self.key {
                    *self.found = map.next_value::<Option<String>>()?;
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }
    let _ = serde::de::Deserializer::deserialize_map(
        &mut serde_json::Deserializer::from_str(json),
        Field {
            key,
            found: &mut found,
        },
    );
    found
}
pub fn parse_osc777(params: &str) -> Option<OmpEvent> {
    let json = params.strip_prefix("notify;warp://cli-agent;")?;
    if json.len() > 16 * 1024 {
        return None;
    }
    let e = serde_json::from_str::<Envelope>(json).ok();
    if e.as_ref()
        .and_then(|e| e.agent.as_deref())
        .is_some_and(|a| a != "omp")
    {
        return None;
    }
    let event = e
        .as_ref()
        .map(|e| e.event.clone())
        .or_else(|| partial_string(json, "event"))?;
    Some(match event.as_str() {
        "session_start" => OmpEvent::SessionStart {
            session_id: e
                .as_ref()
                .and_then(|e| e.session_id.clone())
                .or_else(|| partial_string(json, "session_id"))?,
            cwd: e.as_ref().and_then(|e| e.cwd.clone()),
        },
        "prompt_submit" => OmpEvent::PromptSubmit,
        "permission_request" => OmpEvent::PermissionRequest,
        "permission_replied" => OmpEvent::PermissionReplied,
        "question_asked" => OmpEvent::QuestionAsked,
        "tool_complete" => OmpEvent::ToolComplete {
            tool_name: e
                .as_ref()
                .and_then(|e| e.tool_name.clone())
                .unwrap_or_default(),
        },
        "stop" => OmpEvent::Stop,
        "stop_failure" => OmpEvent::StopFailure {
            aborted: e.as_ref().and_then(|e| e.error_type.as_deref()) == Some("aborted")
                || partial_string(json, "error_type").as_deref() == Some("aborted"),
        },
        _ => return None,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Unknown,
    Working,
    NeedsYou(Attention),
    YourTurn,
    Error,
}
impl Activity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Working => "working",
            Self::NeedsYou(_) => "needs-you",
            Self::YourTurn => "your-turn",
            Self::Error => "error",
        }
    }
    pub fn attention(self) -> Option<Attention> {
        if let Self::NeedsYou(a) = self {
            Some(a)
        } else {
            None
        }
    }
}
impl Serialize for Activity {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attention {
    Approval,
    Question,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTracker {
    activity: Activity,
    session_id: Option<String>,
    label: Option<String>,
    saw_osc777: bool,
    title_activity: Option<Activity>,
}
impl Default for AgentTracker {
    fn default() -> Self {
        Self::new()
    }
}
impl AgentTracker {
    pub fn new() -> Self {
        Self {
            activity: Activity::Unknown,
            session_id: None,
            label: None,
            saw_osc777: false,
            title_activity: None,
        }
    }
    pub fn on_title(&mut self, raw: &str) -> bool {
        let Some((state, label)) = title_parts(raw) else {
            return false;
        };
        self.title_activity = state.map(|state| match state {
            TitleState::Working => Activity::Working,
            TitleState::Idle => Activity::YourTurn,
            TitleState::Attention => Activity::NeedsYou(Attention::Approval),
        });
        let before = self.activity;
        let label_changed = self.label.as_deref() != Some(label);
        if label_changed {
            self.label = Some(label.to_owned());
        }
        // An idle tracker may recover a missed prompt event from the next working title.
        if !self.saw_osc777
            || self.activity == Activity::Unknown
            || self.activity == Activity::YourTurn && state == Some(TitleState::Working)
        {
            if let Some(state) = state {
                self.activity = match state {
                    TitleState::Working => Activity::Working,
                    TitleState::Idle => Activity::YourTurn,
                    TitleState::Attention => Activity::NeedsYou(Attention::Approval),
                };
            }
        }
        self.activity != before || label_changed
    }
    pub fn on_event(&mut self, ev: &OmpEvent) -> bool {
        let before = self.activity;
        let mut session_changed = false;
        self.saw_osc777 = true;
        match ev {
            OmpEvent::SessionStart { session_id, .. } => {
                if self.session_id.as_deref() != Some(session_id) {
                    self.activity = self.title_activity.unwrap_or(Activity::Unknown);
                    self.session_id = Some(session_id.clone());
                    session_changed = true;
                }
            }
            OmpEvent::PromptSubmit
            | OmpEvent::PermissionReplied
            | OmpEvent::ToolComplete { .. } => self.activity = Activity::Working,
            OmpEvent::PermissionRequest => self.activity = Activity::NeedsYou(Attention::Approval),
            OmpEvent::QuestionAsked => self.activity = Activity::NeedsYou(Attention::Question),
            OmpEvent::Stop | OmpEvent::StopFailure { aborted: true } => {
                self.activity = Activity::YourTurn
            }
            OmpEvent::StopFailure { aborted: false } => self.activity = Activity::Error,
        }
        // Protocol bookkeeping alone does not cause a workspace snapshot.
        self.activity != before || session_changed
    }
    pub fn activity(&self) -> Activity {
        self.activity
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}
#[cfg(test)]
mod tests;
