//! A clicked new-mail notification ("default" action) is reported with its
//! id; other actions and other ids are ignored by the caller's lookup.
//! Needs a D-Bus session bus (run under `dbus-run-session`); skipped otherwise.

use std::time::Duration;

#[tokio::test]
async fn default_action_clicks_are_reported_with_their_id() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("no session bus; skipped");
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let watcher = tokio::spawn(neutron_core::gnome::watch_notification_clicks(move |id| {
        let _ = tx.send(id);
    }));
    // Give the match rule time to be registered before emitting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let conn = zbus::Connection::session().await.unwrap();
    for (id, action) in [(41u32, "dismiss"), (42u32, "default")] {
        conn.emit_signal(
            None::<()>,
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "ActionInvoked",
            &(id, action),
        )
        .await
        .unwrap();
    }
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("click not reported")
        .unwrap();
    assert_eq!(got, 42, "only the default action counts as a click");
    watcher.abort();
}
