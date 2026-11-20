use super::{feed::*, store::*};

const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn imported_copy_survives_restart_and_disappears_on_removal() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let sources = CalendarStore::default();
    let added = sources.add_file(dir, " Project ", SAMPLE.into()).unwrap();
    assert_eq!(added.name, "Project");
    assert_eq!(added.kind, SourceKind::File);
    assert_eq!(
        sources
            .add_file(dir, "Duplicate", SAMPLE.into())
            .err()
            .as_deref(),
        Some(DUPLICATE)
    );
    drop(sources);
    let restored = CalendarStore::default();
    assert_eq!(restored.list(dir).unwrap(), vec![added.clone()]);
    assert_eq!(restored.read(dir, added.id, false).unwrap(), SAMPLE);
    restored.remove(dir, added.id).unwrap();
    assert!(restored.list(dir).unwrap().is_empty());
    assert_eq!(
        restored.read(dir, added.id, false).err().as_deref(),
        Some(MISSING)
    );
}

#[test]
fn simultaneous_duplicate_imports_create_one_source() {
    let directory = tempfile::tempdir().unwrap();
    let sources = CalendarStore::default();
    std::thread::scope(|scope| {
        let calls: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| sources.add_file(directory.path(), "Project", SAMPLE.into())))
            .collect();
        assert_eq!(
            calls
                .into_iter()
                .filter_map(|call| call.join().unwrap().ok())
                .count(),
            1
        );
    });
    assert_eq!(sources.list(directory.path()).unwrap().len(), 1);
}

#[test]
fn rejects_invalid_calendars_and_secret_urls_as_display_names() {
    for text in [
        "",
        "<html>BEGIN:VCALENDAR</html>",
        "BEGIN:VCALENDAR\n",
        "junk\nBEGIN:VCALENDAR\nEND:VCALENDAR",
    ] {
        assert!(validate_calendar(text).is_err());
    }
    assert!(validate_calendar(&format!("{SAMPLE}{}", " ".repeat(MAX_BYTES))).is_err());
    assert!(validate_calendar(&format!("\u{feff}{SAMPLE}")).is_ok());
    assert!(validate_name("Work https://example.com/private").is_err());
    assert!(validate_name("work\nfeed").is_err());
}

#[test]
fn subscription_addresses_require_secure_transport_and_redacted_errors() {
    assert_eq!(
        subscription_url("WEBCAL://Example.COM/calendar.ics")
            .unwrap()
            .as_str(),
        "https://example.com/calendar.ics"
    );
    for bad in [
        "https:example.com/calendar",
        "file:///calendar",
        "http://example.com/secret",
        "https://user:example@example.com/calendar",
        "https://example.com/calendar#secret",
        "https://example.com/\nsecret",
    ] {
        assert_eq!(subscription_url(bad).err().as_deref(), Some(INVALID_URL));
    }
}
