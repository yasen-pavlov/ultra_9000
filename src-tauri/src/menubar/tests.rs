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
    assert_eq!(frame.y + frame.height, 834.);
}

#[test]
fn revealed_auto_hide_or_full_screen_bar_anchors_below_status_window_not_screen_top() {
    // visibleFrame includes the entire display while an auto-hidden/full-screen menu bar
    // is revealed. The status window's lower edge is the real exclusion boundary.
    let entire_screen = Rect {
        height: 900.,
        ..visible()
    };
    let frame = anchored_frame(button(), entire_screen, 400.);
    assert_eq!(frame.y + frame.height, 869.);
    assert_eq!(frame.y + frame.height + 6., button().y);
    let notch_screen = Rect {
        y: -200.,
        height: 1100.,
        ..visible()
    };
    let notch_button = Rect {
        y: 862.,
        height: 38.,
        ..button()
    };
    let frame = anchored_frame(notch_button, notch_screen, 400.);
    assert_eq!(frame.y + frame.height, 856.);
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

#[test]
fn unsupported_panel_and_partial_startup_preserve_a_menu_only_app() {
    assert!(panel_compatibility(128, 128, true).is_ok());
    assert!(panel_compatibility(128, 120, true).is_ok());
    assert_eq!(
        panel_compatibility(128, 129, true).unwrap_err(),
        "AppKit panel layout is larger than the Tauri window allocation"
    );
    assert_eq!(
        panel_compatibility(128, 128, false).unwrap_err(),
        "AppKit does not support nonactivating panel conversion"
    );
    assert!(panel_compatibility(128, 129, false).is_err());
    assert_eq!(
        startup_policy(true, true),
        StartupPolicy {
            menu_on_left_click: false,
            destroy_popover: false
        }
    );
    assert_eq!(
        startup_policy(false, true),
        StartupPolicy {
            menu_on_left_click: true,
            destroy_popover: true
        }
    );
    assert_eq!(
        startup_policy(false, false),
        StartupPolicy {
            menu_on_left_click: true,
            destroy_popover: false
        }
    );
}

#[test]
fn status_clicks_toggle_and_context_clicks_preserve_system_dragging() {
    use ClickRoute::*;
    use MouseButton::*;
    use MousePhase::*;
    let route = |button, phase, control, command, open| {
        click_route(ClickTarget::Status, button, phase, control, command, open)
    };
    assert_eq!(route(Left, Down, false, false, false), Show);
    assert_eq!(route(Left, Down, false, false, true), Hide);
    assert_eq!(route(Left, Up, false, false, true), RestoreHighlight);
    assert_eq!(route(Left, Down, true, false, true), ContextDown);
    assert_eq!(route(Left, Up, true, false, false), ContextUp);
    assert_eq!(route(Right, Down, false, false, true), Dismiss);
    assert_eq!(route(Right, Down, true, false, false), Pass);
    assert_eq!(route(Right, Up, false, false, true), Pass);
    assert_eq!(route(Other, Down, false, false, true), Pass);
    for button in [Left, Right, Other] {
        for phase in [Down, Up] {
            for open in [false, true] {
                for control in [false, true] {
                    assert_eq!(
                        route(button, phase, control, true, open),
                        Pass,
                        "Cmd-drag is native"
                    );
                }
            }
        }
    }
}

#[test]
fn outside_mouse_down_dismisses_only_an_open_panel_not_inside_events_or_mouse_up() {
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Other] {
        for phase in [MousePhase::Down, MousePhase::Up] {
            for open in [false, true] {
                assert_eq!(
                    click_route(ClickTarget::Panel, button, phase, false, false, open),
                    ClickRoute::Pass
                );
                let expected = if open && phase == MousePhase::Down {
                    ClickRoute::Dismiss
                } else {
                    ClickRoute::Pass
                };
                assert_eq!(
                    click_route(ClickTarget::Other, button, phase, true, true, open),
                    expected
                );
            }
        }
    }
}

