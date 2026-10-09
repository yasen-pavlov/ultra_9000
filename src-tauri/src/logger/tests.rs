use super::*;

#[test]
fn level_comes_from_the_setting_with_a_warn_default() {
    assert_eq!(level(None), LevelFilter::Warn);
    assert_eq!(level(Some("debug")), LevelFilter::Debug);
    assert_eq!(level(Some("TRACE")), LevelFilter::Trace);
    assert_eq!(level(Some("off")), LevelFilter::Off);
    assert_eq!(level(Some("verbose")), LevelFilter::Warn);
    assert_eq!(level(Some("")), LevelFilter::Warn);
}

#[test]
fn records_are_one_line_with_level_and_target() {
    let mut out = Vec::new();
    write_record(
        &mut out,
        &Record::builder()
            .level(log::Level::Warn)
            .target("ultra9000::runtime")
            .args(format_args!("config watch: {}", "gone"))
            .build(),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "[WARN ultra9000::runtime] config watch: gone\n"
    );
}
