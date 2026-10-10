use super::*;
use crate::status::Attention;

fn item(id: &str, activity: Activity, since: u64) -> ItemState {
    ItemState {
        id: id.into(),
        tool: "omp".into(),
        kind: ToolKind::Agent,
        project_id: "atlas".into(),
        cwd: "/atlas".into(),
        title: id.into(),
        process: Process::Running,
        exit_code: None,
        failure: None,
        activity,
        attention: None,
        unread: true,
        conversation: None,
        model: None,
        started_at: 0,
        state_since: since,
        pid: None,
        foreground_job: None,
        background_jobs: Vec::new(),
    }
}
fn model(items: &[ItemState], visible: bool) -> TrayModel {
    let projects = [ProjectState {
        id: "atlas".into(),
        name: "atlas".into(),
        path: "/atlas".into(),
        pinned: false,
        missing: false,
        default_tool: "omp".into(),
        item_ids: vec![],
    }];
    TrayModel::from_items(items, &projects, |_| "π", visible, true)
}
#[test]
fn waiting_order_cap_remainder_and_attention() {
    let items = [
        item("work", Activity::Working, 90),
        item("turn", Activity::YourTurn, 5),
        item("new-question", Activity::NeedsYou(Attention::Question), 20),
        item("old-approval", Activity::NeedsYou(Attention::Approval), 10),
        item("older-work", Activity::Working, 30),
    ];
    let tray = model(&items, false);
    assert_eq!(
        tray.entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["old-approval", "new-question", "turn"]
    );
    assert_eq!(tray.more_running, 2);
    assert_eq!(tray.header, "Ultra 9000 · hidden · 5 running");
    assert_eq!(tray.tooltip, "Ultra 9000 · 2 need you");
    assert!(tray.attention());
    assert!(tray.notify_checked);
}
#[test]
fn agents_precede_terminals_and_use_recent_activity() {
    let mut shell = item("shell", Activity::Unknown, 99);
    shell.kind = ToolKind::Terminal;
    let mut seen = item("seen", Activity::YourTurn, 80);
    seen.unread = false;
    let mut exited = item("ended", Activity::Unknown, 100);
    exited.process = Process::Exited;
    let tray = model(
        &[
            shell,
            item("old", Activity::Working, 10),
            seen,
            item("new", Activity::Working, 90),
            exited,
        ],
        true,
    );
    assert_eq!(
        tray.entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["new", "seen", "old"]
    );
    assert_eq!(tray.more_running, 1);
    assert_eq!(tray.header, "Ultra 9000 · 4 running");
    assert_eq!(tray.visibility_label(), "Hide Ultra 9000");
    assert!(!tray.attention());
    assert_eq!(tray.tooltip, "Ultra 9000 · 4 running");
}
#[test]
fn markers_lead_the_title_and_detail_keeps_words() {
    let mut i = item(
        "API investigation",
        Activity::NeedsYou(Attention::Question),
        0,
    );
    let e = entry(&i, "π", "atlas", false);
    assert_eq!(
        (e.label.as_str(), e.detail.as_str()),
        ("π !  API investigation", "atlas · needs you")
    );
    i.activity = Activity::Working;
    assert_eq!(
        entry(&i, "π", "atlas", false).label,
        "π …  API investigation"
    );
    i.activity = Activity::OnJob;
    let on_job = entry(&i, "π", "atlas", false);
    assert_eq!(
        (on_job.label.as_str(), on_job.detail.as_str()),
        ("π …  API investigation", "atlas · on job")
    );
    i.activity = Activity::YourTurn;
    assert_eq!(
        entry(&i, "π", "atlas", false).label,
        "π ●  API investigation"
    );
    i.unread = false;
    let seen = entry(&i, "π", "atlas", false);
    assert_eq!(
        (seen.label.as_str(), seen.detail.as_str()),
        ("π  API investigation", "atlas · your turn")
    );
    i.activity = Activity::Unknown;
    assert_eq!(
        entry(&i, ">_", "meridian", false).label,
        ">_  API investigation"
    );
}
#[test]
fn project_is_added_only_to_disambiguate_equal_titles() {
    let mut a = item("a", Activity::Working, 1);
    let mut b = item("b", Activity::Working, 2);
    a.title = "notes".into();
    b.title = "notes".into();
    let unique = item("unique", Activity::Working, 3);
    let tray = model(&[a, b, unique], true);
    let labels: Vec<_> = tray.entries.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(
        labels,
        ["π …  unique", "π …  notes — atlas", "π …  notes — atlas"]
    );
}
#[test]
fn empty_hidden_menu_has_no_remainder_or_attention() {
    let tray = model(&[], false);
    assert_eq!(tray.header, "Ultra 9000 · hidden · 0 running");
    assert_eq!(tray.visibility_label(), "Show Ultra 9000");
    assert_eq!(tray.more_running, 0);
    assert!(tray.entries.is_empty());
    assert!(!tray.attention());
}
#[test]
fn workspace_snapshots_supply_tool_glyphs_and_the_bound_shortcuts() {
    use crate::{runtime::PLATFORM, shortcuts::Keymap, workspace};
    let host = workspace::test_support::install();
    let launch = |tool: &str| {
        let id = workspace::begin_launch(
            tool.into(),
            "/atlas".into(),
            "/atlas".into(),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        host.complete(&id);
        id
    };
    let agent = launch("omp");
    let shell = launch("shell");
    let tray = TrayModel::build(&workspace::snapshot().unwrap(), true, false);
    let label = |id: &str| {
        tray.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.label.clone())
            .unwrap()
    };
    assert!(label(&agent).starts_with("π "), "{}", label(&agent));
    assert!(label(&shell).starts_with(">_ "), "{}", label(&shell));
    assert_eq!(tray.header, "Ultra 9000 · 2 running");
    assert!(!tray.notify_checked);
    let keys = Keymap::defaults(PLATFORM);
    assert_eq!(tray.hide_shortcut, keys.label(Action::Hide));
    assert_eq!(tray.quit_shortcut, keys.label(Action::Quit));
    assert!(tray.hide_shortcut.is_some() && tray.quit_shortcut.is_some());
}
