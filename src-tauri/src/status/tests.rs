use super::*;
#[test]
fn title_parsing() {
    for frame in "⠋⠹⠸⠼⠴⠦⠧⠇⠏○◔◑◕●⠁⠂⠄⠠⠐⠈-\\|/".chars() {
        assert_eq!(
            parse_omp_title(&format!("π {frame} label")),
            Some((Some(TitleState::Working), "label".into()))
        );
    }
    assert_eq!(
        parse_omp_title("π > x"),
        Some((Some(TitleState::Idle), "x".into()))
    );
    assert_eq!(
        parse_omp_title("π ! x"),
        Some((Some(TitleState::Attention), "x".into()))
    );
    assert_eq!(parse_omp_title("π: x"), Some((None, "x".into())));
    assert!(parse_omp_title("shell").is_none());
}
#[test]
fn osc_metadata_and_truncation() {
    assert_eq!(parse_osc777("notify;warp://cli-agent;{\"event\":\"session_start\",\"session_id\":\"abc\",\"query\":\"truncated"), Some(OmpEvent::SessionStart { session_id: "abc".into(), cwd: None }));
    assert_eq!(
        parse_osc777(
            "notify;warp://cli-agent;{\"event\":\"stop_failure\",\"error_type\":\"aborted\"}"
        ),
        Some(OmpEvent::StopFailure { aborted: true })
    );
    assert!(parse_osc777("notify;other;{}").is_none());
    assert!(
        parse_osc777("notify;warp://cli-agent;{\"query\":\"\\\"event\\\":\\\"stop\\\"\"").is_none()
    );
}
#[test]
fn tracker_transitions() {
    let mut t = AgentTracker::new();
    assert!(t.on_title("π ⠋ x"));
    assert!(!t.on_title("π ⠹ x"));
    t.on_event(&OmpEvent::QuestionAsked);
    t.on_title("π > x");
    assert_eq!(t.activity(), Activity::NeedsYou(Attention::Question));
    t.on_event(&OmpEvent::ToolComplete {
        tool_name: "ask".into(),
    });
    assert_eq!(t.activity(), Activity::Working);
    t.on_event(&OmpEvent::Stop);
    assert_eq!(t.activity(), Activity::YourTurn);
    t.on_title("π ⠋ x");
    assert_eq!(t.activity(), Activity::Working);
    t.on_event(&OmpEvent::StopFailure { aborted: false });
    assert_eq!(t.activity(), Activity::Error);
    t.on_event(&OmpEvent::StopFailure { aborted: true });
    assert_eq!(t.activity(), Activity::YourTurn);
}
#[test]
fn session_start_allows_idle_and_attention_title_fallback() {
    for (title, expected) in [
        ("π > idle", Activity::YourTurn),
        ("π ! approval", Activity::NeedsYou(Attention::Approval)),
    ] {
        let mut tracker = AgentTracker::new();
        tracker.on_event(&OmpEvent::SessionStart {
            session_id: "new".into(),
            cwd: None,
        });
        tracker.on_title(title);
        assert_eq!(tracker.activity(), expected);
        tracker.on_event(&OmpEvent::PromptSubmit);
        tracker.on_title(title);
        assert_eq!(tracker.activity(), Activity::Working);
        let mut before = AgentTracker::new();
        before.on_title(title);
        before.on_event(&OmpEvent::SessionStart {
            session_id: "new".into(),
            cwd: None,
        });
        assert_eq!(before.activity(), expected);
    }
}
fn osc(json: &str) -> Option<OmpEvent> {
    parse_osc777(&format!("notify;warp://cli-agent;{json}"))
}
#[test]
fn every_omp_event_kind_decodes_from_complete_metadata() {
    assert_eq!(
        osc(r#"{"event":"session_start","agent":"omp","session_id":"s1","cwd":"/atlas"}"#),
        Some(OmpEvent::SessionStart {
            session_id: "s1".into(),
            cwd: Some("/atlas".into())
        })
    );
    for (event, expected) in [
        ("prompt_submit", OmpEvent::PromptSubmit),
        ("permission_request", OmpEvent::PermissionRequest),
        ("permission_replied", OmpEvent::PermissionReplied),
        ("question_asked", OmpEvent::QuestionAsked),
        ("stop", OmpEvent::Stop),
        ("stop_failure", OmpEvent::StopFailure { aborted: false }),
    ] {
        assert_eq!(osc(&format!(r#"{{"event":"{event}"}}"#)), Some(expected));
    }
    assert_eq!(
        osc(r#"{"event":"tool_complete","tool_name":"bash","response":"secret"}"#),
        Some(OmpEvent::ToolComplete {
            tool_name: "bash".into()
        })
    );
    assert_eq!(
        osc(r#"{"event":"tool_complete"}"#),
        Some(OmpEvent::ToolComplete {
            tool_name: String::new()
        })
    );
}
#[test]
fn foreign_unknown_oversized_and_malformed_signals_are_ignored() {
    assert_eq!(osc(r#"{"event":"stop","agent":"claude"}"#), None);
    assert_eq!(osc(r#"{"event":"compaction"}"#), None);
    assert_eq!(osc("[1, 2]"), None);
    // A session start without an id cannot bind a session.
    assert_eq!(osc(r#"{"event":"session_start"}"#), None);
    let padding = "x".repeat(16 * 1024);
    assert_eq!(osc(&format!(r#"{{"event":"stop","q":"{padding}"}}"#)), None);
}
#[test]
fn truncated_payloads_keep_fields_before_the_cut() {
    assert_eq!(
        osc(r#"{"query":"long","event":"stop_failure","error_type":"aborted","response":"cut"#),
        Some(OmpEvent::StopFailure { aborted: true })
    );
    assert_eq!(
        osc(r#"{"event":"stop_failure","error_type":"provider","response":"cut"#),
        Some(OmpEvent::StopFailure { aborted: false })
    );
    assert_eq!(osc(r#"{"query":"cut before the event"#), None);
}
#[test]
fn unknown_title_tokens_leave_the_tracker_untouched() {
    assert_eq!(parse_omp_title("π ⠋⠋ label"), None);
    assert_eq!(parse_omp_title("π ? label"), None);
    assert_eq!(
        parse_omp_title("π >"),
        Some((Some(TitleState::Idle), String::new()))
    );
    let mut tracker = AgentTracker::default();
    assert!(!tracker.on_title("π ? label"));
    assert!(!tracker.on_title("vim"));
    assert_eq!(tracker, AgentTracker::new());
}
#[test]
fn labels_update_without_state_and_explicit_events_outrank_titles() {
    let mut tracker = AgentTracker::new();
    assert!(tracker.on_title("π: refactor"));
    assert_eq!(tracker.label(), Some("refactor"));
    assert_eq!(tracker.activity(), Activity::Unknown);
    assert!(!tracker.on_title("π: refactor"));
    assert!(tracker.on_event(&OmpEvent::PermissionRequest));
    assert_eq!(tracker.activity(), Activity::NeedsYou(Attention::Approval));
    // After OSC 777, a working title does not override a pending approval.
    assert!(!tracker.on_title("π ⠋ refactor"));
    assert_eq!(tracker.activity(), Activity::NeedsYou(Attention::Approval));
    assert!(tracker.on_event(&OmpEvent::PermissionReplied));
    assert_eq!(tracker.activity(), Activity::Working);
    // Bookkeeping that changes nothing reports no change.
    assert!(!tracker.on_event(&OmpEvent::PromptSubmit));
}
#[test]
fn session_start_rebinds_only_on_a_new_session_id() {
    let mut tracker = AgentTracker::new();
    let start = |id: &str| OmpEvent::SessionStart {
        session_id: id.into(),
        cwd: None,
    };
    assert!(tracker.on_event(&start("a")));
    assert_eq!(tracker.session_id(), Some("a"));
    tracker.on_event(&OmpEvent::PromptSubmit);
    assert!(!tracker.on_event(&start("a")));
    assert_eq!(tracker.activity(), Activity::Working);
    // A new session without a stateful title starts unknown.
    assert!(tracker.on_event(&start("b")));
    assert_eq!(tracker.session_id(), Some("b"));
    assert_eq!(tracker.activity(), Activity::Unknown);
}
#[test]
fn activity_wire_names_and_attention_kind() {
    for (activity, name) in [
        (Activity::Unknown, "unknown"),
        (Activity::Working, "working"),
        (Activity::NeedsYou(Attention::Question), "needs-you"),
        (Activity::YourTurn, "your-turn"),
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
