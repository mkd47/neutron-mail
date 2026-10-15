//! Seed demo events into the session's GNOME calendar (EDS) for GUI tests.
//! Run inside the same D-Bus session as the app. Idempotent (fixed UIDs).

use neutron_core::gnome::EdsCalendar;

fn stamp(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cal = EdsCalendar::connect().await.expect("EDS calendar");
    let now = chrono::Utc::now().timestamp();
    let hour = now - now % 3600;
    let events = [
        (
            "nm-demo-standup",
            "Team standup",
            hour + 2 * 3600,
            900,
            "RRULE:FREQ=DAILY;COUNT=10\r\n",
        ),
        (
            "nm-demo-lunch",
            "Lunch with Ayşe",
            hour + 5 * 3600,
            3600,
            "LOCATION:Kadıköy\r\n",
        ),
        ("nm-demo-1on1", "1:1 with Elena", hour + 26 * 3600, 1800, ""),
        (
            "nm-demo-release",
            "Release 0.4 planning",
            hour + 50 * 3600,
            3600,
            "LOCATION:Room 4\r\n",
        ),
        ("nm-demo-dentist", "Dentist", hour + 75 * 3600, 2700, ""),
    ];
    for (uid, summary, start, len, extra) in events {
        let v = format!(
            "BEGIN:VEVENT\r\nUID:{uid}@neutron.test\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{summary}\r\n{extra}END:VEVENT\r\n",
            stamp(now),
            stamp(start),
            stamp(start + len)
        );
        cal.add_event(&v).await.expect("add event");
    }
    println!("seeded {} events", events.len());
}
