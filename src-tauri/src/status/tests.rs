use super::*;
#[test]
fn activity_wire_names_and_attention_kind() {
    for (activity, name) in [
        (Activity::Unknown, "unknown"),
        (Activity::Working, "working"),
        (Activity::NeedsYou(Attention::Question), "needs-you"),
        (Activity::YourTurn, "your-turn"),
        (Activity::OnJob, "on-job"),
        (Activity::Error, "error"),
    ] {
        assert_eq!(serde_json::to_value(activity).unwrap(), name);
    }
    assert_eq!(
        Activity::NeedsYou(Attention::Question).attention(),
        Some(Attention::Question)
    );
    assert_eq!(Activity::Working.attention(), None);
    assert_eq!(
        serde_json::to_value(Attention::Approval).unwrap(),
        "approval"
    );
}
