//! Live tests against a real evolution-data-server calendar factory.
//! Enabled with NEUTRON_EDS=1 inside a private D-Bus session
//! (testserver/eds-test.sh). Skipped otherwise.

use neutron_core::gnome::EdsCalendar;
use neutron_core::ical;

fn enabled() -> bool {
    let on = std::env::var("NEUTRON_EDS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipping: set NEUTRON_EDS=1 inside a D-Bus session with EDS");
    }
    on
}

fn vevent(uid: &str, summary: &str, start: i64, extra: &str) -> String {
    let f = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .unwrap()
            .format("%Y%m%dT%H%M%SZ")
            .to_string()
    };
    format!(
        "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{summary}\r\n{extra}END:VEVENT\r\n",
        f(start),
        f(start),
        f(start + 3600)
    )
}

#[tokio::test]
async fn eds_calendar_seed_query_and_update() {
    if !enabled() {
        return;
    }
    let cal = EdsCalendar::connect()
        .await
        .expect("connect to EDS calendar");
    assert!(cal.calendar_count() >= 1);
    let now = chrono::Utc::now().timestamp();
    let tag = now.to_string();
    let a = format!("nm-a-{tag}@example.test");
    let b = format!("nm-b-{tag}@example.test");
    let r = format!("nm-r-{tag}@example.test");
    let past = format!("nm-p-{tag}@example.test");
    cal.add_event(&vevent(&a, "Standup", now + 2 * 3600, ""))
        .await
        .unwrap();
    cal.add_event(&vevent(
        &b,
        "Design review",
        now + 26 * 3600,
        "LOCATION:Room 4\r\n",
    ))
    .await
    .unwrap();
    cal.add_event(&vevent(
        &r,
        "Gym",
        now + 3600,
        "RRULE:FREQ=DAILY;COUNT=3\r\n",
    ))
    .await
    .unwrap();
    cal.add_event(&vevent(&past, "Old", now - 10 * 86_400, ""))
        .await
        .unwrap();

    let ev = cal.events_in_range(now, now + 7 * 86_400).await.unwrap();
    let mine: Vec<_> = ev.iter().filter(|e| e.uid.contains(&tag)).collect();
    let names: Vec<&str> = mine.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(
        names,
        ["Gym", "Standup", "Gym", "Design review", "Gym"],
        "{mine:?}"
    );
    assert!(mine.windows(2).all(|w| w[0].start <= w[1].start));
    assert_eq!(mine[3].location, "Room 4");
    assert!(!mine.iter().any(|e| e.summary == "Old"));

    // Updating an existing UID (an updated invitation) replaces it.
    cal.add_event(&vevent(&b, "Design review (moved)", now + 30 * 3600, ""))
        .await
        .unwrap();
    let ev = cal.events_in_range(now, now + 7 * 86_400).await.unwrap();
    let moved: Vec<_> = ev.iter().filter(|e| e.uid == b).collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].summary, "Design review (moved)");
    assert_eq!(moved[0].start, now + 30 * 3600);

    // An invitation event built by Neutron is accepted by EDS.
    let invite = format!("BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n{}END:VCALENDAR\r\n",
        vevent(&format!("nm-i-{tag}@example.test"), "Invited", now + 50 * 3600,
            "ORGANIZER;CN=Elena:mailto:elena@example.test\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:kaan@localhost\r\n"));
    let e = ical::parse(&invite).unwrap().events.remove(0);
    let v = ical::build_calendar_event(&e, "kaan@localhost", "ACCEPTED", now).unwrap();
    cal.add_event(&v).await.unwrap();
    let ev = cal.events_in_range(now, now + 7 * 86_400).await.unwrap();
    assert!(ev
        .iter()
        .any(|x| x.summary == "Invited" && x.start == e.start));
}

#[tokio::test]
async fn eds_calendar_absent_is_an_error_not_a_hang() {
    if std::env::var("NEUTRON_EDS_ABSENT").as_deref() != Ok("1") {
        return;
    }
    let t = std::time::Instant::now();
    assert!(EdsCalendar::connect().await.is_err());
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
}
