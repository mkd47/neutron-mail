//! GNOME Calendar (evolution-data-server): the agenda strip and iMIP
//! invitations. EDS is connected lazily and retried at most once a minute;
//! without EDS the agenda is empty and invitations can still be answered
//! by mail (only "add to calendar" needs EDS).

use std::time::{Duration, Instant};

use neutron_core::gnome::{CalendarEvent, EdsCalendar};
use neutron_core::ical::{self, InviteView};
use neutron_core::{imap, parse, smtp, Error};
use serde::Serialize;
use tauri::State;

use crate::commands::Res;
use crate::AppState;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_AGENDA_DAYS: u32 = 31;

#[derive(Default)]
pub struct Calendar {
    inner: tokio::sync::Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    eds: Option<EdsCalendar>,
    last_attempt: Option<Instant>,
    last_error: Option<String>,
}

impl Inner {
    async fn ensure(&mut self) -> Option<&EdsCalendar> {
        if self.eds.is_none() && !self.last_attempt.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            self.last_attempt = Some(Instant::now());
            match EdsCalendar::connect().await {
                Ok(c) => {
                    tracing::info!(
                        calendars = c.calendar_count(),
                        "GNOME calendar (EDS) connected"
                    );
                    self.eds = Some(c);
                    self.last_error = None;
                }
                Err(e) => {
                    tracing::info!("GNOME calendar unavailable: {e}");
                    self.last_error = Some(e.to_string());
                }
            }
        }
        self.eds.as_ref()
    }
}

#[derive(Serialize)]
pub struct Agenda {
    available: bool,
    error: Option<String>,
    events: Vec<CalendarEvent>,
}

#[tauri::command]
pub async fn upcoming_events(state: State<'_, AppState>, days: u32) -> Res<Agenda> {
    let days = days.clamp(1, MAX_AGENDA_DAYS);
    let mut inner = state.calendar.inner.lock().await;
    let now = unix_now();
    let Some(eds) = inner.ensure().await else {
        return Ok(Agenda {
            available: false,
            error: inner.last_error.clone(),
            events: vec![],
        });
    };
    match eds
        .events_in_range(now, now + i64::from(days) * 86_400)
        .await
    {
        Ok(mut events) => {
            events.truncate(50);
            Ok(Agenda {
                available: true,
                error: None,
                events,
            })
        }
        Err(e) => {
            // Reconnect next time (EDS may have restarted).
            inner.eds = None;
            Ok(Agenda {
                available: false,
                error: Some(e.to_string()),
                events: vec![],
            })
        }
    }
}

fn load_invite(state: &AppState, message_id: i64) -> Res<Option<(InviteView, i64)>> {
    let raw = state.store.raw(message_id)?;
    let Some(ics) = parse::calendar_part(&raw) else {
        return Ok(None);
    };
    let account_id = state.store.message_account(message_id)?;
    let account = state.store.account(account_id)?;
    let recorded = state.store.invite_response(message_id)?;
    Ok(ical::invite_view(&ics, &account.config.email, recorded).map(|v| (v, account_id)))
}

#[tauri::command]
pub fn get_invite(state: State<'_, AppState>, message_id: i64) -> Res<Option<InviteView>> {
    Ok(load_invite(&state, message_id)?.map(|(v, _)| v))
}

#[derive(Serialize)]
pub struct InviteResult {
    invite: InviteView,
    /// "saved", "no_sent_folder" or "failed".
    sent_copy: &'static str,
    calendar_error: Option<String>,
}

/// Send an iMIP REPLY to the organizer and optionally add/update the event
/// in the local GNOME calendar (not for "decline").
#[tauri::command]
pub async fn respond_invite(
    state: State<'_, AppState>,
    message_id: i64,
    partstat: String,
    add_to_calendar: bool,
) -> Res<InviteResult> {
    let ps = ical::valid_partstat(&partstat)?;
    let (view, account_id) = load_invite(&state, message_id)?
        .ok_or_else(|| Error::Invalid("this message has no invitation".into()))?;
    if !view.can_respond {
        return Err(Error::Invalid("this invitation can't be answered".into()));
    }
    let account = state.store.account(account_id)?;
    let password = crate::providers::credential(&state, account_id).await?;
    let cfg = &account.config;
    let now = unix_now();
    let ics = ical::build_reply(&view.event, &cfg.email, &cfg.display_name, ps, now)?;
    let verb = match ps {
        "ACCEPTED" => "Accepted",
        "TENTATIVE" => "Tentative",
        _ => "Declined",
    };
    let who = if cfg.display_name.is_empty() {
        cfg.email.as_str()
    } else {
        cfg.display_name.as_str()
    };
    let subject: String = format!("{verb}: {}", view.event.summary)
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    let text = format!(
        "{who} has {} the invitation \"{}\".\n",
        match ps {
            "ACCEPTED" => "accepted",
            "TENTATIVE" => "tentatively accepted",
            _ => "declined",
        },
        view.event.summary
    );
    let parent = parse::parse(&state.store.raw(message_id)?).message_id;
    let built = smtp::build_imip(
        cfg,
        &view.event.organizer_email,
        &subject,
        &text,
        &ics,
        "REPLY",
        Some(parent.as_str()),
    )?;
    smtp::send_built(cfg, &password, &built).await?;
    tracing::info!(account = account_id, partstat = ps, "invitation answered");

    let mut sent_copy = "no_sent_folder";
    if let Some(sent) = state.store.folder_by_role(account_id, "sent")? {
        let res = async {
            let mut s = imap::connect(cfg, &password).await?;
            imap::append(&mut s, &sent.name, &built.raw, true).await?;
            drop(s.logout().await);
            Ok::<_, Error>(())
        }
        .await;
        sent_copy = if res.is_ok() { "saved" } else { "failed" };
    }

    let mut in_calendar = view.in_calendar;
    let mut calendar_error = None;
    if add_to_calendar && ps != "DECLINED" {
        let vevent = ical::build_calendar_event(&view.event, &cfg.email, ps, now)?;
        let mut inner = state.calendar.inner.lock().await;
        match inner.ensure().await {
            Some(eds) => match eds.add_event(&vevent).await {
                Ok(_) => in_calendar = true,
                Err(e) => calendar_error = Some(e.to_string()),
            },
            None => {
                calendar_error = Some(
                    inner
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "GNOME calendar is not available".into()),
                )
            }
        }
    }
    state
        .store
        .set_invite_response(message_id, ps, in_calendar)?;
    let (invite, _) = load_invite(&state, message_id)?
        .ok_or_else(|| Error::Invalid("invitation vanished".into()))?;
    Ok(InviteResult {
        invite,
        sent_copy,
        calendar_error,
    })
}
