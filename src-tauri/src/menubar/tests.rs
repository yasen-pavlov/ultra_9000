use super::*;
use serde_json::json;

fn visible() -> Rect {
    Rect {
        x: 0.,
        y: 0.,
        width: 1440.,
        height: 875.,
    }
}
fn button() -> Rect {
    Rect {
        x: 1000.,
        y: 875.,
        width: 80.,
        height: 25.,
    }
}

#[test]
fn anchored_top_stays_fixed_across_content_height_changes() {
    let short = anchored_frame(button(), visible(), 160.);
    let tall = anchored_frame(button(), visible(), 600.);
    assert_eq!(short.x, 1000.);
    assert_eq!(short.y + short.height, 869.);
    assert_eq!(tall.y + tall.height, 869.);
    assert_eq!(tall.height, 600.);
}

#[test]
fn clamp_right_left_and_secondary_screen_with_negative_origin() {
    let frame = anchored_frame(
        Rect {
            x: 1430.,
            ..button()
        },
        visible(),
        200.,
    );
    assert_eq!(frame.x + frame.width, 1432.);
    let screen = Rect {
        x: -1920.,
        y: -300.,
        width: 1920.,
        height: 1080.,
    };
    let frame = anchored_frame(
        Rect {
            x: -1910.,
            y: 780.,
            ..button()
        },
        screen,
        200.,
    );
    assert_eq!(frame.x, -1910.);
    assert_eq!(frame.y + frame.height, 774.);
}

#[test]
fn clamp_height_design_limits_and_remaining_screen_space() {
    assert_eq!(anchored_frame(button(), visible(), 10.).height, MIN_HEIGHT);
    assert_eq!(
        anchored_frame(button(), visible(), 9999.).height,
        MAX_HEIGHT
    );
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            anchored_frame(button(), visible(), invalid).height,
            MIN_HEIGHT
        );
    }
    let screen = Rect {
        width: 200.,
        height: 100.,
        ..visible()
    };
    let frame = anchored_frame(button(), screen, 600.);
    assert_eq!(
        frame,
        Rect {
            x: 8.,
            y: 8.,
            width: 184.,
            height: 86.
        }
    );
}

#[test]
fn auto_hidden_menu_bar_uses_actual_status_window_and_visible_frame() {
    let frame = anchored_frame(
        Rect {
            y: 882.,
            ..button()
        },
        visible(),
        400.,
    );
    assert_eq!(frame.y + frame.height, 869.);
    let frame = anchored_frame(
        Rect {
            y: 840.,
            ..button()
        },
        visible(),
        400.,
    );
    assert_eq!(frame.y + frame.height, 869.);
}

#[test]
fn reply_preserves_control_success_fields_and_close_confirmation() {
    let fields = serde_json::from_value(json!({"itemId":"item-2", "visible":true})).unwrap();
    assert_eq!(
        serde_json::to_value(Reply::from(Ok(fields))).unwrap(),
        json!({"ok":true,"itemId":"item-2","visible":true})
    );
    let fields =
        serde_json::from_value(json!({"result":"needs-confirmation","reason":"agent-working"}))
            .unwrap();
    assert_eq!(
        serde_json::to_value(Reply::from(Ok(fields))).unwrap(),
        json!({"ok":true,"result":"needs-confirmation","reason":"agent-working"})
    );
}

#[test]
fn reply_preserves_stable_failure_code_and_message_without_socket_envelope() {
    let reply = Reply::from(Err(Failure {
        error: "unknown_item",
        message: "Item item-9 does not exist.".into(),
    }));
    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        json!({"ok":false,"error":"unknown_item","message":"Item item-9 does not exist."})
    );
}

#[test]
fn capped_counts_and_title_runs_match_segment_order_and_spacing() {
    use crate::tray_model::BarCounts;
    assert_eq!(cap(0), "0");
    assert_eq!(cap(99), "99");
    assert_eq!(cap(100), "99+");
    assert_eq!(cap(usize::MAX), "99+");
    assert!(title_runs(&BarCounts::default()).is_empty());
    let counts = BarCounts {
        needs_you: 100,
        error: 1,
        working: 2,
        on_job: 3,
        unread: 4,
        running: 110,
    };
    assert_eq!(
        title_runs(&counts),
        vec![
            Run::Spacer(6.),
            Run::Pill("99+".into()),
            Run::Spacer(8.),
            Run::Text {
                text: "×".into(),
                tone: Tone::Error,
                kern_after: 3.
            },
            Run::Text {
                text: "1".into(),
                tone: Tone::Error,
                kern_after: 0.
            },
            Run::Spacer(8.),
            Run::Arc,
            Run::Text {
                text: "2".into(),
                tone: Tone::Purple,
                kern_after: 0.
            },
            Run::Spacer(8.),
            Run::Ring,
            Run::Text {
                text: "3".into(),
                tone: Tone::Purple,
                kern_after: 0.
            },
        ]
    );
    assert_eq!(
        accessibility_label(&counts),
        "Ultra 9000, 100 need you, 1 error, 2 working, 3 on job, 4 unread, 110 running"
    );
    let unread = BarCounts {
        unread: 2,
        running: 5,
        ..Default::default()
    };
    assert!(title_runs(&unread).is_empty());
    assert_eq!(
        accessibility_label(&unread),
        "Ultra 9000, 2 unread, 5 running"
    );
    assert_eq!(
        accessibility_label(&BarCounts {
            running: 5,
            ..Default::default()
        }),
        "Ultra 9000, nothing waiting, 5 running"
    );
}

#[test]
fn each_nonzero_segment_starts_with_six_points_and_omits_zero_neighbors() {
    use crate::tray_model::BarCounts;
    for counts in [
        BarCounts {
            needs_you: 1,
            ..Default::default()
        },
        BarCounts {
            error: 1,
            ..Default::default()
        },
        BarCounts {
            working: 1,
            ..Default::default()
        },
        BarCounts {
            on_job: 1,
            ..Default::default()
        },
    ] {
        let runs = title_runs(&counts);
        assert_eq!(runs.first(), Some(&Run::Spacer(6.)));
        assert!(!runs.contains(&Run::Spacer(8.)));
    }
}

#[test]
fn visual_title_key_ignores_running_and_exact_capped_or_unread_totals() {
    use crate::tray_model::BarCounts;
    let original = BarCounts {
        needs_you: 100,
        unread: 1,
        ..Default::default()
    };
    let another = BarCounts {
        needs_you: 500,
        unread: 3,
        running: 25,
        ..Default::default()
    };
    assert_eq!(original.title_key(), another.title_key());
    assert_ne!(
        accessibility_label(&original),
        accessibility_label(&another)
    );
    assert_ne!(
        original.title_key(),
        BarCounts {
            needs_you: 99,
            ..another
        }
        .title_key()
    );
    assert_ne!(
        original.title_key(),
        BarCounts {
            unread: 0,
            ..another
        }
        .title_key()
    );
}
