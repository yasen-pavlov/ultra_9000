//! An agent item's activity as the UI shows it. OMP items get theirs from the status link
//! (`omp_link`); other items stay unknown.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Unknown,
    Working,
    NeedsYou(Attention),
    /// The agent ended its turn while its background jobs run; it resumes when they end.
    OnJob,
    YourTurn,
    Error,
}
impl Activity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Working => "working",
            Self::NeedsYou(_) => "needs-you",
            Self::OnJob => "on-job",
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
#[cfg(test)]
mod tests;