#[test]
fn inactive_toggle_raises_hidden_or_covered_windows_and_hides_a_visible_one() {
    assert!(!toggle_hides_main(false, false, true));
    assert!(!toggle_hides_main(false, true, true));
    assert!(!toggle_hides_main(true, false, true));
    assert!(toggle_hides_main(true, true, true));
    assert!(!toggle_hides_main(true, true, false));
    assert!(!toggle_hides_main(true, false, false));
}

#[test]
fn appearance_palette_and_components_keep_exact_approved_contrast_colors() {
    assert_eq!(Tone::Purple.color(true), 0xc792ea);
    assert_eq!(Tone::Purple.color(false), 0x7443a8);
    assert_eq!(Tone::Error.color(true), 0xff8da1);
    assert_eq!(Tone::Error.color(false), 0xa8172f);
    assert_eq!(color_components(0, 1.), [0., 0., 0., 1.]);
    assert_eq!(color_components(0xffffff, 0.35), [1., 1., 1., 0.35]);
    assert_eq!(color_components(0xff9800, 1.), [1., 152. / 255., 0., 1.]);
}

#[test]
fn pill_metrics_cap_width_and_keep_one_device_pixel_inside_light_bar_edge() {
    for (text, width) in [("1", 22.), ("99", 29.), ("99+", 36.)] {
        let dark = pill_geometry(text, true, 2.);
        assert_eq!(
            dark.frame,
            Rect {
                x: 0.,
                y: 0.,
                width,
                height: 16.
            }
        );
        assert_eq!(dark.radius, 8.);
        assert_eq!((dark.text_x, dark.baseline_y), (9.5, 12.));
        assert!(dark.edge.is_none());
        for scale in [1., 2.] {
            let edge = pill_geometry(text, false, scale).edge.unwrap();
            assert_eq!(edge.thickness * scale, 1.);
            assert_eq!(edge.frame.x, edge.thickness / 2.);
            assert_eq!(edge.frame.width + edge.thickness, width);
            assert_eq!(edge.frame.height + edge.thickness, 16.);
            assert_eq!(edge.radius + edge.thickness / 2., 8.);
        }
    }
    assert_eq!(pill_geometry("1", false, 0.).edge.unwrap().thickness, 1.);
}

#[test]
fn attachment_layout_matches_native_title_baselines_and_kern() {
    let spacer = attachment_layout(&Run::Spacer(6.)).unwrap();
    assert_eq!(
        spacer.bounds,
        Rect {
            x: 0.,
            y: 0.,
            width: 6.,
            height: 0.
        }
    );
    assert_eq!(spacer.kern, 0.);
    let pill = attachment_layout(&Run::Pill("99+".into())).unwrap();
    assert_eq!(
        pill.bounds,
        Rect {
            x: 0.,
            y: -3.5,
            width: 36.,
            height: 16.
        }
    );
    assert_eq!(pill.kern, 0.);
    for run in [Run::Arc, Run::Ring] {
        let layout = attachment_layout(&run).unwrap();
        assert_eq!(
            layout.bounds,
            Rect {
                x: 0.,
                y: -1.5,
                width: 12.,
                height: 12.
            }
        );
        assert_eq!(layout.kern, 3.);
    }
    assert!(attachment_layout(&Run::Text {
        text: "×".into(),
        tone: Tone::Error,
        kern_after: 3.
    })
    .is_none());
}

#[test]
fn visible_resize_keeps_top_and_left_edges_when_the_menu_bar_moves() {
    let current = anchored_frame(button(), visible(), 300.);
    let top = current.y + current.height;
    for screen in [
        visible(),
        Rect {
            height: 900.,
            ..visible()
        },
    ] {
        for requested in [10., 450., 9999.] {
            let resized = resized_frame(current, screen, requested);
            assert_eq!(resized.y + resized.height, top);
            assert_eq!((resized.x, resized.width), (current.x, current.width));
            assert!((MIN_HEIGHT..=MAX_HEIGHT).contains(&resized.height));
        }
    }
}

#[test]
fn fallback_button_help_matches_menu_only_behavior() {
    assert_eq!(button_help(true), "Click to open the menu.");
    assert_eq!(
        button_help(false),
        "Click for the sidebar. Control-click for the menu."
    );
}
